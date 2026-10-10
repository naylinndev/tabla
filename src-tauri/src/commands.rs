//! Tauri commands. The logic lives on `AppState` so the debug-only test
//! bridge (`bridge.rs`) exercises exactly the same code paths.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    alter::AlterRequest,
    config::Driver,
    redis_db::{RedisEdit, RedisOverview, RedisSession, RedisValue, ScanPage},
    users::UserInfo,
    config::{self, ConnectionConfig},
    db::{FetchArgs, LogEntry, TableRelations, ResultSet, RowEdit, Session, TableDetails, TableInfo, TablePage, TableStructure},
    error::{AppError, AppResult},
};

/// Hard cap on rows returned to the UI for a single ad-hoc statement.
const QUERY_ROW_LIMIT: usize = 10_000;

pub enum AnySession {
    Sql(Arc<Session>),
    Redis(Arc<RedisSession>),
}

#[derive(Default)]
pub struct AppState {
    sessions: Mutex<HashMap<String, AnySession>>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvUrl {
    file: String,
    key: String,
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCommand {
    /// Absolute path of the installed Tabla executable; AI tools launch it with `mcp`.
    binary: String,
    /// False when Tabla isn't installed yet (dev build): `binary` is then the
    /// standard install location, which will exist once Tabla is installed.
    installed: bool,
}

impl AppState {
    fn closed() -> AppError {
        AppError::msg("This connection was closed. Reconnect to continue.")
    }

    pub fn get(&self, id: &str) -> AppResult<Arc<Session>> {
        match self.sessions.lock().unwrap().get(id) {
            Some(AnySession::Sql(s)) => Ok(s.clone()),
            Some(AnySession::Redis(_)) => Err(crate::error::not_sql()),
            None => Err(Self::closed()),
        }
    }

    pub fn get_redis(&self, id: &str) -> AppResult<Arc<RedisSession>> {
        match self.sessions.lock().unwrap().get(id) {
            Some(AnySession::Redis(s)) => Ok(s.clone()),
            Some(AnySession::Sql(_)) => Err(AppError::msg("Not a Redis connection")),
            None => Err(Self::closed()),
        }
    }

    pub async fn test(&self, config: ConnectionConfig) -> AppResult<String> {
        let password = config::resolve_password(&config)?;
        if config.driver == Driver::Redis {
            let s = RedisSession::open(&config, password).await?;
            return Ok(s.overview().await?.version);
        }
        let session = Session::connect(&config, password).await?;
        let version = session.server_version().await;
        session.close().await;
        version
    }

    pub async fn open(&self, config: ConnectionConfig) -> AppResult<SessionInfo> {
        let password = config::resolve_password(&config)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        if config.driver == Driver::Redis {
            let s = RedisSession::open(&config, password).await?;
            let ov = s.overview().await?;
            let current = config.database.trim().parse::<u32>().unwrap_or(0).to_string();
            let schemas = ov.databases.iter().map(|d| d.index.to_string()).collect();
            self.sessions.lock().unwrap().insert(session_id.clone(), AnySession::Redis(Arc::new(s)));
            return Ok(SessionInfo { session_id, server_version: ov.version, schemas, current_schema: Some(current) });
        }
        let session = Session::connect(&config, password).await?;
        let server_version = session.server_version().await?;
        let schemas = session.list_schemas().await?;
        let current_schema = session.current_schema().await?;
        self.sessions.lock().unwrap().insert(session_id.clone(), AnySession::Sql(Arc::new(session)));
        Ok(SessionInfo { session_id, server_version, schemas, current_schema })
    }

    pub async fn close(&self, session_id: &str) {
        let session = self.sessions.lock().unwrap().remove(session_id);
        match session {
            Some(AnySession::Sql(s)) => s.close().await,
            Some(AnySession::Redis(s)) => s.close().await,
            None => {}
        }
    }

    pub fn log_since(&self, id: &str, after: u64) -> AppResult<Vec<LogEntry>> {
        match self.sessions.lock().unwrap().get(id) {
            Some(AnySession::Sql(s)) => Ok(s.log_since(after)),
            Some(AnySession::Redis(s)) => Ok(s.log_since(after)),
            None => Err(Self::closed()),
        }
    }

    pub fn clear_log(&self, id: &str) -> AppResult<()> {
        match self.sessions.lock().unwrap().get(id) {
            Some(AnySession::Sql(s)) => s.clear_log(),
            Some(AnySession::Redis(s)) => s.clear_log(),
            None => return Err(Self::closed()),
        }
        Ok(())
    }

    pub async fn query(&self, session_id: &str, sql: &str) -> AppResult<QueryOutcome> {
        let session = self.get(session_id)?;
        let started = Instant::now();
        let results = session.run_script(sql, QUERY_ROW_LIMIT).await?;
        Ok(QueryOutcome { results, elapsed_ms: started.elapsed().as_millis() })
    }
}

/// Finds database URLs in `.env*` files directly inside `dir`
/// (DATABASE_URL, POSTGRES_URL, …). Only DB-looking values are returned.
pub fn scan_env_dir(dir: &Path) -> AppResult<Vec<EnvUrl>> {
    let mut found = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name == ".env" || name.starts_with(".env.")) || !entry.path().is_file() {
            continue;
        }
        if entry.metadata().map(|m| m.len() > 1_000_000).unwrap_or(true) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(entry.path()) else { continue };
        for line in body.lines() {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line).trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            let lower = value.to_ascii_lowercase();
            let is_db = ["postgres://", "postgresql://", "mysql://", "mariadb://", "sqlite:", "file:"]
                .iter()
                .any(|p| lower.starts_with(p));
            if is_db {
                found.push(EnvUrl { file: name.clone(), key: key.trim().to_string(), url: value.to_string() });
            }
        }
    }
    Ok(found)
}

