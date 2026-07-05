//! Tauri command handlers for multi-tool management.
//! All #[tauri::command] functions are defined here to avoid macro namespace pollution.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths;
use crate::manifest;
use crate::runtime_install;
use crate::supervisor;

// ---------------------------------------------------------------------------
// Status response type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatusResponse {
    pub status: String,
    pub port: Option<u16>,
    pub installed: bool,
}

/// Response for listing all tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfoResponse {
    pub tool_id: String,
    pub display_name: String,
    pub status: String,
    pub port: Option<u16>,
    pub installed: bool,
}

// ---------------------------------------------------------------------------
// Request types for register_tool
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RegisterToolRequest {
    pub tool_id: String,
    pub display_name: String,
    pub python_standalone_url: String,
    pub python_standalone_sha256: String,
    pub wheel_url: String,
    pub wheel_sha256: String,
    #[serde(default = "default_port_range_start")]
    pub port_range_start: u16,
    #[serde(default = "default_port_range_end")]
    pub port_range_end: u16,
    #[serde(default)]
    pub health_check_path: Option<String>,
    #[serde(default)]
    pub launch_command: Option<Vec<String>>,
}

fn default_port_range_start() -> u16 {
    18700
}

fn default_port_range_end() -> u16 {
    18799
}

// ---------------------------------------------------------------------------
// Tauri Commands (all in one module)
// ---------------------------------------------------------------------------

/// Get the current status of a specific tool (defaults to first running tool if not specified).
#[tauri::command]
pub async fn get_status(
    state: tauri::State<'_, crate::AppState>,
    tool_id: Option<String>,
) -> Result<ToolStatusResponse, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Resolve which tool to query
    let tid = tool_id.unwrap_or_else(|| {
        // Default to first registered tool's ID if available
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or_default()
    });

    let runtime = paths::runtime_dir(home, &tid);
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

/// List all registered tools with their current status.
#[tauri::command]
pub async fn list_tools(state: tauri::State<'_, crate::AppState>) -> Result<Vec<ToolInfoResponse>, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Collect tool data while holding the registry lock, then drop it before awaiting.
    let tools_data: Vec<(String, manifest::ToolManifest)> = {
        let reg = state.registry.read().map_err(|e| format!("Registry lock poisoned: {}", e))?;
        reg.tool_ids()
            .into_iter()
            .filter_map(|tid| reg.get(&tid).map(|m| (tid, m.clone())))
            .collect()
    };

    let sup = state.supervisor.lock().await;

    let mut results = Vec::new();

    for (tid, manifest) in tools_data {
        let runtime = paths::runtime_dir(home, &tid);
        let installed = runtime.exists();

        let status = if let Some(ref sup_ref) = *sup {
            let s = sup_ref.current_status().await;
            format!("{:?}", s)
        } else {
            "stopped".to_string()
        };

        results.push(ToolInfoResponse {
            tool_id: tid,
            display_name: manifest.display_name.clone(),
            status,
            port: Some(sup.as_ref().map(|s| s.port).unwrap_or(0)),
            installed,
        });
    }

    Ok(results)
}

/// Install a tool (downloads Python interpreter + wheel).
#[tauri::command]
pub async fn install_tool(
    state: tauri::State<'_, crate::AppState>,
    tool_id: Option<String>,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    std::fs::create_dir_all(paths::state_dir(home)).map_err(|e| format!("Failed to create state dir: {}", e))?;
    std::fs::create_dir_all(paths::toolbay_log_dir(home)).map_err(|e| format!("Failed to create log dir: {}", e))?;

    // Resolve tool_id from registry or default
    let tid = tool_id.unwrap_or_else(|| {
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or(crate::manifest::default_tool_id().to_string())
    });

    let manifest = state.registry
        .read()
        .map_err(|e| format!("Registry lock poisoned: {}", e))?
        .get(&tid)
        .ok_or_else(|| format!("Tool '{}' not found in registry", tid))?
        .clone();

    let runtime = paths::runtime_dir(home, &tid);
    
    // Download and install Python standalone
    let python_path = runtime_install::install_standalone_python(&runtime)
        .await
        .map_err(|e| format!("Python install failed: {}", e))?;
    
    // For now, use the tool's own wheel URL from registry (need to extend runtime_install API)
    // Since install_headroom_wheel is hardcoded, we fall back for non-headroom tools
    if tid == crate::manifest::default_tool_id() {
        runtime_install::install_headroom_wheel(&python_path, &runtime)
            .await
            .map_err(|e| format!("Wheel install failed: {}", e))?;
    } else {
        // For custom tools: download wheel directly into the runtime dir
        install_custom_wheel(&manifest, &runtime).await?;
    }

    let size = runtime_install::report_install_size(&runtime)
        .map_err(|e| format!("Size report failed: {}", e))?;
    let size_mb = size as f64 / 1_048_576.0;
    Ok(format!(
        "Installed {} to {}\nTotal size: {:.2} MB",
        manifest.display_name,
        python_path.parent().map(|p| p.to_string_lossy()).unwrap_or_default(),
        size_mb
    ))
}

