//! DuckDB: an in-process analytical database kept in a single file (or in
//! memory with `:memory:`).
//!
//! DuckDB's API is synchronous, so every call runs on a blocking thread with
//! its own connection to the shared database instance; connections are cheap
//! and keep transactions apart. Results stream in as Arrow batches and each
//! value is rendered by Arrow's display formatter, so nested types (lists,
//! structs, maps) arrive as readable text without per-type code.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use duckdb::{
    AccessMode, Config, Connection, InterruptHandle,
    arrow::{
        array::{Array, AsArray},
        datatypes::DataType,
        util::display::{ArrayFormatter, FormatOptions},
    },
};

use crate::{
    db::{ColumnMeta, Progress, ResultSet},
    error::{AppError, AppResult},
};

pub struct DuckDb {
    base: Mutex<Connection>,
    /// External access is off and settings are locked (first AI query).
    sandboxed: std::sync::atomic::AtomicBool,
}

/// One statement sent with bound text parameters; `single` means it must change exactly one row.
pub type Edit = (String, Vec<Option<String>>, bool);

/// A statement that ran on a blocking thread, for the Console log: SQL, start time, outcome.
pub type Logged = (String, std::time::Instant, Result<String, String>);

impl DuckDb {
    pub async fn open(path: &str, read_only: bool) -> AppResult<Self> {
        let path = path.trim().to_string();
        if path.is_empty() {
            return Err(AppError::msg("Choose a DuckDB database file, or enter :memory:"));
        }
        let conn = tokio::task::spawn_blocking(move || -> AppResult<Connection> {
            if path != ":memory:" && !std::path::Path::new(&path).exists() && read_only {
                return Err(AppError::msg(format!("{path} does not exist")));
            }
            // Parquet and JSON are built in. Other extensions download only on an
            // explicit INSTALL, never behind the user's back.
            let mut cfg = Config::default()
                .with("custom_user_agent", "Tabla")
                .and_then(|c| c.with("autoinstall_known_extensions", "false"))
                .map_err(err)?;
            // An in-memory database can't be opened read-only.
            if read_only && path != ":memory:" {
                cfg = cfg.access_mode(AccessMode::ReadOnly).map_err(err)?;
            }
            Connection::open_with_flags(&path, cfg).map_err(|e| {
                let m = message(&e);
                if m.contains("lock") {
                    AppError::msg(format!("{m}\nAnother program has this database open for writing. Close it there, or open it read-only from both."))
                } else {
                    AppError::msg(m)
                }
            })
        })
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
        Ok(DuckDb { base: Mutex::new(conn), sandboxed: Default::default() })
    }

    /// A new connection to the same database.
    pub fn connect(&self) -> AppResult<Connection> {
        self.base.lock().unwrap().try_clone().map_err(err)
    }

