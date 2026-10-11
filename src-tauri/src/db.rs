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
    Ms(crate::mssql::MsPool),
}

/// One statement Tabla sent to the server (Console view). Memory only.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    /// Unix time in milliseconds.
    pub at: u128,
    pub sql: String,
    pub ms: u128,
    pub ok: bool,
    /// Rows returned/affected, or the error message.
    pub info: String,
}

const LOG_CAP: usize = 500;
const LOG_SQL_MAX: usize = 4000;

enum Cancel {
    Pg(String),
    My(String),
    Ms(String),
}

pub struct Session {
    pub pool: Pool,
    pub driver: Driver,
    pub read_only: bool,
    log: std::sync::Mutex<std::collections::VecDeque<LogEntry>>,
    /// Server-side handles for queries that can be cancelled, by query id.
    running: std::sync::Mutex<HashMap<String, Cancel>>,
    /// Kept alive for the lifetime of the pool when connecting through SSH.
    _tunnel: Option<crate::ssh::Tunnel>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
}

#[derive(Serialize, Default, Debug)]
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
    /// "PRI" | "UNI" | "MUL" | "" (Sequel Ace style key column).
    pub key: String,
    /// e.g. "auto increment", "identity", "generated", "on update CURRENT_TIMESTAMP".
    pub extra: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub definition: String,
    pub unique: bool,
    pub primary: bool,
    pub method: String,
    pub columns: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_update: String,
    pub on_delete: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInfo {
    pub name: String,
    /// BEFORE | AFTER | INSTEAD OF
    pub timing: String,
    /// INSERT, UPDATE, DELETE (comma separated)
    pub event: String,
    pub enabled: bool,
    pub definition: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRelations {
    /// Keys on this table pointing elsewhere.
    pub outgoing: Vec<ForeignKey>,
    /// Keys on other tables pointing at this one.
    pub incoming: Vec<ForeignKey>,
    pub triggers: Vec<TriggerInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphColumn {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphTable {
    pub name: String,
    pub kind: String,
    pub columns: Vec<GraphColumn>,
}

/// Everything the ER diagram needs for one schema.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaGraph {
    pub tables: Vec<GraphTable>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TableDetails {
    pub rows_estimate: Option<i64>,
    /// Table + indexes, or the whole database file for SQLite.
    pub total_bytes: Option<i64>,
    pub engine: Option<String>,
    pub collation: Option<String>,
    pub comment: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub ddl: Option<String>,
    /// Engine-specific figures shown in the Info view, e.g. ("Index size", "3670016", "bytes").
    pub stats: Vec<Stat>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub label: String,
    pub value: String,
    /// "bytes" | "number" | "text"
    pub kind: String,
}

fn stat(label: &str, value: Option<String>, kind: &str) -> Option<Stat> {
    value.filter(|v| !v.is_empty()).map(|value| Stat { label: label.into(), value, kind: kind.into() })
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
        let db_host = if cfg.host.trim().is_empty() { "localhost" } else { cfg.host.trim() };
        let db_port = cfg.port_or_default();
        // Through SSH, the DB host/port are resolved on the SSH server and we
        // connect to the local end of the tunnel instead.
        let tunnel = if cfg.ssh.enabled && cfg.driver != Driver::Sqlite {
            let secret = crate::config::resolve_ssh_secret(cfg)?;
            Some(crate::ssh::open(&cfg.ssh, secret, db_host, db_port).await?)
        } else {
            None
        };
        let (host, port) = match &tunnel {
            Some(t) => ("127.0.0.1", t.local_port),
            None => (db_host, db_port),
        };
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
                if !cfg.ssl_ca.is_empty() {
                    o = o.ssl_root_cert(&cfg.ssl_ca);
                }
                if !cfg.ssl_cert.is_empty() {
                    o = o.ssl_client_cert(&cfg.ssl_cert);
                }
                if !cfg.ssl_key.is_empty() {
                    o = o.ssl_client_key(&cfg.ssl_key);
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
                if !cfg.ssl_ca.is_empty() {
                    o = o.ssl_ca(&cfg.ssl_ca);
                }
                if !cfg.ssl_cert.is_empty() {
                    o = o.ssl_client_cert(&cfg.ssl_cert);
                }
                if !cfg.ssl_key.is_empty() {
                    o = o.ssl_client_key(&cfg.ssl_key);
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
            Driver::Mssql => Pool::Ms(crate::mssql::MsPool::connect(crate::mssql::build_config(cfg, host, port, db_host, password.clone())).await?),
            Driver::Redis => return Err(crate::error::not_sql()),
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
        Ok(Session { pool, driver: cfg.driver, read_only: cfg.read_only, log: Default::default(), running: Default::default(), _tunnel: tunnel })
    }

    pub async fn close(&self) {
        match &self.pool {
            Pool::Pg(p) => p.close().await,
            Pool::My(p) => p.close().await,
            Pool::Lite(p) => p.close().await,
            Pool::Ms(p) => p.close().await,
        }
    }

    pub fn record(&self, sql: &str, started: std::time::Instant, outcome: Result<String, String>) {
        let mut log = self.log.lock().unwrap();
        let id = log.back().map(|e| e.id + 1).unwrap_or(1);
        let mut text = redact_secrets(sql.trim());
        if text.len() > LOG_SQL_MAX {
            let cut = (0..=LOG_SQL_MAX).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
            text.truncate(cut);
            text.push('…');
        }
        let (ok, info) = match outcome {
            Ok(i) => (true, i),
            Err(e) => (false, e),
        };
        log.push_back(LogEntry {
            id,
            at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
            sql: text,
            ms: started.elapsed().as_millis(),
            ok,
            info,
        });
        while log.len() > LOG_CAP {
            log.pop_front();
        }
    }

    pub fn log_since(&self, after: u64) -> Vec<LogEntry> {
        self.log.lock().unwrap().iter().filter(|e| e.id > after).cloned().collect()
    }

    pub fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }

    /// Runs a (possibly multi-statement) script, returning one result set per statement.
    pub async fn run_script(&self, sql: &str, max_rows: usize) -> AppResult<Vec<ResultSet>> {
        let started = std::time::Instant::now();
        let res = self.run_script_inner(sql, max_rows).await;
        let outcome = match &res {
            Ok(sets) => Ok(sets
                .iter()
                .map(|s| if s.columns.is_empty() { format!("{} affected", s.rows_affected) } else { format!("{} rows", s.rows.len()) })
                .collect::<Vec<_>>()
                .join(", ")),
            Err(e) => Err(e.to_string()),
        };
        self.record(sql, started, outcome);
        res
    }

    async fn run_script_inner(&self, sql: &str, max_rows: usize) -> AppResult<Vec<ResultSet>> {
        let raw = || sqlx::raw_sql(AssertSqlSafe(sql.to_owned()));
        let mut sets = match &self.pool {
            Pool::Pg(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), pg_cell).await?,
            Pool::My(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), my_cell).await?,
            Pool::Lite(p) => collect(raw().fetch_many(p), max_rows, |q| q.rows_affected(), lite_cell).await?,
            Pool::Ms(p) => {
                let mut conn = p.acquire().await?;
                self.ms_script(&mut conn, sql, max_rows, None).await?
            }
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

    /// Runs a script on a dedicated connection whose server-side id is
    /// registered under `query_id`, so `cancel` can stop it on the server.
    pub async fn run_cancellable(&self, sql: &str, max_rows: usize, query_id: &str, progress: Progress<'_>) -> AppResult<Vec<ResultSet>> {
        let started = std::time::Instant::now();
        let raw = || sqlx::raw_sql(AssertSqlSafe(sql.to_owned()));
        let first = |rows: Vec<PgRow>| rows.first().and_then(|r| r.try_get_unchecked::<Option<String>, _>(0).ok().flatten());
        let res: AppResult<Vec<ResultSet>> = async {
            Ok(match &self.pool {
                Pool::Pg(p) => {
                    let mut conn = p.acquire().await?;
                    let pid = first(sqlx::raw_sql("SELECT pg_backend_pid()::text").fetch_all(&mut *conn).await?).unwrap_or_default();
                    self.running.lock().unwrap().insert(query_id.into(), Cancel::Pg(pid));
                    let out = collect_with(raw().fetch_many(&mut *conn), max_rows, |q| q.rows_affected(), pg_cell, progress).await;
                    self.running.lock().unwrap().remove(query_id);
                    out?
                }
                Pool::My(p) => {
                    let mut conn = p.acquire().await?;
                    let rows = sqlx::raw_sql("SELECT CONNECTION_ID()").fetch_all(&mut *conn).await?;
                    let id = rows.first().and_then(|r| r.try_get_unchecked::<Option<String>, _>(0).ok().flatten()).unwrap_or_default();
                    self.running.lock().unwrap().insert(query_id.into(), Cancel::My(id));
                    let out = collect_with(raw().fetch_many(&mut *conn), max_rows, |q| q.rows_affected(), my_cell, progress).await;
                    self.running.lock().unwrap().remove(query_id);
                    out?
                }
                Pool::Ms(p) => {
                    let mut conn = p.acquire().await?;
                    let spid = crate::mssql::run(&mut conn, "SELECT CAST(@@SPID AS varchar(10))", 1, None).await?;
                    let spid = spid.first().and_then(|s| s.rows.first()).and_then(|r| r.first().cloned().flatten()).unwrap_or_default();
                    self.running.lock().unwrap().insert(query_id.into(), Cancel::Ms(spid));
                    let out = self.ms_script(&mut conn, sql, max_rows, progress).await;
                    // `cancel` removes the entry when it kills the session.
                    let killed = self.running.lock().unwrap().remove(query_id).is_none();
                    if killed {
                        conn.discard();
                        return Err(AppError::msg("Query cancelled"));
                    }
                    out?
                }
                // SQLite runs in-process; queries are local and cannot be interrupted from here.
                Pool::Lite(p) => collect_with(raw().fetch_many(p), max_rows, |q| q.rows_affected(), lite_cell, progress).await?,
            })
        }
        .await;
        let mut res = res.map_err(|e| {
            let m = e.to_string().to_lowercase();
            if m.contains("canceling statement due to user request") || m.contains("query execution was interrupted") {
                AppError::msg("Query cancelled")
            } else {
                e
            }
        });
        if let Ok(sets) = &mut res {
            if sets.len() <= 1 && sets.first().is_none_or(|s| s.columns.is_empty()) && returns_rows(sql) {
                if let Some(columns) = self.describe(sql).await {
                    if sets.is_empty() {
                        sets.push(ResultSet::default());
                    }
                    sets[0].columns = columns;
                }
            }
        }
        let outcome = match &res {
            Ok(sets) => Ok(sets.iter().map(|s| if s.columns.is_empty() { format!("{} affected", s.rows_affected) } else { format!("{} rows", s.rows.len()) }).collect::<Vec<_>>().join(", ")),
            Err(e) => Err(e.to_string()),
        };
        self.record(sql, started, outcome);
        res
    }

    /// Asks the server to stop a running query. Returns false if it already finished.
    pub async fn cancel(&self, query_id: &str) -> AppResult<bool> {
        let handle = {
            let mut running = self.running.lock().unwrap();
            match running.get(query_id) {
                Some(Cancel::Pg(pid)) => Some(format!("SELECT pg_cancel_backend({})", pid.parse::<i64>().unwrap_or(0))),
                Some(Cancel::My(id)) => Some(format!("KILL QUERY {}", id.parse::<u64>().unwrap_or(0))),
                // SQL Server can only stop a query by ending its session. Unregister
                // first so the dying query reports "Query cancelled".
                Some(Cancel::Ms(id)) => {
                    let sql = format!("KILL {}", id.parse::<u64>().unwrap_or(0));
                    running.remove(query_id);
                    Some(sql)
                }
                None => None,
            }
        };
        let Some(sql) = handle else { return Ok(false) };
        match &self.pool {
            Pool::Pg(p) => {
                p.execute(AssertSqlSafe(sql)).await?;
            }
            Pool::My(p) => {
                p.execute(AssertSqlSafe(sql)).await?;
            }
            Pool::Lite(_) => return Ok(false),
            Pool::Ms(p) => {
                let mut conn = p.acquire().await?;
                crate::mssql::run(&mut conn, &sql, 0, None).await?;
            }
        }
        Ok(true)
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
            // SQL Server sends column metadata even for empty results.
            Pool::Ms(_) => return None,
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
            Driver::Mssql => "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)) + N' ' + CAST(SERVERPROPERTY('Edition') AS nvarchar(128))",
            Driver::Redis => return Err(crate::error::not_sql()),
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
            Driver::Mssql => {
                "SELECT name FROM sys.schemas WHERE name NOT IN ('sys','INFORMATION_SCHEMA','guest') AND name NOT LIKE 'db[_]%' \
                 ORDER BY CASE WHEN name = 'dbo' THEN 0 ELSE 1 END, name"
            }
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => "SELECT name FROM pragma_database_list ORDER BY seq",
        };
        Ok(self.text_rows(sql).await?.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect())
    }

    pub async fn current_schema(&self) -> AppResult<Option<String>> {
        let sql = match self.driver {
            Driver::Postgres => "SELECT current_schema()",
            Driver::Mysql => "SELECT DATABASE()",
            Driver::Mssql => "SELECT SCHEMA_NAME()",
            Driver::Redis => return Err(crate::error::not_sql()),
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
            Driver::Mssql => format!(
                "SELECT o.name, RTRIM(o.type), (SELECT SUM(p.rows) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) \
                 FROM sys.objects o WHERE o.schema_id = SCHEMA_ID({lit}) AND o.type IN ('U', 'V') ORDER BY o.name"
            ),
            Driver::Redis => return Err(crate::error::not_sql()),
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
                    "v" | "V" | "VIEW" | "SYSTEM VIEW" | "view" => "view",
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
                     EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)), \
                     CASE WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)) THEN 'PRI' \
                          WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisunique AND i.indnatts = 1 AND i.indkey[0] = a.attnum) THEN 'UNI' \
                          WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND a.attnum = ANY(i.indkey)) THEN 'MUL' ELSE '' END, \
                     CASE WHEN (to_jsonb(a)->>'attidentity') IN ('a','d') THEN 'identity' \
                          WHEN (to_jsonb(a)->>'attgenerated') = 's' THEN 'generated' \
                          WHEN pg_get_expr(ad.adbin, ad.adrelid) LIKE 'nextval(%' THEN 'auto increment' ELSE '' END \
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
                    "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
                     CASE WHEN COLUMN_DEFAULT IS NULL THEN NULL \
                          WHEN EXTRA LIKE '%DEFAULT_GENERATED%' OR DATA_TYPE IN \
                            ('tinyint','smallint','mediumint','int','bigint','decimal','float','double','bit','year') \
                          THEN COLUMN_DEFAULT ELSE QUOTE(COLUMN_DEFAULT) END, \
                     COLUMN_KEY, COLUMN_KEY, TRIM(REPLACE(LOWER(EXTRA), 'default_generated', '')) \
                     FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {s} AND TABLE_NAME = {t} \
                     ORDER BY ORDINAL_POSITION"
                );
                self.text_rows(&sql).await?.into_iter().map(|r| column_from(r, |v| v == "YES", |v| v == "PRI")).collect()
            }
            Driver::Mssql => {
                let obj = d.quote_literal(&d.qualified(schema, table));
                let sql = format!(
                    "SELECT c.name, {MS_TYPE}, CASE WHEN c.is_nullable = 1 THEN '1' ELSE '0' END, dc.definition, \
                     CASE WHEN pk.column_id IS NULL THEN '0' ELSE '1' END, \
                     CASE WHEN pk.column_id IS NOT NULL THEN 'PRI' WHEN uq.column_id IS NOT NULL THEN 'UNI' \
                          WHEN ix.column_id IS NOT NULL THEN 'MUL' ELSE '' END, \
                     CASE WHEN c.is_identity = 1 THEN 'identity' WHEN c.is_computed = 1 THEN 'generated' ELSE '' END \
                     FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id \
                     LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id \
                     OUTER APPLY (SELECT TOP 1 ic.column_id FROM sys.indexes i JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                       WHERE i.object_id = c.object_id AND i.is_primary_key = 1 AND ic.column_id = c.column_id) pk \
                     OUTER APPLY (SELECT TOP 1 ic.column_id FROM sys.indexes i JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                       WHERE i.object_id = c.object_id AND i.is_unique = 1 AND ic.column_id = c.column_id \
                       AND (SELECT COUNT(*) FROM sys.index_columns x WHERE x.object_id = i.object_id AND x.index_id = i.index_id AND x.is_included_column = 0) = 1) uq \
                     OUTER APPLY (SELECT TOP 1 ic.column_id FROM sys.index_columns ic WHERE ic.object_id = c.object_id AND ic.column_id = c.column_id) ix \
                     WHERE c.object_id = OBJECT_ID({obj}) ORDER BY c.column_id"
                );
                self.text_rows(&sql)
                    .await?
                    .into_iter()
                    .map(|r| {
                        let mut c = column_from(r, |v| v == "1", |v| v == "1");
                        c.default_value = c.default_value.map(|v| unwrap_parens(&v).to_string());
                        c
                    })
                    .collect()
            }
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => {
                let sql = format!(
                    "SELECT name, type, \"notnull\" = 0, dflt_value, pk > 0, CASE WHEN pk > 0 THEN 'PRI' ELSE '' END, \
                     CASE WHEN pk > 0 AND upper(type) = 'INTEGER' THEN 'auto increment' ELSE '' END \
                     FROM pragma_table_info({t}, {s})"
                );
                self.text_rows(&sql).await?.into_iter().map(|r| column_from(r, |v| v == "1", |v| v == "1")).collect()
            }
        };

        let index_sql = match d {
            Driver::Postgres => format!(
                "SELECT ci.relname, pg_get_indexdef(ix.indexrelid), ix.indisunique, ix.indisprimary, am.amname, \
                 (SELECT string_agg(pg_get_indexdef(ix.indexrelid, k, true), ', ' ORDER BY k) FROM generate_series(1, ix.indnatts) k) \
                 FROM pg_index ix JOIN pg_class ci ON ci.oid = ix.indexrelid JOIN pg_class ct ON ct.oid = ix.indrelid \
                 JOIN pg_namespace n ON n.oid = ct.relnamespace JOIN pg_am am ON am.oid = ci.relam \
                 WHERE n.nspname = {s} AND ct.relname = {t} ORDER BY ix.indisprimary DESC, ci.relname"
            ),
            Driver::Mysql => format!(
                "SELECT INDEX_NAME, CONCAT(IF(NON_UNIQUE = 0, 'UNIQUE ', ''), INDEX_TYPE, ' (', \
                 GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', '), ')'), IF(NON_UNIQUE = 0, '1', '0'), \
                 IF(INDEX_NAME = 'PRIMARY', '1', '0'), LOWER(INDEX_TYPE), GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', ') \
                 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = {s} AND TABLE_NAME = {t} \
                 GROUP BY INDEX_NAME, NON_UNIQUE, INDEX_TYPE ORDER BY INDEX_NAME = 'PRIMARY' DESC, INDEX_NAME"
            ),
            Driver::Mssql => format!(
                "SELECT i.name, CASE WHEN i.is_primary_key = 1 THEN 'PRIMARY KEY ' WHEN i.is_unique = 1 THEN 'UNIQUE ' ELSE '' END \
                   + i.type_desc + ' (' + x.cols + ')', CASE WHEN i.is_unique = 1 THEN '1' ELSE '0' END, \
                 CASE WHEN i.is_primary_key = 1 THEN '1' ELSE '0' END, LOWER(i.type_desc), x.cols \
                 FROM sys.indexes i CROSS APPLY (SELECT STRING_AGG(COL_NAME(ic.object_id, ic.column_id), ', ') WITHIN GROUP (ORDER BY ic.key_ordinal) AS cols \
                   FROM sys.index_columns ic WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0) x \
                 WHERE i.object_id = OBJECT_ID({}) AND i.type > 0 ORDER BY i.is_primary_key DESC, i.name",
                d.quote_literal(&d.qualified(schema, table))
            ),
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => format!(
                "SELECT il.name, COALESCE(m.sql, '(automatic index)'), il.\"unique\", il.origin = 'pk', 'btree', \
                 (SELECT group_concat(ii.name, ', ') FROM pragma_index_info(il.name, {s}) ii) \
                 FROM pragma_index_list({t}, {s}) il LEFT JOIN {}.sqlite_master m ON m.name = il.name ORDER BY il.name",
                d.quote_ident(schema)
            ),
        };
        let truthy = |v: Option<String>| matches!(v.as_deref(), Some("t" | "1" | "true"));
        let indexes = self
            .text_rows(&index_sql)
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                Some(IndexInfo {
                    name: it.next().flatten()?,
                    definition: it.next().flatten().unwrap_or_default(),
                    unique: truthy(it.next().flatten()),
                    primary: truthy(it.next().flatten()),
                    method: it.next().flatten().unwrap_or_default(),
                    columns: it.next().flatten().unwrap_or_default(),
                })
            })
            .collect();

        Ok(TableStructure { columns, indexes })
    }

    pub async fn schema_graph(&self, schema: &str) -> AppResult<SchemaGraph> {
        let d = self.driver;
        let s = d.quote_literal(schema);
        let tables = self.list_tables(schema).await?;
        let mut by_table: HashMap<String, Vec<GraphColumn>> = HashMap::new();
        let col_sql = match d {
            Driver::Postgres => Some(format!(
                "SELECT c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
                 EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)) \
                 FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {s} AND c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY c.relname, a.attnum"
            )),
            Driver::Mysql => Some(format!(
                "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IF(COLUMN_KEY = 'PRI', '1', '0') FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = {s} ORDER BY TABLE_NAME, ORDINAL_POSITION"
            )),
            Driver::Mssql => Some(format!(
                "SELECT o.name, c.name, {MS_TYPE}, CASE WHEN EXISTS (SELECT 1 FROM sys.indexes i JOIN sys.index_columns ic \
                   ON ic.object_id = i.object_id AND ic.index_id = i.index_id WHERE i.object_id = c.object_id AND i.is_primary_key = 1 \
                   AND ic.column_id = c.column_id) THEN '1' ELSE '0' END \
                 FROM sys.columns c JOIN sys.objects o ON o.object_id = c.object_id JOIN sys.types t ON t.user_type_id = c.user_type_id \
                 WHERE o.schema_id = SCHEMA_ID({s}) AND o.type IN ('U', 'V') ORDER BY o.name, c.column_id"
            )),
            Driver::Sqlite => None,
            Driver::Redis => return Err(crate::error::not_sql()),
        };
        match col_sql {
            Some(sql) => {
                for r in self.text_rows(&sql).await? {
                    let mut it = r.into_iter();
                    let mut n = || it.next().flatten();
                    let (Some(t), Some(c)) = (n(), n()) else { continue };
                    let ty = n().unwrap_or_default();
                    let pk = matches!(n().as_deref(), Some("t" | "1"));
                    by_table.entry(t).or_default().push(GraphColumn { name: c, data_type: ty, primary_key: pk });
                }
            }
            None => {
                for t in &tables {
                    let st = self.table_structure(schema, &t.name).await?;
                    by_table.insert(
                        t.name.clone(),
                        st.columns.into_iter().map(|c| GraphColumn { name: c.name, data_type: c.data_type, primary_key: c.primary_key }).collect(),
                    );
                }
            }
        }
        let mut foreign_keys = Vec::new();
        match d {
            Driver::Sqlite => {
                for t in tables.iter().filter(|t| t.kind == "table") {
                    foreign_keys.extend(self.table_relations(schema, &t.name).await?.outgoing);
                }
            }
            // One query for the whole schema instead of one per table.
            _ => foreign_keys = self.schema_foreign_keys(schema).await?,
        }
        Ok(SchemaGraph {
            tables: tables
                .into_iter()
                .map(|t| GraphTable { columns: by_table.remove(&t.name).unwrap_or_default(), name: t.name, kind: t.kind })
                .collect(),
            foreign_keys,
        })
    }

    async fn schema_foreign_keys(&self, schema: &str) -> AppResult<Vec<ForeignKey>> {
        let d = self.driver;
        let s = d.quote_literal(schema);
        let sql = match d {
            Driver::Postgres => format!("{PG_FK_BASE} AND n.nspname = {s} ORDER BY 3, 1"),
            Driver::Mysql => format!("{MY_FK_BASE} AND k.TABLE_SCHEMA = {s} {MY_FK_GROUP} ORDER BY 3, 1"),
            Driver::Mssql => format!("{MS_FK_BASE} AND fk.schema_id = SCHEMA_ID({s}) ORDER BY 3, 1"),
            _ => return Ok(vec![]),
        };
        Ok(parse_fks(self.text_rows(&sql).await?, d == Driver::Postgres))
    }

    pub async fn table_relations(&self, schema: &str, table: &str) -> AppResult<TableRelations> {
        let d = self.driver;
        let (s, t) = (d.quote_literal(schema), d.quote_literal(table));
        let (outgoing, incoming) = match d {
            Driver::Postgres => {
                (
                    parse_fks(self.text_rows(&format!("{PG_FK_BASE} AND n.nspname = {s} AND c.relname = {t} ORDER BY 1")).await?, true),
                    parse_fks(self.text_rows(&format!("{PG_FK_BASE} AND nf.nspname = {s} AND cf.relname = {t} ORDER BY 2, 3, 1")).await?, true),
                )
            }
            Driver::Mysql => {
                (
                    parse_fks(self.text_rows(&format!("{MY_FK_BASE} AND k.TABLE_SCHEMA = {s} AND k.TABLE_NAME = {t} {MY_FK_GROUP} ORDER BY 1")).await?, false),
                    parse_fks(
                        self.text_rows(&format!(
                            "{MY_FK_BASE} AND k.REFERENCED_TABLE_SCHEMA = {s} AND k.REFERENCED_TABLE_NAME = {t} {MY_FK_GROUP} ORDER BY 3, 1"
                        ))
                        .await?,
                        false,
                    ),
                )
            }
            Driver::Mssql => {
                let obj = d.quote_literal(&d.qualified(schema, table));
                (
                    parse_fks(self.text_rows(&format!("{MS_FK_BASE} AND fk.parent_object_id = OBJECT_ID({obj}) ORDER BY 1")).await?, false),
                    parse_fks(self.text_rows(&format!("{MS_FK_BASE} AND fk.referenced_object_id = OBJECT_ID({obj}) ORDER BY 2, 3, 1")).await?, false),
                )
            }
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => {
                let one = |tbl: &str| {
                    format!(
                        "SELECT 'fk_' || id, {s}, {}, group_concat(\"from\", ', '), {s}, \"table\", group_concat(\"to\", ', '), on_update, on_delete \
                         FROM pragma_foreign_key_list({}, {s}) GROUP BY id ORDER BY id",
                        d.quote_literal(tbl),
                        d.quote_literal(tbl)
                    )
                };
                let outgoing = parse_fks(self.text_rows(&one(table)).await?, false);
                // SQLite has no reverse index: scan the other tables' keys.
                let mut incoming = Vec::new();
                for other in self.list_tables(schema).await?.into_iter().filter(|x| x.kind == "table" && x.name != table) {
                    for fk in parse_fks(self.text_rows(&one(&other.name)).await?, false) {
                        if fk.ref_table == table {
                            incoming.push(fk);
                        }
                    }
                }
                (outgoing, incoming)
            }
        };

        let trig_sql = match d {
            Driver::Postgres => format!(
                "SELECT t.tgname, CASE WHEN t.tgtype & 2 = 2 THEN 'BEFORE' WHEN t.tgtype & 64 = 64 THEN 'INSTEAD OF' ELSE 'AFTER' END, \
                 concat_ws(', ', CASE WHEN t.tgtype & 4 = 4 THEN 'INSERT' END, CASE WHEN t.tgtype & 16 = 16 THEN 'UPDATE' END, \
                   CASE WHEN t.tgtype & 8 = 8 THEN 'DELETE' END, CASE WHEN t.tgtype & 32 = 32 THEN 'TRUNCATE' END), \
                 t.tgenabled <> 'D', pg_get_triggerdef(t.oid, true) \
                 FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT t.tgisinternal AND n.nspname = {s} AND c.relname = {t} ORDER BY t.tgname"
            ),
            Driver::Mysql => format!(
                "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, '1', \
                 CONCAT('CREATE TRIGGER `', TRIGGER_NAME, '` ', ACTION_TIMING, ' ', EVENT_MANIPULATION, ' ON `', EVENT_OBJECT_TABLE, \
                   '` FOR EACH ROW ', ACTION_STATEMENT) \
                 FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = {s} AND EVENT_OBJECT_TABLE = {t} ORDER BY ACTION_ORDER"
            ),
            Driver::Mssql => format!(
                "SELECT tr.name, CASE WHEN tr.is_instead_of_trigger = 1 THEN 'INSTEAD OF' ELSE 'AFTER' END, \
                 (SELECT STRING_AGG(te.type_desc, ', ') FROM sys.trigger_events te WHERE te.object_id = tr.object_id), \
                 CASE WHEN tr.is_disabled = 0 THEN '1' ELSE '0' END, OBJECT_DEFINITION(tr.object_id) \
                 FROM sys.triggers tr WHERE tr.parent_id = OBJECT_ID({}) ORDER BY tr.name",
                d.quote_literal(&d.qualified(schema, table))
            ),
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => format!(
                "SELECT name, CASE WHEN upper(sql) LIKE '%INSTEAD OF%' THEN 'INSTEAD OF' WHEN upper(sql) LIKE '%BEFORE%' THEN 'BEFORE' ELSE 'AFTER' END, \
                 CASE WHEN upper(sql) LIKE '%INSERT%ON%' THEN 'INSERT' WHEN upper(sql) LIKE '%DELETE%ON%' THEN 'DELETE' ELSE 'UPDATE' END, \
                 '1', sql FROM {}.sqlite_master WHERE type = 'trigger' AND tbl_name = {t} ORDER BY name",
                d.quote_ident(schema)
            ),
        };
        let triggers = self
            .text_rows(&trig_sql)
            .await?
            .into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let mut n = || it.next().flatten();
                Some(TriggerInfo {
                    name: n()?,
                    timing: n().unwrap_or_default(),
                    event: n().unwrap_or_default(),
                    enabled: matches!(n().as_deref(), Some("t" | "1" | "true")),
                    definition: n().unwrap_or_default(),
                })
            })
            .collect();
        Ok(TableRelations { outgoing, incoming, triggers })
    }

    pub async fn table_details(&self, schema: &str, table: &str) -> AppResult<TableDetails> {
        let d = self.driver;
        let (s, t) = (d.quote_literal(schema), d.quote_literal(table));
        let first = |rows: Vec<Vec<Option<String>>>| rows.into_iter().next().unwrap_or_default();
        let num = |v: Option<String>| v.and_then(|x| x.parse::<f64>().ok()).map(|f| f as i64);
        let mut out = TableDetails::default();
        match d {
            Driver::Postgres => {
                let r = first(
                    self.text_rows(&format!(
                        "SELECT c.reltuples::bigint, pg_total_relation_size(c.oid), obj_description(c.oid, 'pg_class'), \
                         (SELECT datcollate FROM pg_database WHERE datname = current_database()), c.relkind \
                         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = {s} AND c.relname = {t}"
                    ))
                    .await?,
                );
                let mut it = r.into_iter();
                out.rows_estimate = num(it.next().flatten()).filter(|n| *n >= 0);
                out.total_bytes = num(it.next().flatten());
                out.comment = it.next().flatten();
                out.collation = it.next().flatten();
                out.engine = Some("PostgreSQL heap".into());
                let st = first(
                    self.text_rows(&format!(
                        "SELECT pg_table_size(c.oid), pg_indexes_size(c.oid), s.n_live_tup, s.n_dead_tup, \
                         s.seq_scan, s.idx_scan, s.last_vacuum, s.last_autovacuum, s.last_analyze, s.last_autoanalyze \
                         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                         LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid WHERE n.nspname = {s} AND c.relname = {t}"
                    ))
                    .await?,
                );
                let mut sv = st.into_iter();
                let mut v = || sv.next().flatten();
                let (tbl, idx, live, dead, seq, iscan) = (v(), v(), v(), v(), v(), v());
                let (vac, avac, an, aan) = (v(), v(), v(), v());
                out.stats = [
                    stat("Data size", tbl, "bytes"),
                    stat("Index size", idx, "bytes"),
                    stat("Live rows", live, "number"),
                    stat("Dead rows", dead, "number"),
                    stat("Sequential scans", seq, "number"),
                    stat("Index scans", iscan, "number"),
                    stat("Last vacuum", vac.or(avac), "text"),
                    stat("Last analyze", an.or(aan), "text"),
                ]
                .into_iter()
                .flatten()
                .collect();
                let kind = it.next().flatten().unwrap_or_default();
                out.ddl = Some(if kind == "v" || kind == "m" {
                    let def = first(self.text_rows(&format!("SELECT pg_get_viewdef({}::regclass, true)", d.quote_literal(&d.qualified(schema, table)))).await?);
                    format!(
                        "CREATE {}VIEW {} AS\n{}",
                        if kind == "m" { "MATERIALIZED " } else { "" },
                        d.qualified(schema, table),
                        def.into_iter().next().flatten().unwrap_or_default()
                    )
                } else {
                    self.pg_ddl(schema, table).await?
                });
            }
            Driver::Mysql => {
                let r = first(
                    self.text_rows(&format!(
                        "SELECT TABLE_ROWS, DATA_LENGTH + INDEX_LENGTH, ENGINE, TABLE_COLLATION, TABLE_COMMENT, CREATE_TIME, UPDATE_TIME, \
                         ROW_FORMAT, AVG_ROW_LENGTH, AUTO_INCREMENT, DATA_LENGTH, INDEX_LENGTH, DATA_FREE, MAX_DATA_LENGTH \
                         FROM information_schema.TABLES WHERE TABLE_SCHEMA = {s} AND TABLE_NAME = {t}"
                    ))
                    .await?,
                );
                let mut it = r.into_iter();
                out.rows_estimate = num(it.next().flatten());
                out.total_bytes = num(it.next().flatten());
                out.engine = it.next().flatten();
                out.collation = it.next().flatten();
                out.comment = it.next().flatten().filter(|c| !c.is_empty());
                out.created = it.next().flatten();
                out.updated = it.next().flatten();
                out.stats = [
                    stat("Row format", it.next().flatten(), "text"),
                    stat("Avg. row length", it.next().flatten(), "bytes"),
                    stat("Auto increment", it.next().flatten(), "number"),
                    stat("Data size", it.next().flatten(), "bytes"),
                    stat("Index size", it.next().flatten(), "bytes"),
                    stat("Free data size", it.next().flatten(), "bytes"),
                    stat("Max data size", it.next().flatten().filter(|v| v != "0"), "bytes"),
                ]
                .into_iter()
                .flatten()
                .collect();
                let ddl = first(self.text_rows(&format!("SHOW CREATE TABLE {}", d.qualified(schema, table))).await?);
                out.ddl = ddl.into_iter().nth(1).flatten();
            }
            Driver::Mssql => {
                let obj = d.quote_literal(&d.qualified(schema, table));
                let pages = |cond: &str| {
                    format!(
                        "(SELECT CAST(SUM(CAST(a.{cond} AS bigint)) * 8192 AS varchar(30)) FROM sys.partitions p \
                         JOIN sys.allocation_units a ON a.container_id = p.partition_id WHERE p.object_id = o.object_id"
                    )
                };
                let r = first(
                    self.text_rows(&format!(
                        "SELECT (SELECT SUM(p.rows) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)), \
                         {}), CONVERT(varchar(23), o.create_date, 121), CONVERT(varchar(23), o.modify_date, 121), \
                         (SELECT CAST(ep.value AS nvarchar(4000)) FROM sys.extended_properties ep WHERE ep.class = 1 AND ep.major_id = o.object_id \
                           AND ep.minor_id = 0 AND ep.name = 'MS_Description'), \
                         CAST(DATABASEPROPERTYEX(DB_NAME(), 'Collation') AS nvarchar(128)), RTRIM(o.type), \
                         {} AND p.index_id IN (0, 1)), {} AND p.index_id > 1), \
                         (SELECT COUNT(*) FROM sys.indexes i WHERE i.object_id = o.object_id AND i.type > 0), \
                         (SELECT CAST(IDENT_CURRENT({obj}) AS varchar(30))) \
                         FROM sys.objects o WHERE o.object_id = OBJECT_ID({obj})",
                        pages("total_pages"),
                        pages("used_pages"),
                        pages("used_pages")
                    ))
                    .await?,
                );
                let mut it = r.into_iter();
                let mut v = || it.next().flatten();
                out.rows_estimate = num(v());
                out.total_bytes = num(v());
                out.created = v();
                out.updated = v();
                out.comment = v();
                out.collation = v();
                let kind = v().unwrap_or_default();
                out.engine = Some("SQL Server".into());
                out.stats = [
                    stat("Data size", v(), "bytes"),
                    stat("Index size", v(), "bytes"),
                    stat("Indexes", v(), "number"),
                    stat("Identity", v(), "number"),
                ]
                .into_iter()
                .flatten()
                .collect();
                out.ddl = Some(if kind == "V" {
                    first(self.text_rows(&format!("SELECT OBJECT_DEFINITION(OBJECT_ID({obj}))")).await?).into_iter().next().flatten().unwrap_or_default()
                } else {
                    self.pg_ddl(schema, table).await?
                });
            }
            Driver::Redis => return Err(crate::error::not_sql()),
            Driver::Sqlite => {
                let q = d.quote_ident(schema);
                let ddl = first(self.text_rows(&format!("SELECT sql FROM {q}.sqlite_master WHERE name = {t}")).await?);
                out.ddl = ddl.into_iter().next().flatten();
                let cnt = first(self.text_rows(&format!("SELECT count(*) FROM {}", d.qualified(schema, table))).await?);
                out.rows_estimate = num(cnt.into_iter().next().flatten());
                let size = first(self.text_rows(&format!("SELECT page_count * page_size FROM pragma_page_count({s}), pragma_page_size({s})")).await?);
                out.total_bytes = num(size.into_iter().next().flatten());
                out.engine = Some("SQLite".into());
            }
        }
        Ok(out)
    }

    /// Reconstructs a readable CREATE TABLE for Postgres and SQL Server (columns, primary key, indexes).
    async fn pg_ddl(&self, schema: &str, table: &str) -> AppResult<String> {
        let d = self.driver;
        let st = self.table_structure(schema, table).await?;
        let mut lines: Vec<String> = st
            .columns
            .iter()
            .map(|c| {
                let mut l = format!("  {} {}", d.quote_ident(&c.name), c.data_type);
                if d == Driver::Mssql && c.extra == "identity" {
                    l.push_str(" IDENTITY(1,1)");
                }
                if !c.nullable {
                    l.push_str(" NOT NULL");
                }
                if let Some(def) = &c.default_value {
                    l.push_str(&format!(" DEFAULT {def}"));
                }
                l
            })
            .collect();
        let pk: Vec<String> = st.columns.iter().filter(|c| c.primary_key).map(|c| d.quote_ident(&c.name)).collect();
        if !pk.is_empty() {
            lines.push(format!("  PRIMARY KEY ({})", pk.join(", ")));
        }
        let mut ddl = format!("CREATE TABLE {} (\n{}\n);", d.qualified(schema, table), lines.join(",\n"));
        for i in st.indexes.iter().filter(|i| !i.primary) {
            if d == Driver::Mssql {
                let cols: Vec<String> = i.columns.split(", ").map(|c| d.quote_ident(c)).collect();
                let unique = if i.unique { "UNIQUE " } else { "" };
                ddl.push_str(&format!("\nCREATE {unique}INDEX {} ON {} ({});", d.quote_ident(&i.name), d.qualified(schema, table), cols.join(", ")));
            } else {
                ddl.push_str(&format!("\n{};", i.definition));
            }
        }
        Ok(ddl)
    }

    pub async fn fetch_rows(&self, a: &FetchArgs) -> AppResult<TablePage> {
        let d = self.driver;
        let from = d.qualified(&a.schema, &a.table);
        let filter = a.filter.as_deref().map(str::trim).filter(|f| !f.is_empty());
        let where_sql = filter.map(|f| format!(" WHERE {f}")).unwrap_or_default();
        let order_sql = match &a.sort {
            Some(s) => format!(" ORDER BY {} {}", d.quote_ident(&s.column), if s.desc { "DESC" } else { "ASC" }),
            // Without an explicit sort, order by primary key so paging is stable
            // (physical order shifts after updates and pages could skip/repeat rows).
            None => {
                let pks: Vec<String> = self
                    .table_structure(&a.schema, &a.table)
                    .await?
                    .columns
                    .into_iter()
                    .filter(|c| c.primary_key)
                    .map(|c| d.quote_ident(&c.name))
                    .collect();
                if pks.is_empty() {
                    // SQL Server's OFFSET/FETCH needs an ORDER BY.
                    if d == Driver::Mssql { " ORDER BY (SELECT NULL)".into() } else { String::new() }
                } else {
                    format!(" ORDER BY {}", pks.join(", "))
                }
            }
        };
        let sql = if d == Driver::Mssql {
            format!("SELECT * FROM {from}{where_sql}{order_sql} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", a.offset, a.limit)
        } else {
            format!("SELECT * FROM {from}{where_sql}{order_sql} LIMIT {} OFFSET {}", a.limit, a.offset)
        };

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

    /// Runs one statement inside a read-only transaction that is always rolled
    /// back, with a server-side timeout. Used for AI assistants (MCP).
    /// Callers must validate the SQL with `ai_guard::check` first.
    pub async fn run_readonly(&self, sql: &str, max_rows: usize) -> AppResult<ResultSet> {
        let body = AssertSqlSafe(sql.to_owned());
        macro_rules! guarded {
            ($pool:expr, $begin:expr, $timeout:expr, $cell:expr) => {{
                let mut conn = $pool.acquire().await?;
                conn.execute($begin).await?;
                let out = async {
                    conn.execute($timeout).await?;
                    collect(sqlx::raw_sql(body).fetch_many(&mut *conn), max_rows, |q| q.rows_affected(), $cell).await
                }
                .await;
                // Always roll back, even after an error, before the connection returns to the pool.
                let _ = conn.execute("ROLLBACK").await;
                out?
            }};
        }
        let sets = match &self.pool {
            Pool::Pg(p) => guarded!(p, "BEGIN READ ONLY", "SET LOCAL statement_timeout = 15000", pg_cell),
            Pool::My(p) => guarded!(p, "START TRANSACTION READ ONLY", "SET SESSION MAX_EXECUTION_TIME = 15000", my_cell),
            Pool::Lite(p) => guarded!(p, "BEGIN", "SELECT 1", lite_cell),
            // SQL Server has no read-only transactions: the statement is
            // validated by ai_guard, then run in a transaction that is
            // always rolled back, with a client-side timeout.
            Pool::Ms(p) => {
                let mut conn = p.acquire().await?;
                crate::mssql::run(&mut conn, "SET LOCK_TIMEOUT 15000; BEGIN TRAN; SELECT 1", 1, None).await?;
                let out = tokio::time::timeout(Duration::from_secs(15), crate::mssql::run(&mut conn, sql, max_rows, None)).await;
                match out {
                    Ok(res) => {
                        let rolled_back = crate::mssql::run(&mut conn, "IF @@TRANCOUNT > 0 ROLLBACK", 0, None).await;
                        if rolled_back.is_err() {
                            conn.discard();
                        }
                        res?
                    }
                    Err(_) => {
                        conn.discard();
                        return Err(AppError::msg("Query took longer than 15 seconds and was stopped"));
                    }
                }
            }
        };
        Ok(sets.into_iter().next().unwrap_or_default())
    }

    /// Compact, AI-friendly description of every table in a schema.
    pub async fn schema_overview(&self, schema: &str, max_tables: usize) -> AppResult<String> {
        let tables = self.list_tables(schema).await?;
        let mut out = String::new();
        for t in tables.iter().take(max_tables) {
            let st = self.table_structure(schema, &t.name).await?;
            let cols = st
                .columns
                .iter()
                .map(|c| {
                    let mut s = format!("{} {}", c.name, c.data_type);
                    if c.primary_key {
                        s.push_str(" PK");
                    }
                    if !c.nullable {
                        s.push_str(" NOT NULL");
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join(", ");
            let kind = if t.kind == "table" { "" } else { " (view)" };
            out.push_str(&format!("{}{}: {}\n", t.name, kind, cols));
        }
        if tables.len() > max_tables {
            out.push_str(&format!("... and {} more tables\n", tables.len() - max_tables));
        }
        Ok(out)
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
                    let sql_text = sql.clone();
                    let mut q = sqlx::query(AssertSqlSafe(sql));
                    for b in binds {
                        q = q.bind(b);
                    }
                    let started = std::time::Instant::now();
                    let res = q.execute(&mut *tx).await;
                    self.record(&sql_text, started, res.as_ref().map(|r| format!("{} affected", r.rows_affected())).map_err(crate::error::db_message));
                    let affected = res?.rows_affected();
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
            Pool::Ms(p) => self.ms_exec_tx(p, stmts).await?,
        })
    }

    /// Runs a script on SQL Server. Read-only connections run it inside a
    /// transaction that is always rolled back, since SQL Server has no
    /// read-only session mode.
    async fn ms_script(&self, conn: &mut crate::mssql::MsConn<'_>, sql: &str, max_rows: usize, progress: Progress<'_>) -> AppResult<Vec<ResultSet>> {
        if !self.read_only {
            return crate::mssql::run(conn, sql, max_rows, progress).await;
        }
        let lower = sql.to_ascii_lowercase();
        if lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|w| w == "commit" || w == "save") {
            return Err(AppError::msg("This connection is read-only"));
        }
        crate::mssql::run(conn, "BEGIN TRAN; SELECT 1", 1, None).await?;
        let out = crate::mssql::run(conn, sql, max_rows, progress).await;
        if crate::mssql::run(conn, "IF @@TRANCOUNT > 0 ROLLBACK", 0, None).await.is_err() {
            conn.discard();
        }
        out
    }

    async fn ms_exec_tx(&self, pool: &crate::mssql::MsPool, stmts: Vec<(String, Vec<Option<String>>, bool)>) -> AppResult<u64> {
        let mut conn = pool.acquire().await?;
        crate::mssql::run(&mut conn, "SET XACT_ABORT ON; BEGIN TRAN; SELECT 1", 1, None).await?;
        let res: AppResult<u64> = async {
            let mut total = 0u64;
            for (sql, binds, single) in stmts {
                let params: Vec<&dyn tiberius::ToSql> = binds.iter().map(|b| b as &dyn tiberius::ToSql).collect();
                let started = std::time::Instant::now();
                let r = conn.client().execute(sql.as_str(), &params).await;
                let affected = r.as_ref().map(|r| r.rows_affected().last().copied().unwrap_or(0));
                self.record(&sql, started, affected.map(|n| format!("{n} affected")).map_err(crate::error::ms_message));
                drop(params);
                let affected = conn.check(r).map(|r| r.rows_affected().last().copied().unwrap_or(0))?;
                if single && affected != 1 {
                    return Err(AppError::msg(format!(
                        "Expected to change 1 row but {affected} matched — nothing was saved. \
                         The data may have changed; refresh and try again."
                    )));
                }
                total += affected;
            }
            Ok(total)
        }
        .await;
        let end = if res.is_ok() { "COMMIT" } else { "IF @@TRANCOUNT > 0 ROLLBACK" };
        if let Err(e) = crate::mssql::run(&mut conn, end, 0, None).await {
            conn.discard();
            res?;
            return Err(e);
        }
        res
    }
}