/// Start a tool by its ID.
#[tauri::command]
pub async fn start_tool(
    state: tauri::State<'_, crate::AppState>,
    tool_id: Option<String>,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Resolve tool_id from registry or default
    let tid = tool_id.unwrap_or_else(|| {
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or(crate::manifest::default_tool_id().to_string())
    });

    let manifest = state.registry
        .read()
        .map_err(|e| format!("Registry lock poisoned: {}", e))?
        .get(&tid)
        .ok_or_else(|| format!("Tool '{}' not found in registry", tid))?
        .clone();

    let runtime = paths::runtime_dir(home, &tid);
    let python_path = find_python_in_runtime(&runtime).map_err(|e| format!("Python not found: {}", e))?;

    let ports_path = paths::ports_file(home);

    {
        let mut sup_opt = state.supervisor.lock().await;
        if sup_opt.is_none() {
            // Allocate port first using manifest's range
            let port = allocate_port(&tid, &manifest, home).map_err(|e| format!("Port allocation failed: {}", e))?;
            let sup = supervisor::Supervisor::new(python_path.clone(), port, home.clone());
            *sup_opt = Some(sup);
        }
        
        // Get the supervisor and start it
        let sup_ref = sup_opt.as_mut().ok_or("Supervisor not initialized")?;
        
        // Apply config patches for this tool
        apply_config_patches(home, &tid, &manifest).map_err(|e| format!("Config patch failed: {}", e))?;

        sup_ref.start(&ports_path).await.map_err(|e| format!("Start failed: {}", e))?;
    }

    Ok(format!("Tool '{}' started successfully", manifest.display_name))
}

/// Stop the running tool.
#[tauri::command]
pub async fn stop_tool(state: tauri::State<'_, crate::AppState>) -> Result<String, String> {
    let sup = state.supervisor.lock().await;
    let sup_ref = sup.as_ref().ok_or("Tool is not running")?;
    sup_ref.stop().await.map_err(|e| format!("Stop failed: {}", e))?;
    Ok("Tool stopped successfully".to_string())
}

/// Restart the tool.
#[tauri::command]
pub async fn restart_tool(
    state: tauri::State<'_, crate::AppState>,
    tool_id: Option<String>,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Resolve tool_id from registry or default  
    let tid = tool_id.unwrap_or_else(|| {
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or(crate::manifest::default_tool_id().to_string())
    });

    let manifest = state.registry
        .read()
        .map_err(|e| format!("Registry lock poisoned: {}", e))?
        .get(&tid)
        .ok_or_else(|| format!("Tool '{}' not found in registry", tid))?
        .clone();

    let ports_path = paths::ports_file(home);

    {
        let mut sup = state.supervisor.lock().await;
        let sup_ref = sup.as_mut().ok_or("Tool is not running")?;
        sup_ref.restart(&ports_path).await.map_err(|e| format!("Restart failed: {}", e))?;
    }

    Ok(format!("Tool '{}' restarted successfully", manifest.display_name))
}

