//! Oracle Database through Oracle's own client library (Instant Client),
//! which ODPI-C loads at runtime. Tabla can download Instant Client itself
//! (see `oracle_client`), or use one already installed.
//!
//! The Oracle API is synchronous, so each call runs on a blocking thread with
//! a connection from a small pool. Every session uses ISO date formats, so
//! dates and timestamps read as text can be written back as text. Script
//! statements commit one by one, like the other engines; edits commit together.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use oracle::{Connection, Connector, sql_type::OracleType};

use crate::{
    config::{ConnectionConfig, SslMode},
    db::{ColumnMeta, Progress, ResultSet},
    error::{AppError, AppResult},
};

const POOL_IDLE: usize = 4;

/// Session settings applied to every connection: unambiguous, round-trippable formats.
const SESSION_SETUP: &str = "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD HH24:MI:SS' \
     NLS_TIMESTAMP_FORMAT = 'YYYY-MM-DD HH24:MI:SS.FF' NLS_TIMESTAMP_TZ_FORMAT = 'YYYY-MM-DD HH24:MI:SS.FF TZH:TZM' \
     NLS_NUMERIC_CHARACTERS = '.,'";

struct Spec {
    user: String,
    password: String,
    connect_string: String,
}

pub struct OraPool {
    inner: Arc<Inner>,
}

struct Inner {
    spec: Spec,
    idle: Mutex<Vec<Connection>>,
}

/// A pooled connection; returned to the pool when dropped unless discarded.
pub struct OraConn {
    conn: Option<Arc<Connection>>,
    inner: Arc<Inner>,
    discard: bool,
}

impl OraConn {
    pub fn get(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }

    /// A handle that can interrupt this connection's running call from another thread.
    pub fn handle(&self) -> Arc<Connection> {
        self.conn.clone().unwrap()
    }

    pub fn discard(&mut self) {
        self.discard = true;
    }
}

impl Drop for OraConn {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        // Only a connection no cancel handle still points to goes back to the pool.
        let Ok(conn) = Arc::try_unwrap(conn) else { return };
        if self.discard || conn.rollback().is_err() {
            return;
        }
        let mut idle = self.inner.idle.lock().unwrap();
        if idle.len() < POOL_IDLE {
            idle.push(conn);
        }
    }
}

/// `host:port/service` for EZConnect, or the database field verbatim when it
/// is a full connect descriptor `(DESCRIPTION=…)` or a TNS alias.
pub fn connect_string(cfg: &ConnectionConfig, host: &str, port: u16) -> String {
    let db = cfg.database.trim();
    if db.starts_with('(') || (!db.is_empty() && cfg.host.trim().is_empty()) {
        return db.to_string();
    }
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let service = if db.is_empty() { "FREEPDB1" } else { db };
    if cfg.ssl_mode == SslMode::Disable {
        format!("//{host}:{port}/{service}")
    } else {
        // TCPS. The server certificate is checked against the system's trusted CAs,
        // and its name against the host unless the mode is plain "require".
        let dn = if cfg.ssl_mode == SslMode::VerifyFull { "true" } else { "false" };
        format!("tcps://{host}:{port}/{service}?ssl_server_dn_match={dn}")
    }
}

impl OraPool {
    pub async fn connect(cfg: &ConnectionConfig, host: &str, port: u16, password: Option<String>) -> AppResult<Self> {
        crate::oracle_client::init()?;
        let spec = Spec { user: cfg.user.trim().to_string(), password: password.unwrap_or_default(), connect_string: connect_string(cfg, host, port) };
        let inner = Arc::new(Inner { spec, idle: Mutex::new(Vec::new()) });
        let pool = OraPool { inner };
        // Fail fast with the real error (bad password, unknown service…).
        pool.with_conn(|_| Ok(())).await?;
        Ok(pool)
    }

