//! `tabla mcp`: a Model Context Protocol server over stdio.
//!
//! AI coding tools (Claude Code, Cursor, Claude Desktop, Windsurf, …) launch
//! this as a subprocess. It exposes only connections the user opted in with
//! "Allow AI assistants", and only ever reads: every query is validated by
//! `ai_guard` and runs in a read-only transaction that is rolled back.
//! Logs go to stderr; stdout carries protocol messages only.

use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    ai_guard,
    config::{self, ConnectionConfig},
    db::Session,
    error::{AppError, AppResult},
};

const DEFAULT_ROWS: usize = 100;
const MAX_ROWS: usize = 500;
const MAX_CELL_CHARS: usize = 1000;

const INSTRUCTIONS: &str = "Tabla gives read-only access to the user's databases. \
Call list_connections first, then get_schema to learn the tables before writing SQL. \
run_query accepts one read statement (SELECT/WITH/SHOW/EXPLAIN). \
Writes are blocked: suggest the SQL and let the user run it in Tabla.";

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
    sessions: HashMap<String, Session>,
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
        s.close().await;
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
    let schema = json!({ "type": "string", "description": "Schema (Postgres) or database (MySQL). Optional; defaults to the connection's current one." });
    json!([
        {
            "name": "list_connections",
            "description": "List the databases the user has shared with AI assistants in Tabla.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_schema",
            "description": "Get every table in a schema with its columns, types and primary keys. Call this before writing SQL.",
            "inputSchema": { "type": "object", "properties": { "connection": conn, "schema": schema }, "required": ["connection"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "describe_table",
            "description": "Columns (type, nullable, default, primary key) and indexes of one table.",
            "inputSchema": {
                "type": "object",
                "properties": { "connection": conn, "table": { "type": "string" }, "schema": schema },
                "required": ["connection", "table"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "run_query",
            "description": "Run ONE read-only SQL statement (SELECT, WITH, SHOW, EXPLAIN) and get rows as JSON. Writes and multiple statements are rejected. Always add a LIMIT on large tables.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection": conn,
                    "sql": { "type": "string" },
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
    Ok(config::load()?.into_iter().filter(|c| c.ai_access && c.driver != config::Driver::Redis).collect())
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
async fn session<'a>(state: &'a mut State, args: &Value) -> AppResult<&'a Session> {
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
        let s = Session::connect(&ro, password).await?;
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

async fn get_schema(state: &mut State, args: &Value) -> AppResult<String> {
    let s = session(state, args).await?;
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
    let s = session(state, args).await?;
    let schema = schema_of(s, args).await?;
    let st = s.table_structure(&schema, &table).await?;
    if st.columns.is_empty() {
        return Err(AppError::msg(format!("Table `{table}` not found in `{schema}`")));
    }
    Ok(serde_json::to_string_pretty(&json!({ "schema": schema, "table": table, "columns": st.columns, "indexes": st.indexes }))?)
}

async fn run_query(state: &mut State, args: &Value) -> AppResult<String> {
    let sql = str_arg(args, "sql").ok_or_else(|| AppError::msg("`sql` is required"))?;
    let stmt = ai_guard::check(sql).map_err(AppError::Msg)?;
    let max = args.get("max_rows").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
    let s = session(state, args).await?;
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
