//! Microsoft SQL Server / Azure SQL over TDS (tiberius).
//!
//! sqlx has no SQL Server driver, so this module provides the small pool and
//! the "every value as a display string" decoding the rest of `db.rs`
//! expects. Connections are plain TCP (optionally through the SSH tunnel)
//! with TLS negotiated by TDS itself.

use std::{sync::Mutex, time::Duration};

use futures_util::TryStreamExt;
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, QueryItem};
use tokio::{net::TcpStream, sync::Semaphore};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::{
    config::{ConnectionConfig, SslMode},
    db::{ColumnMeta, Progress, ResultSet},
    error::{AppError, AppResult},
};

pub type MsClient = Client<Compat<TcpStream>>;

const MAX_CONNECTIONS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BINARY_PREVIEW: usize = 2048;

pub struct MsPool {
    config: Config,
    idle: Mutex<Vec<MsClient>>,
    slots: Semaphore,
}

/// A pooled connection. It goes back to the pool on drop unless an I/O error
/// left it in an unknown state.
pub struct MsConn<'a> {
    pool: &'a MsPool,
    client: Option<MsClient>,
    broken: bool,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for MsConn<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.client.take()
            && !self.broken
        {
            self.pool.idle.lock().unwrap().push(c);
        }
    }
}

impl MsConn<'_> {
    pub fn client(&mut self) -> &mut MsClient {
        self.client.as_mut().expect("connection used after release")
    }

    /// Records whether an error means the connection can't be reused.
    /// Server errors (bad SQL, constraint violations) leave it healthy.
    pub fn check<T>(&mut self, r: Result<T, tiberius::error::Error>) -> AppResult<T> {
        if let Err(e) = &r
            && !matches!(e, tiberius::error::Error::Server(_))
        {
            self.broken = true;
        }
        Ok(r?)
    }

    pub fn discard(&mut self) {
        self.broken = true;
    }
}

pub fn build_config(cfg: &ConnectionConfig, host: &str, port: u16, cert_host: &str, password: Option<String>) -> Config {
    let mut c = Config::new();
    c.host(host);
    c.port(port);
    c.application_name("Tabla");
    c.authentication(AuthMethod::sql_server(&cfg.user, password.unwrap_or_default()));
    if !cfg.database.trim().is_empty() {
        c.database(cfg.database.trim());
    }
    let has_ca = !cfg.ssl_ca.trim().is_empty();
    match cfg.ssl_mode {
        SslMode::Disable => c.encryption(EncryptionLevel::NotSupported),
        // Encrypt when the server can, without verifying (libpq "prefer").
        SslMode::Prefer => {
            c.encryption(EncryptionLevel::On);
            if !has_ca {
                c.trust_cert();
            }
        }
        SslMode::Require => {
            c.encryption(EncryptionLevel::Required);
            if !has_ca {
                c.trust_cert();
            }
        }
        SslMode::VerifyFull => c.encryption(EncryptionLevel::Required),
    }
    if has_ca {
        c.trust_cert_ca(cfg.ssl_ca.trim());
    }
    // Through an SSH tunnel we dial 127.0.0.1 but the certificate names the real host.
    if host != cert_host {
        c.hostname_in_certificate(cert_host);
    }
    c
}

async fn dial(config: &Config) -> AppResult<MsClient> {
    let mut config = config.clone();
    // Azure SQL may redirect to another gateway once; follow it.
    for _ in 0..2 {
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(config.get_addr()))
            .await
            .map_err(|_| AppError::msg("Timed out connecting to SQL Server"))??;
        tcp.set_nodelay(true)?;
        match Client::connect(config.clone(), tcp.compat_write()).await {
            Ok(c) => return Ok(c),
            Err(tiberius::error::Error::Routing { host, port }) => {
                config.host(&host);
                config.port(port);
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::msg("SQL Server redirected the connection too many times"))
}

impl MsPool {
    pub async fn connect(config: Config) -> AppResult<Self> {
        let first = dial(&config).await?;
        Ok(MsPool { config, idle: Mutex::new(vec![first]), slots: Semaphore::new(MAX_CONNECTIONS) })
    }

    pub async fn acquire(&self) -> AppResult<MsConn<'_>> {
        let permit = tokio::time::timeout(Duration::from_secs(10), self.slots.acquire())
            .await
            .map_err(|_| AppError::msg("All SQL Server connections are busy; try again"))?
            .map_err(|_| AppError::msg("Connection closed"))?;
        let idle = self.idle.lock().unwrap().pop();
        let client = match idle {
            Some(c) => c,
            None => dial(&self.config).await?,
        };
        Ok(MsConn { pool: self, client: Some(client), broken: false, _permit: permit })
    }

    pub async fn close(&self) {
        self.slots.close();
        let idle: Vec<MsClient> = std::mem::take(&mut *self.idle.lock().unwrap());
        for c in idle {
            let _ = c.close().await;
        }
    }
}

