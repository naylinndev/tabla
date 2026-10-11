//! DEBUG BUILDS ONLY. Serves the backend commands over HTTP on 127.0.0.1 so
//! the UI can be driven end-to-end in a normal browser during development.
//!
//! Guard rails: localhost only, a required per-run token (`TABLA_BRIDGE_TOKEN`),
//! only the Vite dev origin is allowed by CORS, and `TABLA_CONFIG_DIR` must be
//! set so the user's real connection profiles are never touched. Native dialogs
//! are replaced by fixed paths from env vars.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{
    commands::{self, AppState},
    config::{self, ConnectionConfig},
    db::{FetchArgs, RowEdit},
    error::{AppError, AppResult},
};

const ADDR: &str = "127.0.0.1:1430";
const ORIGIN: &str = "http://localhost:1420";

pub fn run() {
    let token = std::env::var("TABLA_BRIDGE_TOKEN").unwrap_or_default();
    if token.len() < 16 || std::env::var_os("TABLA_CONFIG_DIR").is_none() {
        eprintln!("tabla bridge: set TABLA_BRIDGE_TOKEN (16+ chars) and TABLA_CONFIG_DIR");
        std::process::exit(2);
    }
    // One dispatch function holds every command's future; debug builds need room for it.
    let rt = tokio::runtime::Builder::new_multi_thread().thread_stack_size(16 << 20).enable_all().build().unwrap();
    rt.block_on(async move {
        let listener = TcpListener::bind(ADDR).await.expect("bind 127.0.0.1:1430");
        eprintln!("tabla bridge: listening on http://{ADDR}");
        let state = std::sync::Arc::new(AppState::default());
        loop {
            let Ok((sock, _)) = listener.accept().await else { continue };
            let (state, token) = (state.clone(), token.clone());
            tokio::spawn(async move {
                let _ = handle(sock, &state, &token).await;
            });
        }
    });
}

async fn handle(mut sock: TcpStream, state: &AppState, token: &str) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let method = lines.next().unwrap_or("").split(' ').next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut got_token = String::new();
    let mut origin = String::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "x-tabla-token" => got_token = v.trim().to_string(),
                "origin" => origin = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let cors = format!(
        "Access-Control-Allow-Origin: {ORIGIN}\r\nAccess-Control-Allow-Headers: content-type, x-tabla-token\r\nAccess-Control-Allow-Methods: POST\r\n"
    );
    if method == "OPTIONS" {
        return sock.write_all(format!("HTTP/1.1 204 No Content\r\n{cors}Content-Length: 0\r\n\r\n").as_bytes()).await;
    }
    if method != "POST" || got_token != token || (!origin.is_empty() && origin != ORIGIN) {
        return sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
    }
    while buf.len() < header_end + len {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = &buf[header_end..(header_end + len).min(buf.len())];
    let reply = match serde_json::from_slice::<Value>(body) {
        Ok(req) => {
            let cmd = req["cmd"].as_str().unwrap_or("").to_string();
            match dispatch(state, &cmd, req["args"].clone()).await {
                Ok(v) => json!({ "ok": v }),
                Err(e) => json!({ "err": e.to_string() }),
            }
        }
        Err(e) => json!({ "err": e.to_string() }),
    };
    let out = reply.to_string();
    sock.write_all(
        format!("HTTP/1.1 200 OK\r\n{cors}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{out}", out.len())
            .as_bytes(),
    )
    .await
}

fn arg<T: for<'de> Deserialize<'de>>(args: &Value, key: &str) -> AppResult<T> {
    Ok(serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))?)
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