/// SQL Server's default definitions come wrapped in parentheses: `((0))`, `(getdate())`.
fn unwrap_parens(v: &str) -> &str {
    let mut s = v.trim();
    while s.starts_with('(') && s.ends_with(')') {
        // Only strip when the outer pair encloses everything: `(a)+(b)` stays.
        let inner = &s[1..s.len() - 1];
        let mut depth = 0i32;
        let balanced = inner.chars().all(|c| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth >= 0
        }) && depth == 0;
        if !balanced {
            break;
        }
        s = inner.trim();
    }
    s
}

/// A column's declared type with its length/precision, e.g. `nvarchar(50)`, `decimal(12,2)`.
/// Expects `c` = sys.columns and `t` = sys.types.
const MS_TYPE: &str = "CASE WHEN t.name IN ('varchar','char','varbinary','binary') \
       THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')' \
     WHEN t.name IN ('nvarchar','nchar') \
       THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
     WHEN t.name IN ('decimal','numeric') THEN t.name + '(' + CAST(c.precision AS varchar(5)) + ',' + CAST(c.scale AS varchar(5)) + ')' \
     WHEN t.name IN ('datetime2','time','datetimeoffset') THEN t.name + '(' + CAST(c.scale AS varchar(5)) + ')' \
     ELSE t.name END";

