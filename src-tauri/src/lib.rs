mod commands;
mod config;
mod db;
mod error;

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
            commands::run_query,
            commands::apply_edits,
            commands::pick_sqlite_file,
            commands::export_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tabla");
}