/// Where installers put the Tabla executable on each platform.
fn install_locations() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // Homebrew's cask links `tabla` here; prefer it (short and stable across updates).
        v.push("/opt/homebrew/bin/tabla".into());
        v.push("/usr/local/bin/tabla".into());
        v.push("/Applications/Tabla.app/Contents/MacOS/tabla".into());
        if let Some(h) = dirs::home_dir() {
            v.push(h.join("Applications/Tabla.app/Contents/MacOS/tabla"));
        }
    }
    #[cfg(windows)]
    {
        if let Some(d) = dirs::data_local_dir() {
            v.push(d.join("Tabla").join("tabla.exe"));
            v.push(d.join("Programs").join("Tabla").join("tabla.exe"));
        }
        v.push(r"C:\Program Files\Tabla\tabla.exe".into());
    }
    #[cfg(target_os = "linux")]
    {
        v.push("/usr/bin/tabla".into());
        v.push("/usr/local/bin/tabla".into());
    }
    v
}

pub fn mcp_info() -> AppResult<McpCommand> {
    let exe = std::env::current_exe()?;
    let is_dev_build = exe.components().any(|c| c.as_os_str() == "target");
    let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
    // An installed copy (or an AppImage, which runs from wherever it lives) is used as-is.
    if !is_dev_build {
        return Ok(McpCommand { binary: path(&exe), installed: true });
    }
    let locations = install_locations();
    if let Some(found) = locations.iter().find(|p| p.exists()) {
        return Ok(McpCommand { binary: path(found), installed: true });
    }
    let fallback = locations
        .iter()
        .find(|p| p.to_string_lossy().contains(".app/") || cfg!(not(target_os = "macos")))
        .map(|p| path(p))
        .unwrap_or_else(|| path(&exe));
    Ok(McpCommand { binary: fallback, installed: false })
}

#[tauri::command]
pub fn list_connections() -> AppResult<Vec<ConnectionConfig>> {
    config::load()
}

#[tauri::command]
pub fn save_connection(config: ConnectionConfig) -> AppResult<ConnectionConfig> {
    config::upsert(config)
}

#[tauri::command]
pub fn delete_connection(id: String) -> AppResult<()> {
    config::remove(&id)
}

#[tauri::command]
pub async fn test_connection(state: State<'_, AppState>, config: ConnectionConfig) -> AppResult<String> {
    state.test(config).await
}