const MS_FK_BASE: &str = "SELECT fk.name, SCHEMA_NAME(fk.schema_id), OBJECT_NAME(fk.parent_object_id), \
     (SELECT STRING_AGG(COL_NAME(fkc.parent_object_id, fkc.parent_column_id), ', ') WITHIN GROUP (ORDER BY fkc.constraint_column_id) \
       FROM sys.foreign_key_columns fkc WHERE fkc.constraint_object_id = fk.object_id), \
     OBJECT_SCHEMA_NAME(fk.referenced_object_id), OBJECT_NAME(fk.referenced_object_id), \
     (SELECT STRING_AGG(COL_NAME(fkc.referenced_object_id, fkc.referenced_column_id), ', ') WITHIN GROUP (ORDER BY fkc.constraint_column_id) \
       FROM sys.foreign_key_columns fkc WHERE fkc.constraint_object_id = fk.object_id), \
     REPLACE(fk.update_referential_action_desc, '_', ' '), REPLACE(fk.delete_referential_action_desc, '_', ' ') \
     FROM sys.foreign_keys fk WHERE 1 = 1";

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
            Driver::Mssql => format!("@P{}", self.n),
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

const PG_FK_BASE: &str = "SELECT con.conname, n.nspname, c.relname, \
     (SELECT string_agg(a.attname, ', ' ORDER BY k.n) FROM unnest(con.conkey) WITH ORDINALITY k(attnum, n) \
       JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
     nf.nspname, cf.relname, \
     (SELECT string_agg(a.attname, ', ' ORDER BY k.n) FROM unnest(con.confkey) WITH ORDINALITY k(attnum, n) \
       JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum), \
     con.confupdtype::text, con.confdeltype::text \
     FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
     JOIN pg_class cf ON cf.oid = con.confrelid JOIN pg_namespace nf ON nf.oid = cf.relnamespace WHERE con.contype = 'f'";

