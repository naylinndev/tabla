//! Oracle's data dictionary for the structure, relations and ER views.
//!
//! Oracle's ALL_* views are complex, and a query joining ALL_CONSTRAINTS to
//! other views can take seconds just to parse. So each view is read on its own
//! with bound names (parsed once, shared by every table) and joined here.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    db::{ColumnInfo, ForeignKey, GraphColumn, IndexInfo, Session, TableStructure, TriggerInfo},
    error::AppResult,
};

type Rows = Vec<Vec<Option<String>>>;

/// A constraint from ALL_CONSTRAINTS with its columns in order.
#[derive(Clone)]
struct Constraint {
    owner: String,
    name: String,
    table: String,
    kind: String,
    index_name: Option<String>,
    r_owner: Option<String>,
    r_name: Option<String>,
    delete_rule: Option<String>,
    columns: Vec<String>,
}

const COLUMNS: &str = "SELECT table_name, column_name, data_type, char_length, char_used, data_length, data_precision, data_scale, \
     nullable, data_default_vc, identity_column, virtual_column FROM all_tab_cols \
     WHERE owner = :s AND (:t IS NULL OR table_name = :t) AND hidden_column = 'NO' ORDER BY table_name, column_id";

const CONSTRAINTS: &str = "SELECT owner, constraint_name, table_name, constraint_type, index_name, r_owner, r_constraint_name, delete_rule \
     FROM all_constraints WHERE owner = :s AND (:t IS NULL OR table_name = :t) AND constraint_type IN ('P', 'U', 'R')";

const CONS_COLUMNS: &str = "SELECT constraint_name, column_name FROM all_cons_columns \
     WHERE owner = :s AND (:t IS NULL OR table_name = :t) ORDER BY constraint_name, position";

/// The type as written in DDL: VARCHAR2(50), NUMBER(12,2), RAW(16)…
fn ora_type(r: &[Option<String>]) -> String {
    let get = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let (ty, char_len, char_used, data_len, prec, scale) = (get(2), get(3), get(4), get(5), get(6), get(7));
    match ty.as_str() {
        "VARCHAR2" | "CHAR" if char_used == "C" => format!("{ty}({char_len} CHAR)"),
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => format!("{ty}({char_len})"),
        "RAW" => format!("RAW({data_len})"),
        "NUMBER" if !prec.is_empty() && scale != "0" && !scale.is_empty() => format!("NUMBER({prec},{scale})"),
        "NUMBER" if !prec.is_empty() => format!("NUMBER({prec})"),
        "NUMBER" if scale == "0" => "INTEGER".into(),
        "FLOAT" if !prec.is_empty() => format!("FLOAT({prec})"),
        _ => ty,
    }
}

