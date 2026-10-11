mod ai_guard;

/// Tests that change structure on the shared test databases take these locks,
/// so parallel tests never race on the same catalog.
#[cfg(test)]
pub(crate) mod test_lock {
    pub static PG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    pub static MYSQL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
}
mod alter;
#[cfg(debug_assertions)]
mod bridge;
mod commands;
mod config;
mod db;
mod error;
mod mcp;
mod redis_db;
mod ssh;
mod users;

pub fn run_mcp() {
    mcp::run();
}

/// Debug builds only: serve the backend over localhost HTTP for UI testing in a browser.
#[cfg(debug_assertions)]
pub fn run_bridge() {
    bridge::run();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_connections,
            commands::save_connection,
            commands::delete_connection,
            commands::test_connection,
            commands::open_session,
            commands::close_session,
            commands::list_schemas,
            commands::list_tables,
            commands::table_structure,
            commands::fetch_rows,
            commands::table_details,
            commands::table_relations,
            commands::list_users,
            commands::user_grants,
            commands::query_log,
            commands::clear_query_log,
            commands::redis_overview,
            commands::redis_select,
            commands::redis_scan,
            commands::redis_get,
            commands::redis_edit,
            commands::redis_command,
            commands::run_query,
            commands::cancel_query,
            commands::apply_edits,
            commands::plan_alter,
            commands::apply_alter,
            commands::pick_sqlite_file,
            commands::pick_file,
            commands::pick_csv_file,
            commands::export_file,
            commands::scan_env_files,
            commands::mcp_command,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tabla");
}
