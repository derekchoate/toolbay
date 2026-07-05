//! Toolbay — macOS menu bar supervisor for background CLI/AI tools.

pub mod paths;
mod commands;
mod config_patch;
mod ledger;
mod manifest_headroom;
mod runtime_install;
mod supervisor;

use std::path::PathBuf;
use tauri::Manager;

/// Application-wide shared state.
pub struct AppState {
    pub home: Option<PathBuf>,
    pub supervisor: std::sync::Arc<tokio::sync::Mutex<Option<supervisor::Supervisor>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            home: None,
            supervisor: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let home = paths::resolve_home();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            app.set_dock_visibility(false);
            let state = AppState {
                home,
                supervisor: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            };
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::install_tool,
            commands::start_tool,
            commands::stop_tool,
            commands::restart_tool,
            commands::uninstall_tool,
            commands::tail_log,
            commands::open_logs_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}