/// Uninstall a tool by its ID.
#[tauri::command]
pub async fn uninstall_tool(
    state: tauri::State<'_, crate::AppState>,
    tool_id: Option<String>,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Resolve tool_id from registry or default
    let tid = tool_id.unwrap_or_else(|| {
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or(crate::manifest::default_tool_id().to_string())
    });

    let manifest = state.registry
        .read()
        .map_err(|e| format!("Registry lock poisoned: {}", e))?
        .get(&tid)
        .ok_or_else(|| format!("Tool '{}' not found in registry", tid))?
        .clone();

    // Stop if running
    {
        let sup = state.supervisor.lock().await;
        if let Some(ref sup_ref) = *sup {
            sup_ref.stop().await.map_err(|e| format!("Stop failed during uninstall: {}", e))?;
        }
    }

    // Remove runtime directory
    let runtime = paths::runtime_dir(home, &tid);
    if runtime.exists() {
        std::fs::remove_dir_all(&runtime).map_err(|e| format!("Failed to remove runtime dir: {}", e))?;
    }

    // Reverse config patches
    let ledger_path = paths::patches_file(home);
    let mut led = crate::ledger::PatchLedger::load(&ledger_path);
    let entries = led.reverse_patches_for_tool(&tid);

    // Unregister from registry
    {
        let mut reg = state.registry.write().map_err(|e| format!("Registry lock poisoned: {}", e))?;
        reg.unregister(&tid);
        
        // Save updated registry to disk
        let tools_path = paths::tools_file(home);
        if let Err(e) = reg.save(&tools_path) {
            eprintln!("Warning: failed to save registry after unregister: {}", e);
        }
    }

    Ok(format!(
        "Uninstalled '{}'. Reversed {} config patch(es).",
        manifest.display_name,
        entries.len()
    ))
}

