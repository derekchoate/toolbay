//! Toolbay — macOS menu bar supervisor for background CLI/AI tools.

pub mod paths;
mod commands;
mod config_patch;
mod ledger;
mod log_stream;
mod manifest;
mod manifest_headroom;
mod runtime_install;
mod supervisor;

use std::collections::HashMap;
use std::path::PathBuf;
use tauri::Manager;

/// Tracks read offsets for incremental log reading per tool.
#[derive(Debug, Default)]
pub struct LogOffsets {
    offsets: HashMap<String, u64>,
}

impl LogOffsets {
    pub fn new() -> Self {
        Self {
            offsets: HashMap::new(),
        }
    }

    pub fn get(&self, tool_id: &str) -> u64 {
        *self.offsets.get(tool_id).unwrap_or(&0u64)
    }

    pub fn set(&mut self, tool_id: String, offset: u64) {
        self.offsets.insert(tool_id, offset);
    }
}

/// Application-wide shared state.
pub struct AppState {
    pub home: Option<PathBuf>,
    pub supervisor: std::sync::Arc<tokio::sync::Mutex<Option<supervisor::Supervisor>>>,
    /// Multi-tool registry loaded from disk or built-in defaults.
    pub registry: std::sync::Arc<std::sync::RwLock<manifest::ToolRegistry>>,
    /// Log read offsets for incremental tail_log reads.
    pub log_offsets: std::sync::Arc<std::sync::Mutex<LogOffsets>>,
    /// Real-time log stream manager (pushes new lines via Tauri events).
    pub log_stream: std::sync::Arc<log_stream::LogStreamManager>,
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

            // Create log stream manager with the AppHandle.
            let log_stream = std::sync::Arc::new(
                log_stream::LogStreamManager::new(app.app_handle().clone(), home.clone())
            );

            let state = AppState {
                home: Some(home),
                supervisor: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                registry,
                log_offsets: std::sync::Arc::new(std::sync::Mutex::new(LogOffsets::new())),
                log_stream,
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
            commands::start_log_stream,
            commands::stop_log_stream,
            commands::stop_all_log_streams,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}