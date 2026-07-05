//! Tauri command handlers for headroom-ai.
//! All #[tauri::command] functions are defined here to avoid macro namespace pollution.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::manifest_headroom as M;
use crate::runtime_install;
use crate::supervisor;
use crate::ledger;

// ---------------------------------------------------------------------------
// Status response type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatusResponse {
    pub status: String,
    pub port: Option<u16>,
    pub installed: bool,
}

// ---------------------------------------------------------------------------
// Tauri Commands (all in one module)
// ---------------------------------------------------------------------------

/// Get the current status of headroom-ai.
#[tauri::command]
pub async fn get_status(state: tauri::State<'_, crate::AppState>) -> Result<ToolStatusResponse, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;
    let tool_id = M::TOOL_ID;

    let runtime = paths::runtime_dir(home, tool_id);
    let installed = runtime.exists();

    let sup = state.supervisor.lock().await;
    if let Some(ref sup_ref) = *sup {
        let status = sup_ref.current_status().await;
        Ok(ToolStatusResponse {
            status: format!("{:?}", status),
            port: Some(sup_ref.port),
            installed,
        })
    } else {
        Ok(ToolStatusResponse {
            status: "stopped".to_string(),
            port: None,
            installed,
        })
    }
}

/// Install headroom-ai (downloads Python interpreter + wheel).
#[tauri::command]
pub async fn install_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    std::fs::create_dir_all(paths::state_dir(home)).map_err(|e| format!("Failed to create state dir: {}", e))?;
    std::fs::create_dir_all(paths::toolbay_log_dir(home)).map_err(|e| format!("Failed to create log dir: {}", e))?;

    let (python_path, size) = runtime_install::install_headroom_ai(home).await.map_err(|e| format!("Install failed: {}", e))?;

    let size_mb = size as f64 / 1_048_576.0;
    Ok(format!(
        "Installed to {}\nTotal size: {:.2} MB",
        python_path.parent().map(|p| p.to_string_lossy()).unwrap_or_default(),
        size_mb
    ))
}

/// Start headroom-ai.
#[tauri::command]
pub async fn start_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;
    let tool_id = M::TOOL_ID;

    let runtime = paths::runtime_dir(home, tool_id);
    let python_path = find_python_in_runtime(&runtime).map_err(|e| format!("Python not found: {}", e))?;

    let ports_path = paths::ports_file(home);

    {
        let mut sup_opt = state.supervisor.lock().await;
        if sup_opt.is_none() {
            let sup = supervisor::Supervisor::new(python_path.clone(), 0, home.clone());
            *sup_opt = Some(sup);
        }
        let sup_ref = sup_opt.as_mut().ok_or("Supervisor not initialized")?;
        sup_ref.start(&ports_path).await.map_err(|e| format!("Start failed: {}", e))?;
    }

    Ok("Tool started successfully".to_string())
}

/// Stop headroom-ai.
#[tauri::command]
pub async fn stop_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let sup = state.supervisor.lock().await;
    let sup_ref = sup.as_ref().ok_or("Tool is not running")?;
    sup_ref.stop().await.map_err(|e| format!("Stop failed: {}", e))?;
    Ok("Tool stopped successfully".to_string())
}

/// Restart headroom-ai.
#[tauri::command]
pub async fn restart_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;
    let ports_path = paths::ports_file(home);

    {
        let mut sup = state.supervisor.lock().await;
        let sup_ref = sup.as_mut().ok_or("Tool is not running")?;
        sup_ref.restart(&ports_path).await.map_err(|e| format!("Restart failed: {}", e))?;
    }

    Ok("Tool restarted successfully".to_string())
}

/// Uninstall headroom-ai.
#[tauri::command]
pub async fn uninstall_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;
    let tool_id = M::TOOL_ID;

    {
        let sup = state.supervisor.lock().await;
        if let Some(ref sup_ref) = *sup {
            sup_ref.stop().await.map_err(|e| format!("Stop failed during uninstall: {}", e))?;
        }
    }

    let runtime = paths::runtime_dir(home, tool_id);
    if runtime.exists() {
        std::fs::remove_dir_all(&runtime).map_err(|e| format!("Failed to remove runtime dir: {}", e))?;
    }

    let ledger_path = paths::patches_file(home);
    let mut led = ledger::PatchLedger::load(&ledger_path);
    let entries = led.reverse_patches_for_tool(tool_id);

    Ok(format!(
        "Uninstalled. Reversed {} config patch(es).",
        entries.len()
    ))
}

/// Tail the log file for headroom-ai.
#[tauri::command]
pub async fn tail_log(_lines: Option<usize>) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("Could not resolve home directory")?;
    let tool_id = M::TOOL_ID;
    let log_path = paths::log_file(&home, tool_id);

    if !log_path.exists() {
        return Ok(String::new());
    }

    let content = std::fs::read_to_string(&log_path).map_err(|e| format!("Failed to read log: {}", e))?;
    let n = _lines.unwrap_or(50);

    let lines_vec: Vec<&str> = content.lines().collect();
    let result: Vec<String> = lines_vec.iter().rev().take(n).rev().map(|l| l.to_string()).collect();
    Ok(result.join("\n"))
}

/// Open the logs directory in Finder.
#[tauri::command]
pub fn open_logs_dir(_app: tauri::AppHandle) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not resolve home directory")?;
    let log_dir = paths::toolbay_log_dir(&home);

    if log_dir.exists() {
        std::process::Command::new("open")
            .arg(&log_dir)
            .spawn()
            .map_err(|e| format!("Failed to open logs dir: {}", e))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Find the Python interpreter in a runtime directory.
fn find_python_in_runtime(runtime_dir: &Path) -> Result<PathBuf, String> {
    use std::fs;

    let candidates = [
        runtime_dir.join("bin").join("python3"),
        runtime_dir.join("bin").join("python"),
        runtime_dir.join("Scripts").join("python.exe"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    for entry in fs::read_dir(runtime_dir).map_err(|e| format!("Failed to read runtime dir: {}", e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy().to_lowercase();

        if file_name_str.contains("python") && file_name_str.contains("macos") {
            let path = entry.path();
            for sub in ["bin/python3", "bin/python"] {
                let candidate = path.join(sub);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err("Could not find Python interpreter in runtime directory".to_string())
}