    /// Runs a script, one result set per statement. `on_start` receives the
    /// interrupt handle before the first statement runs, for cancelling.
    pub async fn run(&self, sql: &str, max_rows: usize, progress: Progress<'_>, on_start: impl FnOnce(Arc<InterruptHandle>)) -> AppResult<Vec<ResultSet>> {
        let conn = self.connect()?;
        on_start(conn.interrupt_handle());
        let sql = sql.to_string();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let mut task = tokio::task::spawn_blocking(move || {
            let mut sets = Vec::new();
            for stmt in split_statements(&sql) {
                sets.push(query(&conn, stmt, max_rows, &count)?);
            }
            Ok::<_, AppError>(sets)
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

    /// Runs `f` with a fresh connection on a blocking thread.
    pub async fn with_conn<T: Send + 'static>(&self, f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static) -> AppResult<T> {
        let conn = self.connect()?;
        tokio::task::spawn_blocking(move || f(&conn)).await.map_err(|e| AppError::msg(e.to_string()))?
    }

    /// Applies edits in one transaction; every `single` statement must change exactly one row.
    /// Also returns what ran, for the Console log.
    pub async fn exec_tx(&self, stmts: Vec<Edit>) -> AppResult<(AppResult<u64>, Vec<Logged>)> {
        self.with_conn(move |c| {
            c.execute_batch("BEGIN TRANSACTION").map_err(err)?;
            let mut log = Vec::new();
            let res = (|| {
                let mut total = 0u64;
                for (sql, binds, single) in &stmts {
                    let started = std::time::Instant::now();
                    let r = c.prepare(sql).and_then(|mut s| s.execute(duckdb::params_from_iter(binds.iter())));
                    log.push((sql.clone(), started, r.as_ref().map(|n| format!("{n} affected")).map_err(message)));
                    let affected = r.map_err(err)? as u64;
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
            let end = if res.is_ok() { "COMMIT" } else { "ROLLBACK" };
            if let Err(e) = c.execute_batch(end) {
                return Ok((res.and(Err(err(e))), log));
            }
            Ok((res, log))
        })
        .await
    }

    /// Runs statements in order inside one transaction, all or nothing.
    /// Also returns what ran, for the Console log.
    pub async fn exec_script_tx(&self, stmts: Vec<String>) -> AppResult<(AppResult<()>, Vec<Logged>)> {
        self.with_conn(move |c| {
            c.execute_batch("BEGIN TRANSACTION").map_err(err)?;
            let mut log = Vec::new();
            for s in &stmts {
                let started = std::time::Instant::now();
                let r = c.execute_batch(s);
                log.push((s.clone(), started, r.as_ref().map(|_| "ok".to_string()).map_err(message)));
                if let Err(e) = r {
                    let _ = c.execute_batch("ROLLBACK");
                    return Ok((Err(err(e)), log));
                }
            }
            Ok((c.execute_batch("COMMIT").map_err(err), log))
        })
        .await
    }

    /// One read statement for an AI assistant: no file or network access, run
    /// in a transaction that is always rolled back, stopped after 15 seconds.
    pub async fn run_readonly(&self, sql: &str, max_rows: usize) -> AppResult<ResultSet> {
        let stmts = split_statements(sql);
        let [stmt] = stmts.as_slice() else { return Err(AppError::msg("Only one statement per call is allowed")) };
        let stmt = stmt.to_string();
        let conn = self.connect()?;
        // Turning external access off can't be undone and applies to the whole
        // database instance; only the `tabla mcp` process runs AI queries.
        if !self.sandboxed.load(Ordering::SeqCst) {
            conn.execute_batch("SET enable_external_access = false; SET lock_configuration = true").map_err(err)?;
            self.sandboxed.store(true, Ordering::SeqCst);
        }
        let interrupt = conn.interrupt_handle();
        let task = tokio::task::spawn_blocking(move || {
            conn.execute_batch("BEGIN TRANSACTION").map_err(err)?;
            let out = query(&conn, &stmt, max_rows, &AtomicUsize::new(0));
            let _ = conn.execute_batch("ROLLBACK");
            out
        });
        match tokio::time::timeout(Duration::from_secs(15), task).await {
            Ok(res) => res.map_err(|e| AppError::msg(e.to_string()))?,
            Err(_) => {
                interrupt.interrupt();
                Err(AppError::msg("Query took longer than 15 seconds and was stopped"))
            }
        }
    }
}

pub fn err(e: duckdb::Error) -> AppError {
    AppError::msg(message(&e))
}

/// DuckDB's message without the Rust wrapper's error-code prefix.
pub fn message(e: &duckdb::Error) -> String {
    match e {
        duckdb::Error::DuckDBFailure(_, Some(m)) => m.trim().to_string(),
        other => other.to_string(),
    }
}

/// First keyword of a statement, lowercased, skipping leading comments and parentheses.
fn head(sql: &str) -> String {
    let mut s = sql;
    loop {
        s = s.trim_start().trim_start_matches('(');
        if s.starts_with("--") {
            s = s.find('\n').map(|p| &s[p..]).unwrap_or("");
        } else if s.starts_with("/*") {
            s = s.find("*/").map(|p| &s[p + 2..]).unwrap_or("");
        } else {
            break;
        }
    }
    s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next().unwrap_or("").to_ascii_lowercase()
}

/// Statements whose result is only a status or a row count, not data to show.
fn is_command(head: &str) -> bool {
    matches!(
        head,
        "insert" | "update" | "delete" | "merge" | "copy" | "create" | "drop" | "alter" | "attach" | "detach" | "use" | "set" | "reset"
            | "begin" | "start" | "commit" | "end" | "rollback" | "abort" | "install" | "load" | "checkpoint" | "force" | "vacuum"
            | "analyze" | "comment" | "truncate" | "export" | "import" | "prepare" | "deallocate"
    )
}

fn query(conn: &Connection, sql: &str, max_rows: usize, count: &AtomicUsize) -> AppResult<ResultSet> {
    let mut stmt = conn.prepare(sql).map_err(err)?;
    // Executes in streaming mode; batches are fetched below with `step`,
    // which reports errors instead of panicking like the iterator does.
    drop(stmt.stream_arrow([]).map_err(err)?);
    let schema = stmt.schema();
    let mut set = ResultSet::default();
    let returning = sql.to_ascii_lowercase().contains("returning");
    if is_command(&head(sql)) && !returning {
        // DML and CREATE TABLE AS report a single `Count` column.
        let counts = schema.fields().len() == 1 && schema.field(0).name() == "Count";
        while let Some(batch) = stmt.step().map_err(err)? {
            if counts {
                let col = batch.column(0);
                if let Some(a) = col.as_primitive_opt::<duckdb::arrow::datatypes::Int64Type>() {
                    set.rows_affected += a.iter().flatten().sum::<i64>().max(0) as u64;
                }
            }
        }
        return Ok(set);
    }
    set.columns = schema.fields().iter().map(|f| ColumnMeta { name: f.name().clone(), type_name: type_name(f.data_type()) }).collect();
    let opts = FormatOptions::default()
        .with_null("NULL")
        .with_date_format(Some("%Y-%m-%d"))
        .with_datetime_format(Some("%Y-%m-%d %H:%M:%S%.f"))
        .with_timestamp_format(Some("%Y-%m-%d %H:%M:%S%.f"))
        .with_timestamp_tz_format(Some("%Y-%m-%d %H:%M:%S%.f%:z"))
        .with_time_format(Some("%H:%M:%S%.f"));
    'fetch: while let Some(batch) = stmt.step().map_err(err)? {
        let cols: Vec<&dyn Array> = batch.columns().iter().map(|c| c.as_ref()).collect();
        let fmts = cols.iter().map(|c| ArrayFormatter::try_new(*c, &opts)).collect::<Result<Vec<_>, _>>().map_err(|e| AppError::msg(e.to_string()))?;
        for row in 0..batch.len() {
            if set.rows.len() >= max_rows {
                set.truncated = true;
                break 'fetch;
            }
            set.rows.push(cols.iter().zip(&fmts).map(|(c, f)| cell(*c, f, row)).collect());
            count.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(set)
}

fn cell(col: &dyn Array, fmt: &ArrayFormatter, row: usize) -> Option<String> {
    if col.is_null(row) {
        return None;
    }
    match col.data_type() {
        DataType::Binary => Some(crate::db::hex(col.as_binary::<i32>().value(row))),
        DataType::LargeBinary => Some(crate::db::hex(col.as_binary::<i64>().value(row))),
        DataType::FixedSizeBinary(_) => Some(crate::db::hex(col.as_fixed_size_binary().value(row))),
        _ => Some(fmt.value(row).to_string()),
    }
}

/// DuckDB's name for the Arrow type a column arrives as.
fn type_name(t: &DataType) -> String {
    match t {
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int8 => "TINYINT".into(),
        DataType::Int16 => "SMALLINT".into(),
        DataType::Int32 => "INTEGER".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::UInt8 => "UTINYINT".into(),
        DataType::UInt16 => "USMALLINT".into(),
        DataType::UInt32 => "UINTEGER".into(),
        DataType::UInt64 => "UBIGINT".into(),
        DataType::Float16 | DataType::Float32 => "FLOAT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Decimal128(38, 0) => "HUGEINT".into(),
        DataType::Decimal128(p, s) | DataType::Decimal256(p, s) => format!("DECIMAL({p},{s})"),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "VARCHAR".into(),
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView | DataType::FixedSizeBinary(_) => "BLOB".into(),
        DataType::Date32 | DataType::Date64 => "DATE".into(),
        DataType::Time32(_) | DataType::Time64(_) => "TIME".into(),
        DataType::Timestamp(_, Some(_)) => "TIMESTAMP WITH TIME ZONE".into(),
        DataType::Timestamp(_, None) => "TIMESTAMP".into(),
        DataType::Interval(_) | DataType::Duration(_) => "INTERVAL".into(),
        DataType::List(f) | DataType::LargeList(f) => format!("{}[]", type_name(f.data_type())),
        DataType::FixedSizeList(f, n) => format!("{}[{n}]", type_name(f.data_type())),
        DataType::Struct(fs) => format!("STRUCT({})", fs.iter().map(|f| format!("{} {}", f.name(), type_name(f.data_type()))).collect::<Vec<_>>().join(", ")),
        DataType::Map(f, _) => match f.data_type() {
            DataType::Struct(kv) if kv.len() == 2 => format!("MAP({}, {})", type_name(kv[0].data_type()), type_name(kv[1].data_type())),
            _ => "MAP".into(),
        },
        DataType::Dictionary(_, _) => "ENUM".into(),
        DataType::Union(_, _) => "UNION".into(),
        DataType::Null => "NULL".into(),
        other => other.to_string(),
    }
}

/// Splits a script on `;` outside quotes, comments and `$tag$` blocks.
/// DuckDB strings use '' for quotes, never backslash escapes.
pub fn split_statements(sql: &str) -> Vec<&str> {
    let b = sql.as_bytes();
    let (mut out, mut start, mut i) = (Vec::new(), 0usize, 0usize);
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1;
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
                i += 2;
            }
            b'$' => {
                let rest = &sql[i..];
                match rest[1..].find('$').map(|p| p + 2) {
                    Some(n) if rest[1..n - 1].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                        let tag = &rest[..n];
                        i = match sql[i + n..].find(tag) {
                            Some(e) => i + n + e + n,
                            None => b.len(),
                        };
                    }
                    _ => i += 1,
                }
            }
            b';' => {
                out.push(&sql[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(&sql[start.min(sql.len())..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty() && !head(s).is_empty()).collect()
}

#[cfg(test)]
mod tests {
    //! DuckDB runs in-process, so these tests always run, on a scratch file.
    use std::sync::Arc;

    use crate::{
        alter::{AlterRequest, ColumnChange, ColumnDef},
        config::{ConnectionConfig, Driver},
        db::{CellValue, FetchArgs, RowEdit, Session, SortSpec},
    };

    fn cfg(path: &std::path::Path, read_only: bool) -> ConnectionConfig {
        serde_json::from_value(serde_json::json!({
            "id": "duck-test", "name": "d", "driver": Driver::Duckdb, "database": path.to_string_lossy(), "readOnly": read_only
        }))
        .unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tabla-{name}-{}.duckdb", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("duckdb.wal"));
        p
    }

    #[test]
    fn splits_statements() {
        assert_eq!(super::split_statements("SELECT 'a;b''c'; -- x;\nSELECT $$d;e$$ /* ; */;  ;"), vec!["SELECT 'a;b''c'", "-- x;\nSELECT $$d;e$$ /* ; */"]);
        // Backslashes are plain characters in DuckDB strings.
        assert_eq!(super::split_statements(r"SELECT 'C:\'; SELECT 2"), vec![r"SELECT 'C:\'", "SELECT 2"]);
        assert!(super::split_statements("-- only a comment").is_empty());
    }

    #[test]
    fn duckdb_end_to_end() {
        crate::test_lock::big_stack(end_to_end());
    }

    async fn end_to_end() {
        let path = scratch("e2e");
        let s = Session::connect(&cfg(&path, false), None).await.unwrap();
        assert!(s.server_version().await.unwrap().starts_with('v'));
        let sets = s
            .run_script(
                "CREATE SEQUENCE item_seq;
                 CREATE TABLE owners (id INTEGER PRIMARY KEY, name VARCHAR UNIQUE);
                 CREATE TABLE items (
                   id BIGINT PRIMARY KEY DEFAULT nextval('item_seq'), owner_id INTEGER REFERENCES owners(id),
                   title VARCHAR NOT NULL DEFAULT 'untitled', price DECIMAL(10,2), tags VARCHAR[], meta STRUCT(a INTEGER, b VARCHAR),
                   seen_at TIMESTAMP, data BLOB, note VARCHAR
                 );
                 CREATE INDEX ix_items_title ON items (title);
                 INSERT INTO owners VALUES (1, 'ann'), (2, 'bo');
                 INSERT INTO items (owner_id, title, price, tags, meta, seen_at, data, note) VALUES
                   (1, 'lamp', 12.5, ['a', 'b'], {'a': 1, 'b': 'x'}, TIMESTAMP '2024-05-01 10:30:00', '\\x01\\x02'::BLOB, NULL),
                   (2, 'it''s', 3, [], NULL, NULL, NULL, 'C:\\temp'),
                   (1, 'desk', 99.99, NULL, NULL, NULL, NULL, NULL);
                 CREATE VIEW cheap AS SELECT id, title FROM items WHERE price < 50;
                 SELECT id, title, price, tags, meta, seen_at, data, note FROM items ORDER BY id",
                100,
            )
            .await
            .unwrap();
        // One result per statement; DML reports its row count.
        assert_eq!(sets.len(), 8);
        assert_eq!(sets[4].rows_affected, 2);
        assert_eq!(sets[5].rows_affected, 3);
        let rows = &sets[7].rows;
        assert_eq!(rows[0][2].as_deref(), Some("12.50"));
        assert_eq!(rows[0][3].as_deref(), Some("[a, b]"));
        assert_eq!(rows[0][4].as_deref(), Some("{a: 1, b: x}"));
        assert_eq!(rows[0][5].as_deref(), Some("2024-05-01 10:30:00"));
        assert_eq!(rows[0][6].as_deref(), Some("0x0102"));
        assert!(rows[0][7].is_none() && rows[1][4].is_none());
        assert_eq!(rows[1][1].as_deref(), Some("it's"));
        assert_eq!(rows[1][7].as_deref(), Some(r"C:\temp"));
        assert_eq!(sets[7].columns[2].type_name, "DECIMAL(10,2)");
        assert_eq!(sets[7].columns[3].type_name, "VARCHAR[]");
        let empty = s.run_script("SELECT * FROM items WHERE id < 0", 10).await.unwrap();
        assert_eq!((empty[0].columns.len(), empty[0].rows.len()), (9, 0));
        let err = s.run_script("SELECT * FROM nope", 1).await.unwrap_err().to_string();
        assert!(err.contains("nope") && !err.contains("DuckDBFailure"), "{err}");

        // Browsing
        assert_eq!(s.current_schema().await.unwrap().as_deref(), Some("main"));
        assert_eq!(s.list_schemas().await.unwrap(), vec!["main".to_string()]);
        let tables = s.list_tables("main").await.unwrap();
        assert_eq!(tables.iter().map(|t| (t.name.as_str(), t.kind.as_str())).collect::<Vec<_>>(), vec![("cheap", "view"), ("items", "table"), ("owners", "table")]);
        let st = s.table_structure("main", "items").await.unwrap();
        let col = |n: &str| st.columns.iter().find(|c| c.name == n).unwrap();
        assert!(col("id").primary_key && col("id").key == "PRI" && col("id").extra == "auto increment");
        assert!(!col("title").nullable && col("price").nullable);
        assert_eq!(col("title").default_value.as_deref(), Some("'untitled'"));
        assert_eq!(col("title").key, "MUL");
        assert_eq!(s.table_structure("main", "owners").await.unwrap().columns[1].key, "UNI");
        assert!(st.indexes.iter().any(|i| i.primary && i.columns == "id"));
        assert!(st.indexes.iter().any(|i| i.name == "ix_items_title" && !i.unique));

        let page = s
            .fetch_rows(&FetchArgs { schema: "main".into(), table: "items".into(), filter: Some("owner_id = 1".into()), sort: Some(SortSpec { column: "price".into(), desc: true }), limit: 1, offset: 0 })
            .await
            .unwrap();
        assert_eq!((page.total, page.result.rows[0][2].as_deref()), (Some(2), Some("desk")));

        // Edits bind text and cast it to each column's type.
        let key = |v: &str| vec![CellValue { column: "id".into(), value: Some(v.into()) }];
        let n = s
            .apply_edits("main", "items", vec![RowEdit::Update { key: key("2"), values: vec![CellValue { column: "price".into(), value: Some("7.25".into()) }, CellValue { column: "note".into(), value: None }] }])
            .await
            .unwrap();
        assert_eq!(n, 1);
        let back = s.run_script("SELECT price, note FROM items WHERE id = 2", 1).await.unwrap();
        assert_eq!(back[0].rows[0], vec![Some("7.25".into()), None]);
        s.apply_edits("main", "items", vec![RowEdit::Insert { values: vec![CellValue { column: "title".into(), value: Some("new".into()) }, CellValue { column: "tags".into(), value: Some("[x, y]".into()) }] }])
            .await
            .unwrap();
        // A key that matches nothing rolls the whole batch back.
        let err = s
            .apply_edits("main", "items", vec![RowEdit::Delete { key: key("4") }, RowEdit::Delete { key: key("999") }])
            .await;
        assert!(err.is_err());
        let n = s.run_script("SELECT count(*) FROM items", 1).await.unwrap();
        assert_eq!(n[0].rows[0][0].as_deref(), Some("4"));

        // Structure editing, in one transaction.
        let def = |n: &str, t: &str, null: bool, d: Option<&str>| ColumnDef { name: n.into(), data_type: t.into(), nullable: null, default_value: d.map(Into::into) };
        s.run_script("CREATE TABLE plain (id INTEGER PRIMARY KEY, name VARCHAR); INSERT INTO plain VALUES (1, 'a')", 1).await.unwrap();
        let req = |changes| AlterRequest { schema: "main".into(), table: "plain".into(), changes, order: vec![] };
        s.apply_alter(&req(vec![ColumnChange { original: None, column: Some(def("score", "INTEGER", false, Some("0"))) }])).await.unwrap();
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("score", "INTEGER", false, Some("0"))), column: Some(def("points", "BIGINT", true, None)) }]))
            .await
            .unwrap();
        let st = s.table_structure("main", "plain").await.unwrap();
        let p = st.columns.iter().find(|c| c.name == "points").unwrap();
        assert_eq!((p.data_type.as_str(), p.nullable, p.default_value.as_deref()), ("BIGINT", true, None));
        s.apply_alter(&req(vec![ColumnChange { original: Some(def("points", "BIGINT", true, None)), column: None }])).await.unwrap();
        assert_eq!(s.table_structure("main", "plain").await.unwrap().columns.len(), 2);
        // Tables that foreign keys point at can't be altered; the error says why.
        let err = s
            .apply_alter(&AlterRequest { schema: "main".into(), table: "owners".into(), changes: vec![ColumnChange { original: Some(def("name", "VARCHAR", true, None)), column: Some(def("name", "VARCHAR", false, None)) }], order: vec![] })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("depend on"), "{err}");

        // Relations, ER diagram and info.
        let rel = s.table_relations("main", "items").await.unwrap();
        assert_eq!(rel.outgoing.len(), 1);
        assert_eq!((rel.outgoing[0].ref_table.as_str(), rel.outgoing[0].columns.join(","), rel.outgoing[0].ref_columns.join(",")), ("owners", "owner_id".into(), "id".into()));
        assert_eq!(s.table_relations("main", "owners").await.unwrap().incoming.len(), 1);
        let g = s.schema_graph("main").await.unwrap();
        assert_eq!(g.tables.len(), 4);
        assert_eq!(g.foreign_keys.len(), 1);
        assert!(g.tables.iter().find(|t| t.name == "items").unwrap().columns.iter().any(|c| c.name == "id" && c.primary_key));
        let info = s.table_details("main", "items").await.unwrap();
        assert_eq!((info.engine.as_deref(), info.rows_estimate), (Some("DuckDB"), Some(4)));
        assert!(info.ddl.unwrap().starts_with("CREATE TABLE items"));
        assert!(info.total_bytes.unwrap() > 0);
        assert!(s.table_details("main", "cheap").await.unwrap().ddl.unwrap().contains("CREATE VIEW"));
        assert!(s.list_users().await.is_err());

        // The Create Table dialog's sequence-backed id.
        s.run_script(
            "CREATE SEQUENCE \"main\".\"t2_id_seq\"; CREATE TABLE \"main\".\"t2\" (\"id\" BIGINT DEFAULT nextval('\"main\".\"t2_id_seq\"') NOT NULL, \"name\" VARCHAR, PRIMARY KEY (\"id\"))",
            1,
        )
        .await
        .unwrap();
        s.apply_edits("main", "t2", vec![RowEdit::Insert { values: vec![CellValue { column: "name".into(), value: Some("a".into()) }] }]).await.unwrap();
        assert_eq!(s.run_script("SELECT id FROM t2", 1).await.unwrap()[0].rows[0][0].as_deref(), Some("1"));

        // Queries can be cancelled.
        let s = Arc::new(s);
        let s2 = s.clone();
        let started = std::time::Instant::now();
        let task = tokio::spawn(async move { s2.run_cancellable("SELECT count(*) FROM range(10000000000) a", 10, "d1", None).await });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(s.cancel("d1").await.unwrap());
        assert_eq!(task.await.unwrap().unwrap_err().to_string(), "Query cancelled");
        assert!(started.elapsed().as_secs() < 10);
        assert!(!s.cancel("d1").await.unwrap());
        s.close().await;
        drop(s);

        // Read-only connections can't write.
        let r = Session::connect(&cfg(&path, true), None).await.unwrap();
        assert!(r.run_script("SELECT count(*) FROM items", 1).await.is_ok());
        assert!(r.run_script("DELETE FROM items", 1).await.is_err());
        assert!(r.apply_edits("main", "items", vec![RowEdit::Delete { key: key("1") }]).await.is_err());

        // AI queries: one statement, no file access, always rolled back.
        assert_eq!(r.run_readonly("SELECT count(*) FROM items", 10).await.unwrap().rows[0][0].as_deref(), Some("4"));
        let secret = path.with_extension("txt");
        std::fs::write(&secret, "secret").unwrap();
        let err = r.run_readonly(&format!("SELECT * FROM read_text('{}')", secret.to_string_lossy()), 10).await.unwrap_err().to_string();
        assert!(err.to_lowercase().contains("disabled"), "{err}");
        assert!(r.run_readonly("SELECT 1; SELECT 2", 10).await.is_err());
        r.close().await;
        drop(r);
        let _ = std::fs::remove_file(&secret);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn duckdb_in_memory_reads_files() {
        crate::test_lock::big_stack(async {
            let csv = std::env::temp_dir().join(format!("tabla-{}.csv", std::process::id()));
            std::fs::write(&csv, "a,b\n1,x\n2,y\n").unwrap();
            let s = Session::connect(&cfg(std::path::Path::new(":memory:"), false), None).await.unwrap();
            let sets = s.run_script(&format!("SELECT sum(a) FROM '{}'", csv.to_string_lossy()), 10).await.unwrap();
            assert_eq!(sets[0].rows[0][0].as_deref(), Some("3"));
            let _ = std::fs::remove_file(&csv);
        });
    }
}