const MY_FK_BASE: &str = "SELECT k.CONSTRAINT_NAME, k.TABLE_SCHEMA, k.TABLE_NAME, \
     GROUP_CONCAT(k.COLUMN_NAME ORDER BY k.ORDINAL_POSITION SEPARATOR ', '), k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, \
     GROUP_CONCAT(k.REFERENCED_COLUMN_NAME ORDER BY k.ORDINAL_POSITION SEPARATOR ', '), r.UPDATE_RULE, r.DELETE_RULE \
     FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
     ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME \
     WHERE k.REFERENCED_TABLE_NAME IS NOT NULL";

const MY_FK_GROUP: &str =
    "GROUP BY k.CONSTRAINT_NAME, k.TABLE_SCHEMA, k.TABLE_NAME, k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, r.UPDATE_RULE, r.DELETE_RULE";

/// Rows: name, schema, table, columns, ref schema, ref table, ref columns, on update, on delete.
fn parse_fks(rows: Vec<Vec<Option<String>>>, pg: bool) -> Vec<ForeignKey> {
    let split = |v: Option<String>| -> Vec<String> { v.unwrap_or_default().split(", ").filter(|x| !x.is_empty()).map(str::to_string).collect() };
    let act = |v: Option<String>| {
        let v = v.unwrap_or_default();
        if pg {
            match v.as_str() {
                "r" => "RESTRICT",
                "c" => "CASCADE",
                "n" => "SET NULL",
                "d" => "SET DEFAULT",
                _ => "NO ACTION",
            }
            .to_string()
        } else if v.is_empty() {
            "NO ACTION".into()
        } else {
            v.to_uppercase()
        }
    };
    rows.into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            let mut n = || it.next().flatten();
            let (name, sch, tbl, cols, rs, rt, rc, up, del) = (n()?, n(), n(), n(), n(), n(), n(), n(), n());
            Some(ForeignKey {
                name,
                schema: sch.unwrap_or_default(),
                table: tbl.unwrap_or_default(),
                columns: split(cols),
                ref_schema: rs.unwrap_or_default(),
                ref_table: rt.unwrap_or_default(),
                ref_columns: split(rc),
                on_update: act(up),
                on_delete: act(del),
            })
        })
        .collect()
}

