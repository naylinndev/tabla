//! `tabla mcp`: a Model Context Protocol server over stdio.
//!
//! AI coding tools (Claude Code, Cursor, Claude Desktop, Windsurf, …) launch
//! this as a subprocess. It exposes only connections the user opted in with
//! "Allow AI assistants", and only ever reads: every query is validated by
//! `ai_guard` and runs in a read-only transaction that is rolled back. Redis
//! and MongoDB connections open read-only and accept only an allowlist of
//! read commands.
//! Logs go to stderr; stdout carries protocol messages only.

use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    ai_guard,
    config::{self, ConnectionConfig},
    db::Session,
    error::{AppError, AppResult},
    mongo_db::MongoSession,
    redis_db::RedisSession,
};

const DEFAULT_ROWS: usize = 100;
const MAX_ROWS: usize = 500;
const MAX_CELL_CHARS: usize = 1000;
/// Longest reply text for Redis and MongoDB commands.
const MAX_REPLY_CHARS: usize = 50_000;

const INSTRUCTIONS: &str = "Tabla gives read-only access to the user's databases. \
Call list_connections first, then get_schema to learn the tables before writing SQL. \
run_query accepts one read statement (SELECT/WITH/SHOW/EXPLAIN). \
On Redis, run_query takes one read command (GET, HGETALL, SCAN…); on MongoDB, one mongosh-style read \
(db.coll.find({...}).limit(20), aggregate, countDocuments, distinct). \
Writes are blocked: suggest the change and let the user run it in Tabla.";

pub fn run() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start runtime");
    if let Err(e) = rt.block_on(serve()) {
        eprintln!("tabla mcp: {e}");
        std::process::exit(1);
    }
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Conn>,
}

// One per shared connection, so the size of the largest variant doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Conn {
    Sql(Session),
    Redis(RedisSession),
    Mongo(MongoSession),
}

async fn serve() -> std::io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut state = State::default();
    eprintln!("tabla mcp: ready");

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                write(&mut stdout, json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}))
                    .await?;
                continue;
            }
        };
        // Notifications (no id) never get a response.
        let Some(id) = req.get("id").cloned() else { continue };
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let reply = match req.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tabla", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tools() }),
            "tools/call" => call_tool(&mut state, &params).await,
            other => {
                let err = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {other}")}});
                write(&mut stdout, err).await?;
                continue;
            }
        };
        write(&mut stdout, json!({"jsonrpc": "2.0", "id": id, "result": reply})).await?;
    }
    for s in state.sessions.values() {
        match s {
            Conn::Sql(s) => s.close().await,
            Conn::Redis(s) => s.close().await,
            Conn::Mongo(s) => s.close().await,
        }
    }
    Ok(())
}

async fn write(out: &mut tokio::io::Stdout, v: Value) -> std::io::Result<()> {
    let mut s = v.to_string();
    s.push('\n');
    out.write_all(s.as_bytes()).await?;
    out.flush().await
}