const ROWCOUNT_COL: &str = "__tabla_rowcount";

/// Runs a batch as-is (so `SET`, `USE` and transaction statements behave as
/// in SSMS). For batches that only write, a trailing `SELECT @@ROWCOUNT`
/// reports how many rows the last statement changed.
pub async fn run(conn: &mut MsConn<'_>, sql: &str, max_rows: usize, progress: Progress<'_>) -> AppResult<Vec<ResultSet>> {
    // `GO` is a client-side batch separator (sqlcmd/SSMS), not T-SQL.
    let batches = split_go(sql);
    if batches.len() > 1 {
        let mut all = Vec::new();
        for b in batches {
            all.extend(run_batch(conn, b, max_rows, progress).await?);
        }
        return Ok(all);
    }
    run_batch(conn, sql, max_rows, progress).await
}

/// Splits a script on lines that contain only `GO` (optionally `GO -- note`).
pub fn split_go(sql: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut pos = 0;
    for line in sql.split_inclusive('\n') {
        let t = line.trim();
        let word = t.split("--").next().unwrap_or("").trim();
        if word.eq_ignore_ascii_case("go") {
            let chunk = &sql[start..pos];
            if !chunk.trim().is_empty() {
                out.push(chunk);
            }
            start = pos + line.len();
        }
        pos += line.len();
    }
    let tail = &sql[start..];
    if !tail.trim().is_empty() {
        out.push(tail);
    }
    out
}