fn column_from(r: Vec<Option<String>>, nullable: impl Fn(&str) -> bool, pk: impl Fn(&str) -> bool) -> ColumnInfo {
    let mut it = r.into_iter();
    ColumnInfo {
        name: it.next().flatten().unwrap_or_default(),
        data_type: it.next().flatten().unwrap_or_default(),
        nullable: it.next().flatten().is_some_and(|v| nullable(&v)),
        default_value: it.next().flatten(),
        primary_key: it.next().flatten().is_some_and(|v| pk(&v)),
        key: it.next().flatten().unwrap_or_default(),
        extra: it.next().flatten().unwrap_or_default(),
    }
}

fn returns_rows(sql: &str) -> bool {
    let head = sql.trim_start().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    matches!(head.as_str(), "select" | "with" | "show" | "values" | "table" | "pragma" | "explain" | "describe" | "desc")
}

/// Called with the running row count while a query streams in.
pub type Progress<'a> = Option<&'a (dyn Fn(usize) + Send + Sync)>;

async fn collect<Q, R: Row>(
    stream: BoxStream<'_, Result<Either<Q, R>, sqlx::Error>>,
    max_rows: usize,
    affected: fn(&Q) -> u64,
    cell: fn(&R, usize) -> Option<String>,
) -> AppResult<Vec<ResultSet>> {
    collect_with(stream, max_rows, affected, cell, None).await
}