fn tools() -> Value {
    let conn = json!({ "type": "string", "description": "Connection name exactly as shown by list_connections" });
    let schema = json!({ "type": "string", "description": "Schema (Postgres), database (MySQL, MongoDB). Optional; defaults to the connection's current one." });
    json!([
        {
            "name": "list_connections",
            "description": "List the databases the user has shared with AI assistants in Tabla.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_schema",
            "description": "Get every table in a schema with its columns, types and primary keys. Call this before writing SQL. On MongoDB: collections with fields sampled from documents. On Redis: databases and a sample of keys with their types.",
            "inputSchema": { "type": "object", "properties": { "connection": conn, "schema": schema }, "required": ["connection"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "describe_table",
            "description": "Columns (type, nullable, default, primary key) and indexes of one table. On MongoDB: a collection's sampled fields and indexes. On Redis: the value of one key (pass the key as `table`).",
            "inputSchema": {
                "type": "object",
                "properties": { "connection": conn, "table": { "type": "string" }, "schema": schema },
                "required": ["connection", "table"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "run_query",
            "description": "Run ONE read-only SQL statement (SELECT, WITH, SHOW, EXPLAIN) and get rows as JSON. Writes and multiple statements are rejected. Always add a LIMIT on large tables. On Redis, pass one read command (e.g. HGETALL user:1). On MongoDB, pass one mongosh-style read such as db.orders.find({status: 'paid'}).sort({at: -1}).limit(20) or db.orders.aggregate([...]).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn,
                    "sql": { "type": "string" },
                    "schema": schema,
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": MAX_ROWS, "default": DEFAULT_ROWS }
                },
                "required": ["connection", "sql"]
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

fn text(s: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": s.into() }], "isError": is_error })
}

async fn call_tool(state: &mut State, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let res = match name {
        "list_connections" => list_connections(),
        "get_schema" => get_schema(state, &args).await,
        "describe_table" => describe_table(state, &args).await,
        "run_query" => run_query(state, &args).await,
        _ => Err(AppError::msg(format!("Unknown tool: {name}"))),
    };
    match res {
        Ok(s) => text(s, false),
        Err(e) => text(e.to_string(), true),
    }
}

fn shared() -> AppResult<Vec<ConnectionConfig>> {
    Ok(config::load()?.into_iter().filter(|c| c.ai_access).collect())
}

fn list_connections() -> AppResult<String> {
    let list = shared()?;
    if list.is_empty() {
        return Ok("No databases are shared with AI yet. Ask the user to open Tabla, edit a connection, \
                   and turn on \"Allow AI assistants\"."
            .into());
    }
    let lines: Vec<String> = list
        .iter()
        .map(|c| {
            let engine = match c.driver {
                config::Driver::Postgres => "PostgreSQL",
                config::Driver::Mysql => "MySQL",
                config::Driver::Sqlite => "SQLite",
                config::Driver::Redis => "Redis",
                config::Driver::Mssql => "SQL Server",
                config::Driver::Clickhouse => "ClickHouse",
                config::Driver::Duckdb => "DuckDB",
                config::Driver::Oracle => "Oracle",
                config::Driver::Mongodb => "MongoDB",
            };
            let provider = if c.provider.is_empty() || c.provider == format!("{:?}", c.driver).to_lowercase() {
                String::new()
            } else {
                format!(" via {}", c.provider)
            };
            let tag = if c.tag.is_empty() { String::new() } else { format!(" [{}]", c.tag) };
            format!("- {}: {engine}{provider}{tag}", c.name)
        })
        .collect();
    Ok(format!("Shared connections (read-only):\n{}", lines.join("\n")))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Resolves a connection by name (or id), re-reading config each call so a
/// user turning AI access off takes effect immediately.
async fn session<'a>(state: &'a mut State, args: &Value) -> AppResult<&'a Conn> {
    let wanted = str_arg(args, "connection").ok_or_else(|| AppError::msg("`connection` is required"))?;
    let list = shared()?;
    let cfg = list
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(wanted) || c.id == wanted)
        .ok_or_else(|| {
            let names: Vec<_> = list.iter().map(|c| c.name.as_str()).collect();
            AppError::msg(format!("No shared connection named \"{wanted}\". Available: {}", names.join(", ")))
        })?;
    if !state.sessions.contains_key(&cfg.id) {
        let mut ro = cfg.clone();
        ro.read_only = true; // SQLite opens the file read-only; servers default to read-only transactions.
        let password = config::resolve_password(&ro)?;
        let s = match ro.driver {
            config::Driver::Redis => Conn::Redis(RedisSession::open(&ro, password).await?),
            config::Driver::Mongodb => Conn::Mongo(MongoSession::open(&ro, password).await?),
            _ => Conn::Sql(Session::connect(&ro, password).await?),
        };
        state.sessions.insert(cfg.id.clone(), s);
    }
    Ok(&state.sessions[&cfg.id])
}

async fn schema_of(s: &Session, args: &Value) -> AppResult<String> {
    if let Some(sc) = str_arg(args, "schema") {
        return Ok(sc.to_string());
    }
    Ok(match s.current_schema().await? {
        Some(sc) => sc,
        None => s.list_schemas().await?.into_iter().next().unwrap_or_default(),
    })
}

fn clip(mut s: String) -> String {
    if s.chars().count() > MAX_REPLY_CHARS {
        s = s.chars().take(MAX_REPLY_CHARS).collect::<String>() + "\n… (truncated)";
    }
    s
}

fn mongo_db(s: &MongoSession, args: &Value) -> String {
    str_arg(args, "schema").map(str::to_string).unwrap_or_else(|| s.default_db.clone())
}

async fn mongo_fields(s: &MongoSession, db: &str, coll: &str) -> AppResult<String> {
    let fields = s.sample_fields(db, coll, 20).await?;
    Ok(fields.iter().map(|(f, t)| format!("{f} {}", t.join("|"))).collect::<Vec<_>>().join(", "))
}

async fn get_schema(state: &mut State, args: &Value) -> AppResult<String> {
    let s = match session(state, args).await? {
        Conn::Sql(s) => s,
        Conn::Redis(r) => {
            let o = r.overview().await?;
            let dbs: Vec<String> = o.databases.iter().filter(|d| d.keys > 0).map(|d| format!("db{}: {} keys", d.index, d.keys)).collect();
            let page = r.scan("*", 0, 200).await?;
            let keys: Vec<String> = page.keys.iter().take(200).map(|k| format!("- {} ({})", k.key, k.kind)).collect();
            return Ok(clip(format!(
                "Redis {} ({}), {}.\nSample of keys in the connected database:\n{}",
                o.version,
                if o.mode.is_empty() { "standalone" } else { &o.mode },
                if dbs.is_empty() { "no keys".to_string() } else { dbs.join(", ") },
                if keys.is_empty() { "(none)".to_string() } else { keys.join("\n") }
            )));
        }
        Conn::Mongo(m) => {
            let db = mongo_db(m, args);
            let others: Vec<String> = m.overview().await?.databases.into_iter().filter(|d| *d != db).collect();
            let mut out = format!("MongoDB database `{db}` (others: {}):\n", others.join(", "));
            for c in m.collections(&db).await?.into_iter().take(100) {
                let fields = if c.kind == "view" { String::new() } else { mongo_fields(m, &db, &c.name).await.unwrap_or_default() };
                out.push_str(&format!("- {} [{}]: {fields}\n", c.name, c.kind));
            }
            return Ok(clip(out));
        }
    };
    let schema = schema_of(s, args).await?;
    let overview = s.schema_overview(&schema, 200).await?;
    let schemas = s.list_schemas().await?;
    Ok(format!(
        "Schema `{schema}` (others: {}):\n{}",
        schemas.iter().filter(|x| **x != schema).cloned().collect::<Vec<_>>().join(", "),
        if overview.is_empty() { "(no tables)\n".to_string() } else { overview }
    ))
}

async fn describe_table(state: &mut State, args: &Value) -> AppResult<String> {
    let table = str_arg(args, "table").ok_or_else(|| AppError::msg("`table` is required"))?.to_string();
    let s = match session(state, args).await? {
        Conn::Sql(s) => s,
        Conn::Redis(r) => {
            let mut v = r.get(&table).await?;
            if v.kind == "none" {
                return Err(AppError::msg(format!("Key `{table}` not found")));
            }
            v.truncated |= v.rows.len() > 200;
            v.rows.truncate(200);
            return Ok(clip(serde_json::to_string_pretty(&v)?));
        }
        Conn::Mongo(m) => {
            let db = mongo_db(m, args);
            let fields = mongo_fields(m, &db, &table).await?;
            let indexes = m.indexes(&db, &table).await?;
            return Ok(clip(serde_json::to_string_pretty(&json!({ "database": db, "collection": table, "sampledFields": fields, "indexes": indexes }))?));
        }
    };
    let schema = schema_of(s, args).await?;
    let st = s.table_structure(&schema, &table).await?;
    if st.columns.is_empty() {
        return Err(AppError::msg(format!("Table `{table}` not found in `{schema}`")));
    }
    Ok(serde_json::to_string_pretty(&json!({ "schema": schema, "table": table, "columns": st.columns, "indexes": st.indexes }))?)
}

async fn run_query(state: &mut State, args: &Value) -> AppResult<String> {
    let sql = str_arg(args, "sql").ok_or_else(|| AppError::msg("`sql` is required"))?;
    let max = args.get("max_rows").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
    let s = match session(state, args).await? {
        Conn::Sql(s) => s,
        Conn::Redis(r) => {
            let name = crate::redis_db::split_args(sql).map_err(AppError::Msg)?.into_iter().next().unwrap_or_default();
            if !crate::redis_db::ai_allowed(&name) {
                return Err(AppError::msg(format!("`{name}` is not allowed. AI access is read-only: use GET, HGETALL, LRANGE, SMEMBERS, ZRANGE, SCAN, TYPE, TTL and other reads.")));
            }
            return Ok(clip(r.command(sql, false).await?));
        }
        Conn::Mongo(m) => {
            crate::mongo_db::ai_check(sql)?;
            let db = mongo_db(m, args);
            return Ok(clip(m.shell(&db, sql, false).await?));
        }
    };
    let stmt = ai_guard::check(sql).map_err(AppError::Msg)?;
    let mut rs = s.run_readonly(&stmt, max).await?;
    for row in &mut rs.rows {
        for cell in row.iter_mut().flatten() {
            if cell.chars().count() > MAX_CELL_CHARS {
                *cell = cell.chars().take(MAX_CELL_CHARS).collect::<String>() + "…";
            }
        }
    }
    let columns: Vec<_> = rs.columns.iter().map(|c| json!({ "name": c.name, "type": c.type_name })).collect();
    Ok(serde_json::to_string(&json!({
        "columns": columns,
        "rows": rs.rows,
        "rowCount": rs.rows.len(),
        "truncated": rs.truncated,
    }))?)
}