async fn dispatch(state: &AppState, cmd: &str, a: Value) -> AppResult<Value> {
    let sid = || arg::<String>(&a, "sessionId");
    Ok(match cmd {
        "list_connections" => json!(config::load()?),
        "save_connection" => json!(config::upsert(arg::<ConnectionConfig>(&a, "config")?)?),
        "secret_status" => json!(config::secret_status(&arg::<String>(&a, "id")?)),
        "delete_connection" => json!(config::remove(&arg::<String>(&a, "id")?)?),
        "test_connection" => json!(state.test(arg(&a, "config")?).await?),
        "open_session" => json!(state.open(arg(&a, "config")?).await?),
        "close_session" => {
            state.close(&sid()?).await;
            Value::Null
        }
        "list_schemas" => json!(state.get(&sid()?)?.list_schemas().await?),
        "list_tables" => json!(state.get(&sid()?)?.list_tables(&arg::<String>(&a, "schema")?).await?),
        "table_structure" => json!(
            state.get(&sid()?)?.table_structure(&arg::<String>(&a, "schema")?, &arg::<String>(&a, "table")?).await?
        ),
        "query_log" => json!(state.log_since(&sid()?, arg::<u64>(&a, "after")?)?),
        "clear_query_log" => json!(state.clear_log(&sid()?)?),
        "redis_overview" => json!(state.get_redis(&sid()?)?.overview().await?),
        "redis_select" => json!(state.get_redis(&sid()?)?.select(arg(&a, "db")?).await?),
        "redis_scan" => json!(state.get_redis(&sid()?)?.scan(&arg::<String>(&a, "pattern")?, arg(&a, "cursor")?, 500).await?),
        "redis_get" => json!(state.get_redis(&sid()?)?.get(&arg::<String>(&a, "key")?).await?),
        "redis_edit" => json!(state.get_redis(&sid()?)?.edit(arg(&a, "edit")?).await?),
        "redis_command" => json!(state.get_redis(&sid()?)?.command(&arg::<String>(&a, "line")?, arg(&a, "allowWrite")?).await?),
        "mongo_overview" => json!(state.get_mongo(&sid()?)?.overview().await?),
        "mongo_collections" => json!(state.get_mongo(&sid()?)?.collections(&arg::<String>(&a, "db")?).await?),
        "mongo_find" => json!(state.get_mongo(&sid()?)?.find(&arg(&a, "args")?).await?),
        "mongo_insert" => json!(
            state.get_mongo(&sid()?)?.insert(&arg::<String>(&a, "db")?, &arg::<String>(&a, "collection")?, &arg::<String>(&a, "json")?).await?
        ),
        "mongo_replace" => json!(
            state
                .get_mongo(&sid()?)?
                .replace(&arg::<String>(&a, "db")?, &arg::<String>(&a, "collection")?, &arg::<String>(&a, "id")?, &arg::<String>(&a, "json")?)
                .await?
        ),
        "mongo_set_field" => json!(
            state
                .get_mongo(&sid()?)?
                .set_field(
                    &arg::<String>(&a, "db")?,
                    &arg::<String>(&a, "collection")?,
                    &arg::<String>(&a, "id")?,
                    &arg::<String>(&a, "field")?,
                    arg::<Option<String>>(&a, "value")?.as_deref()
                )
                .await?
        ),
        "mongo_delete" => json!(
            state.get_mongo(&sid()?)?.delete(&arg::<String>(&a, "db")?, &arg::<String>(&a, "collection")?, &arg::<Vec<String>>(&a, "ids")?).await?
        ),
        "mongo_indexes" => json!(state.get_mongo(&sid()?)?.indexes(&arg::<String>(&a, "db")?, &arg::<String>(&a, "collection")?).await?),
        "mongo_shell" => json!(
            state.get_mongo(&sid()?)?.shell(&arg::<String>(&a, "db")?, &arg::<String>(&a, "line")?, arg(&a, "allowWrite")?).await?
        ),
        "list_users" => json!(state.get(&sid()?)?.list_users().await?),
        "user_grants" => json!(state.get(&sid()?)?.user_grants(&arg::<String>(&a, "name")?, &arg::<String>(&a, "host")?).await?),
        "schema_graph" => json!(state.get(&sid()?)?.schema_graph(&arg::<String>(&a, "schema")?).await?),
        "table_relations" => json!(
            state.get(&sid()?)?.table_relations(&arg::<String>(&a, "schema")?, &arg::<String>(&a, "table")?).await?
        ),
        "table_details" => json!(
            state.get(&sid()?)?.table_details(&arg::<String>(&a, "schema")?, &arg::<String>(&a, "table")?).await?
        ),
        "fetch_rows" => json!(state.get(&sid()?)?.fetch_rows(&arg::<FetchArgs>(&a, "args")?).await?),
        "run_query" => json!(
            state
                .query(&sid()?, &arg::<String>(&a, "sql")?, arg::<Option<String>>(&a, "queryId")?.as_deref(), arg(&a, "maxRows")?, None)
                .await?
        ),
        "cancel_query" => json!(state.get(&sid()?)?.cancel(&arg::<String>(&a, "queryId")?).await?),
        "apply_edits" => json!(
            state
                .get(&sid()?)?
                .apply_edits(&arg::<String>(&a, "schema")?, &arg::<String>(&a, "table")?, arg::<Vec<RowEdit>>(&a, "edits")?)
                .await?
        ),
        "plan_alter" => json!(state.get(&sid()?)?.plan_alter(&arg(&a, "req")?).await?),
        "apply_alter" => json!(state.get(&sid()?)?.apply_alter(&arg(&a, "req")?).await?),
        "pick_csv_file" => match env_path("TABLA_BRIDGE_CSV") {
            Some(p) => json!(commands::read_text_file(&p)?),
            None => Value::Null,
        },
        "pick_file" => json!(env_path("TABLA_BRIDGE_PICK").map(|p| p.to_string_lossy().into_owned())),
        "pick_sqlite_file" => json!(env_path("TABLA_BRIDGE_SQLITE").map(|p| p.to_string_lossy().into_owned())),
        "pick_duckdb_file" => json!(env_path("TABLA_BRIDGE_DUCKDB").map(|p| p.to_string_lossy().into_owned())),
        "export_file" => {
            let dir = env_path("TABLA_BRIDGE_OUT").ok_or_else(|| AppError::msg("TABLA_BRIDGE_OUT not set"))?;
            let name: String = arg(&a, "defaultName")?;
            let safe = name.replace(['/', '\\'], "_");
            let path = dir.join(safe);
            std::fs::write(&path, arg::<String>(&a, "contents")?)?;
            json!(path.to_string_lossy())
        }
        "scan_env_files" => match env_path("TABLA_BRIDGE_PROJECT") {
            Some(d) => json!(commands::scan_env_dir(&d)?),
            None => Value::Null,
        },
        "mcp_command" => json!(commands::mcp_info()?),
        "check_update" => json!(tokio::task::spawn_blocking(crate::updates::check).await.map_err(|e| AppError::msg(e.to_string()))??),
        "open_project_page" => Value::Null,
        "load_app_state" => json!(crate::vault::load_state()?),
        "save_app_state" => {
            crate::vault::save_state(&arg(&a, "state")?)?;
            Value::Null
        }
        "oracle_client_status" => json!(tokio::task::spawn_blocking(crate::oracle_client::status).await.map_err(|e| AppError::msg(e.to_string()))?),
        other => return Err(AppError::msg(format!("bridge: unknown command {other}"))),
    })
}