async fn collect_with<Q, R: Row>(
    mut stream: BoxStream<'_, Result<Either<Q, R>, sqlx::Error>>,
    max_rows: usize,
    affected: fn(&Q) -> u64,
    cell: fn(&R, usize) -> Option<String>,
    progress: Progress<'_>,
) -> AppResult<Vec<ResultSet>> {
    let mut sets = Vec::new();
    let mut cur = ResultSet::default();
    let mut total = 0usize;
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
                total += 1;
                if let Some(p) = progress {
                    if total % 1000 == 0 {
                        p(total);
                    }
                }
            }
        }
    }
    if !cur.columns.is_empty() {
        sets.push(cur);
    }
    Ok(sets)
}

/// Masks the literal after PASSWORD / IDENTIFIED BY so account changes never
/// leave a password in the Console log.
pub fn redact_secrets(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < sql.len() {
        let hit = ["password ", "password=", "identified by "]
            .iter()
            .filter_map(|k| lower[i..].find(k).map(|p| (p, k.len())))
            .min_by_key(|(p, _)| *p);
        let Some((p, klen)) = hit else {
            out.push_str(&sql[i..]);
            break;
        };
        let after = i + p + klen;
        out.push_str(&sql[i..after]);
        let rest = &sql[after..];
        // Skip whitespace, SQL Server's `=` and N'' prefix: PASSWORD = N'…'.
        let mut t = rest.trim_start();
        if let Some(x) = t.strip_prefix('=') {
            t = x.trim_start();
        }
        if t.starts_with("N'") || t.starts_with("n'") {
            t = &t[1..];
        }
        let trimmed = t;
        let ws = rest.len() - trimmed.len();
        if let Some(body) = trimmed.strip_prefix('\'') {
            // Find the closing quote, honoring '' and \' escapes.
            let b = body.as_bytes();
            let mut j = 0;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                } else if b[j] == b'\'' && b.get(j + 1) == Some(&b'\'') {
                    j += 2;
                } else if b[j] == b'\'' {
                    break;
                } else {
                    j += 1;
                }
            }
            out.push_str(&rest[..ws]);
            out.push_str("'•••'");
            i = after + ws + 1 + (j + 1).min(body.len());
        } else {
            i = after;
        }
    }
    out
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
        let _serial = crate::test_lock::PG.lock().await;
        let Some(cfg) = cfg_from_env("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        assert!(!s.server_version().await.unwrap().is_empty());
        roundtrip(&s, "public", "users", "name").await;

        // Multi-statement script, empty result, and odd types decode as text.
        let sets = s
            .run_script("SELECT profile, tags, feeling, avatar, balance FROM users WHERE avatar IS NOT NULL ORDER BY id LIMIT 1; SELECT * FROM empty_t; UPDATE users SET age = age WHERE id < 3", 100)
            .await
            .unwrap();
        assert_eq!(sets.len(), 3);
        assert!(sets[0].rows[0][3].as_deref().unwrap().starts_with("\\x"));
        // Mid-script empty SELECTs carry no column metadata (sqlx only exposes it with rows).
        assert!(sets[1].rows.is_empty());
        assert_eq!(sets[2].rows_affected, 2);

        let empty = s.run_script("SELECT * FROM empty_t", 100).await.unwrap();
        assert_eq!(empty[0].columns.len(), 2);

        let st = s.table_structure("public", "users").await.unwrap();
        let id = st.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!((id.key.as_str(), id.extra.as_str()), ("PRI", "auto increment"));
        assert_eq!(st.columns.iter().find(|c| c.name == "email").unwrap().key, "UNI");
        let pk = st.indexes.iter().find(|i| i.primary).unwrap();
        assert_eq!((pk.columns.as_str(), pk.method.as_str(), pk.unique), ("id", "btree", true));
        let info = s.table_details("public", "users").await.unwrap();
        assert!(info.total_bytes.unwrap() > 0);
        assert!(info.ddl.unwrap().starts_with("CREATE TABLE \"public\".\"users\""));

        // Second line of defense for AI queries: even unvalidated writes fail.
        let err = s.run_readonly("DELETE FROM users WHERE id = -1", 10).await.unwrap_err().to_string();
        assert!(err.contains("read-only"), "{err}");
        assert!(s.run_readonly("SELECT count(*) FROM users", 10).await.is_ok(), "connection reusable after rollback");
    }

    #[tokio::test]
    async fn mysql() {
        let _serial = crate::test_lock::MYSQL.lock().await;
        let Some(cfg) = cfg_from_env("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        roundtrip(&s, "tabla_test", "products", "title").await;
        let sets = s.run_script("SELECT meta, img, price FROM products ORDER BY id", 100).await.unwrap();
        assert_eq!(sets[0].rows.len(), 3);
        assert!(sets[0].rows[1][1].is_none());

        let st = s.table_structure("tabla_test", "products").await.unwrap();
        let id = st.columns.iter().find(|c| c.name == "id").unwrap();
        assert_eq!((id.key.as_str(), id.extra.as_str()), ("PRI", "auto_increment"));
        assert_eq!(st.columns.iter().find(|c| c.name == "sku").unwrap().key, "UNI");
        let info = s.table_details("tabla_test", "products").await.unwrap();
        assert_eq!(info.engine.as_deref(), Some("InnoDB"));
        assert!(info.ddl.unwrap().contains("CREATE TABLE `products`"));

        let err = s.run_readonly("UPDATE products SET stock = stock WHERE id = -1", 10).await.unwrap_err().to_string();
        assert!(err.to_lowercase().contains("read only"), "{err}");
    }

    #[tokio::test]
    async fn sqlite() {
        let Some(cfg) = cfg_from_env("TABLA_TEST_SQLITE", Driver::Sqlite) else { return };
        let s = Session::connect(&cfg, None).await.unwrap();
        roundtrip(&s, "main", "notes", "title").await;
        let sets = s.run_script("SELECT data, score FROM notes ORDER BY id", 100).await.unwrap();
        assert_eq!(sets[0].rows[0][0].as_deref(), Some("0x0102"));
        let st = s.table_structure("main", "notes").await.unwrap();
        assert_eq!(st.columns[0].key, "PRI");
        let info = s.table_details("main", "notes").await.unwrap();
        assert!(info.ddl.unwrap().starts_with("CREATE TABLE notes"));
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

#[cfg(test)]
mod redact_tests {
    use super::redact_secrets;

    #[test]
    fn masks_passwords() {
        assert_eq!(redact_secrets("CREATE ROLE a LOGIN PASSWORD 'se''cr;et' VALID UNTIL 'x'"), "CREATE ROLE a LOGIN PASSWORD '•••' VALID UNTIL 'x'");
        assert_eq!(redact_secrets("ALTER USER 'u'@'%' IDENTIFIED BY 'p\\'w'; SELECT 1"), "ALTER USER 'u'@'%' IDENTIFIED BY '•••'; SELECT 1");
        assert_eq!(redact_secrets("SELECT password FROM t"), "SELECT password FROM t");
        assert_eq!(redact_secrets("CREATE LOGIN [a] WITH PASSWORD = N'x;y'"), "CREATE LOGIN [a] WITH PASSWORD = N'•••'");
        assert_eq!(redact_secrets("ALTER USER [a] WITH PASSWORD=N'z'"), "ALTER USER [a] WITH PASSWORD=N'•••'");
    }
}

#[cfg(test)]
mod relation_tests {
    //! Self-contained fixtures (rel_parent / rel_child) on the same test servers as `tests`.
    use super::*;

    fn cfg(var: &str, driver: Driver) -> Option<ConnectionConfig> {
        let v = std::env::var(var).ok()?;
        let mut c: ConnectionConfig = serde_json::from_value(serde_json::json!({
            "id": "rel-test", "name": "rel", "driver": driver, "sslMode": "disable"
        }))
        .unwrap();
        if driver == Driver::Sqlite {
            c.database = v;
        } else {
            let p: Vec<&str> = v.split(':').collect();
            (c.host, c.port, c.user, c.database) = (p[0].into(), p[1].parse().unwrap(), p[2].into(), p[3].into());
        }
        Some(c)
    }

    async fn check(s: &Session, schema: &str, setup: &str, teardown: &str) {
        s.run_script(teardown, 1).await.ok();
        s.run_script(setup, 1).await.unwrap();
        let child = s.table_relations(schema, "rel_child").await.unwrap();
        assert_eq!(child.outgoing.len(), 1, "outgoing");
        let fk = &child.outgoing[0];
        assert_eq!((fk.ref_table.as_str(), fk.columns.join(","), fk.ref_columns.join(",")), ("rel_parent", "parent_id".into(), "id".into()));
        assert_eq!(fk.on_delete, "CASCADE");
        assert_eq!(child.triggers.len(), 1, "triggers");
        assert_eq!(child.triggers[0].timing, "BEFORE");
        assert!(child.triggers[0].event.contains("INSERT"));
        assert!(child.triggers[0].definition.to_uppercase().contains("TRIGGER"));
        let parent = s.table_relations(schema, "rel_parent").await.unwrap();
        assert_eq!(parent.incoming.len(), 1, "incoming");
        assert_eq!(parent.incoming[0].table, "rel_child");
        let g = s.schema_graph(schema).await.unwrap();
        let child = g.tables.iter().find(|t| t.name == "rel_child").expect("table in graph");
        assert!(child.columns.iter().any(|c| c.name == "parent_id"));
        assert!(child.columns.iter().any(|c| c.name == "id" && c.primary_key));
        assert!(g.foreign_keys.iter().any(|f| f.table == "rel_child" && f.ref_table == "rel_parent"), "graph fks");
        s.run_script(teardown, 1).await.unwrap();
    }

    #[tokio::test]
    async fn postgres_relations() {
        let _serial = crate::test_lock::PG.lock().await;
        let Some(c) = cfg("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        check(
            &s,
            "public",
            "CREATE TABLE rel_parent (id serial PRIMARY KEY, name text); \
             CREATE TABLE rel_child (id serial PRIMARY KEY, parent_id int REFERENCES rel_parent(id) ON DELETE CASCADE, note text); \
             CREATE FUNCTION rel_touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.note := coalesce(NEW.note, 'x'); RETURN NEW; END $$; \
             CREATE TRIGGER rel_child_touch BEFORE INSERT ON rel_child FOR EACH ROW EXECUTE FUNCTION rel_touch();",
            "DROP TABLE IF EXISTS rel_child; DROP TABLE IF EXISTS rel_parent; DROP FUNCTION IF EXISTS rel_touch();",
        )
        .await;
    }

    #[tokio::test]
    async fn mysql_relations() {
        let _serial = crate::test_lock::MYSQL.lock().await;
        let Some(c) = cfg("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        let db = c.database.clone();
        check(
            &s,
            &db,
            "CREATE TABLE rel_parent (id INT AUTO_INCREMENT PRIMARY KEY, name TEXT); \
             CREATE TABLE rel_child (id INT AUTO_INCREMENT PRIMARY KEY, parent_id INT, note TEXT, \
               CONSTRAINT fk_rel FOREIGN KEY (parent_id) REFERENCES rel_parent(id) ON DELETE CASCADE); \
             CREATE TRIGGER rel_child_touch BEFORE INSERT ON rel_child FOR EACH ROW SET NEW.note = IFNULL(NEW.note, 'x');",
            "DROP TABLE IF EXISTS rel_child; DROP TABLE IF EXISTS rel_parent;",
        )
        .await;
    }

    #[tokio::test]
    async fn sqlite_relations() {
        let Some(c) = cfg("TABLA_TEST_SQLITE", Driver::Sqlite) else { return };
        let s = Session::connect(&c, None).await.unwrap();
        check(
            &s,
            "main",
            "CREATE TABLE rel_parent (id INTEGER PRIMARY KEY, name TEXT); \
             CREATE TABLE rel_child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES rel_parent(id) ON DELETE CASCADE, note TEXT); \
             CREATE TRIGGER rel_child_touch BEFORE INSERT ON rel_child BEGIN SELECT 1; END;",
            "DROP TABLE IF EXISTS rel_child; DROP TABLE IF EXISTS rel_parent;",
        )
        .await;
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    use std::{sync::Arc, time::Instant};

    fn cfg(var: &str, driver: Driver) -> Option<ConnectionConfig> {
        let v = std::env::var(var).ok()?;
        let p: Vec<&str> = v.split(':').collect();
        Some(
            serde_json::from_value(serde_json::json!({
                "id": "cancel-test", "name": "c", "driver": driver, "sslMode": "disable",
                "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
            }))
            .unwrap(),
        )
    }

    async fn cancel_roundtrip(s: Arc<Session>, sleep_sql: &str) -> (AppResult<Vec<ResultSet>>, u128) {
        let s2 = s.clone();
        let sql = sleep_sql.to_string();
        let started = Instant::now();
        let task = tokio::spawn(async move { s2.run_cancellable(&sql, 100, "q1", None).await });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(s.cancel("q1").await.unwrap(), "query was registered");
        let res = task.await.unwrap();
        let ms = started.elapsed().as_millis();
        assert!(!s.cancel("q1").await.unwrap(), "finished queries are unregistered");
        // The pool is still healthy afterwards.
        assert_eq!(s.run_script("SELECT 1", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("1"));
        (res, ms)
    }

    #[tokio::test]
    async fn postgres_cancel() {
        let Some(c) = cfg("TABLA_TEST_PG", Driver::Postgres) else { return };
        let s = Arc::new(Session::connect(&c, None).await.unwrap());
        let (res, ms) = cancel_roundtrip(s, "SELECT pg_sleep(30)").await;
        assert_eq!(res.unwrap_err().to_string(), "Query cancelled");
        assert!(ms < 5000, "took {ms} ms");
    }

    #[tokio::test]
    async fn mysql_cancel() {
        let Some(c) = cfg("TABLA_TEST_MYSQL", Driver::Mysql) else { return };
        let s = Arc::new(Session::connect(&c, None).await.unwrap());
        // MySQL's SLEEP() returns 1 instead of failing when interrupted.
        let (res, ms) = cancel_roundtrip(s, "SELECT SLEEP(30)").await;
        if let Ok(sets) = res {
            assert_eq!(sets[0].rows[0][0].as_deref(), Some("1"));
        }
        assert!(ms < 5000, "took {ms} ms");
    }
}
