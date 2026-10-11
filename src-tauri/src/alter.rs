//! Structure editing: turns column changes into ALTER TABLE statements per
//! engine. The UI always previews the exact SQL before applying it.
//!
//! - PostgreSQL: full support, applied in one transaction (all or nothing).
//! - MySQL/MariaDB: full support including physical column order (AFTER/FIRST).
//!   DDL auto-commits there, so statements apply one by one.
//! - SQL Server: full support except column order, applied in one
//!   transaction. Defaults are named constraints there, so changing or
//!   dropping one looks up and drops the existing constraint first.
//! - ClickHouse: add, drop, rename, retype and defaults. Nullability is part
//!   of the type there (`Nullable(T)`). Statements apply one by one.
//! - DuckDB: add, drop, rename, retype, nullability and defaults, applied in
//!   one transaction.
//! - Oracle: add, drop, rename, retype, nullability and defaults. DDL commits
//!   implicitly there, so statements apply one by one.
//! - SQLite: add, rename and drop only (SQLite can't alter a column's type,
//!   nullability or default in place).

use serde::Deserialize;
use sqlx::{AssertSqlSafe, Executor};

use crate::{
    config::Driver,
    db::{Pool, Session},
    error::{AppError, AppResult},
};

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    /// Raw SQL expression (e.g. `now()`, `'draft'`, `0`); None/empty = no default.
    pub default_value: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ColumnChange {
    /// None = new column.
    pub original: Option<ColumnDef>,
    /// None = drop the column.
    pub column: Option<ColumnDef>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AlterRequest {
    pub schema: String,
    pub table: String,
    pub changes: Vec<ColumnChange>,
    /// Desired physical column order (MySQL only); empty = unchanged.
    #[serde(default)]
    pub order: Vec<String>,
}

/// ClickHouse: the column type with `Nullable(…)` added or removed to match `nullable`.
fn ch_type(c: &ColumnDef) -> String {
    let t = c.data_type.trim();
    let is_nullable = t.starts_with("Nullable(") || t.starts_with("LowCardinality(Nullable(");
    match (c.nullable, is_nullable) {
        (true, false) => match t.strip_prefix("LowCardinality(").and_then(|x| x.strip_suffix(')')) {
            Some(inner) => format!("LowCardinality(Nullable({inner}))"),
            None => format!("Nullable({t})"),
        },
        (false, true) => {
            if let Some(inner) = t.strip_prefix("LowCardinality(Nullable(").and_then(|x| x.strip_suffix("))")) {
                format!("LowCardinality({inner})")
            } else {
                t.strip_prefix("Nullable(").and_then(|x| x.strip_suffix(')')).unwrap_or(t).to_string()
            }
        }
        _ => t.to_string(),
    }
}

fn default_of(c: &ColumnDef) -> Option<&str> {
    c.default_value.as_deref().map(str::trim).filter(|d| !d.is_empty())
}

fn validate(c: &ColumnDef) -> AppResult<()> {
    if c.name.trim().is_empty() {
        return Err(AppError::msg("Every column needs a name"));
    }
    if c.data_type.trim().is_empty() {
        return Err(AppError::msg(format!("Column `{}` needs a type", c.name)));
    }
    // Types and defaults are SQL fragments; block statement separators so a
    // typo can't turn into a second statement.
    for frag in [c.data_type.as_str(), c.default_value.as_deref().unwrap_or("")] {
        if frag.contains(';') || frag.contains("--") || frag.contains("/*") {
            return Err(AppError::msg(format!("Column `{}` contains `;` or a comment, which isn't allowed here", c.name)));
        }
    }
    Ok(())
}

impl Session {
    /// MySQL: each column's exact definition from SHOW CREATE TABLE, so a
    /// reorder keeps AUTO_INCREMENT, COMMENT, collation, generated columns etc.
    async fn mysql_column_defs(&self, from: &str) -> AppResult<Vec<(String, String)>> {
        let rows = self.run_script(&format!("SHOW CREATE TABLE {from}"), 1).await?;
        let ddl = rows.first().and_then(|r| r.rows.first()).and_then(|r| r.get(1).cloned().flatten()).unwrap_or_default();
        Ok(ddl
            .lines()
            .filter_map(|l| {
                let t = l.trim().trim_end_matches(',');
                let rest = t.strip_prefix('`')?;
                let end = rest.find('`')?;
                Some((rest[..end].replace("``", "`"), t.to_string()))
            })
            .collect())
    }

    async fn mysql_extra(&self, schema: &str, table: &str, column: &str) -> AppResult<String> {
        let d = self.driver;
        let sql = format!(
            "SELECT EXTRA, COLUMN_COMMENT FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} AND COLUMN_NAME = {}",
            d.quote_literal(schema),
            d.quote_literal(table),
            d.quote_literal(column)
        );
        let rows = self.run_script(&sql, 1).await?;
        let row = rows.first().and_then(|r| r.rows.first()).cloned().unwrap_or_default();
        let mut extra = String::new();
        if let Some(e) = row.first().cloned().flatten() {
            let e = e.to_ascii_lowercase();
            if e.contains("auto_increment") {
                extra.push_str(" AUTO_INCREMENT");
            }
            if let Some(p) = e.find("on update") {
                extra.push(' ');
                extra.push_str(&e[p..].to_uppercase());
            }
        }
        if let Some(c) = row.get(1).cloned().flatten().filter(|c| !c.is_empty()) {
            extra.push_str(&format!(" COMMENT {}", d.quote_literal(&c)));
        }
        Ok(extra)
    }

    /// SQL Server: name of the default constraint on a column, if any.
    async fn mssql_default_constraint(&self, from: &str, column: &str) -> AppResult<Option<String>> {
        let d = self.driver;
        let sql = format!(
            "SELECT dc.name FROM sys.default_constraints dc JOIN sys.columns c ON c.object_id = dc.parent_object_id \
             AND c.column_id = dc.parent_column_id WHERE dc.parent_object_id = OBJECT_ID({}) AND c.name = {}",
            d.quote_literal(from),
            d.quote_literal(column)
        );
        let rows = self.run_script(&sql, 1).await?;
        Ok(rows.first().and_then(|r| r.rows.first()).and_then(|r| r.first().cloned().flatten()))
    }

    /// Builds the statements for a structure change without running them.
    pub async fn plan_alter(&self, req: &AlterRequest) -> AppResult<Vec<String>> {
        let d = self.driver;
        let q = |s: &str| d.quote_ident(s);
        let from = d.qualified(&req.schema, &req.table);
        let mut out = Vec::new();

        for ch in &req.changes {
            match (&ch.original, &ch.column) {
                (None, None) => {}
                (None, Some(c)) if d == Driver::Clickhouse => {
                    validate(c)?;
                    let mut s = format!("ALTER TABLE {from} ADD COLUMN {} {}", q(&c.name), ch_type(c));
                    if let Some(def) = default_of(c) {
                        s.push_str(&format!(" DEFAULT {def}"));
                    }
                    out.push(s);
                }
                (None, Some(c)) if d == Driver::Oracle => {
                    validate(c)?;
                    let mut s = format!("ALTER TABLE {from} ADD ({} {}", q(&c.name), c.data_type.trim());
                    if let Some(def) = default_of(c) {
                        s.push_str(&format!(" DEFAULT {def}"));
                    }
                    if !c.nullable {
                        s.push_str(" NOT NULL");
                    }
                    s.push(')');
                    out.push(s);
                }
                (None, Some(c)) => {
                    validate(c)?;
                    // SQL Server's ADD takes no COLUMN keyword.
                    let add = if d == Driver::Mssql { "ADD" } else { "ADD COLUMN" };
                    let mut s = format!("ALTER TABLE {from} {add} {} {}", q(&c.name), c.data_type.trim());
                    // DuckDB can't add a column with a constraint; set NOT NULL afterwards.
                    let late_not_null = d == Driver::Duckdb && !c.nullable;
                    if !c.nullable && !late_not_null {
                        if d == Driver::Sqlite && default_of(c).is_none() {
                            return Err(AppError::msg(format!(
                                "SQLite needs a default value to add NOT NULL column `{}`",
                                c.name
                            )));
                        }
                        s.push_str(" NOT NULL");
                    }
                    if let Some(def) = default_of(c) {
                        s.push_str(&format!(" DEFAULT {def}"));
                    }
                    out.push(s);
                    if late_not_null {
                        out.push(format!("ALTER TABLE {from} ALTER COLUMN {} SET NOT NULL", q(&c.name)));
                    }
                }
                (Some(o), None) => {
                    if d == Driver::Mssql
                        && let Some(dc) = self.mssql_default_constraint(&from, &o.name).await?
                    {
                        out.push(format!("ALTER TABLE {from} DROP CONSTRAINT {}", q(&dc)));
                    }
                    out.push(format!("ALTER TABLE {from} DROP COLUMN {}", q(&o.name)));
                }
                (Some(o), Some(c)) => {
                    if o == c {
                        continue;
                    }
                    validate(c)?;
                    match d {
                        Driver::Postgres | Driver::Duckdb => {
                            if o.name != c.name {
                                out.push(format!("ALTER TABLE {from} RENAME COLUMN {} TO {}", q(&o.name), q(&c.name)));
                            }
                            let col = q(&c.name);
                            if o.data_type.trim() != c.data_type.trim() {
                                let t = c.data_type.trim();
                                out.push(format!("ALTER TABLE {from} ALTER COLUMN {col} TYPE {t} USING {col}::{t}"));
                            }
                            if o.nullable != c.nullable {
                                let verb = if c.nullable { "DROP" } else { "SET" };
                                out.push(format!("ALTER TABLE {from} ALTER COLUMN {col} {verb} NOT NULL"));
                            }
                            if default_of(o) != default_of(c) {
                                out.push(match default_of(c) {
                                    Some(def) => format!("ALTER TABLE {from} ALTER COLUMN {col} SET DEFAULT {def}"),
                                    None => format!("ALTER TABLE {from} ALTER COLUMN {col} DROP DEFAULT"),
                                });
                            }
                        }
                        Driver::Mysql => {
                            let mut s = format!(
                                "ALTER TABLE {from} CHANGE COLUMN {} {} {} {}",
                                q(&o.name),
                                q(&c.name),
                                c.data_type.trim(),
                                if c.nullable { "NULL" } else { "NOT NULL" }
                            );
                            if let Some(def) = default_of(c) {
                                s.push_str(&format!(" DEFAULT {def}"));
                            }
                            s.push_str(&self.mysql_extra(&req.schema, &req.table, &o.name).await?);
                            out.push(s);
                        }
                        Driver::Mssql => {
                            if o.name != c.name {
                                // sp_rename takes the object path as a string: schema.table.column.
                                let path = format!("{}.{}", from, q(&o.name));
                                out.push(format!("EXEC sp_rename {}, {}, 'COLUMN'", d.quote_literal(&path), d.quote_literal(&c.name)));
                            }
                            let col = q(&c.name);
                            let type_changed = o.data_type.trim() != c.data_type.trim() || o.nullable != c.nullable;
                            let default_changed = default_of(o) != default_of(c);
                            // A default constraint blocks ALTER COLUMN, so drop it first and re-add after.
                            let mut had_default = false;
                            if (type_changed || default_changed)
                                && let Some(dc) = self.mssql_default_constraint(&from, &o.name).await?
                            {
                                out.push(format!("ALTER TABLE {from} DROP CONSTRAINT {}", q(&dc)));
                                had_default = true;
                            }
                            if type_changed {
                                let null = if c.nullable { "NULL" } else { "NOT NULL" };
                                out.push(format!("ALTER TABLE {from} ALTER COLUMN {col} {} {null}", c.data_type.trim()));
                            }
                            if let Some(def) = default_of(c).filter(|_| default_changed || had_default) {
                                out.push(format!("ALTER TABLE {from} ADD DEFAULT {def} FOR {col}"));
                            }
                        }
                        Driver::Clickhouse => {
                            if o.name != c.name {
                                out.push(format!("ALTER TABLE {from} RENAME COLUMN {} TO {}", q(&o.name), q(&c.name)));
                            }
                            let col = q(&c.name);
                            let (ot, nt) = (ch_type(o), ch_type(c));
                            if ot != nt {
                                out.push(format!("ALTER TABLE {from} MODIFY COLUMN {col} {nt}"));
                            }
                            if default_of(o) != default_of(c) {
                                out.push(match default_of(c) {
                                    Some(def) => format!("ALTER TABLE {from} MODIFY COLUMN {col} DEFAULT {def}"),
                                    None => format!("ALTER TABLE {from} MODIFY COLUMN {col} REMOVE DEFAULT"),
                                });
                            }
                        }
                        Driver::Oracle => {
                            if o.name != c.name {
                                out.push(format!("ALTER TABLE {from} RENAME COLUMN {} TO {}", q(&o.name), q(&c.name)));
                            }
                            let col = q(&c.name);
                            if o.data_type.trim() != c.data_type.trim() {
                                out.push(format!("ALTER TABLE {from} MODIFY ({col} {})", c.data_type.trim()));
                            }
                            if default_of(o) != default_of(c) {
                                out.push(format!("ALTER TABLE {from} MODIFY ({col} DEFAULT {})", default_of(c).unwrap_or("NULL")));
                            }
                            if o.nullable != c.nullable {
                                out.push(format!("ALTER TABLE {from} MODIFY ({col} {})", if c.nullable { "NULL" } else { "NOT NULL" }));
                            }
                        }
                        Driver::Redis | Driver::Mongodb => return Err(crate::error::not_sql()),
                        Driver::Sqlite => {
                            if o.data_type.trim() != c.data_type.trim() || o.nullable != c.nullable || default_of(o) != default_of(c) {
                                return Err(AppError::msg(format!(
                                    "SQLite can't change the type, nullability or default of `{}` in place. \
                                     Create a new table with the structure you want and copy the data across.",
                                    o.name
                                )));
                            }
                            if o.name != c.name {
                                out.push(format!("ALTER TABLE {from} RENAME COLUMN {} TO {}", q(&o.name), q(&c.name)));
                            }
                        }
                    }
                }
            }
        }

        if !req.order.is_empty() && d == Driver::Mysql {
            // Apply after renames/adds so every name in `order` exists.
            let current = self.mysql_column_defs(&from).await?;
            let renamed = |name: &str| -> String {
                req.changes
                    .iter()
                    .find_map(|c| match (&c.original, &c.column) {
                        (Some(o), Some(n)) if o.name == name => Some(n.name.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| name.to_string())
            };
            let current_order: Vec<String> = current.iter().map(|(n, _)| renamed(n)).collect();
            if current_order != req.order {
                for (i, name) in req.order.iter().enumerate() {
                    // Definition by the column's *original* name, rewritten to its new name.
                    let Some((orig, def)) = current.iter().find(|(n, _)| renamed(n) == *name) else { continue };
                    let def = if orig != name { def.replacen(&q(orig), &q(name), 1) } else { def.clone() };
                    let pos = if i == 0 { "FIRST".to_string() } else { format!("AFTER {}", q(&req.order[i - 1])) };
                    out.push(format!("ALTER TABLE {from} MODIFY COLUMN {def} {pos}"));
                }
            }
        }
        Ok(out)
    }

    pub async fn apply_alter(&self, req: &AlterRequest) -> AppResult<Vec<String>> {
        if self.read_only {
            return Err(AppError::msg("This connection is read-only"));
        }
        let stmts = self.plan_alter(req).await?;
        if stmts.is_empty() {
            return Ok(stmts);
        }
        macro_rules! in_tx {
            ($pool:expr) => {{
                let mut tx = $pool.begin().await?;
                for s in &stmts {
                    let started = std::time::Instant::now();
                    let res = tx.execute(AssertSqlSafe(s.clone())).await;
                    self.record(s, started, res.as_ref().map(|_| "ok".to_string()).map_err(crate::error::db_message));
                    res?;
                }
                tx.commit().await?;
            }};
        }
        match &self.pool {
            Pool::Pg(p) => in_tx!(p),
            Pool::Lite(p) => in_tx!(p),
            Pool::Duck(dk) => {
                let (res, log) = dk.exec_script_tx(stmts.clone()).await?;
                for (sql, started, outcome) in log {
                    self.record(&sql, started, outcome);
                }
                res.map_err(|e| {
                    let m = e.to_string();
                    if m.starts_with("Dependency Error") {
                        AppError::msg(format!(
                            "{m}\nDuckDB can't change a table while indexes or other tables' foreign keys depend on it. \
                             Drop those first, or create a new table with the structure you want and copy the data across."
                        ))
                    } else {
                        e
                    }
                })?;
            }
            // No transactions: apply in order and say how far it got.
            Pool::Ch(c) => {
                for (i, s) in stmts.iter().enumerate() {
                    let started = std::time::Instant::now();
                    let res = c.query(s, 0, None, &[("mutations_sync", "2".into())]).await;
                    self.record(s, started, res.as_ref().map(|_| "ok".to_string()).map_err(|e| e.to_string()));
                    if let Err(e) = res {
                        return Err(AppError::msg(format!("{e} (statement {} of {}; the {i} before it were applied)", i + 1, stmts.len())));
                    }
                }
            }
            // SQL Server DDL is transactional too.
            Pool::Ms(p) => {
                let mut conn = p.acquire().await?;
                crate::mssql::run(&mut conn, "SET XACT_ABORT ON; BEGIN TRAN; SELECT 1", 1, None).await?;
                let mut failed = None;
                for s in &stmts {
                    let started = std::time::Instant::now();
                    let res = crate::mssql::run(&mut conn, s, 0, None).await;
                    self.record(s, started, res.as_ref().map(|_| "ok".to_string()).map_err(|e| e.to_string()));
                    if let Err(e) = res {
                        failed = Some(e);
                        break;
                    }
                }
                let end = if failed.is_some() { "IF @@TRANCOUNT > 0 ROLLBACK" } else { "COMMIT" };
                if crate::mssql::run(&mut conn, end, 0, None).await.is_err() {
                    conn.discard();
                }
                if let Some(e) = failed {
                    return Err(e);
                }
            }
            // Oracle commits each DDL statement implicitly too.
            Pool::Ora(_) => {
                for (i, s) in stmts.iter().enumerate() {
                    if let Err(e) = self.run_script(s, 0).await {
                        return Err(AppError::msg(format!(
                            "{e} (statement {} of {}; the {i} before it were applied, because Oracle can't roll back structure changes)",
                            i + 1,
                            stmts.len()
                        )));
                    }
                }
            }
            // MySQL commits each DDL statement implicitly.
            Pool::My(p) => {
                for (i, s) in stmts.iter().enumerate() {
                    let started = std::time::Instant::now();
                    let res = p.execute(AssertSqlSafe(s.clone())).await;
                    self.record(s, started, res.as_ref().map(|_| "ok".to_string()).map_err(crate::error::db_message));
                    if let Err(e) = res {
                        return Err(AppError::msg(format!(
                            "{} (statement {} of {}; the {} before it were applied, because MySQL can't roll back structure changes)",
                            AppError::from(e),
                            i + 1,
                            stmts.len(),
                            i
                        )));
                    }
                }
            }
        }
        Ok(stmts)
    }
}

#[cfg(test)]
mod tests {
    //! Uses the same env vars as db.rs tests; works on a scratch table.
    use super::*;
    use crate::config::{ConnectionConfig, SslMode};

    fn cfg(var: &str, driver: Driver) -> Option<ConnectionConfig> {
        let v = std::env::var(var).ok()?;
        let mut c = ConnectionConfig {
            id: "alter-test".into(),
            name: "t".into(),
            driver,
            provider: String::new(),
            host: String::new(),
            port: 0,
            user: String::new(),
            password: None,
            database: String::new(),
            ssl_mode: SslMode::Disable,
            ssl_ca: String::new(),
            ssl_cert: String::new(),
            ssl_key: String::new(),
            ssh: Default::default(),
            ssh_secret: None,
            color: String::new(),
            tag: String::new(),
            read_only: false,
            ai_access: false,
        };
        if driver == Driver::Sqlite {
            c.database = v;
        } else {
            let p: Vec<&str> = v.split(':').collect();
            (c.host, c.port, c.user, c.database) = (p[0].into(), p[1].parse().unwrap(), p[2].into(), p[3].into());
        }
        Some(c)
    }

    fn col(name: &str, ty: &str, nullable: bool, def: Option<&str>) -> ColumnDef {
        ColumnDef { name: name.into(), data_type: ty.into(), nullable, default_value: def.map(Into::into) }
    }

    async fn columns(s: &Session, schema: &str) -> Vec<(String, String, bool, Option<String>)> {
        s.table_structure(schema, "alter_t")
            .await
            .unwrap()
            .columns
            .into_iter()
            .map(|c| (c.name, c.data_type.to_lowercase(), c.nullable, c.default_value))
            .collect()
    }

    #[tokio::test]
    async fn postgres_alter() {
        let _serial = crate::test_lock::PG.lock().await;
        let Some(c) = cfg("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        s.run_script("DROP TABLE IF EXISTS alter_t; CREATE TABLE alter_t (id serial PRIMARY KEY, title text, n int); INSERT INTO alter_t (title, n) VALUES ('a', 1)", 1).await.unwrap();
        let req = AlterRequest {
            schema: "public".into(),
            table: "alter_t".into(),
            changes: vec![
                ColumnChange { original: Some(col("title", "text", true, None)), column: Some(col("headline", "varchar(120)", false, Some("'untitled'"))) },
                ColumnChange { original: Some(col("n", "integer", true, None)), column: Some(col("n", "bigint", true, None)) },
                ColumnChange { original: None, column: Some(col("created_at", "timestamptz", false, Some("now()"))) },
            ],
            order: vec![],
        };
        let plan = s.plan_alter(&req).await.unwrap();
        assert_eq!(plan.len(), 6, "{plan:#?}");
        s.apply_alter(&req).await.unwrap();
        let cols = columns(&s, "public").await;
        assert_eq!(cols[1].0, "headline");
        assert_eq!(cols[1].1, "character varying(120)");
        assert!(!cols[1].2);
        assert_eq!(cols[2].1, "bigint");
        assert_eq!(cols[3].0, "created_at");

        // A failing statement rolls the whole batch back.
        let bad = AlterRequest {
            schema: "public".into(),
            table: "alter_t".into(),
            changes: vec![
                ColumnChange { original: None, column: Some(col("ok_col", "text", true, None)) },
                ColumnChange { original: Some(col("headline", "varchar(120)", false, None)), column: Some(col("headline", "integer", false, None)) },
            ],
            order: vec![],
        };
        assert!(s.apply_alter(&bad).await.is_err());
        assert!(!columns(&s, "public").await.iter().any(|c| c.0 == "ok_col"), "rolled back");

        // Drop
        let drop = AlterRequest { schema: "public".into(), table: "alter_t".into(), changes: vec![ColumnChange { original: Some(col("n", "bigint", true, None)), column: None }], order: vec![] };
        s.apply_alter(&drop).await.unwrap();
        assert!(!columns(&s, "public").await.iter().any(|c| c.0 == "n"));
        // Separators are rejected.
        let inj = AlterRequest { schema: "public".into(), table: "alter_t".into(), changes: vec![ColumnChange { original: None, column: Some(col("x", "int; DROP TABLE users", true, None)) }], order: vec![] };
        assert!(s.plan_alter(&inj).await.is_err());
        s.run_script("DROP TABLE alter_t", 1).await.unwrap();
    }

    #[tokio::test]
    async fn mysql_alter_and_reorder_keeps_auto_increment() {
        let _serial = crate::test_lock::MYSQL.lock().await;
        let Some(c) = cfg("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        let db = c.database.clone();
        s.run_script("DROP TABLE IF EXISTS alter_t; CREATE TABLE alter_t (id INT AUTO_INCREMENT PRIMARY KEY, title VARCHAR(50) COMMENT 'the title', n INT)", 1).await.unwrap();
        let req = AlterRequest {
            schema: db.clone(),
            table: "alter_t".into(),
            changes: vec![
                ColumnChange { original: Some(col("title", "varchar(50)", true, None)), column: Some(col("headline", "varchar(100)", false, Some("'x'"))) },
                ColumnChange { original: None, column: Some(col("price", "decimal(10,2)", true, None)) },
            ],
            order: vec!["n".into(), "id".into(), "headline".into(), "price".into()],
        };
        s.apply_alter(&req).await.unwrap();
        let names: Vec<String> = columns(&s, &db).await.into_iter().map(|c| c.0).collect();
        assert_eq!(names, ["n", "id", "headline", "price"]);
        let ddl = s.run_script("SHOW CREATE TABLE alter_t", 1).await.unwrap()[0].rows[0][1].clone().unwrap();
        assert!(ddl.contains("AUTO_INCREMENT"), "{ddl}");
        assert!(ddl.contains("COMMENT 'the title'"), "{ddl}");
        s.run_script("DROP TABLE alter_t", 1).await.unwrap();
    }

    #[tokio::test]
    async fn sqlite_alter_limits() {
        let Some(c) = cfg("TABLA_TEST_SQLITE", Driver::Sqlite) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        s.run_script("DROP TABLE IF EXISTS alter_t; CREATE TABLE alter_t (id INTEGER PRIMARY KEY, title TEXT, n INTEGER)", 1).await.unwrap();
        let ok = AlterRequest {
            schema: "main".into(),
            table: "alter_t".into(),
            changes: vec![
                ColumnChange { original: Some(col("title", "TEXT", true, None)), column: Some(col("headline", "TEXT", true, None)) },
                ColumnChange { original: None, column: Some(col("score", "REAL", false, Some("0"))) },
                ColumnChange { original: Some(col("n", "INTEGER", true, None)), column: None },
            ],
            order: vec![],
        };
        s.apply_alter(&ok).await.unwrap();
        let names: Vec<String> = columns(&s, "main").await.into_iter().map(|c| c.0).collect();
        assert_eq!(names, ["id", "headline", "score"]);
        let bad = AlterRequest { schema: "main".into(), table: "alter_t".into(), changes: vec![ColumnChange { original: Some(col("headline", "TEXT", true, None)), column: Some(col("headline", "INTEGER", true, None)) }], order: vec![] };
        assert!(s.plan_alter(&bad).await.unwrap_err().to_string().contains("can't change"));
        s.run_script("DROP TABLE alter_t", 1).await.unwrap();
    }
}