impl Session {
    async fn ora_constraints(&self, schema: &str, table: &str) -> AppResult<Vec<Constraint>> {
        let mut cols: HashMap<String, Vec<String>> = HashMap::new();
        for r in self.ora_rows(CONS_COLUMNS, schema, table).await? {
            let mut it = r.into_iter();
            if let (Some(c), Some(col)) = (it.next().flatten(), it.next().flatten()) {
                cols.entry(c).or_default().push(col);
            }
        }
        Ok(self
            .ora_rows(CONSTRAINTS, schema, table)
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                let (owner, name, table, kind) = (n()?, n()?, n()?, n()?);
                let (index_name, r_owner, r_name, delete_rule) = (n(), n(), n(), n());
                let columns = cols.remove(&name).unwrap_or_default();
                Some(Constraint { owner, name, table, kind, index_name, r_owner, r_name, delete_rule, columns })
            })
            .collect())
    }

    /// Runs a dictionary query; an empty `table` binds NULL (all tables).
    async fn ora_rows(&self, sql: &str, schema: &str, table: &str) -> AppResult<Rows> {
        self.cat_rows(sql, schema, table).await
    }

    pub(crate) async fn ora_structure(&self, schema: &str, table: &str) -> AppResult<TableStructure> {
        let cons = self.ora_constraints(schema, table).await?;
        let pk: HashSet<&str> = cons.iter().filter(|c| c.kind == "P").flat_map(|c| c.columns.iter().map(String::as_str)).collect();
        let unique: HashSet<&str> = cons.iter().filter(|c| c.kind == "U" && c.columns.len() == 1).map(|c| c.columns[0].as_str()).collect();

        // Indexes with their columns in order.
        let mut idx_cols: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in self
            .ora_rows("SELECT index_name, column_name FROM all_ind_columns WHERE table_owner = :s AND table_name = :t ORDER BY index_name, column_position", schema, table)
            .await?
        {
            let mut it = r.into_iter();
            if let (Some(i), Some(c)) = (it.next().flatten(), it.next().flatten()) {
                idx_cols.entry(i).or_default().push(c);
            }
        }
        let indexed: HashSet<&str> = idx_cols.values().flatten().map(String::as_str).collect();
        let pk_index = cons.iter().find(|c| c.kind == "P").and_then(|c| c.index_name.clone());
        let mut indexes: Vec<IndexInfo> = self
            .ora_rows("SELECT index_name, uniqueness, index_type FROM all_indexes WHERE table_owner = :s AND table_name = :t AND index_type <> 'LOB'", schema, table)
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                let (name, uniqueness, kind) = (n()?, n().unwrap_or_default(), n().unwrap_or_default());
                let cols = idx_cols.get(&name).cloned().unwrap_or_default().join(", ");
                let unique = uniqueness == "UNIQUE";
                Some(IndexInfo {
                    definition: format!("CREATE {}INDEX \"{}\" ON \"{}\" ({cols})", if unique { "UNIQUE " } else { "" }, name, table),
                    primary: pk_index.as_deref() == Some(name.as_str()),
                    method: kind.to_lowercase(),
                    columns: cols,
                    unique,
                    name,
                })
            })
            .collect();
        indexes.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.name.cmp(&b.name)));

        let columns = self
            .ora_rows(COLUMNS, schema, table)
            .await?
            .into_iter()
            .filter_map(|r| {
                let data_type = ora_type(&r);
                let mut it = r.into_iter().skip(1);
                let mut n = || it.next().flatten();
                let name = n()?;
                for _ in 0..6 {
                    n();
                }
                let (nullable, default, identity, virt) = (n(), n(), n(), n());
                let identity = identity.as_deref() == Some("YES");
                let primary_key = pk.contains(name.as_str());
                Some(ColumnInfo {
                    key: if primary_key {
                        "PRI"
                    } else if unique.contains(name.as_str()) {
                        "UNI"
                    } else if indexed.contains(name.as_str()) {
                        "MUL"
                    } else {
                        ""
                    }
                    .into(),
                    extra: if identity {
                        "identity"
                    } else if virt.as_deref() == Some("YES") {
                        "generated"
                    } else {
                        ""
                    }
                    .into(),
                    // An identity column's default is its hidden sequence.
                    default_value: default.map(|d| d.trim().to_string()).filter(|d| !identity && !d.is_empty()),
                    nullable: nullable.as_deref() == Some("Y"),
                    data_type,
                    primary_key,
                    name,
                })
            })
            .collect();
        Ok(TableStructure { columns, indexes })
    }

    /// Columns of every table in a schema, for the ER diagram.
    pub(crate) async fn ora_graph_columns(&self, schema: &str) -> AppResult<HashMap<String, Vec<GraphColumn>>> {
        let cons = self.ora_constraints(schema, "").await?;
        let pk: HashSet<(&str, &str)> = cons.iter().filter(|c| c.kind == "P").flat_map(|c| c.columns.iter().map(move |col| (c.table.as_str(), col.as_str()))).collect();
        let mut out: HashMap<String, Vec<GraphColumn>> = HashMap::new();
        for r in self.ora_rows(COLUMNS, schema, "").await? {
            let data_type = ora_type(&r);
            let (Some(t), Some(c)) = (r.first().cloned().flatten(), r.get(1).cloned().flatten()) else { continue };
            let primary_key = pk.contains(&(t.as_str(), c.as_str()));
            out.entry(t).or_default().push(GraphColumn { name: c, data_type, primary_key });
        }
        Ok(out)
    }

    /// Foreign keys declared in a schema (or on one table), each with the
    /// referenced table and columns.
    pub(crate) async fn ora_foreign_keys(&self, schema: &str, table: &str) -> AppResult<Vec<ForeignKey>> {
        let cons = self.ora_constraints(schema, table).await?;
        let mut targets: HashMap<(String, String), Constraint> = HashMap::new();
        if table.is_empty() {
            for c in cons.iter().filter(|c| c.kind != "R") {
                targets.insert((c.owner.clone(), c.name.clone()), c.clone());
            }
        }
        let mut out = Vec::new();
        for fk in cons.iter().filter(|c| c.kind == "R") {
            let (Some(ro), Some(rn)) = (&fk.r_owner, &fk.r_name) else { continue };
            let target = match targets.get(&(ro.clone(), rn.clone())) {
                Some(t) => t.clone(),
                None => match self.ora_constraint_by_name(ro, rn).await? {
                    Some(t) => t,
                    None => continue,
                },
            };
            out.push(ForeignKey {
                name: fk.name.clone(),
                schema: fk.owner.clone(),
                table: fk.table.clone(),
                columns: fk.columns.clone(),
                ref_schema: target.owner.clone(),
                ref_table: target.table.clone(),
                ref_columns: target.columns.clone(),
                on_update: "NO ACTION".into(),
                on_delete: fk.delete_rule.clone().unwrap_or_else(|| "NO ACTION".into()),
            });
        }
        out.sort_by(|a, b| (&a.table, &a.name).cmp(&(&b.table, &b.name)));
        Ok(out)
    }

    async fn ora_constraint_by_name(&self, owner: &str, name: &str) -> AppResult<Option<Constraint>> {
        // :t carries the constraint name here.
        let rows = self.ora_rows("SELECT table_name FROM all_constraints WHERE owner = :s AND constraint_name = :t", owner, name).await?;
        let Some(table) = rows.into_iter().next().and_then(|r| r.into_iter().next().flatten()) else { return Ok(None) };
        let columns = self
            .ora_rows("SELECT column_name FROM all_cons_columns WHERE owner = :s AND constraint_name = :t ORDER BY position", owner, name)
            .await?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        Ok(Some(Constraint {
            owner: owner.into(),
            name: name.into(),
            table,
            kind: String::new(),
            index_name: None,
            r_owner: None,
            r_name: None,
            delete_rule: None,
            columns,
        }))
    }

    /// Foreign keys in any schema that point at this table's keys.
    pub(crate) async fn ora_incoming(&self, schema: &str, table: &str) -> AppResult<Vec<ForeignKey>> {
        let keys: Vec<Constraint> = self.ora_constraints(schema, table).await?.into_iter().filter(|c| c.kind != "R").collect();
        let mut out = Vec::new();
        for key in &keys {
            let refs = self
                .ora_rows(
                    "SELECT owner, constraint_name, table_name, delete_rule FROM all_constraints \
                     WHERE constraint_type = 'R' AND r_owner = :s AND r_constraint_name = :t",
                    schema,
                    &key.name,
                )
                .await?;
            for r in refs {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                let (Some(owner), Some(name), Some(child)) = (n(), n(), n()) else { continue };
                let delete_rule = n();
                let columns = self
                    .ora_rows("SELECT column_name FROM all_cons_columns WHERE owner = :s AND constraint_name = :t ORDER BY position", &owner, &name)
                    .await?
                    .into_iter()
                    .filter_map(|r| r.into_iter().next().flatten())
                    .collect();
                out.push(ForeignKey {
                    name,
                    schema: owner,
                    table: child,
                    columns,
                    ref_schema: schema.into(),
                    ref_table: table.into(),
                    ref_columns: key.columns.clone(),
                    on_update: "NO ACTION".into(),
                    on_delete: delete_rule.unwrap_or_else(|| "NO ACTION".into()),
                });
            }
        }
        out.sort_by(|a, b| (&a.schema, &a.table, &a.name).cmp(&(&b.schema, &b.table, &b.name)));
        Ok(out)
    }

    pub(crate) async fn ora_triggers(&self, schema: &str, table: &str) -> AppResult<Vec<TriggerInfo>> {
        // The body is a LONG column, which can't be concatenated in SQL.
        Ok(self
            .ora_rows(
                "SELECT trigger_name, trigger_type, triggering_event, status, description, trigger_body \
                 FROM all_triggers WHERE table_owner = :s AND table_name = :t ORDER BY trigger_name",
                schema,
                table,
            )
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                let (name, kind, event, status, desc, body) = (n()?, n().unwrap_or_default(), n().unwrap_or_default(), n(), n(), n());
                let timing = if kind.starts_with("BEFORE") {
                    "BEFORE"
                } else if kind.starts_with("INSTEAD OF") {
                    "INSTEAD OF"
                } else {
                    "AFTER"
                };
                Some(TriggerInfo {
                    name,
                    timing: timing.into(),
                    event: event.replace(" OR ", ", "),
                    enabled: status.as_deref() == Some("ENABLED"),
                    definition: format!("CREATE OR REPLACE TRIGGER {}{}", desc.unwrap_or_default(), body.unwrap_or_default()).trim_end().to_string(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_types() {
        let row = |v: &[&str]| v.iter().map(|s| if s.is_empty() { None } else { Some(s.to_string()) }).collect::<Vec<_>>();
        assert_eq!(super::ora_type(&row(&["T", "C", "VARCHAR2", "50", "B", "50", "", ""])), "VARCHAR2(50)");
        assert_eq!(super::ora_type(&row(&["T", "C", "VARCHAR2", "50", "C", "200", "", ""])), "VARCHAR2(50 CHAR)");
        assert_eq!(super::ora_type(&row(&["T", "C", "NUMBER", "", "", "22", "12", "2"])), "NUMBER(12,2)");
        assert_eq!(super::ora_type(&row(&["T", "C", "NUMBER", "", "", "22", "10", "0"])), "NUMBER(10)");
        assert_eq!(super::ora_type(&row(&["T", "C", "NUMBER", "", "", "22", "", "0"])), "INTEGER");
        assert_eq!(super::ora_type(&row(&["T", "C", "NUMBER", "", "", "22", "", ""])), "NUMBER");
        assert_eq!(super::ora_type(&row(&["T", "C", "RAW", "", "", "16", "", ""])), "RAW(16)");
        assert_eq!(super::ora_type(&row(&["T", "C", "TIMESTAMP(6)", "", "", "11", "", "6"])), "TIMESTAMP(6)");
    }
}