    /// Runs `f` with a pooled connection on a blocking thread.
    pub async fn with_conn<T: Send + 'static>(&self, f: impl FnOnce(&mut OraConn) -> AppResult<T> + Send + 'static) -> AppResult<T> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut c = Inner::acquire(&inner)?;
            let out = f(&mut c);
            if out.as_ref().is_err_and(is_fatal) {
                c.discard();
            }
            out
        })
        .await
        .map_err(|e| AppError::msg(e.to_string()))?
    }

    /// Runs a script; `on_start` receives a handle for cancelling it.
    pub async fn run(&self, sql: &str, max_rows: usize, read_only: bool, progress: Progress<'_>, on_start: impl FnOnce(Arc<Connection>)) -> AppResult<Vec<ResultSet>> {
        let inner = self.inner.clone();
        let mut conn = tokio::task::spawn_blocking(move || Inner::acquire(&inner)).await.map_err(|e| AppError::msg(e.to_string()))??;
        on_start(conn.handle());
        let sql = sql.to_string();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let mut task = tokio::task::spawn_blocking(move || {
            let out = run_script(conn.get(), &sql, max_rows, read_only, &count);
            if out.as_ref().is_err_and(is_fatal) {
                conn.discard();
            }
            out
        });
        let mut reported = 0;
        loop {
            tokio::select! {
                res = &mut task => return res.map_err(|e| AppError::msg(e.to_string()))?,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    let n = seen.load(Ordering::Relaxed);
                    if let (Some(p), true) = (progress, n != reported) {
                        p(n);
                        reported = n;
                    }
                }
            }
        }
    }

    /// A catalog query with named binds (e.g. `:s` for the schema). Binding
    /// instead of inlining names lets Oracle parse each query once and reuse
    /// it for every table; parsing dictionary queries is slow.
    pub async fn query_named(&self, sql: &str, binds: Vec<(&'static str, String)>) -> AppResult<ResultSet> {
        let sql = sql.to_string();
        self.with_conn(move |c| {
            let used: Vec<(&str, &dyn oracle::sql_type::ToSql)> =
                binds.iter().filter(|(n, _)| sql.contains(&format!(":{n}"))).map(|(n, v)| (*n, v as &dyn oracle::sql_type::ToSql)).collect();
            let mut stmt = c.get().statement(&sql).build().map_err(err)?;
            collect(stmt.query_named(&used).map_err(err)?, usize::MAX, &AtomicUsize::new(0))
        })
        .await
    }

    pub async fn server_version(&self) -> AppResult<String> {
        self.with_conn(|c| {
            let (v, banner) = c.get().server_version().map_err(err)?;
            // "23.26.3.0.0 Free" from the first banner line's edition.
            let edition = ["Enterprise", "Standard", "Express", "Free", "Personal"].into_iter().find(|e| banner.contains(e));
            Ok(match edition {
                Some(e) => format!("{v} {e}"),
                None => v.to_string(),
            })
        })
        .await
    }

    /// One read statement for an AI assistant: checked by Oracle's own parser
    /// to be a query, run in a read-only transaction that is always rolled
    /// back, and stopped after 15 seconds.
    pub async fn run_readonly(&self, sql: &str, max_rows: usize) -> AppResult<ResultSet> {
        let stmts = split_statements(sql);
        let [stmt] = stmts.as_slice() else { return Err(AppError::msg("Only one statement per call is allowed")) };
        let stmt = stmt.text.clone();
        self.with_conn(move |c| {
            let conn = c.get();
            conn.execute("SET TRANSACTION READ ONLY", &[]).map_err(err)?;
            conn.set_call_timeout(Some(Duration::from_secs(15))).map_err(err)?;
            let out = query_only(conn, &stmt, max_rows, &AtomicUsize::new(0));
            let _ = conn.set_call_timeout(None);
            let _ = conn.rollback();
            out.map_err(|e| if e.to_string().contains("DPI-1067") { AppError::msg("Query took longer than 15 seconds and was stopped") } else { e })
        })
        .await
    }
}

impl Inner {
    fn acquire(this: &Arc<Inner>) -> AppResult<OraConn> {
        let pooled = this.idle.lock().unwrap().pop();
        let conn = match pooled {
            // Idle connections may have been dropped by the server or a firewall.
            Some(c) if c.ping().is_ok() => c,
            _ => this.open()?,
        };
        Ok(OraConn { conn: Some(Arc::new(conn)), inner: this.clone(), discard: false })
    }

