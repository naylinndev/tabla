//! ClickHouse over its HTTP interface (port 8123, or 8443 with TLS).
//!
//! Every statement is one POST. Results come back as
//! TabSeparatedWithNamesAndTypes, which streams, keeps every value as text
//! and carries the column types. Read-only connections send `readonly=2`,
//! so the server itself refuses writes.

use std::time::Duration;

use futures_util::StreamExt;

use crate::{
    config::{ConnectionConfig, SslMode},
    db::{ColumnMeta, Progress, ResultSet},
    error::{AppError, AppResult},
};

pub struct ChClient {
    http: reqwest::Client,
    url: String,
    user: String,
    password: String,
    database: String,
    read_only: bool,
}

/// Extra per-request settings, e.g. ("query_id", "…") or ("param_p1", "…").
pub type Params<'a> = &'a [(&'a str, String)];

impl ChClient {
    pub async fn connect(cfg: &ConnectionConfig, host: &str, port: u16, password: Option<String>) -> AppResult<Self> {
        let tls = cfg.ssl_mode != SslMode::Disable;
        let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).user_agent("Tabla");
        if tls {
            if matches!(cfg.ssl_mode, SslMode::Prefer | SslMode::Require) && cfg.ssl_ca.trim().is_empty() {
                // Encrypt without verifying, like libpq's sslmode=require.
                b = b.tls_danger_accept_invalid_certs(true);
            }
            if !cfg.ssl_ca.trim().is_empty() {
                let pem = std::fs::read(cfg.ssl_ca.trim())?;
                for c in reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| AppError::msg(format!("CA certificate: {e}")))? {
                    b = b.add_root_certificate(c);
                }
            }
            if !cfg.ssl_cert.trim().is_empty() && !cfg.ssl_key.trim().is_empty() {
                let mut pem = std::fs::read(cfg.ssl_cert.trim())?;
                pem.push(b'\n');
                pem.extend(std::fs::read(cfg.ssl_key.trim())?);
                b = b.identity(reqwest::Identity::from_pem(&pem).map_err(|e| AppError::msg(format!("Client certificate: {e}")))?);
            }
        }
        let http = b.build().map_err(|e| AppError::msg(e.to_string()))?;
        let scheme = if tls { "https" } else { "http" };
        let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        let c = ChClient {
            http,
            url: format!("{scheme}://{host}:{port}/"),
            user: if cfg.user.trim().is_empty() { "default".into() } else { cfg.user.trim().into() },
            password: password.unwrap_or_default(),
            database: cfg.database.trim().to_string(),
            read_only: cfg.read_only,
        };
        c.query("SELECT 1", 1, None, &[]).await?;
        Ok(c)
    }

    async fn post(&self, sql: &str, params: Params<'_>) -> AppResult<reqwest::Response> {
        let mut q: Vec<(&str, String)> = vec![("default_format", "TabSeparatedWithNamesAndTypes".into())];
        if !self.database.is_empty() {
            q.push(("database", self.database.clone()));
        }
        if self.read_only {
            q.push(("readonly", "2".into()));
        }
        q.extend(params.iter().cloned());
        let res = self
            .http
            .post(&self.url)
            .query(&q)
            .header("X-ClickHouse-User", &self.user)
            .header("X-ClickHouse-Key", &self.password)
            .body(sql.to_string())
            .send()
            .await
            .map_err(|e| AppError::msg(transport_message(&e)))?;
        if !res.status().is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::msg(clean_error(&body)));
        }
        Ok(res)
    }

    /// Runs one statement; returns its result set (or the rows it wrote).
    pub async fn query(&self, sql: &str, max_rows: usize, progress: Progress<'_>, params: Params<'_>) -> AppResult<ResultSet> {
        let res = self.post(sql, params).await?;
        let written = res
            .headers()
            .get("X-ClickHouse-Summary")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
            .and_then(|v| v.get("written_rows")?.as_str()?.parse::<u64>().ok())
            .unwrap_or(0);
        let mut stream = res.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut lines = 0usize;
        let mut set = ResultSet::default();
        let mut names: Vec<String> = Vec::new();
        'read: while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AppError::msg(transport_message(&e)))?;
            buf.extend_from_slice(&chunk);
            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                match lines {
                    0 => names = split_tsv(&line).into_iter().map(Option::unwrap_or_default).collect(),
                    1 => {
                        set.columns = names
                            .iter()
                            .zip(split_tsv(&line))
                            .map(|(n, t)| ColumnMeta { name: n.clone(), type_name: t.unwrap_or_default() })
                            .collect()
                    }
                    _ => {
                        if set.rows.len() >= max_rows {
                            set.truncated = true;
                            break 'read;
                        }
                        // A server error mid-stream arrives as text in the body.
                        if line.starts_with("Code: ") && line.contains("DB::Exception") {
                            return Err(AppError::msg(clean_error(&line)));
                        }
                        set.rows.push(split_tsv(&line));
                        if let Some(p) = progress
                            && set.rows.len() % 1000 == 0
                        {
                            p(set.rows.len());
                        }
                    }
                }
                lines += 1;
            }
        }
        if lines == 0 {
            set.rows_affected = written;
        }
        Ok(set)
    }

    /// Runs a script statement by statement (the HTTP interface takes one at a time).
    pub async fn run(&self, sql: &str, max_rows: usize, progress: Progress<'_>, params: Params<'_>) -> AppResult<Vec<ResultSet>> {
        let mut out = Vec::new();
        for stmt in split_statements(sql) {
            out.push(self.query(stmt, max_rows, progress, params).await?);
        }
        Ok(out)
    }
}

