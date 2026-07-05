//! Toolbay — macOS menu bar supervisor for background CLI/AI tools.

pub mod paths;
mod commands;
mod config_patch;
mod ledger;
mod manifest;
mod manifest_headroom;
mod runtime_install;
mod supervisor;

use std::path::PathBuf;
use tauri::Manager;

/// Application-wide shared state.
pub struct AppState {
    pub home: Option<PathBuf>,
    pub supervisor: std::sync::Arc<tokio::sync::Mutex<Option<supervisor::Supervisor>>>,
    /// Multi-tool registry loaded from disk or built-in defaults.
    pub registry: std::sync::Arc<std::sync::RwLock<manifest::ToolRegistry>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            home: None,
            supervisor: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            registry: std::sync::Arc::new(std::sync::RwLock::new(manifest::ToolRegistry::new())),
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

            let home = home.clone().ok_or_else(|| "Could not resolve home directory".to_string())?;

            // Load registry from disk (falls back to built-in defaults)
            let tools_path = paths::tools_file(&home);
            let registry = std::sync::Arc::new(std::sync::RwLock::new(
                manifest::ToolRegistry::load(&tools_path).unwrap_or_default()
            ));

            // Persist initial load so the file exists
            if let Err(e) = registry.read().unwrap().save(&tools_path) {
                eprintln!("Warning: failed to save initial registry: {}", e);
            }

            let state = AppState {
                home: Some(home),
                supervisor: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                registry,
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
            commands::list_tools,
            commands::register_tool,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}