    fn open(&self) -> AppResult<Connection> {
        let s = &self.spec;
        let conn = Connector::new(&s.user, &s.password, &s.connect_string).driver_name("Tabla").connect().map_err(err)?;
        conn.execute(SESSION_SETUP, &[]).map_err(err)?;
        Ok(conn)
    }
}

pub fn err(e: oracle::Error) -> AppError {
    AppError::msg(message(&e))
}

/// Oracle's message without the driver's "OCI Error:" prefix or the help link.
pub fn message(e: &oracle::Error) -> String {
    let raw = match e.db_error() {
        Some(db) => db.message().to_string(),
        None => e.to_string(),
    };
    let raw = raw.strip_prefix("OCI Error: ").unwrap_or(&raw).to_string();
    raw.lines().filter(|l| !l.starts_with("Help: https://docs.oracle.com/")).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Errors after which the connection can't be reused.
fn is_fatal(e: &AppError) -> bool {
    let m = e.to_string();
    ["ORA-03113", "ORA-03114", "ORA-03135", "ORA-02396", "DPI-1010", "DPI-1080", "ORA-01012", "ORA-00028"].iter().any(|c| m.contains(c))
}

/// A statement from a script, and whether it's a PL/SQL block.
#[derive(Debug, PartialEq)]
pub struct Stmt {
    pub text: String,
    pub plsql: bool,
}

/// Splits a script the way SQL*Plus does: SQL statements end at `;`, PL/SQL
/// blocks (BEGIN, DECLARE, CREATE PROCEDURE/FUNCTION/PACKAGE/TRIGGER/TYPE)
/// end at a line holding only `/`. Quotes, q'[…]' literals and comments are
/// respected. SQL statements lose their `;`, which Oracle would reject.
pub fn split_statements(sql: &str) -> Vec<Stmt> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    let mut plsql: Option<bool> = None;
    let mut push = |text: &str, plsql: bool| {
        let t = text.trim();
        if !t.is_empty() && !first_words(t, 1).is_empty() {
            out.push(Stmt { text: t.to_string(), plsql });
        }
    };
    while i < b.len() {
        if plsql.is_none() && !b[i].is_ascii_whitespace() && !starts_comment(b, i) {
            plsql = Some(is_plsql_start(&sql[i..]));
        }
        match b[i] {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            // q'[...]', q'{...}', q'<...>', q'(...)' or q'X...X'
            b'q' | b'Q' if b.get(i + 1) == Some(&b'\'') && b.get(i + 2).is_some() && (i == 0 || !is_word(b[i - 1])) => {
                let open = b[i + 2];
                let close = match open {
                    b'[' => b']',
                    b'{' => b'}',
                    b'<' => b'>',
                    b'(' => b')',
                    c => c,
                };
                i += 3;
                while i + 1 < b.len() && !(b[i] == close && b[i + 1] == b'\'') {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1;
            }
            b';' if plsql == Some(false) => {
                push(&sql[start..i], false);
                start = i + 1;
                plsql = None;
                i += 1;
            }
            // A line with only "/" ends the current statement or block.
            b'/' if line_is_slash(sql, i) => {
                push(&sql[start..i], plsql == Some(true));
                let eol = sql[i..].find('\n').map(|p| i + p + 1).unwrap_or(b.len());
                start = eol;
                plsql = None;
                i = eol;
            }
            _ => i += 1,
        }
    }
    push(&sql[start.min(sql.len())..], plsql == Some(true));
    out
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'#'
}

fn starts_comment(b: &[u8], i: usize) -> bool {
    (b[i] == b'-' && b.get(i + 1) == Some(&b'-')) || (b[i] == b'/' && b.get(i + 1) == Some(&b'*'))
}

fn line_is_slash(sql: &str, i: usize) -> bool {
    let line_start = sql[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = sql[i..].find('\n').map(|p| i + p).unwrap_or(sql.len());
    sql[line_start..line_end].trim() == "/"
}

/// The first `n` words, uppercased, skipping comments.
fn first_words(sql: &str, n: usize) -> Vec<String> {
    let mut s = sql;
    let mut words = Vec::new();
    while words.len() < n {
        s = s.trim_start();
        if s.starts_with("--") {
            s = s.find('\n').map(|p| &s[p..]).unwrap_or("");
            continue;
        }
        if s.starts_with("/*") {
            s = s.find("*/").map(|p| &s[p + 2..]).unwrap_or("");
            continue;
        }
        let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
        if end == 0 {
            break;
        }
        words.push(s[..end].to_ascii_uppercase());
        s = &s[end..];
    }
    words
}

fn is_plsql_start(sql: &str) -> bool {
    let w = first_words(sql, 6);
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    match w.as_slice() {
        ["BEGIN", ..] | ["DECLARE", ..] => true,
        ["CREATE", rest @ ..] => {
            let rest: Vec<&str> = rest.iter().copied().filter(|x| !matches!(*x, "OR" | "REPLACE" | "EDITIONABLE" | "NONEDITIONABLE")).collect();
            matches!(rest.first(), Some(&("PROCEDURE" | "FUNCTION" | "PACKAGE" | "TRIGGER" | "TYPE" | "LIBRARY")))
        }
        _ => false,
    }
}

pub fn run_script(conn: &Connection, sql: &str, max_rows: usize, read_only: bool, count: &AtomicUsize) -> AppResult<Vec<ResultSet>> {
    let mut sets = Vec::new();
    for s in split_statements(sql) {
        let mut stmt = conn.statement(&s.text).build().map_err(err)?;
        if stmt.is_query() {
            sets.push(fetch(&mut stmt, max_rows, count)?);
            continue;
        }
        if read_only {
            return Err(AppError::msg("This connection is read-only"));
        }
        stmt.execute(&[]).map_err(err)?;
        let rows_affected = if stmt.is_dml() { stmt.row_count().map_err(err)? } else { 0 };
        conn.commit().map_err(err)?;
        sets.push(ResultSet { rows_affected, ..Default::default() });
    }
    Ok(sets)
}

/// Runs a single statement that Oracle's parser confirms is a query.
fn query_only(conn: &Connection, sql: &str, max_rows: usize, count: &AtomicUsize) -> AppResult<ResultSet> {
    let mut stmt = conn.statement(sql).build().map_err(err)?;
    if !stmt.is_query() {
        return Err(AppError::msg("Only queries are allowed here"));
    }
    fetch(&mut stmt, max_rows, count)
}

fn fetch(stmt: &mut oracle::Statement, max_rows: usize, count: &AtomicUsize) -> AppResult<ResultSet> {
    collect(stmt.query(&[]).map_err(err)?, max_rows, count)
}

fn collect(rows: oracle::ResultSet<oracle::Row>, max_rows: usize, count: &AtomicUsize) -> AppResult<ResultSet> {
    let binary: Vec<bool> = rows.column_info().iter().map(|c| matches!(c.oracle_type(), OracleType::Raw(_) | OracleType::BLOB | OracleType::LongRaw)).collect();
    let mut set = ResultSet {
        columns: rows.column_info().iter().map(|c| ColumnMeta { name: c.name().to_string(), type_name: c.oracle_type().to_string() }).collect(),
        ..Default::default()
    };
    for row in rows {
        if set.rows.len() >= max_rows {
            set.truncated = true;
            break;
        }
        let row = row.map_err(err)?;
        let cells = binary
            .iter()
            .enumerate()
            .map(|(i, bin)| {
                if *bin {
                    row.get::<_, Option<Vec<u8>>>(i).map(|v| v.map(|b| crate::db::hex(&b)))
                } else {
                    row.get::<_, Option<String>>(i)
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        set.rows.push(cells);
        count.fetch_add(1, Ordering::Relaxed);
    }
    Ok(set)
}

/// Applies edits in one transaction; every `single` statement must change exactly one row.
pub fn exec_tx(conn: &Connection, stmts: &[(String, Vec<Option<String>>, bool)], log: &mut Vec<crate::duck::Logged>) -> AppResult<u64> {
    let res = (|| {
        let mut total = 0u64;
        for (sql, binds, single) in stmts {
            let started = std::time::Instant::now();
            let params: Vec<&dyn oracle::sql_type::ToSql> = binds.iter().map(|b| b as &dyn oracle::sql_type::ToSql).collect();
            let r = conn.execute(sql, &params).and_then(|s| s.row_count());
            log.push((sql.clone(), started, r.as_ref().map(|n| format!("{n} affected")).map_err(message)));
            let affected = r.map_err(err)?;
            if *single && affected != 1 {
                return Err(AppError::msg(format!(
                    "Expected to change 1 row but {affected} matched — nothing was saved. \
                     The data may have changed; refresh and try again."
                )));
            }
            total += affected;
        }
        Ok(total)
    })();
    let end = if res.is_ok() { conn.commit() } else { conn.rollback() };
    end.map_err(err)?;
    res
}

#[cfg(test)]
mod split_tests {
    use super::*;

    fn texts(sql: &str) -> Vec<(String, bool)> {
        split_statements(sql).into_iter().map(|s| (s.text, s.plsql)).collect()
    }

    #[test]
    fn splits_sql_and_plsql() {
        let script = "SELECT 'a;b' FROM dual;\n-- note;\nINSERT INTO t VALUES (q'[it's; fine]');\n\
            CREATE OR REPLACE PROCEDURE p AS\nBEGIN\n  NULL; -- x\nEND;\n/\n\
            BEGIN p; END;\n/\nselect 2 from dual";
        assert_eq!(
            texts(script),
            vec![
                ("SELECT 'a;b' FROM dual".into(), false),
                ("-- note;\nINSERT INTO t VALUES (q'[it's; fine]')".into(), false),
                ("CREATE OR REPLACE PROCEDURE p AS\nBEGIN\n  NULL; -- x\nEND;".into(), true),
                ("BEGIN p; END;".into(), true),
                ("select 2 from dual".into(), false),
            ]
        );
        // A block at the end of the script needs no slash.
        assert_eq!(texts("DECLARE x NUMBER; BEGIN x := 1; END;"), vec![("DECLARE x NUMBER; BEGIN x := 1; END;".into(), true)]);
        assert_eq!(texts("SELECT 1/2 FROM dual"), vec![("SELECT 1/2 FROM dual".into(), false)]);
        assert!(texts("-- only a comment\n").is_empty());
        assert_eq!(texts("CREATE TABLE t (a NUMBER);\nCREATE INDEX i ON t (a);").len(), 2);
    }
}

#[cfg(test)]
mod tests {
    //! Needs a server and Oracle Instant Client on the library path, e.g.
    //!   TABLA_TEST_ORACLE="127.0.0.1:1521:tabla:FREEPDB1" TABLA_TEST_ORACLE_PASSWORD=…
    use std::sync::Arc;

    use crate::{
        alter::{AlterRequest, ColumnChange, ColumnDef},
        config::{ConnectionConfig, Driver},
        db::{CellValue, FetchArgs, RowEdit, Session, SortSpec},
    };

    fn cfg() -> Option<(ConnectionConfig, Option<String>)> {
        let v = std::env::var("TABLA_TEST_ORACLE").ok()?;
        let pw = std::env::var("TABLA_TEST_ORACLE_PASSWORD").ok();
        let p: Vec<&str> = v.split(':').collect();
        let c = serde_json::from_value(serde_json::json!({
            "id": "ora-test", "name": "o", "driver": Driver::Oracle, "sslMode": "disable",
            "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
        }))
        .unwrap();
        Some((c, pw))
    }

    #[test]
    fn oracle_end_to_end() {
        crate::test_lock::big_stack(end_to_end());
    }

    async fn end_to_end() {
        let Some((c, pw)) = cfg() else { return };
        let s = Session::connect(&c, pw.clone()).await.unwrap();
        let schema = c.user.to_uppercase();
        assert!(s.server_version().await.unwrap().starts_with(|ch: char| ch.is_ascii_digit()));
        // Scripts: SQL statements end at `;`, PL/SQL blocks at `/`.
        s.run_script(
            "BEGIN
               FOR t IN (SELECT table_name FROM user_tables WHERE table_name IN ('ORA_ITEMS', 'ORA_OWNERS', 'ORA_PLAIN')) LOOP
                 EXECUTE IMMEDIATE 'DROP TABLE \"' || t.table_name || '\" CASCADE CONSTRAINTS PURGE';
               END LOOP;
               FOR v IN (SELECT view_name FROM user_views WHERE view_name = 'ORA_CHEAP') LOOP
                 EXECUTE IMMEDIATE 'DROP VIEW ORA_CHEAP';
               END LOOP;
             END;
             /",
            1,
        )
        .await
        .unwrap();
        let sets = s
            .run_script(
                "CREATE TABLE ora_owners (id NUMBER(10) PRIMARY KEY, name VARCHAR2(40) UNIQUE);
                 CREATE TABLE ora_items (
                   id NUMBER(10) GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, owner_id NUMBER(10) REFERENCES ora_owners(id) ON DELETE CASCADE,
                   title VARCHAR2(100) DEFAULT 'untitled' NOT NULL, price NUMBER(10,2), born DATE, seen TIMESTAMP, notes CLOB, img BLOB, bin RAW(4),
                   flag NUMBER(1), j CLOB CHECK (j IS JSON)
                 );
                 CREATE INDEX ora_items_title ON ora_items (title);
                 INSERT INTO ora_owners VALUES (1, 'ann');
                 INSERT INTO ora_owners VALUES (2, 'bo');
                 INSERT INTO ora_items (owner_id, title, price, born, seen, notes, img, bin, j) VALUES
                   (1, 'lamp', 12.5, DATE '2024-05-01', TIMESTAMP '2024-05-01 10:30:00.25', 'long; text', HEXTORAW('0102'), HEXTORAW('ABCD'), '{\"a\":1}');
                 INSERT INTO ora_items (owner_id, title, price) SELECT 2, 'n' || level, level FROM dual CONNECT BY level <= 3;
                 CREATE VIEW ora_cheap AS SELECT id, title FROM ora_items WHERE price < 50;
                 CREATE OR REPLACE TRIGGER ora_items_touch BEFORE INSERT ON ora_items FOR EACH ROW
                 BEGIN
                   :NEW.title := NVL(:NEW.title, 'x');
                 END;
                 /
                 SELECT id, title, price, born, seen, notes, img, bin, j FROM ora_items ORDER BY id",
                100,
            )
            .await
            .unwrap();
        assert_eq!(sets.len(), 10);
        assert_eq!(sets[6].rows_affected, 3);
        let r = &sets[9].rows;
        assert_eq!(r.len(), 4);
        assert_eq!(r[0][1].as_deref(), Some("lamp"));
        assert_eq!(r[0][2].as_deref(), Some("12.5"));
        assert_eq!(r[0][3].as_deref(), Some("2024-05-01 00:00:00"));
        assert!(r[0][4].as_deref().unwrap().starts_with("2024-05-01 10:30:00.25"));
        assert_eq!(r[0][5].as_deref(), Some("long; text"));
        assert_eq!(r[0][6].as_deref(), Some("0x0102"));
        assert_eq!(r[0][7].as_deref(), Some("0xabcd"));
        assert!(r[1][5].is_none() && r[1][6].is_none());
        assert_eq!(sets[9].columns[2].type_name, "NUMBER(10,2)");
        let err = s.run_script("SELECT * FROM nope", 1).await.unwrap_err().to_string();
        assert!(err.starts_with("ORA-00942") && !err.contains("Help:"), "{err}");
        let empty = s.run_script("SELECT * FROM ora_items WHERE id < 0", 10).await.unwrap();
        assert_eq!((empty[0].columns.len(), empty[0].rows.len()), (11, 0));

        // Browsing
        assert_eq!(s.current_schema().await.unwrap().as_deref(), Some(schema.as_str()));
        assert_eq!(s.list_schemas().await.unwrap().first(), Some(&schema));
        let tables = s.list_tables(&schema).await.unwrap();
        let kind = |n: &str| tables.iter().find(|t| t.name == n).map(|t| t.kind.clone());
        assert_eq!((kind("ORA_ITEMS").as_deref(), kind("ORA_CHEAP").as_deref()), (Some("table"), Some("view")));
        let st = s.table_structure(&schema, "ORA_ITEMS").await.unwrap();
        let col = |n: &str| st.columns.iter().find(|c| c.name == n).unwrap();
        assert!(col("ID").primary_key && col("ID").key == "PRI" && col("ID").extra == "identity" && col("ID").default_value.is_none());
        assert_eq!(col("TITLE").data_type, "VARCHAR2(100)");
        assert_eq!(col("TITLE").default_value.as_deref(), Some("'untitled'"));
        assert!(!col("TITLE").nullable && col("PRICE").nullable);
        assert_eq!((col("PRICE").data_type.as_str(), col("BIN").data_type.as_str()), ("NUMBER(10,2)", "RAW(4)"));
        assert_eq!(col("TITLE").key, "MUL");
        assert_eq!(s.table_structure(&schema, "ORA_OWNERS").await.unwrap().columns[1].key, "UNI");
        assert!(st.indexes.iter().any(|i| i.primary && i.columns == "ID"));
        assert!(st.indexes.iter().any(|i| i.name == "ORA_ITEMS_TITLE" && !i.unique && i.columns == "TITLE"));

        let page = s
            .fetch_rows(&FetchArgs { schema: schema.clone(), table: "ORA_ITEMS".into(), filter: Some("\"OWNER_ID\" = 2".into()), sort: Some(SortSpec { column: "PRICE".into(), desc: true }), limit: 2, offset: 1 })
            .await
            .unwrap();
        assert_eq!(page.total, Some(3));
        assert_eq!(page.result.rows.len(), 2);
        assert_eq!(page.result.rows[0][3].as_deref(), Some("2"));

        // Edits: one transaction; dates and binary values round-trip as text.
        let id = r[0][0].clone().unwrap();
        let key = |v: &str| vec![CellValue { column: "ID".into(), value: Some(v.into()) }];
        let n = s
            .apply_edits(
                &schema,
                "ORA_ITEMS",
                vec![RowEdit::Update {
                    key: key(&id),
                    values: vec![
                        CellValue { column: "PRICE".into(), value: Some("7.25".into()) },
                        CellValue { column: "BORN".into(), value: Some("2023-01-02 03:04:05".into()) },
                        CellValue { column: "BIN".into(), value: Some("0x0a0b".into()) },
                        CellValue { column: "NOTES".into(), value: None },
                    ],
                }],
            )
            .await
            .unwrap();
        assert_eq!(n, 1);
        let back = s.run_script(&format!("SELECT price, born, bin, notes FROM ora_items WHERE id = {id}"), 1).await.unwrap();
        assert_eq!(back[0].rows[0], vec![Some("7.25".into()), Some("2023-01-02 03:04:05".into()), Some("0x0a0b".into()), None]);
        s.apply_edits(&schema, "ORA_ITEMS", vec![RowEdit::Insert { values: vec![CellValue { column: "TITLE".into(), value: Some("new".into()) }] }]).await.unwrap();
        s.apply_edits(&schema, "ORA_OWNERS", vec![RowEdit::Insert { values: vec![CellValue { column: "ID".into(), value: Some("3".into()) }] }]).await.unwrap();
        let err = s.apply_edits(&schema, "ORA_OWNERS", vec![RowEdit::Delete { key: key("3") }, RowEdit::Delete { key: key("999") }]).await;
        assert!(err.is_err());
        assert_eq!(s.run_script("SELECT COUNT(*) FROM ora_owners", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("3"));

        // Structure editing
        s.run_script("CREATE TABLE ora_plain (id NUMBER PRIMARY KEY, name VARCHAR2(20)); INSERT INTO ora_plain VALUES (1, 'a')", 1).await.unwrap();
        let def = |n: &str, t: &str, null: bool, d: Option<&str>| ColumnDef { name: n.into(), data_type: t.into(), nullable: null, default_value: d.map(Into::into) };
        let req = |changes| AlterRequest { schema: schema.clone(), table: "ORA_PLAIN".into(), changes, order: vec![] };
        s.apply_alter(&req(vec![ColumnChange { original: None, column: Some(def("SCORE", "NUMBER(5)", false, Some("0"))) }])).await.unwrap();
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("SCORE", "NUMBER(5)", false, Some("0"))), column: Some(def("POINTS", "NUMBER(8)", true, None)) }]))
            .await
            .unwrap();
        let st = s.table_structure(&schema, "ORA_PLAIN").await.unwrap();
        let p = st.columns.iter().find(|c| c.name == "POINTS").unwrap();
        assert_eq!((p.data_type.as_str(), p.nullable), ("NUMBER(8)", true));
        assert!(p.default_value.as_deref().is_none_or(|d| d == "NULL"), "{:?}", p.default_value);
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("POINTS", "NUMBER(8)", true, None)), column: None }])).await.unwrap();
        assert_eq!(s.table_structure(&schema, "ORA_PLAIN").await.unwrap().columns.len(), 2);

        // Relations, triggers, ER diagram and info.
        let rel = s.table_relations(&schema, "ORA_ITEMS").await.unwrap();
        assert_eq!(rel.outgoing.len(), 1);
        let fk = &rel.outgoing[0];
        assert_eq!((fk.ref_table.as_str(), fk.columns.join(","), fk.on_delete.as_str()), ("ORA_OWNERS", "OWNER_ID".into(), "CASCADE"));
        assert_eq!(rel.triggers.len(), 1);
        assert_eq!((rel.triggers[0].timing.as_str(), rel.triggers[0].event.as_str(), rel.triggers[0].enabled), ("BEFORE", "INSERT", true));
        assert!(rel.triggers[0].definition.starts_with("CREATE OR REPLACE TRIGGER") && rel.triggers[0].definition.contains("NVL"));
        assert_eq!(s.table_relations(&schema, "ORA_OWNERS").await.unwrap().incoming.len(), 1);
        let g = s.schema_graph(&schema).await.unwrap();
        assert!(g.foreign_keys.iter().any(|f| f.table == "ORA_ITEMS" && f.ref_table == "ORA_OWNERS"));
        assert!(g.tables.iter().find(|t| t.name == "ORA_ITEMS").unwrap().columns.iter().any(|c| c.name == "ID" && c.primary_key));
        let info = s.table_details(&schema, "ORA_ITEMS").await.unwrap();
        assert!(info.engine.unwrap().starts_with("Oracle"));
        assert!(info.ddl.unwrap().contains("CREATE TABLE"));
        assert!(info.total_bytes.unwrap_or(0) > 0);
        assert!(s.table_details(&schema, "ORA_CHEAP").await.unwrap().ddl.unwrap().contains("VIEW"));
        assert!(!s.list_users().await.unwrap().is_empty());
        assert!(!s.user_grants(&schema, "").await.unwrap().is_empty());

        // AI queries: Oracle's parser decides what's a query; always rolled back.
        assert_eq!(s.run_readonly("SELECT COUNT(*) FROM ora_owners", 10).await.unwrap().rows[0][0].as_deref(), Some("3"));
        assert!(s.run_readonly("DELETE FROM ora_owners", 10).await.is_err());
        assert!(s.run_readonly("BEGIN DELETE FROM ora_owners; END;", 10).await.is_err());
        assert_eq!(s.run_script("SELECT COUNT(*) FROM ora_owners", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("3"));

        // Read-only connections only run queries.
        let mut ro = c.clone();
        ro.read_only = true;
        let r = Session::connect(&ro, pw.clone()).await.unwrap();
        assert!(r.run_script("SELECT COUNT(*) FROM ora_items", 1).await.is_ok());
        assert!(r.run_script("DELETE FROM ora_items", 1).await.is_err());
        assert!(r.run_script("CREATE TABLE ora_x (a NUMBER)", 1).await.is_err());

        // Cancel
        let s = Arc::new(s);
        let s2 = s.clone();
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move { s2.run_cancellable("SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c", 10, "o1", None).await });
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        assert!(s.cancel("o1").await.unwrap());
        assert_eq!(task.await.unwrap().unwrap_err().to_string(), "Query cancelled");
        assert!(started.elapsed().as_secs() < 15, "took {:?}", started.elapsed());
        assert!(!s.cancel("o1").await.unwrap());
        assert_eq!(s.run_script("SELECT 1 FROM dual", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("1"));

        s.run_script("DROP VIEW ora_cheap; DROP TABLE ora_items PURGE; DROP TABLE ora_owners PURGE; DROP TABLE ora_plain PURGE", 1).await.unwrap();
    }
}