fn transport_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "Timed out connecting to ClickHouse".into();
    }
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg = s.to_string();
        src = s.source();
    }
    msg
}

/// "Code: 60. DB::Exception: Table x doesn't exist. (UNKNOWN_TABLE) (version 24.8.1)"
/// becomes "Table x doesn't exist. (UNKNOWN_TABLE)".
pub fn clean_error(body: &str) -> String {
    let mut s = body.trim();
    if let Some(p) = s.find("DB::Exception: ") {
        s = &s[p + "DB::Exception: ".len()..];
    }
    let s = match s.rfind(" (version ") {
        Some(p) => &s[..p],
        None => s,
    };
    let s = s.trim();
    if s.is_empty() { "ClickHouse returned an error".into() } else { s.to_string() }
}

/// One TSV line: tabs separate fields, `\N` is NULL, backslash escapes.
fn split_tsv(line: &str) -> Vec<Option<String>> {
    line.split('\t')
        .map(|f| {
            if f == "\\N" {
                return None;
            }
            if !f.contains('\\') {
                return Some(f.to_string());
            }
            let mut out = String::with_capacity(f.len());
            let mut it = f.chars();
            while let Some(c) = it.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('0') => out.push('\0'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some(o) => out.push(o),
                    None => out.push('\\'),
                }
            }
            Some(out)
        })
        .collect()
}

