// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Must run before anything else: it may restart the process (Linux only).
    tabla_lib::prepare_process();
    match std::env::args().nth(1).as_deref() {
        // Headless MCP server for AI assistants.
        Some("mcp") => tabla_lib::run_mcp(),
        // Debug builds only: backend over localhost HTTP for browser-based UI tests.
        #[cfg(debug_assertions)]
        Some("bridge") => tabla_lib::run_bridge(),
        _ => tabla_lib::run(),
    }
}
