use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    config::{self, ConnectionConfig},
    db::{FetchArgs, ResultSet, RowEdit, Session, TablePage, TableStructure, TableInfo},
    error::{AppError, AppResult},
};

/// Hard cap on rows returned to the UI for a single ad-hoc statement.
const QUERY_ROW_LIMIT: usize = 10_000;

#[derive(Default)]
pub struct AppState {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl AppState {
    fn get(&self, id: &str) -> AppResult<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::msg("This connection was closed. Reconnect to continue."))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    session_id: String,
    server_version: String,
    schemas: Vec<String>,
    current_schema: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutcome {
    results: Vec<ResultSet>,
    elapsed_ms: u128,
}

#[tauri::command]
pub fn list_connections(app: AppHandle) -> AppResult<Vec<ConnectionConfig>> {
    config::load(&app)
}

#[tauri::command]
pub fn save_connection(app: AppHandle, config: ConnectionConfig) -> AppResult<ConnectionConfig> {
    config::upsert(&app, config)
}

#[tauri::command]
pub fn delete_connection(app: AppHandle, id: String) -> AppResult<()> {
    config::remove(&app, &id)
}

#[tauri::command]
pub async fn test_connection(config: ConnectionConfig) -> AppResult<String> {
    let password = config::resolve_password(&config)?;
    let session = Session::connect(&config, password).await?;
    let version = session.server_version().await;
    session.close().await;
    version
}

#[tauri::command]
pub async fn open_session(state: State<'_, AppState>, config: ConnectionConfig) -> AppResult<SessionInfo> {
    let password = config::resolve_password(&config)?;
    let session = Session::connect(&config, password).await?;
    let server_version = session.server_version().await?;
    let schemas = session.list_schemas().await?;
    let current_schema = session.current_schema().await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state.sessions.lock().unwrap().insert(session_id.clone(), Arc::new(session));
    Ok(SessionInfo { session_id, server_version, schemas, current_schema })
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
    let session = state.sessions.lock().unwrap().remove(&session_id);
    if let Some(s) = session {
        s.close().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn list_schemas(state: State<'_, AppState>, session_id: String) -> AppResult<Vec<String>> {
    state.get(&session_id)?.list_schemas().await
}

#[tauri::command]
pub async fn list_tables(state: State<'_, AppState>, session_id: String, schema: String) -> AppResult<Vec<TableInfo>> {
    state.get(&session_id)?.list_tables(&schema).await
}

#[tauri::command]
pub async fn table_structure(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> AppResult<TableStructure> {
    state.get(&session_id)?.table_structure(&schema, &table).await
}

#[tauri::command]
pub async fn fetch_rows(state: State<'_, AppState>, session_id: String, args: FetchArgs) -> AppResult<TablePage> {
    state.get(&session_id)?.fetch_rows(&args).await
}

#[tauri::command]
pub async fn run_query(state: State<'_, AppState>, session_id: String, sql: String) -> AppResult<QueryOutcome> {
    let session = state.get(&session_id)?;
    let started = Instant::now();
    let results = session.run_script(&sql, QUERY_ROW_LIMIT).await?;
    Ok(QueryOutcome { results, elapsed_ms: started.elapsed().as_millis() })
}

#[tauri::command]
pub async fn apply_edits(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
    edits: Vec<RowEdit>,
) -> AppResult<u64> {
    state.get(&session_id)?.apply_edits(&schema, &table, edits).await
}

/// Native file picker for SQLite databases. The path is chosen by the user,
/// never supplied by the web layer.
#[tauri::command]
pub async fn pick_sqlite_file(app: AppHandle) -> AppResult<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .add_filter("SQLite database", &["db", "sqlite", "sqlite3", "db3"])
        .add_filter("All files", &["*"])
        .blocking_pick_file();
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

/// Saves exported data through a native "Save as" dialog, so the web layer
/// can never write to an arbitrary path on its own.
#[tauri::command]
pub async fn export_file(app: AppHandle, default_name: String, contents: String) -> AppResult<Option<String>> {
    let ext = default_name.rsplit('.').next().unwrap_or("txt").to_string();
    let picked = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter(ext.to_uppercase(), &[ext.as_str()])
        .blocking_save_file();
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, contents)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}