#[tauri::command]
pub async fn open_session(state: State<'_, AppState>, config: ConnectionConfig) -> AppResult<SessionInfo> {
    state.open(config).await
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
    state.close(&session_id).await;
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
pub fn query_log(state: State<'_, AppState>, session_id: String, after: u64) -> AppResult<Vec<LogEntry>> {
    state.log_since(&session_id, after)
}

#[tauri::command]
pub fn clear_query_log(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
    state.clear_log(&session_id)
}

#[tauri::command]
pub async fn redis_overview(state: State<'_, AppState>, session_id: String) -> AppResult<RedisOverview> {
    state.get_redis(&session_id)?.overview().await
}

#[tauri::command]
pub async fn redis_select(state: State<'_, AppState>, session_id: String, db: u32) -> AppResult<()> {
    state.get_redis(&session_id)?.select(db).await
}

#[tauri::command]
pub async fn redis_scan(state: State<'_, AppState>, session_id: String, pattern: String, cursor: u64) -> AppResult<ScanPage> {
    state.get_redis(&session_id)?.scan(&pattern, cursor, 500).await
}

#[tauri::command]
pub async fn redis_get(state: State<'_, AppState>, session_id: String, key: String) -> AppResult<RedisValue> {
    state.get_redis(&session_id)?.get(&key).await
}

#[tauri::command]
pub async fn redis_edit(state: State<'_, AppState>, session_id: String, edit: RedisEdit) -> AppResult<()> {
    state.get_redis(&session_id)?.edit(edit).await
}

#[tauri::command]
pub async fn redis_command(state: State<'_, AppState>, session_id: String, line: String, allow_write: bool) -> AppResult<String> {
    state.get_redis(&session_id)?.command(&line, allow_write).await
}

#[tauri::command]
pub async fn list_users(state: State<'_, AppState>, session_id: String) -> AppResult<Vec<UserInfo>> {
    state.get(&session_id)?.list_users().await
}

#[tauri::command]
pub async fn user_grants(state: State<'_, AppState>, session_id: String, name: String, host: String) -> AppResult<Vec<String>> {
    state.get(&session_id)?.user_grants(&name, &host).await
}

#[tauri::command]
pub async fn table_relations(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> AppResult<TableRelations> {
    state.get(&session_id)?.table_relations(&schema, &table).await
}

#[tauri::command]
pub async fn table_details(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> AppResult<TableDetails> {
    state.get(&session_id)?.table_details(&schema, &table).await
}

#[tauri::command]
pub async fn fetch_rows(state: State<'_, AppState>, session_id: String, args: FetchArgs) -> AppResult<TablePage> {
    state.get(&session_id)?.fetch_rows(&args).await
}

#[tauri::command]
pub async fn run_query(state: State<'_, AppState>, session_id: String, sql: String) -> AppResult<QueryOutcome> {
    state.query(&session_id, &sql).await
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

#[tauri::command]
pub async fn plan_alter(state: State<'_, AppState>, session_id: String, req: AlterRequest) -> AppResult<Vec<String>> {
    state.get(&session_id)?.plan_alter(&req).await
}

#[tauri::command]
pub async fn apply_alter(state: State<'_, AppState>, session_id: String, req: AlterRequest) -> AppResult<Vec<String>> {
    state.get(&session_id)?.apply_alter(&req).await
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

/// Native picker for SSH keys and TLS certificates. Returns only the path;
/// Tabla never copies or stores the file's contents.
#[tauri::command]
pub async fn pick_file(app: AppHandle, kind: String) -> AppResult<Option<String>> {
    let mut d = app.dialog().file();
    if kind == "ssh-key" {
        if let Some(home) = dirs::home_dir() {
            d = d.set_directory(home.join(".ssh"));
        }
    } else {
        d = d.add_filter("Certificates and keys", &["pem", "crt", "cer", "key"]).add_filter("All files", &["*"]);
    }
    Ok(d.blocking_pick_file().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
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

/// Lets the user pick a project folder and lists database URLs found in its `.env*` files.
#[tauri::command]
pub async fn scan_env_files(app: AppHandle) -> AppResult<Option<Vec<EnvUrl>>> {
    let Some(dir) = app.dialog().file().blocking_pick_folder().and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    scan_env_dir(&dir).map(Some)
}

#[tauri::command]
pub fn mcp_command() -> AppResult<McpCommand> {
    mcp_info()
}
