//! Database sessions: one connection pool per open connection.
//!
//! All reads go through `raw_sql`, which uses the text protocol on Postgres
//! and MySQL. That lets every value be decoded to a display string without
//! per-type mapping, and supports multi-statement scripts. Writes (cell
//! edits) use prepared statements with bound parameters.

use std::{collections::HashMap, time::Duration};

use futures_util::{TryStreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use sqlx::{
    AssertSqlSafe, Column, Either, Executor, Row, SqlSafeStr, TypeInfo, ValueRef,
    mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow, MySqlSslMode},
    postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow, PgSslMode},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow},
};

use crate::{
    config::{ConnectionConfig, Driver, SslMode},
    error::{AppError, AppResult},
};

const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BINARY_PREVIEW: usize = 2048;

pub enum Pool {
    Pg(PgPool),
    My(MySqlPool),
    Lite(SqlitePool),
}

pub struct Session {
    pub pool: Pool,
    pub driver: Driver,
    pub read_only: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: u64,
    pub truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    /// "table" | "view" | "materialized" | "foreign"
    pub kind: String,
    pub rows_estimate: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableStructure {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: String,
    pub desc: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchArgs {
    pub schema: String,
    pub table: String,
    pub filter: Option<String>,
    pub sort: Option<SortSpec>,
    pub limit: u32,
    pub offset: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePage {
    pub result: ResultSet,
    pub total: Option<i64>,
    pub sql: String,
}

#[derive(Deserialize)]
pub struct CellValue {
    pub column: String,
    pub value: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RowEdit {
    Update { key: Vec<CellValue>, values: Vec<CellValue> },
    Insert { values: Vec<CellValue> },
    Delete { key: Vec<CellValue> },
}

fn pg_ssl(m: SslMode) -> PgSslMode {
    match m {
        SslMode::Disable => PgSslMode::Disable,
        SslMode::Prefer => PgSslMode::Prefer,
        SslMode::Require => PgSslMode::Require,
        SslMode::VerifyFull => PgSslMode::VerifyFull,
    }
}

fn my_ssl(m: SslMode) -> MySqlSslMode {
    match m {
        SslMode::Disable => MySqlSslMode::Disabled,
        SslMode::Prefer => MySqlSslMode::Preferred,
        SslMode::Require => MySqlSslMode::Required,
        SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
    }
}

impl Session {
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> AppResult<Self> {
        let host = if cfg.host.trim().is_empty() { "localhost" } else { cfg.host.trim() };
        let port = cfg.port_or_default();
        let pool = match cfg.driver {
            Driver::Postgres => {
                let mut o = PgConnectOptions::new()
                    .host(host)
                    .port(port)
                    .username(&cfg.user)
                    .ssl_mode(pg_ssl(cfg.ssl_mode))
                    .application_name("Tabla");
                if !cfg.database.is_empty() {
                    o = o.database(&cfg.database);
                }
                if let Some(p) = &password {
                    o = o.password(p);
                }
                let mut po = PgPoolOptions::new().max_connections(4).acquire_timeout(ACQUIRE_TIMEOUT);
                if cfg.read_only {
                    po = po.after_connect(|conn, _| {
                        Box::pin(async move {
                            conn.execute("SET default_transaction_read_only = on").await?;
                            Ok(())
                        })
                    });
                }
                Pool::Pg(po.connect_with(o).await?)
            }
            Driver::Mysql => {
                let mut o = MySqlConnectOptions::new()
                    .host(host)
                    .port(port)
                    .username(&cfg.user)
                    .ssl_mode(my_ssl(cfg.ssl_mode));
                if !cfg.database.is_empty() {
                    o = o.database(&cfg.database);
                }
                if let Some(p) = &password {
                    o = o.password(p);
                }
                let mut po = MySqlPoolOptions::new().max_connections(4).acquire_timeout(ACQUIRE_TIMEOUT);
                if cfg.read_only {
                    po = po.after_connect(|conn, _| {
                        Box::pin(async move {
                            conn.execute("SET SESSION TRANSACTION READ ONLY").await?;
                            Ok(())
                        })
                    });
                }
                Pool::My(po.connect_with(o).await?)
            }
            Driver::Sqlite => {
                if cfg.database.trim().is_empty() {
                    return Err(AppError::msg("Choose a SQLite database file"));
                }
                let o = SqliteConnectOptions::new()
                    .filename(cfg.database.trim())
                    .create_if_missing(false)
                    .read_only(cfg.read_only)
                    .foreign_keys(true);
                Pool::Lite(SqlitePoolOptions::new().max_connections(2).connect_with(o).await?)
            }
        };
        Ok(Session { pool, driver: cfg.driver, read_only: cfg.read_only })
    }

    pub async fn close(&self) {
        match &self.pool {
            Pool::Pg(p) => p.close().await,
            Pool::My(p) => p.close().await,
            Pool::Lite(p) => p.close().await,
        }
    }

    /// Runs a (possibly multi-statement) script, returning one result set per statement.
    pub async fn run_script(&self, sql: &str, max_rows: usize) -> AppResult<Vec<ResultSet>> {
        let raw = || sqlx::raw_sql(AssertSqlSafe(sql.to_owned()));
        let mut sets = match &self.pool {
            Pool::Pg(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), pg_cell).await?,
            Pool::My(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), my_cell).await?,
            Pool::Lite(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), lite_cell).await?,
        };

        // A SELECT with zero rows carries no column metadata; ask the server.
        if sets.len() <= 1 && sets.first().is_none_or(|s| s.columns.is_empty()) && returns_rows(sql) {
            if let Some(columns) = self.describe(sql).await {
                if sets.is_empty() {
                    sets.push(ResultSet::default());
                }
                sets[0].columns = columns;
            }
        }
        Ok(sets)
    }

    async fn describe(&self, sql: &str) -> Option<Vec<ColumnMeta>> {
        fn meta<C: Column>(cols: &[C]) -> Vec<ColumnMeta> {
            cols.iter()
                .map(|c| ColumnMeta { name: c.name().to_string(), type_name: c.type_info().name().to_string() })
                .collect()
        }
        let s = AssertSqlSafe(sql.to_owned()).into_sql_str();
        let cols = match &self.pool {
            Pool::Pg(p) => meta(p.describe(s).await.ok()?.columns()),
            Pool::My(p) => meta(p.describe(s).await.ok()?.columns()),
            Pool::Lite(p) => meta(p.describe(s).await.ok()?.columns()),
        };
        (!cols.is_empty()).then_some(cols)
    }

    async fn text_rows(&self, sql: &str) -> AppResult<Vec<Vec<Option<String>>>> {
        Ok(self.run_script(sql, usize::MAX).await?.into_iter().next().map(|s| s.rows).unwrap_or_default())
    }

    pub async fn server_version(&self) -> AppResult<String> {
        let sql = match self.driver {
            Driver::Postgres => "SHOW server_version",
            Driver::Mysql => "SELECT VERSION()",
            Driver::Sqlite => "SELECT sqlite_version()",
        };
        let rows = self.text_rows(sql).await?;
        Ok(rows.into_iter().next().and_then(|r| r.into_iter().next().flatten()).unwrap_or_default())
    }

    pub async fn list_schemas(&self) -> AppResult<Vec<String>> {
        let sql = match self.driver {
            Driver::Postgres => {
                "SELECT nspname FROM pg_namespace WHERE nspname NOT IN ('pg_catalog','information_schema') \
                 AND nspname NOT LIKE 'pg_toast%' AND nspname NOT LIKE 'pg_temp%' ORDER BY nspname = 'public' DESC, nspname"
            }
            Driver::Mysql => {
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN \
                 ('information_schema','mysql','performance_schema','sys') ORDER BY SCHEMA_NAME"
            }
            Driver::Sqlite => "SELECT name FROM pragma_database_list ORDER BY seq",
        };
        Ok(self.text_rows(sql).await?.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect())
    }

    pub async fn current_schema(&self) -> AppResult<Option<String>> {
        let sql = match self.driver {
            Driver::Postgres => "SELECT current_schema()",
            Driver::Mysql => "SELECT DATABASE()",
            Driver::Sqlite => return Ok(Some("main".into())),
        };
        Ok(self.text_rows(sql).await?.into_iter().next().and_then(|r| r.into_iter().next().flatten()))
    }

    pub async fn list_tables(&self, schema: &str) -> AppResult<Vec<TableInfo>> {
        let d = self.driver;
        let lit = d.quote_literal(schema);
        let sql = match d {
            Driver::Postgres => format!(
                "SELECT c.relname, c.relkind, GREATEST(c.reltuples, 0)::bigint FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {lit} AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname"
            ),
            Driver::Mysql => format!(
                "SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = {lit} ORDER BY TABLE_NAME"
            ),
            Driver::Sqlite => format!(
                "SELECT name, type, NULL FROM {}.sqlite_master WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
                d.quote_ident(schema)
            ),
        };
        let rows = self.text_rows(&sql).await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let name = it.next().flatten()?;
                let raw_kind = it.next().flatten().unwrap_or_default();
                let kind = match raw_kind.as_str() {
                    "v" | "VIEW" | "SYSTEM VIEW" | "view" => "view",
                    "m" => "materialized",
                    "f" => "foreign",
                    _ => "table",
                };
                let rows_estimate = it.next().flatten().and_then(|v| v.parse().ok());
                Some(TableInfo { name, kind: kind.into(), rows_estimate })
            })
            .collect())
    }

    pub async fn table_structure(&self, schema: &str, table: &str) -> AppResult<TableStructure> {
        let d = self.driver;
        let (s, t) = (d.quote_literal(schema), d.quote_literal(table));
        let columns: Vec<ColumnInfo> = match d {
            Driver::Postgres => {
                let sql = format!(
                    "SELECT a.attname, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
                     pg_get_expr(ad.adbin, ad.adrelid), \
                     EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)) \
                     FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
                     WHERE n.nspname = {s} AND c.relname = {t} AND a.attnum > 0 AND NOT a.attisdropped \
                     ORDER BY a.attnum"
                );
                self.text_rows(&sql).await?.into_iter().map(|r| column_from(r, |v| v == "t", |v| v == "t")).collect()
            }
            Driver::Mysql => {
                let sql = format!(
                    "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY \
                     FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {s} AND TABLE_NAME = {t} \
                     ORDER BY ORDINAL_POSITION"
                );
                self.text_rows(&sql).await?.into_iter().map(|r| column_from(r, |v| v == "YES", |v| v == "PRI")).collect()
            }
            Driver::Sqlite => {
                let sql = format!(
                    "SELECT name, type, \"notnull\" = 0, dflt_value, pk > 0 FROM pragma_table_info({t}, {s})"
                );
                self.text_rows(&sql).await?.into_iter().map(|r| column_from(r, |v| v == "1", |v| v == "1")).collect()
            }
        };

        let index_sql = match d {
            Driver::Postgres => format!(
                "SELECT indexname, indexdef FROM pg_indexes WHERE schemaname = {s} AND tablename = {t} ORDER BY indexname"
            ),
            Driver::Mysql => format!(
                "SELECT INDEX_NAME, CONCAT(IF(NON_UNIQUE = 0, 'UNIQUE ', ''), INDEX_TYPE, ' (', \
                 GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', '), ')') \
                 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = {s} AND TABLE_NAME = {t} \
                 GROUP BY INDEX_NAME, NON_UNIQUE, INDEX_TYPE ORDER BY INDEX_NAME"
            ),
            Driver::Sqlite => format!(
                "SELECT name, COALESCE(sql, '(automatic index)') FROM {}.sqlite_master \
                 WHERE type = 'index' AND tbl_name = {t} ORDER BY name",
                d.quote_ident(schema)
            ),
        };
        let indexes = self
            .text_rows(&index_sql)
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                Some(IndexInfo { name: it.next().flatten()?, definition: it.next().flatten().unwrap_or_default() })
            })
            .collect();

        Ok(TableStructure { columns, indexes })
    }

    pub async fn fetch_rows(&self, a: &FetchArgs) -> AppResult<TablePage> {
        let d = self.driver;
        let from = d.qualified(&a.schema, &a.table);
        let filter = a.filter.as_deref().map(str::trim).filter(|f| !f.is_empty());
        let where_sql = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
        let order_sql = a
            .sort
            .as_ref()
            .map(|s| format!(" ORDER BY {} {}", d.quote_ident(&s.column), if s.desc { "DESC" } else { "ASC" }))
            .unwrap_or_default();
        let sql = format!("SELECT * FROM {from}{where_sql}{order_sql} LIMIT {} OFFSET {}", a.limit, a.offset);

        let mut sets = self.run_script(&sql, a.limit as usize).await?;
        let mut result = if sets.is_empty() { ResultSet::default() } else { sets.swap_remove(0) };
        if result.columns.is_empty() {
            // Empty table: still show its columns.
            let st = self.table_structure(&a.schema, &a.table).await?;
            result.columns = st
                .columns
                .into_iter()
                .map(|c| ColumnMeta { name: c.name, type_name: c.data_type })
                .collect();
        }
        let count_sql = format!("SELECT COUNT(*) FROM {from}{where_sql}");
        let total = self
            .text_rows(&count_sql)
            .await?
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .and_then(|v| v.parse().ok());
        Ok(TablePage { result, total, sql })
    }

    /// Applies cell edits, inserts and deletes in a single transaction.
    /// Every UPDATE/DELETE must hit exactly one row or the whole batch rolls back.
    pub async fn apply_edits(&self, schema: &str, table: &str, edits: Vec<RowEdit>) -> AppResult<u64> {
        if self.read_only {
            return Err(AppError::msg("This connection is read-only"));
        }
        let d = self.driver;
        let from = d.qualified(schema, table);

        // Postgres needs explicit casts because parameters are bound as text.
        let types: HashMap<String, String> = if d == Driver::Postgres {
            self.table_structure(schema, table).await?.columns.into_iter().map(|c| (c.name, c.data_type)).collect()
        } else {
            HashMap::new()
        };
        let mut stmts: Vec<(String, Vec<Option<String>>, bool)> = Vec::new();
        for edit in edits {
            let mut ph = Placeholders { driver: d, types: &types, n: 0 };
            match edit {
                RowEdit::Update { key, values } => {
                    if values.is_empty() {
                        continue;
                    }
                    if key.is_empty() {
                        return Err(AppError::msg("Editing requires a primary key"));
                    }
                    let mut binds = Vec::new();
                    let set = values
                        .into_iter()
                        .map(|v| {
                            let s = format!("{} = {}", d.quote_ident(&v.column), ph.next(&v.column));
                            binds.push(v.value);
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let cond = where_key(d, key, &mut binds, &mut ph);
                    stmts.push((format!("UPDATE {from} SET {set} WHERE {cond}"), binds, true));
                }
                RowEdit::Insert { values } => {
                    let sql = if values.is_empty() {
                        match d {
                            Driver::Mysql => format!("INSERT INTO {from} () VALUES ()"),
                            _ => format!("INSERT INTO {from} DEFAULT VALUES"),
                        }
                    } else {
                        let cols = values.iter().map(|v| d.quote_ident(&v.column)).collect::<Vec<_>>().join(", ");
                        let phs = values.iter().map(|v| ph.next(&v.column)).collect::<Vec<_>>().join(", ");
                        format!("INSERT INTO {from} ({cols}) VALUES ({phs})")
                    };
                    stmts.push((sql, values.into_iter().map(|v| v.value).collect(), false));
                }
                RowEdit::Delete { key } => {
                    if key.is_empty() {
                        return Err(AppError::msg("Deleting requires a primary key"));
                    }
                    let mut binds = Vec::new();
                    let cond = where_key(d, key, &mut binds, &mut ph);
                    stmts.push((format!("DELETE FROM {from} WHERE {cond}"), binds, true));
                }
            }
        }

        macro_rules! exec_tx {
            ($pool:expr) => {{
                let mut tx = $pool.begin().await?;
                let mut total = 0u64;
                for (sql, binds, single) in stmts {
                    let mut q = sqlx::query(AssertSqlSafe(sql));
                    for b in binds {
                        q = q.bind(b);
                    }
                    let affected = q.execute(&mut *tx).await?.rows_affected();
                    if single && affected != 1 {
                        // Dropping `tx` rolls everything back.
                        return Err(AppError::msg(format!(
                            "Expected to change 1 row but {affected} matched — nothing was saved. \
                             The data may have changed; refresh and try again."
                        )));
                    }
                    total += affected;
                }
                tx.commit().await?;
                total
            }};
        }
        Ok(match &self.pool {
            Pool::Pg(p) => exec_tx!(p),
            Pool::My(p) => exec_tx!(p),
            Pool::Lite(p) => exec_tx!(p),
        })
    }
}

struct Placeholders<'a> {
    driver: Driver,
    types: &'a HashMap<String, String>,
    n: usize,
}

