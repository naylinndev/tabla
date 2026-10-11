mod ai_guard;

/// Tests that change structure on the shared test databases take these locks,
/// so parallel tests never race on the same catalog.
#[cfg(test)]
pub(crate) mod test_lock {
    pub static PG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    pub static MYSQL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Runs an async test body on a thread with a large stack. Debug builds
    /// of long end-to-end tests keep the whole body on the test thread's
    /// 2 MB stack (the app itself spawns futures onto the heap).
    pub fn big_stack<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
        std::thread::Builder::new()
            .stack_size(32 << 20)
            .spawn(move || tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_stack_size(32 << 20).enable_all().build().unwrap().block_on(f))
            .unwrap()
            .join()
            .unwrap();
    }
}
mod alter;
#[cfg(debug_assertions)]
mod bridge;
mod clickhouse;
mod commands;
mod config;
mod db;
mod duck;
mod error;
mod mcp;
mod mssql;
mod redis_db;
mod ssh;
mod updates;
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
    let context = tauri::generate_context!();
    let mut builder = tauri::Builder::default();
    // Release builds signed with the updater key carry its public key; others
    // keep the "open the release page" flow.
    if updates::updater_configured(context.config()) {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        // Used from Rust only (open_project_page validates the URL); no web permission is granted.
        .plugin(tauri_plugin_opener::init())
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
            commands::schema_graph,
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
            commands::pick_duckdb_file,
            commands::pick_file,
            commands::pick_csv_file,
            commands::export_file,
            commands::scan_env_files,
            commands::mcp_command,
            commands::check_update,
            commands::install_update,
            commands::open_project_page,
        ])
        .run(context)
        .expect("error while running Tabla");
}