/// Tail the log file for a tool.
#[tauri::command]
pub async fn tail_log(
    state: tauri::State<'_, crate::AppState>,
    lines: Option<usize>,
    tool_id: Option<String>,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Resolve tool_id from registry or default
    let tid = tool_id.unwrap_or_else(|| {
        let reg = state.registry.read().unwrap();
        reg.tool_ids().first().cloned().unwrap_or(crate::manifest::default_tool_id().to_string())
    });

    let log_path = paths::log_file(home, &tid);

    if !log_path.exists() {
        return Ok(String::new());
    }

    let content = std::fs::read_to_string(&log_path).map_err(|e| format!("Failed to read log: {}", e))?;
    let n = lines.unwrap_or(50);

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

/// Register a new custom tool in the registry.
#[tauri::command]
pub async fn register_tool(
    state: tauri::State<'_, crate::AppState>,
    req: RegisterToolRequest,
) -> Result<String, String> {
    let home = state.home.as_ref().ok_or("Home directory not resolved")?;

    // Validate port range
    if req.port_range_start >= req.port_range_end {
        return Err("port_range_start must be less than port_range_end".to_string());
    }

    let health_check_path = req.health_check_path.unwrap_or_else(|| "/health".to_string());
    let launch_command = req.launch_command.unwrap_or_else(|| vec!["python".to_string(), "-m".to_string()]);

    let manifest = manifest::ToolManifest {
        tool_id: req.tool_id,
        display_name: req.display_name,
        python_standalone_url: req.python_standalone_url,
        python_standalone_sha256: req.python_standalone_sha256,
        wheel_url: req.wheel_url,
        wheel_sha256: req.wheel_sha256,
        port_range_start: req.port_range_start,
        port_range_end: req.port_range_end,
        health_check_path,
        health_check_timeout_secs: 5,
        max_restart_attempts: 5,
        restart_backoff_secs: vec![1, 2, 5, 10, 30],
        config_patches: Vec::new(),
        launch_command,
    };

    // Register in memory
    {
        let mut reg = state.registry.write().map_err(|e| format!("Registry lock poisoned: {}", e))?;
        
        if !reg.register(manifest.clone()) && reg.get(&manifest.tool_id).is_some() {
            // Tool exists but we're replacing it - that's OK, register() returns false but updates
        }

        // Save to disk
        let tools_path = paths::tools_file(home);
        reg.save(&tools_path).map_err(|e| format!("Failed to save registry: {}", e))?;
    }

    Ok(format!("Registered tool '{}' ({})", manifest.display_name, manifest.tool_id))
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Find the Python interpreter in a runtime directory.
fn find_python_in_runtime(runtime_dir: &PathBuf) -> Result<PathBuf, String> {
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

/// Allocate a port for a tool using the manifest's port range.
fn allocate_port(tool_id: &str, manifest: &manifest::ToolManifest, home: &PathBuf) -> Result<u16, String> {
    let ports_path = paths::ports_file(home);
    let mut state = supervisor::PortsState::load(&ports_path).unwrap_or_default();
    let port = state.allocate_port(tool_id, manifest.port_range_start, manifest.port_range_end)
        .map_err(|e| format!("Port allocation failed: {}", e))?;
    state.save(&ports_path).map_err(|e| format!("Failed to save ports: {}", e))?;
    Ok(port)
}

/// Apply config patches for a tool (if any are defined in the manifest).
fn apply_config_patches(home: &PathBuf, tool_id: &str, manifest: &manifest::ToolManifest) -> Result<(), String> {
    if manifest.config_patches.is_empty() {
        return Ok(());
    }

    use crate::config_patch;

    let ledger_path = paths::patches_file(home);

    for patch in &manifest.config_patches {
        let target_full = home.join(&patch.rel_path);
        
        match patch.strategy.as_str() {
            "json-merge-key" => {
                if let (Some(key), Some(value)) = (&patch.json_key, &patch.json_value) {
                    config_patch::apply_json_merge_key(&target_full, key, value.clone(), tool_id, home, &ledger_path)
                        .map_err(|e| format!("JSON merge failed: {}", e))?;
                }
            }
            "toml-block-insert" => {
                if let Some(block) = &patch.toml_block {
                    config_patch::apply_toml_block_insert(&target_full, block, tool_id, home, &ledger_path)
                        .map_err(|e| format!("TOML insert failed: {}", e))?;
                }
            }
            _ => {
                eprintln!("Unknown config patch strategy: {}", patch.strategy);
            }
        }
    }

    Ok(())
}

/// Download and install a wheel for a custom tool (not using the hardcoded headroom function).
async fn install_custom_wheel(manifest: &manifest::ToolManifest, install_dir: &PathBuf) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use futures::StreamExt;
    use std::fs::{self, File};
    use std::io::{BufReader, Write};

    let wheel_archive_path = install_dir.join(format!("{}.whl", manifest.tool_id));

    // Download and verify the wheel
    let resp = reqwest::get(&manifest.wheel_url)
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {} from {}", resp.status(), manifest.wheel_url));
    }

    let mut hasher = Sha256::new();
    let mut file = File::create(&wheel_archive_path).map_err(|e| format!("Failed to create wheel file: {}", e))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|e| format!("Write error: {}", e))?;
    }

    file.flush().map_err(|e| format!("Flush error: {}", e))?;
    drop(file);

    // Verify hash
    let computed_hex = format!("{:x}", hasher.finalize());
    let expected_clean = manifest.wheel_sha256.trim().to_lowercase();
    let computed_clean = computed_hex.clone();

    if expected_clean != computed_clean {
        let _ = fs::remove_file(&wheel_archive_path);
        return Err(format!("SHA-256 mismatch: expected {}, got {}", expected_clean, computed_clean));
    }

    // Unpack the wheel (wheels are ZIP files)
    use zip::ZipArchive;
    
    let file = File::open(&wheel_archive_path).map_err(|e| format!("Failed to open wheel: {}", e))?;
    let mut archive = ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("Failed to unzip wheel: {}", e))?;

    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| format!("Failed to extract: {}", e))?;
        let outpath = install_dir.join(f.name().replace('\\', "/"));

        if f.name().ends_with('/') {
            fs::create_dir_all(&outpath).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            let mut outfile = File::create(&outpath).map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut f, &mut outfile).map_err(|e| format!("Copy error: {}", e))?;
        }
    }

    // Clean up wheel archive
    let _ = fs::remove_file(&wheel_archive_path);

    Ok(())
}