impl Placeholders<'_> {
    fn next(&mut self, col: &str) -> String {
        self.n += 1;
        match self.driver {
            Driver::Postgres => {
                let ty = self.types.get(col).map(String::as_str).unwrap_or("text");
                format!("CAST(CAST(${} AS text) AS {ty})", self.n)
            }
            _ => "?".into(),
        }
    }
}

fn where_key(d: Driver, key: Vec<CellValue>, binds: &mut Vec<Option<String>>, ph: &mut Placeholders) -> String {
    key.into_iter()
        .map(|k| match k.value {
            None => format!("{} IS NULL", d.quote_ident(&k.column)),
            Some(v) => {
                let s = format!("{} = {}", d.quote_ident(&k.column), ph.next(&k.column));
                binds.push(Some(v));
                s
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn column_from(r: Vec<Option<String>>, nullable: impl Fn(&str) -> bool, pk: impl Fn(&str) -> bool) -> ColumnInfo {
    let mut it = r.into_iter();
    ColumnInfo {
        name: it.next().flatten().unwrap_or_default(),
        data_type: it.next().flatten().unwrap_or_default(),
        nullable: it.next().flatten().is_some_and(|v| nullable(&v)),
        default_value: it.next().flatten(),
        primary_key: it.next().flatten().is_some_and(|v| pk(&v)),
    }
}

fn returns_rows(sql: &str) -> bool {
    let head = sql.trim_start().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    matches!(head.as_str(), "select" | "with" | "show" | "values" | "table" | "pragma" | "explain" | "describe" | "desc")
}

async fn collect<Q, R: Row>(
    mut stream: BoxStream<'_, Result<Either<Q, R>, sqlx::Error>>,
    max_rows: usize,
    affected: fn(&Q) -> u64,
    cell: fn(&R, usize) -> Option<String>,
) -> AppResult<Vec<ResultSet>> {
    let mut sets = Vec::new();
    let mut cur = ResultSet::default();
    while let Some(item) = stream.try_next().await? {
        match item {
            Either::Left(q) => {
                cur.rows_affected = affected(&q);
                sets.push(std::mem::take(&mut cur));
            }
            Either::Right(row) => {
                if cur.columns.is_empty() {
                    cur.columns = row
                        .columns()
                        .iter()
                        .map(|c| ColumnMeta { name: c.name().to_string(), type_name: c.type_info().name().to_string() })
                        .collect();
                }
                if cur.rows.len() >= max_rows {
                    cur.truncated = true;
                    break;
                }
                cur.rows.push((0..row.len()).map(|i| cell(&row, i)).collect());
            }
        }
    }
    if !cur.columns.is_empty() {
        sets.push(cur);
    }
    Ok(sets)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len().min(MAX_BINARY_PREVIEW) * 2);
    s.push_str("0x");
    for b in bytes.iter().take(MAX_BINARY_PREVIEW) {
        s.push_str(&format!("{b:02x}"));
    }
    if bytes.len() > MAX_BINARY_PREVIEW {
        s.push('…');
    }
    s
}

// Text protocol: every value arrives as text, so decode as a string unchecked.
fn pg_cell(row: &PgRow, i: usize) -> Option<String> {
    row.try_get_unchecked::<Option<String>, _>(i).unwrap_or_else(|_| Some("<unreadable>".into()))
}

fn my_cell(row: &MySqlRow, i: usize) -> Option<String> {
    match row.try_get_unchecked::<Option<String>, _>(i) {
        Ok(v) => v,
        Err(_) => row.try_get_unchecked::<Option<Vec<u8>>, _>(i).ok().flatten().map(|b| hex(&b)),
    }
}

fn lite_cell(row: &SqliteRow, i: usize) -> Option<String> {
    let kind = {
        let raw = row.try_get_raw(i).ok()?;
        if raw.is_null() {
            return None;
        }
        raw.type_info().name().to_string()
    };
    match kind.as_str() {
        "INTEGER" => row.try_get_unchecked::<i64, _>(i).ok().map(|v| v.to_string()),
        "REAL" => row.try_get_unchecked::<f64, _>(i).ok().map(|v| v.to_string()),
        "BLOB" => row.try_get_unchecked::<Vec<u8>, _>(i).ok().map(|b| hex(&b)),
        _ => row
            .try_get_unchecked::<String, _>(i)
            .ok()
            .or_else(|| row.try_get_unchecked::<Vec<u8>, _>(i).ok().map(|b| hex(&b))),
    }
}

#[cfg(test)]
mod tests {
    //! Integration tests against real servers. Each test is skipped unless its
    //! env var is set, e.g.:
    //!   TABLA_TEST_PG="localhost:5432:user:tabla_test"
    //!   TABLA_TEST_MYSQL="localhost:3306:root:tabla_test"
    //!   TABLA_TEST_SQLITE="/path/to/tabla_test.db"
    //! See tests/fixtures/seed.md for the expected schema.
    use super::*;

    fn cfg_from_env(var: &str, driver: Driver) -> Option<ConnectionConfig> {
        let v = std::env::var(var).ok()?;
        let mut cfg = ConnectionConfig {
            id: "test".into(),
            name: "test".into(),
            driver,
            provider: String::new(),
            host: String::new(),
            port: 0,
            user: String::new(),
            password: None,
            database: String::new(),
            ssl_mode: SslMode::Disable,
            color: String::new(),
            tag: String::new(),
            read_only: false,
        };
        if driver == Driver::Sqlite {
            cfg.database = v;
        } else {
            let p: Vec<&str> = v.split(':').collect();
            cfg.host = p[0].into();
            cfg.port = p[1].parse().unwrap();
            cfg.user = p[2].into();
            cfg.database = p[3].into();
        }
        Some(cfg)
    }

    async fn roundtrip(s: &Session, schema: &str, table: &str, edit_col: &str) {
        let schemas = s.list_schemas().await.unwrap();
        assert!(schemas.iter().any(|x| x == schema), "schemas: {schemas:?}");
        let tables = s.list_tables(schema).await.unwrap();
        assert!(tables.iter().any(|t| t.name == table));

        let st = s.table_structure(schema, table).await.unwrap();
        let pk = st.columns.iter().find(|c| c.primary_key).expect("pk").name.clone();

        let page = s
            .fetch_rows(&FetchArgs {
                schema: schema.into(),
                table: table.into(),
                filter: None,
                sort: Some(SortSpec { column: pk.clone(), desc: false }),
                limit: 2,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(page.result.rows.len(), 2);
        assert!(page.total.unwrap() >= 2);
        let pk_idx = page.result.columns.iter().position(|c| c.name == pk).unwrap();
        let id = page.result.rows[0][pk_idx].clone();

        let n = s
            .apply_edits(
                schema,
                table,
                vec![RowEdit::Update {
                    key: vec![CellValue { column: pk.clone(), value: id.clone() }],
                    values: vec![CellValue { column: edit_col.into(), value: Some("Edited by Tabla test".into()) }],
                }],
            )
            .await
            .unwrap();
        assert_eq!(n, 1);

        // A key that matches nothing must roll back with an error.
        let err = s
            .apply_edits(
                schema,
                table,
                vec![RowEdit::Delete { key: vec![CellValue { column: pk, value: Some("-999".into()) }] }],
            )
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn postgres() {
        let Some(cfg) = cfg_from_env("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        assert!(!s.server_version().await.unwrap().is_empty());
        roundtrip(&s, "public", "users", "name").await;

        // Multi-statement script, empty result, and odd types decode as text.
        let sets = s
            .run_script("SELECT profile, tags, feeling, avatar, balance FROM users LIMIT 1; SELECT * FROM empty_t; UPDATE users SET age = age WHERE id < 3", 100)
            .await
            .unwrap();
        assert_eq!(sets.len(), 3);
        assert!(sets[0].rows[0][3].as_deref().unwrap().starts_with("\\x"));
        // Mid-script empty SELECTs carry no column metadata (sqlx only exposes it with rows).
        assert!(sets[1].rows.is_empty());
        assert_eq!(sets[2].rows_affected, 2);

        let empty = s.run_script("SELECT * FROM empty_t", 100).await.unwrap();
        assert_eq!(empty[0].columns.len(), 2);
    }

    #[tokio::test]
    async fn mysql() {
        let Some(cfg) = cfg_from_env("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        roundtrip(&s, "tabla_test", "products", "title").await;
        let sets = s.run_script("SELECT meta, img, price FROM products ORDER BY id", 100).await.unwrap();
        assert_eq!(sets[0].rows.len(), 3);
        assert!(sets[0].rows[1][1].is_none());
    }

    #[tokio::test]
    async fn sqlite() {
        let Some(cfg) = cfg_from_env("TABLA_TEST_SQLITE", Driver::Sqlite) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        roundtrip(&s, "main", "notes", "title").await;
        let sets = s.run_script("SELECT data, score FROM notes ORDER BY id", 100).await.unwrap();
        assert_eq!(sets[0].rows[0][0].as_deref(), Some("0x0102"));
    }

    #[tokio::test]
    async fn read_only_rejects_edits() {
        let Some(mut cfg) = cfg_from_env("TABLA_TEST_SQLITE", Driver::Sqlite) else { return };
        cfg.read_only = true;
        let s = Session::connect(&cfg, None).await.unwrap();
        assert!(s.apply_edits("main", "notes", vec![RowEdit::Insert { values: vec![] }]).await.is_err());
        assert!(s.run_script("DELETE FROM notes", 10).await.is_err());
    }
}