/// Splits on `;` outside quotes and comments.
pub fn split_statements(sql: &str) -> Vec<&str> {
    let b = sql.as_bytes();
    let (mut out, mut start, mut i) = (Vec::new(), 0, 0);
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
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
                i += 1;
            }
            b';' => {
                out.push(&sql[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&sql[start.min(sql.len())..]);
    out.into_iter().filter(|s| !s.trim().is_empty() && !only_comments(s)).map(str::trim).collect()
}

fn only_comments(s: &str) -> bool {
    s.lines().all(|l| {
        let t = l.trim();
        t.is_empty() || t.starts_with("--")
    })
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn parses_tsv_fields() {
        assert_eq!(split_tsv("1\t\\N\ta\\tb\\\\c\\nd"), vec![Some("1".into()), None, Some("a\tb\\c\nd".into())]);
        assert_eq!(split_tsv(""), vec![Some(String::new())]);
    }

    #[test]
    fn splits_scripts() {
        assert_eq!(split_statements("SELECT 1; SELECT ';' -- x;\n; /* ; */ SELECT 2;"), vec!["SELECT 1", "SELECT ';' -- x;", "/* ; */ SELECT 2"]);
        assert_eq!(split_statements("SELECT 'it\\'s;'"), vec!["SELECT 'it\\'s;'"]);
        assert!(split_statements("  ;\n-- only a comment\n").is_empty());
    }

    #[test]
    fn cleans_errors() {
        assert_eq!(
            clean_error("Code: 60. DB::Exception: Unknown table expression identifier 'nope' in scope SELECT * FROM nope. (UNKNOWN_TABLE) (version 24.8.4.13 (official build))\n"),
            "Unknown table expression identifier 'nope' in scope SELECT * FROM nope. (UNKNOWN_TABLE)"
        );
    }
}

#[cfg(test)]
mod integration {
    //! Runs against a real ClickHouse when TABLA_TEST_CLICKHOUSE="host:port:user:database"
    //! is set (password in TABLA_TEST_CLICKHOUSE_PASSWORD, optional).
    use std::sync::Arc;

    use crate::{
        alter::{AlterRequest, ColumnChange, ColumnDef},
        config::{ConnectionConfig, Driver},
        db::{CellValue, FetchArgs, RowEdit, Session, SortSpec},
    };

    fn cfg() -> Option<(ConnectionConfig, Option<String>)> {
        let v = std::env::var("TABLA_TEST_CLICKHOUSE").ok()?;
        let pw = std::env::var("TABLA_TEST_CLICKHOUSE_PASSWORD").ok().filter(|p| !p.is_empty());
        let p: Vec<&str> = v.split(':').collect();
        let c = serde_json::from_value(serde_json::json!({
            "id": "ch-test", "name": "c", "driver": Driver::Clickhouse, "sslMode": "disable",
            "host": p[0], "port": p[1].parse::<u16>().unwrap(), "user": p[2], "database": p[3]
        }))
        .unwrap();
        Some((c, pw))
    }

    #[test]
    fn clickhouse_end_to_end() {
        crate::test_lock::big_stack(end_to_end());
    }

    async fn end_to_end() {
        let Some((c, pw)) = cfg() else { return };
        let s = Session::connect(&c, pw.clone()).await.unwrap();
        assert!(!s.server_version().await.unwrap().is_empty());
        s.run_script(
            "DROP TABLE IF EXISTS ch_events; DROP VIEW IF EXISTS ch_events_v;
             CREATE TABLE ch_events (
               id UInt64, name String DEFAULT 'untitled', score Nullable(Float64), tags Array(String),
               at DateTime DEFAULT now(), kind LowCardinality(String), note Nullable(String),
               INDEX ix_score score TYPE minmax GRANULARITY 1
             ) ENGINE = MergeTree ORDER BY id;
             INSERT INTO ch_events (id, name, score, tags, kind, note) VALUES
               (1, 'signup', 1.5, ['a','b'], 'web', 'tab\there'), (2, 'login', NULL, [], 'app', NULL), (3, 'it''s', 3, ['x'], 'web', 'line\nbreak');
             CREATE VIEW ch_events_v AS SELECT id, name FROM ch_events",
            10,
        )
        .await
        .unwrap();

        assert_eq!(s.current_schema().await.unwrap().as_deref(), Some("tabla_test"));
        assert!(s.list_schemas().await.unwrap().iter().any(|x| x == "tabla_test"));
        let tables = s.list_tables("tabla_test").await.unwrap();
        let t = tables.iter().find(|t| t.name == "ch_events").unwrap();
        assert_eq!((t.kind.as_str(), t.rows_estimate), ("table", Some(3)));
        assert_eq!(tables.iter().find(|t| t.name == "ch_events_v").unwrap().kind, "view");

        let st = s.table_structure("tabla_test", "ch_events").await.unwrap();
        let col = |n: &str| st.columns.iter().find(|c| c.name == n).unwrap();
        assert!(col("id").primary_key);
        assert_eq!(col("id").key, "PRI");
        assert!(col("score").nullable && !col("name").nullable);
        assert_eq!(col("name").default_value.as_deref(), Some("'untitled'"));
        assert!(st.indexes.iter().any(|i| i.primary && i.columns == "id"));
        assert!(st.indexes.iter().any(|i| i.name == "ix_score"));

        // Values, NULLs, escapes and types survive the TSV round trip.
        let sets = s.run_script("SELECT id, score, tags, note, name FROM ch_events ORDER BY id", 100).await.unwrap();
        let r = &sets[0].rows;
        assert_eq!(r[0][1].as_deref(), Some("1.5"));
        assert_eq!(r[0][2].as_deref(), Some("['a','b']"));
        assert_eq!(r[0][3].as_deref(), Some("tab\there"));
        assert!(r[1][1].is_none() && r[1][3].is_none());
        assert_eq!(r[2][3].as_deref(), Some("line\nbreak"));
        assert_eq!(r[2][4].as_deref(), Some("it's"));
        assert_eq!(sets[0].columns[1].type_name, "Nullable(Float64)");
        let empty = s.run_script("SELECT * FROM ch_events WHERE id = 0", 10).await.unwrap();
        assert_eq!(empty[0].columns.len(), 7);
        let err = s.run_script("SELECT * FROM nope", 1).await.unwrap_err().to_string();
        assert!(err.contains("UNKNOWN_TABLE") && !err.contains("version"), "{err}");

        let page = s
            .fetch_rows(&FetchArgs { schema: "tabla_test".into(), table: "ch_events".into(), filter: Some("`kind` = 'web'".into()), sort: Some(SortSpec { column: "id".into(), desc: true }), limit: 1, offset: 0 })
            .await
            .unwrap();
        assert_eq!((page.total, page.result.rows[0][0].as_deref()), (Some(2), Some("3")));

        // Edits: typed parameters; updates run as synchronous mutations.
        let key = |v: &str| vec![CellValue { column: "id".into(), value: Some(v.into()) }];
        let n = s
            .apply_edits("tabla_test", "ch_events", vec![RowEdit::Update { key: key("2"), values: vec![CellValue { column: "score".into(), value: Some("9.25".into()) }, CellValue { column: "note".into(), value: None }] }])
            .await
            .unwrap();
        assert_eq!(n, 1);
        let back = s.run_script("SELECT score, note FROM ch_events WHERE id = 2", 1).await.unwrap();
        assert_eq!(back[0].rows[0], vec![Some("9.25".into()), None]);
        s.apply_edits("tabla_test", "ch_events", vec![RowEdit::Insert { values: vec![CellValue { column: "id".into(), value: Some("4".into()) }, CellValue { column: "tags".into(), value: Some("['z']".into()) }] }])
            .await
            .unwrap();
        assert!(s.apply_edits("tabla_test", "ch_events", vec![RowEdit::Delete { key: key("999") }]).await.is_err());
        s.apply_edits("tabla_test", "ch_events", vec![RowEdit::Delete { key: key("4") }]).await.unwrap();
        let n = s.run_script("SELECT count() FROM ch_events", 1).await.unwrap();
        assert_eq!(n[0].rows[0][0].as_deref(), Some("3"));

        // Structure editing
        let def = |n: &str, t: &str, null: bool, d: Option<&str>| ColumnDef { name: n.into(), data_type: t.into(), nullable: null, default_value: d.map(Into::into) };
        let req = |changes| AlterRequest { schema: "tabla_test".into(), table: "ch_events".into(), changes, order: vec![] };
        s.apply_alter(&req(vec![ColumnChange { original: None, column: Some(def("extra", "UInt8", true, Some("7"))) }])).await.unwrap();
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("extra", "Nullable(UInt8)", true, Some("7"))), column: Some(def("extra2", "Nullable(UInt8)", false, None)) }]))
            .await
            .unwrap();
        let st = s.table_structure("tabla_test", "ch_events").await.unwrap();
        let e = st.columns.iter().find(|c| c.name == "extra2").unwrap();
        assert_eq!((e.data_type.as_str(), e.nullable, e.default_value.as_deref()), ("UInt8", false, None));
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("extra2", "UInt8", false, None)), column: None }])).await.unwrap();

        // Info view
        let info = s.table_details("tabla_test", "ch_events").await.unwrap();
        assert_eq!(info.engine.as_deref(), Some("MergeTree"));
        assert!(info.ddl.unwrap().starts_with("CREATE TABLE tabla_test.ch_events"));
        assert!(s.table_relations("tabla_test", "ch_events").await.unwrap().outgoing.is_empty());
        assert_eq!(s.schema_graph("tabla_test").await.unwrap().tables.len(), tables.len());

        // AI queries and read-only connections are enforced by the server.
        assert!(s.run_readonly("INSERT INTO ch_events (id) VALUES (100)", 10).await.is_err());
        let mut ro = c.clone();
        ro.read_only = true;
        let r = Session::connect(&ro, pw.clone()).await.unwrap();
        assert!(r.run_script("SELECT count() FROM ch_events", 1).await.is_ok());
        assert!(r.run_script("INSERT INTO ch_events (id) VALUES (101)", 1).await.is_err());

        assert!(!s.list_users().await.unwrap().is_empty());
        s.run_script("DROP VIEW ch_events_v; DROP TABLE ch_events", 1).await.unwrap();
    }

    #[test]
    fn clickhouse_cancel() {
        crate::test_lock::big_stack(cancel());
    }

    async fn cancel() {
        let Some((c, pw)) = cfg() else { return };
        let s = Arc::new(Session::connect(&c, pw).await.unwrap());
        let s2 = s.clone();
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move { s2.run_cancellable("SELECT sleepEachRow(1) FROM numbers(30) SETTINGS max_block_size = 1", 100, "ch1", None).await });
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert!(s.cancel("ch1").await.unwrap());
        let res = task.await.unwrap();
        assert!(started.elapsed().as_secs() < 15, "took {:?}", started.elapsed());
        if let Err(e) = res {
            assert_eq!(e.to_string(), "Query cancelled");
        }
    }
}