async fn run_batch(conn: &mut MsConn<'_>, sql: &str, max_rows: usize, progress: Progress<'_>) -> AppResult<Vec<ResultSet>> {
    let probe = !returns_rows(sql) && !is_module_ddl(sql);
    let batch = if probe { format!("{sql}\n;SELECT @@ROWCOUNT AS {ROWCOUNT_COL}") } else { sql.to_string() };
    let sql = batch.as_str();
    let (sets, failure, stopped) = {
        let mut sets: Vec<ResultSet> = Vec::new();
        let mut total = 0usize;
        let mut stopped = false;
        let mut failure = None;
        match conn.client().simple_query(sql).await {
            Err(e) => failure = Some(e),
            Ok(mut stream) => loop {
                match stream.try_next().await {
                    Ok(Some(QueryItem::Metadata(m))) => sets.push(ResultSet {
                        columns: m.columns().iter().map(|c| ColumnMeta { name: c.name().to_string(), type_name: type_name(c.column_type()) }).collect(),
                        ..Default::default()
                    }),
                    Ok(Some(QueryItem::Row(row))) => {
                        let Some(cur) = sets.last_mut() else { continue };
                        if cur.rows.len() >= max_rows {
                            cur.truncated = true;
                            stopped = true;
                            break;
                        }
                        cur.rows.push(row.cells().map(|(_, d)| cell(d)).collect());
                        total += 1;
                        if let Some(p) = progress
                            && total.is_multiple_of(1000)
                        {
                            p(total);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            },
        }
        (sets, failure, stopped)
    };
    if let Some(e) = failure {
        return Err(conn.check::<()>(Err(e)).unwrap_err());
    }
    if stopped {
        // The rest of the response is still on the wire; don't reuse this connection.
        conn.discard();
    }
    let mut sets = sets;
    if probe {
        let count = match sets.last() {
            Some(last) if last.columns.len() == 1 && last.columns[0].name == ROWCOUNT_COL => {
                let n = last.rows.first().and_then(|r| r.first().cloned().flatten()).and_then(|v| v.parse().ok()).unwrap_or(0);
                sets.pop();
                n
            }
            _ => 0,
        };
        if sets.is_empty() {
            sets.push(ResultSet { rows_affected: count, ..Default::default() });
        }
    }
    Ok(sets)
}

/// CREATE/ALTER PROCEDURE, FUNCTION, TRIGGER and VIEW must be alone in their batch.
fn is_module_ddl(sql: &str) -> bool {
    let cleaned = strip_comments(sql).to_ascii_lowercase();
    let mut words = cleaned.split_whitespace();
    let (a, mut b) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    if b == "or" {
        // CREATE OR ALTER …
        words.next();
        b = words.next().unwrap_or("");
    }
    matches!(a, "create" | "alter") && matches!(b, "procedure" | "proc" | "function" | "trigger" | "view")
}

/// Statement keywords whose batches produce result sets.
fn returns_rows(sql: &str) -> bool {
    let cleaned = strip_comments(sql).to_ascii_lowercase();
    cleaned.split(';').any(|stmt| {
        let first = stmt.split_whitespace().next().unwrap_or("");
        matches!(first, "select" | "with" | "exec" | "execute" | "declare" | "if" | "begin" | "values" | "while")
            || first.starts_with("sp_")
            || stmt.contains(" output ")
    })
}

fn strip_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut it = sql.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = it.next() {
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                out.push(c);
            }
            '[' => {
                quote = Some(']');
                out.push(c);
            }
            '-' if it.peek() == Some(&'-') => {
                for n in it.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if it.peek() == Some(&'*') => {
                it.next();
                let mut prev = ' ';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn type_name(t: ColumnType) -> String {
    match t {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money | ColumnType::Money4 => "money",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln | ColumnType::Numericn => "decimal",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigBinary => "binary",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
    .to_string()
}

pub fn cell(d: &ColumnData<'static>) -> Option<String> {
    use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
    match d {
        ColumnData::U8(v) => v.map(|x| x.to_string()),
        ColumnData::I16(v) => v.map(|x| x.to_string()),
        ColumnData::I32(v) => v.map(|x| x.to_string()),
        ColumnData::I64(v) => v.map(|x| x.to_string()),
        ColumnData::F32(v) => v.map(|x| x.to_string()),
        ColumnData::F64(v) => v.map(|x| x.to_string()),
        ColumnData::Bit(v) => v.map(|b| if b { "1" } else { "0" }.to_string()),
        ColumnData::String(v) => v.as_ref().map(|s| s.to_string()),
        ColumnData::Guid(v) => v.map(|g| g.to_string().to_uppercase()),
        ColumnData::Binary(v) => v.as_ref().map(|b| {
            let mut s = String::from("0x");
            for byte in b.iter().take(MAX_BINARY_PREVIEW) {
                s.push_str(&format!("{byte:02X}"));
            }
            if b.len() > MAX_BINARY_PREVIEW {
                s.push('…');
            }
            s
        }),
        ColumnData::Numeric(v) => v.map(|n| n.to_string()),
        ColumnData::Xml(v) => v.as_ref().map(|x| x.to_string()),
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            NaiveDateTime::from_sql(d).ok().flatten().map(|t| t.format("%Y-%m-%d %H:%M:%S%.f").to_string())
        }
        ColumnData::Date(_) => NaiveDate::from_sql(d).ok().flatten().map(|t| t.format("%Y-%m-%d").to_string()),
        ColumnData::Time(_) => NaiveTime::from_sql(d).ok().flatten().map(|t| t.format("%H:%M:%S%.f").to_string()),
        ColumnData::DateTimeOffset(_) => {
            DateTime::<FixedOffset>::from_sql(d).ok().flatten().map(|t| t.format("%Y-%m-%d %H:%M:%S%.f %:z").to_string())
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn detects_row_returning_batches() {
        assert!(returns_rows("SELECT 1"));
        assert!(returns_rows("  -- comment\n with x as (select 1) select * from x"));
        assert!(returns_rows("UPDATE t SET a = 1; SELECT @@ROWCOUNT"));
        assert!(returns_rows("EXEC sp_who"));
        assert!(!returns_rows("UPDATE t SET a = 'select'"));
        assert!(!returns_rows("/* select */ DELETE FROM t"));
        assert!(!returns_rows("INSERT INTO t (a) VALUES (1)"));
        assert!(!returns_rows("CREATE TABLE [select] (a int)"));
    }

    #[test]
    fn splits_go_batches() {
        assert_eq!(split_go("SELECT 1"), vec!["SELECT 1"]);
        assert_eq!(split_go("CREATE VIEW v AS SELECT 1\nGO\nSELECT * FROM v\n  go -- done\n"), vec!["CREATE VIEW v AS SELECT 1\n", "SELECT * FROM v\n"]);
        assert_eq!(split_go("SELECT 'GO'\nGO\n"), vec!["SELECT 'GO'\n"]);
        assert_eq!(split_go("SELECT 1 AS go_live"), vec!["SELECT 1 AS go_live"]);
    }

    #[test]
    fn detects_module_ddl() {
        assert!(is_module_ddl("CREATE PROCEDURE p AS SELECT 1"));
        assert!(is_module_ddl("create or alter view v as select 1"));
        assert!(is_module_ddl("-- note\nALTER TRIGGER t ON x AFTER INSERT AS BEGIN END"));
        assert!(!is_module_ddl("CREATE TABLE t (a int)"));
    }
}

#[cfg(test)]
mod integration {
    //! Runs against a real SQL Server when TABLA_TEST_MSSQL="host:port:user:database"
    //! and TABLA_TEST_MSSQL_PASSWORD are set (CI uses a throwaway container).
    use std::sync::Arc;

    use crate::{
        alter::{AlterRequest, ColumnChange, ColumnDef},
        config::{ConnectionConfig, Driver},
        db::{CellValue, FetchArgs, RowEdit, Session, SortSpec},
    };

    fn cfg() -> Option<(ConnectionConfig, String)> {
        let v = std::env::var("TABLA_TEST_MSSQL").ok()?;
        let pw = std::env::var("TABLA_TEST_MSSQL_PASSWORD").ok()?;
        let p: Vec<&str> = v.split(':').collect();
        let c = serde_json::from_value(serde_json::json!({
            "id": "mssql-test", "name": "m", "driver": Driver::Mssql, "sslMode": "require",
            "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
        }))
        .unwrap();
        Some((c, pw))
    }

    const SETUP: &str = "
        IF OBJECT_ID('dbo.ms_orders_v') IS NOT NULL DROP VIEW dbo.ms_orders_v;
        IF OBJECT_ID('dbo.ms_orders') IS NOT NULL DROP TABLE dbo.ms_orders;
        IF OBJECT_ID('dbo.ms_products') IS NOT NULL DROP TABLE dbo.ms_products;
        IF OBJECT_ID('dbo.ms_empty') IS NOT NULL DROP TABLE dbo.ms_empty;
        CREATE TABLE dbo.ms_products (
          id int IDENTITY(1,1) PRIMARY KEY, sku nvarchar(32) UNIQUE, title nvarchar(200) DEFAULT N'untitled',
          price decimal(10,2), stock int, img varbinary(16), flag bit, uid uniqueidentifier,
          made datetime2(3) DEFAULT SYSDATETIME(), born date, at datetimeoffset(0)
        );
        CREATE TABLE dbo.ms_orders (
          id int IDENTITY(1,1) PRIMARY KEY, product_id int NOT NULL,
          qty int NOT NULL DEFAULT 1,
          CONSTRAINT fk_orders_product FOREIGN KEY (product_id) REFERENCES dbo.ms_products(id) ON DELETE CASCADE
        );
        CREATE INDEX ix_orders_product ON dbo.ms_orders (product_id);
        CREATE TABLE dbo.ms_empty (a int, b nvarchar(10));
        INSERT INTO dbo.ms_products (sku, title, price, stock, img, flag, uid, born, at) VALUES
          (N'A1', N'Keyboard', 49.90, 10, 0x89504E47, 1, '6F9619FF-8B86-D011-B42D-00C04FC964FF', '2024-02-29', '2024-01-02 03:04:05 +07:00'),
          (N'B2', N'Mouse', 19.00, NULL, NULL, 0, NULL, NULL, NULL),
          (N'C3', N'Monitor ''27\"', 299.00, 3, NULL, NULL, NULL, NULL, NULL);
        INSERT INTO dbo.ms_orders (product_id, qty) VALUES (1, 2), (1, 1), (3, 5);
    ";

    /// Connects and queries on a default 2 MB test-thread stack, plain and with TLS.
    #[tokio::test]
    async fn mssql_connect_default_stack() {
        let Some((mut c, pw)) = cfg() else { return };
        for mode in [crate::config::SslMode::Disable, crate::config::SslMode::Require] {
            eprintln!("--> mssql step: connect {mode:?}");
            c.ssl_mode = mode;
            let s = Session::connect(&c, Some(pw.clone())).await.unwrap();
            eprintln!("--> mssql step: query {mode:?}");
            assert_eq!(s.run_script("SELECT 1", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("1"));
            s.close().await;
        }
    }

    #[test]
    fn mssql_end_to_end() {
        crate::test_lock::big_stack(end_to_end());
    }

    async fn end_to_end() {
        let Some((c, pw)) = cfg() else { return };
        eprintln!("--> mssql step: e2e start");
        let s = Session::connect(&c, Some(pw.clone())).await.unwrap();
        assert!(!s.server_version().await.unwrap().is_empty());
        s.run_script(SETUP, 10).await.unwrap();
        s.run_script("CREATE VIEW dbo.ms_orders_v AS SELECT o.id, p.title FROM dbo.ms_orders o JOIN dbo.ms_products p ON p.id = o.product_id", 1)
            .await
            .unwrap();
        s.run_script(
            "CREATE OR ALTER TRIGGER dbo.trg_orders_qty ON dbo.ms_orders AFTER UPDATE AS BEGIN SET NOCOUNT ON; END",
            1,
        )
        .await
        .unwrap();

        assert_eq!(s.current_schema().await.unwrap().as_deref(), Some("dbo"));
        assert_eq!(s.list_schemas().await.unwrap().first().map(String::as_str), Some("dbo"));
        let tables = s.list_tables("dbo").await.unwrap();
        let p = tables.iter().find(|t| t.name == "ms_products").expect("table listed");
        assert_eq!((p.kind.as_str(), p.rows_estimate), ("table", Some(3)));
        assert_eq!(tables.iter().find(|t| t.name == "ms_orders_v").unwrap().kind, "view");

        eprintln!("--> mssql step: Structure");
        // Structure
        let st = s.table_structure("dbo", "ms_products").await.unwrap();
        let col = |n: &str| st.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!((col("id").key.as_str(), col("id").extra.as_str(), col("id").primary_key), ("PRI", "identity", true));
        assert_eq!(col("sku").key, "UNI");
        assert_eq!(col("sku").data_type, "nvarchar(32)");
        assert_eq!(col("price").data_type, "decimal(10,2)");
        assert_eq!(col("title").default_value.as_deref(), Some("N'untitled'"));
        assert!(st.indexes.iter().any(|i| i.primary && i.columns == "id"));

        eprintln!("--> mssql step: Values decode as display strin");
        // Values decode as display strings.
        let sets = s.run_script("SELECT price, img, flag, uid, made, born, at, title FROM dbo.ms_products ORDER BY id", 100).await.unwrap();
        let r = &sets[0].rows;
        assert_eq!(r[0][0].as_deref(), Some("49.90"));
        assert_eq!(r[0][1].as_deref(), Some("0x89504E47"));
        assert_eq!(r[0][2].as_deref(), Some("1"));
        assert_eq!(r[0][3].as_deref(), Some("6F9619FF-8B86-D011-B42D-00C04FC964FF"));
        assert!(r[0][4].as_deref().unwrap().contains('-'));
        assert_eq!(r[0][5].as_deref(), Some("2024-02-29"));
        assert_eq!(r[0][6].as_deref(), Some("2024-01-02 03:04:05 +07:00"));
        assert!(r[1][1].is_none());
        assert_eq!(r[2][7].as_deref(), Some("Monitor '27\""));
        assert_eq!(sets[0].columns[0].type_name, "decimal");

        // Multiple result sets; empty ones still carry columns. Write-only batches report counts.
        let sets = s.run_script("SELECT TOP 1 * FROM dbo.ms_products; SELECT * FROM dbo.ms_empty", 100).await.unwrap();
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[1].columns.len(), 2);
        let upd = s.run_script("UPDATE dbo.ms_products SET stock = stock", 1).await.unwrap();
        assert_eq!(upd[0].rows_affected, 3);
        assert_eq!(s.run_script("SET NOCOUNT OFF", 1).await.unwrap().len(), 1);

        eprintln!("--> mssql step: Paging ");
        // Paging (with and without an explicit sort), filter and count.
        let page = s
            .fetch_rows(&FetchArgs { schema: "dbo".into(), table: "ms_products".into(), filter: None, sort: Some(SortSpec { column: "price".into(), desc: true }), limit: 2, offset: 0 })
            .await
            .unwrap();
        assert_eq!(page.total, Some(3));
        assert_eq!(page.result.rows.len(), 2);
        let page = s
            .fetch_rows(&FetchArgs { schema: "dbo".into(), table: "ms_empty".into(), filter: Some("[a] = 1".into()), sort: None, limit: 50, offset: 0 })
            .await
            .unwrap();
        assert_eq!((page.total, page.result.columns.len()), (Some(0), 2));

        eprintln!("--> mssql step: Edits are bound");
        // Edits are bound parameters inside one transaction.
        let key = |v: &str| vec![CellValue { column: "id".into(), value: Some(v.into()) }];
        let n = s
            .apply_edits("dbo", "ms_products", vec![RowEdit::Update { key: key("2"), values: vec![CellValue { column: "title".into(), value: Some("Édité ✓".into()) }, CellValue { column: "price".into(), value: Some("21.50".into()) }] }])
            .await
            .unwrap();
        assert_eq!(n, 1);
        let back = s.run_script("SELECT title, price FROM dbo.ms_products WHERE id = 2", 1).await.unwrap();
        assert_eq!(back[0].rows[0], vec![Some("Édité ✓".into()), Some("21.50".into())]);
        let n = s.apply_edits("dbo", "ms_products", vec![RowEdit::Insert { values: vec![CellValue { column: "sku".into(), value: Some("D4".into()) }] }]).await.unwrap();
        assert_eq!(n, 1);
        let err = s.apply_edits("dbo", "ms_products", vec![RowEdit::Update { key: key("2"), values: vec![CellValue { column: "title".into(), value: Some("x".into()) }] }, RowEdit::Delete { key: key("-999") }]).await;
        assert!(err.is_err());
        let still = s.run_script("SELECT title FROM dbo.ms_products WHERE id = 2", 1).await.unwrap();
        assert_eq!(still[0].rows[0][0].as_deref(), Some("Édité ✓"), "failed batch rolled back");
        // A server error surfaces the plain message and leaves the pool usable.
        let err = s.run_script("SELECT * FROM nope", 1).await.unwrap_err().to_string();
        assert_eq!(err, "Invalid object name 'nope'.");

        eprintln!("--> mssql step: Relations");
        // Relations, triggers, ERD
        let rel = s.table_relations("dbo", "ms_orders").await.unwrap();
        assert_eq!(rel.outgoing.len(), 1);
        let fk = &rel.outgoing[0];
        assert_eq!((fk.ref_table.as_str(), fk.columns.join(","), fk.on_delete.as_str()), ("ms_products", "product_id".into(), "CASCADE"));
        assert_eq!(rel.triggers.len(), 1);
        assert_eq!(rel.triggers[0].event, "UPDATE");
        assert_eq!(s.table_relations("dbo", "ms_products").await.unwrap().incoming.len(), 1);
        let g = s.schema_graph("dbo").await.unwrap();
        assert!(g.foreign_keys.iter().any(|f| f.name == "fk_orders_product"));
        assert!(g.tables.iter().find(|t| t.name == "ms_products").unwrap().columns.iter().any(|c| c.primary_key));

        eprintln!("--> mssql step: Info view and DDL");
        // Info view and DDL
        let info = s.table_details("dbo", "ms_orders").await.unwrap();
        assert!(info.total_bytes.unwrap() > 0);
        let ddl = info.ddl.unwrap();
        assert!(ddl.starts_with("CREATE TABLE [dbo].[ms_orders]"), "{ddl}");
        assert!(ddl.contains("IDENTITY(1,1)") && ddl.contains("CREATE INDEX [ix_orders_product]"), "{ddl}");
        assert!(s.table_details("dbo", "ms_orders_v").await.unwrap().ddl.unwrap().contains("CREATE VIEW"));

        eprintln!("--> mssql step: Structure editing");
        // Structure editing, including default constraints.
        let def = |n: &str, t: &str, null: bool, d: Option<&str>| ColumnDef { name: n.into(), data_type: t.into(), nullable: null, default_value: d.map(Into::into) };
        let req = |changes| AlterRequest { schema: "dbo".into(), table: "ms_orders".into(), changes, order: vec![] };
        s.apply_alter(&req(vec![ColumnChange { original: None, column: Some(def("note", "nvarchar(20)", false, Some("N'none'"))) }])).await.unwrap();
        let planned = s
            .plan_alter(&req(vec![ColumnChange { original: Some(def("note", "nvarchar(20)", false, Some("N'none'"))), column: Some(def("memo", "nvarchar(50)", true, Some("N'-'"))) }]))
            .await
            .unwrap();
        assert!(planned[0].starts_with("EXEC sp_rename"), "{planned:?}");
        assert!(planned.iter().any(|p| p.contains("DROP CONSTRAINT")), "{planned:?}");
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("note", "nvarchar(20)", false, Some("N'none'"))), column: Some(def("memo", "nvarchar(50)", true, Some("N'-'"))) }]))
            .await
            .unwrap();
        let st = s.table_structure("dbo", "ms_orders").await.unwrap();
        let memo = st.columns.iter().find(|c| c.name == "memo").unwrap();
        assert_eq!((memo.data_type.as_str(), memo.nullable, memo.default_value.as_deref()), ("nvarchar(50)", true, Some("N'-'")));
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("memo", "nvarchar(50)", true, Some("N'-'"))), column: None }])).await.unwrap();
        // A failing change rolls back the whole batch.
        let bad = s
            .apply_alter(&req(vec![
                ColumnChange { original: None, column: Some(def("tmp", "int", true, None)) },
                ColumnChange { original: None, column: Some(def("tmp2", "notatype", true, None)) },
            ]))
            .await;
        assert!(bad.is_err());
        assert!(!s.table_structure("dbo", "ms_orders").await.unwrap().columns.iter().any(|c| c.name == "tmp"));

        eprintln!("--> mssql step: AI queries always");
        // AI queries always roll back.
        let _ = s.run_readonly("DELETE FROM dbo.ms_orders", 10).await;
        let left = s.run_readonly("SELECT COUNT(*) FROM dbo.ms_orders", 10).await.unwrap();
        assert_eq!(left.rows[0][0].as_deref(), Some("3"));

        eprintln!("--> mssql step: Read-only connections");
        // Read-only connections never persist writes.
        let mut ro = c.clone();
        ro.read_only = true;
        let r = Session::connect(&ro, Some(pw)).await.unwrap();
        r.run_script("DELETE FROM dbo.ms_orders", 1).await.unwrap();
        assert!(r.run_script("BEGIN TRAN; DELETE FROM dbo.ms_orders; COMMIT", 1).await.is_err());
        assert!(r.apply_edits("dbo", "ms_orders", vec![RowEdit::Delete { key: key("1") }]).await.is_err());
        let left = s.run_script("SELECT COUNT(*) FROM dbo.ms_orders", 1).await.unwrap();
        assert_eq!(left[0].rows[0][0].as_deref(), Some("3"));

        assert!(!s.list_users().await.unwrap().is_empty());
        s.run_script("DROP VIEW dbo.ms_orders_v; DROP TABLE dbo.ms_orders; DROP TABLE dbo.ms_products; DROP TABLE dbo.ms_empty", 1).await.unwrap();
        s.close().await;
    }

    #[test]
    fn mssql_cancel() {
        crate::test_lock::big_stack(cancel());
    }

    async fn cancel() {
        let Some((c, pw)) = cfg() else { return };
        let s = Arc::new(Session::connect(&c, Some(pw)).await.unwrap());
        let s2 = s.clone();
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move { s2.run_cancellable("WAITFOR DELAY '00:00:30'", 10, "q1", None).await });
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        assert!(s.cancel("q1").await.unwrap());
        let res = task.await.unwrap();
        assert_eq!(res.unwrap_err().to_string(), "Query cancelled");
        assert!(started.elapsed().as_secs() < 10);
        assert!(!s.cancel("q1").await.unwrap());
        assert_eq!(s.run_script("SELECT 1", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("1"));
    }
}

