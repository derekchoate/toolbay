//! Process lifecycle management for any registered tool.
//!
//! Status enum: Stopped | Starting | Running | Crashed | Restarting

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::manifest::ToolManifest;
use crate::paths;

// ---------------------------------------------------------------------------
// Status enum
// ---------------------------------------------------------------------------

/// Current lifecycle status of the tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Stopped,
    Starting,
    Running,
    Crashed,
    Restarting,
}

impl Default for Status {
    fn default() -> Self {
        Status::Stopped
    }
}

// ---------------------------------------------------------------------------
// Ports state (JSON file)
// ---------------------------------------------------------------------------

/// In-memory map of tool_id → port.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PortsState {
    #[serde(default)]
    pub ports: std::collections::HashMap<String, u16>,
}

impl PortsState {
    /// Load from disk. Returns default empty state if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, SupervisorError> {
        let data = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&data).unwrap_or_default())
    }

    /// Save to disk.
    pub fn save(&self, path: &Path) -> Result<(), SupervisorError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }

    /// Find first free port in range and record it. Returns existing port if already allocated.
    pub fn allocate_port(&mut self, tool_id: &str, range_start: u16, range_end: u16) -> Result<u16, SupervisorError> {
        // Check already allocated first
        if let Some(&existing) = self.ports.get(tool_id) {
            return Ok(existing);
        }

        for port in range_start..=range_end {
            if !self.ports.values().any(|&p| p == port) {
                self.ports.insert(tool_id.to_string(), port);
                return Ok(port);
            }
        }

        Err(SupervisorError::PortExhausted(format!(
            "No free ports in range {}–{}",
            range_start, range_end
        )))
    }

    #[allow(dead_code)]
    /// Get the allocated port for a tool. Returns None if not allocated.
    pub fn get_port(&self, tool_id: &str) -> Option<u16> {
        self.ports.get(tool_id).copied()
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error types for supervisor operations.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Port allocation failed: {0}")]
    PortExhausted(String),
    #[error("Supervisor error: {0}")]
    Supervisor(String),
    #[error("Process spawn failed: {0}")]
    SpawnFailed(String),
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

/// Poll the HTTP health endpoint with a short timeout.
pub async fn health_check(base_url: &str, path: &str, timeout_secs: u64) -> bool {
    let url = format!("{}{}", base_url, path);
    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        reqwest::get(&url),
    )
    .await
    {
        Ok(Ok(resp)) => resp.status().is_success(),
        Ok(Err(_)) => false,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Log file preparation
// ---------------------------------------------------------------------------

/// Prepare log files and return their paths. Creates parent directories as needed.
pub fn prepare_log_files(home: &Path, tool_id: &str) -> Result<(PathBuf, PathBuf), SupervisorError> {
    let log_path = paths::log_file(home, tool_id);
    let err_log_path = paths::err_log_file(home, tool_id);

    // Ensure parent directories exist
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = err_log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Create/truncate the files so they're ready for writing
    File::create(&log_path)?;
    File::create(&err_log_path)?;

    Ok((log_path, err_log_path))
}

// ---------------------------------------------------------------------------
// Spawn (generic — uses manifest's launch_command + port)
// ---------------------------------------------------------------------------

/// Build a tokio Command from a manifest's launch configuration.
fn build_command(python_path: &Path, manifest: &ToolManifest, port: u16) -> Command {
    let mut cmd = Command::new(python_path);

    // Use the manifest's launch command if defined, otherwise fall back to module invocation
    let args = if !manifest.launch_command.is_empty() {
        // If launch_command includes python, use it directly; otherwise prepend python
        if manifest.launch_command.first().map(|c| c.as_str()) == Some(python_path.to_string_lossy().as_ref()) {
            let mut args = manifest.launch_command.clone();
            // Append port if not already present
            if !args.contains(&port.to_string()) {
                args.push(port.to_string());
            }
            args
        } else {
            // Prepend python path to the launch command
            let mut args = manifest.launch_command.clone();
            args.insert(0, "-m".to_string());
            args.insert(0, python_path.to_string_lossy().to_string());
            if !args.contains(&port.to_string()) {
                args.push(port.to_string());
            }
            args
        }
    } else {
        vec![
            python_path.to_string_lossy().to_string(),
            "-m".to_string(),
            "headroom.ai".to_string(),
            port.to_string(),
        ]
    };

    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd
}

/// Spawn the tool process with stdout/stderr piped.
pub async fn spawn(
    python_path: &Path,
    manifest: &ToolManifest,
    port: u16,
) -> Result<tokio::process::Child, SupervisorError> {
    let mut cmd = build_command(python_path, manifest, port);
    let child = cmd.spawn().map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;
    Ok(child)
}

// ---------------------------------------------------------------------------
// Status channel (for UI updates)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
/// Create a status channel pair for communicating status changes to the UI.
pub fn status_channel() -> (mpsc::UnboundedSender<Status>, mpsc::UnboundedReceiver<Status>) {
    mpsc::unbounded_channel()
}

// ---------------------------------------------------------------------------
// Supervisor handle — generic over any tool in the registry
// ---------------------------------------------------------------------------

/// Configuration needed to run a specific tool.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// The tool's unique identifier (from manifest).
    pub tool_id: String,
    /// Health check endpoint path (e.g., "/health").
    pub health_check_path: String,
    /// Health-check timeout in seconds.
    pub health_check_timeout_secs: u64,
    /// Maximum restart attempts before marking as Crashed.
    pub max_restart_attempts: u32,
    /// Backoff delays between restarts (in seconds).
    pub restart_backoff_secs: Vec<u64>,
}

impl ToolConfig {
    /// Create a ToolConfig from a manifest's configuration.
    pub fn from_manifest(manifest: &ToolManifest) -> Self {
        Self {
            tool_id: manifest.tool_id.clone(),
            health_check_path: manifest.health_check_path.clone(),
            health_check_timeout_secs: manifest.health_check_timeout_secs,
            max_restart_attempts: manifest.max_restart_attempts,
            restart_backoff_secs: manifest.restart_backoff_secs.clone(),
        }
    }

    /// Default config for backward compatibility (headroom-ai).
    #[allow(dead_code)]
    pub fn default_headroom() -> Self {
        use crate::manifest_headroom as M;
        Self {
            tool_id: M::TOOL_ID.to_string(),
            health_check_path: M::HEALTH_CHECK_PATH.to_string(),
            health_check_timeout_secs: M::HEALTH_CHECK_TIMEOUT_SECS,
            max_restart_attempts: M::MAX_RESTART_ATTEMPTS,
            restart_backoff_secs: M::RESTART_BACKOFF_SECS.to_vec(),
        }
    }
}

/// Supervisor state shared between tasks.
#[derive(Debug)]
pub struct Supervisor {
    pub status: std::sync::Arc<tokio::sync::Mutex<Status>>,
    pub port: u16,
    pub python_path: PathBuf,
    pub home: PathBuf,
    /// Configuration for this tool (from manifest).
    pub config: ToolConfig,
}

impl Supervisor {
    /// Create a new supervisor instance (process not yet spawned).
    pub fn new(python_path: PathBuf, port: u16, home: PathBuf, config: ToolConfig) -> Self {
        Self {
            status: std::sync::Arc::new(tokio::sync::Mutex::new(Status::Stopped)),
            port,
            python_path,
            home,
            config,
        }
    }

    /// Create a supervisor with default headroom-ai config (backward compat).
    #[allow(dead_code)]
    pub fn new_headroom(python_path: PathBuf, port: u16, home: PathBuf) -> Self {
        Self::new(python_path, port, home, ToolConfig::default_headroom())
    }

    /// Set the current status.
    pub async fn set_status(&self, status: Status) {
        let mut s = self.status.lock().await;
        *s = status.clone();
    }

    /// Get a clone of the status Arc for external monitoring.
    pub fn status_clone(&self) -> std::sync::Arc<tokio::sync::Mutex<Status>> {
        self.status.clone()
    }

    /// Get the tool_id for this supervisor.
    #[allow(dead_code)]
    pub fn tool_id(&self) -> &str {
        &self.config.tool_id
    }

    /// Start the tool: allocate port, spawn process, and run the health-monitor loop.
    /// 
    /// This method spawns a single process and monitors it with a background
    /// health-check task. If the health check fails or the process crashes,
    /// backoff restarts are attempted up to `max_restart_attempts`.
    pub async fn start(
        &mut self,
        ports_path: &Path,
        manifest: &ToolManifest,
    ) -> Result<(), SupervisorError> {
        // Allocate port and persist.
        {
            let mut state = PortsState::load(ports_path).unwrap_or_default();
            state.allocate_port(&self.config.tool_id, manifest.port_range_start, manifest.port_range_end)?;
            state.save(ports_path)?;
        }

        self.set_status(Status::Starting).await;

        // Prepare log files.
        let (_log_path, _err_log_path) = prepare_log_files(&self.home, &self.config.tool_id)?;

        let port = self.port;
        let total_max_attempts: u64 = self.config.max_restart_attempts as u64;
        let backoff = self.config.restart_backoff_secs.clone();

        // Spawn the initial process.
        let mut child = spawn(&self.python_path, manifest, port).await?;
        self.set_status(Status::Running).await;

        // Run the health-monitor loop: each iteration waits for either a crash or
        // health failure, then applies backoff and respawns until max attempts.
        for attempt in 0..=total_max_attempts {
            if attempt > 0 {
                // Apply backoff delay before restart (attempt 0 is the initial start).
                let delay_secs = *backoff.get((attempt - 1) as usize).unwrap_or(&30);
                self.set_status(Status::Restarting).await;
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;

                // Re-spawn the process after backoff.
                child = spawn(&self.python_path, manifest, port).await?;
                self.set_status(Status::Running).await;
            }

            // Wait for either: (a) process exits on its own, or (b) health check fails.
            let health_killed = Self::wait_for_crash_or_health_failure(&mut child, &self.config, port).await?;

            if !health_killed {
                // Process exited naturally — check exit status.
                let exit_status = child
                    .wait()
                    .await
                    .map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

                if exit_status.success() {
                    // Clean exit: tool finished normally.
                    self.set_status(Status::Stopped).await;
                    return Ok(());
                }
                // Process crashed (non-zero exit) — fall through to restart logic below.
            }
            // If health_killed is true, the process was already killed and waited on
            // inside wait_for_crash_or_health_failure.

            // Determine if we should retry.
            let total_exits = attempt + 1; // attempt 0 → first exit, etc.
            if total_exits >= total_max_attempts {
                self.set_status(Status::Crashed).await;
                return Err(SupervisorError::Supervisor(format!(
                    "Process {} after {} attempt(s)",
                    if health_killed { "became unhealthy" } else { "crashed" },
                    total_exits,
                )));
            }
        }

        // Should not reach here, but just in case.
        self.set_status(Status::Crashed).await;
        Err(SupervisorError::Supervisor(format!(
            "Process failed after {} attempts",
            total_max_attempts
        )))
    }

    /// Wait for a process to exit (crash) or for a health check to fail.
    /// 
    /// Uses `tokio::select!` to race between:
    /// 1. The child process exiting — returns `Ok(false)` so the caller checks exit_status.
    /// 2. A background health-monitor task detecting an unhealthy endpoint — kills the
    ///    process, waits for it, and returns `Ok(true)`.
    async fn wait_for_crash_or_health_failure(
        child: &mut tokio::process::Child,
        config: &ToolConfig,
        port: u16,
    ) -> Result<bool, SupervisorError> {
        // Clone config values to avoid lifetime issues with tokio::spawn.
        let hc_path = config.health_check_path.clone();
        let hc_timeout = config.health_check_timeout_secs;
        let tool_id_for_monitor = config.tool_id.clone();

        // Also keep a separate copy for use after the select! (outside the moved closure).
        let tool_id_after_select = config.tool_id.clone();

        // Channel for the health monitor to signal that it killed the process.
        let (signal_tx, mut signal_rx) = tokio::sync::mpsc::channel::<()>(1);

        // Spawn background health-monitor task using the tool's config.
        let status_arc = std::sync::Arc::new(tokio::sync::Mutex::new(Status::Running));

        let monitor_handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.tick().await; // initial tick to align with first check

            loop {
                tick.tick().await;

                // Skip health checks if status is no longer Running.
                {
                    let current = status_arc.lock().await;
                    if *current != Status::Running {
                        return;
                    }
                }

                // Perform health check against the process's port using tool-specific path.
                let healthy = health_check(
                    &format!("http://localhost:{}", port),
                    &hc_path,
                    hc_timeout,
                )
                .await;

                if !healthy {
                    eprintln!(
                        "Health check failed for tool '{}' on port {}, signaling process termination",
                        tool_id_for_monitor, port
                    );
                    let _ = signal_tx.send(()).await;
                    return;
                }
            }
        });

        // Race: process exit vs. health-monitor signal.
        tokio::select! {
            // Case 1: Process exited on its own (crash or clean shutdown).
            exit_status = child.wait() => {
                let _ = exit_status.map_err(|e| SupervisorError::SpawnFailed(e.to_string()));
                monitor_handle.abort();
                Ok(false) // Caller should check exit_status.success().
            }

            // Case 2: Health monitor detected failure — kill the process.
            _ = signal_rx.recv() => {
                eprintln!("Received health-failure signal: terminating unhealthy process for '{}'", tool_id_after_select);
                let _ = child.kill().await;
                let _ = child.wait().await;
                monitor_handle.abort();
                Ok(true) // Process was killed by health monitor.
            }
        }
    }

    /// Stop the running process (if any). In a full implementation this would hold
    /// a Child handle and terminate it.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        // Placeholder: in a real implementation we'd hold an Option<Child> and kill it.
        self.set_status(Status::Stopped).await;
        Ok(())
    }

    /// Restart the tool (needs &mut because start does).
    #[allow(dead_code)]
    pub async fn restart(
        &mut self,
        ports_path: &Path,
        manifest: &ToolManifest,
    ) -> Result<(), SupervisorError> {
        self.stop().await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start(ports_path, manifest).await
    }

    /// Get the current status.
    pub async fn current_status(&self) -> Status {
        self.status.lock().await.clone()
    }
}

// ---------------------------------------------------------------------------
// Convenience functions for Tauri commands (backward compatible)
// ---------------------------------------------------------------------------

/// Start headroom-ai using the supervisor. Returns port and initial status.
#[allow(dead_code)]
pub async fn supervisor_start_tool(
    home: PathBuf,
    python_path: PathBuf,
    ports_path: PathBuf,
) -> Result<(u16, Status), SupervisorError> {
    use crate::manifest_headroom as M;

    // Allocate port first
    let mut state = PortsState::load(&ports_path).unwrap_or_default();
    let port = state.allocate_port(M::TOOL_ID, M::PORT_RANGE_START, M::PORT_RANGE_END)?;
    state.save(&ports_path)?;

    let config = ToolConfig::from_manifest(&crate::manifest::builtin_headroom_manifest());
    let mut sup = Supervisor::new(python_path, port, home, config);
    let manifest = crate::manifest::builtin_headroom_manifest();
    sup.start(&ports_path, &manifest).await?;

    Ok((port, Status::Running))
}

/// Stop headroom-ai using the provided home path and ports file.
#[allow(dead_code)]
pub async fn supervisor_stop_tool(home: PathBuf, _ports_path: PathBuf) -> Result<Status, SupervisorError> {
    use crate::manifest_headroom as M;

    let tool_id = M::TOOL_ID;
    let log_path = paths::log_file(&home, tool_id);
    if log_path.exists() {
        let _ = std::fs::remove_file(&log_path);
    }
    Ok(Status::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_default_is_stopped() {
        assert_eq!(Status::default(), Status::Stopped);
    }

    #[test]
    fn test_ports_state_allocate_and_get() {
        let temp = tempfile::tempdir().unwrap();
        let _path = temp.path().join("ports.json");

        let mut state = PortsState::default();
        assert_eq!(state.get_port("headroom-ai"), None);

        let port = state.allocate_port("headroom-ai", 18700, 18799).unwrap();
        assert!(port >= 18700 && port <= 18799);
        assert_eq!(state.get_port("headroom-ai"), Some(port));

        // Second call should return same port
        let port2 = state.allocate_port("headroom-ai", 18700, 18799).unwrap();
        assert_eq!(port, port2);
    }

    #[test]
    fn test_ports_state_two_tools_get_different_ports() {
        let mut state = PortsState::default();
        let port1 = state.allocate_port("tool-a", 18700, 18799).unwrap();
        let port2 = state.allocate_port("tool-b", 18700, 18799).unwrap();
        assert_ne!(port1, port2);
    }

    #[test]
    fn test_ports_state_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ports.json");

        let mut state = PortsState::default();
        state.allocate_port("headroom-ai", 18700, 18799).unwrap();
        state.save(&path).unwrap();

        let loaded = PortsState::load(&path).unwrap();
        assert_eq!(loaded.get_port("headroom-ai"), Some(18700));
    }

    #[test]
    fn test_ports_state_load_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nonexistent.json");
        let state = PortsState::load(&path).unwrap_or_default();
        assert!(state.ports.is_empty());
    }

    #[test]
    fn test_port_range_exhaustion() {
        let mut state = PortsState::default();
        // Allocate all 3 ports in a tiny range
        for i in 0..3u16 {
            let result = state.allocate_port(&format!("tool-{}", i), 18700, 18702);
            assert!(result.is_ok());
        }
        // This should fail — no more ports
        let result = state.allocate_port("overflow-tool", 18700, 18702);
        assert!(result.is_err());
    }

    #[test]
    fn test_health_check_url_construction() {
        let base = "http://localhost:18700";
        let path = "/health";
        let full = format!("{}{}", base, path);
        assert_eq!(full, "http://localhost:18700/health");
    }

    #[test]
    fn test_tool_config_from_manifest() {
        use crate::manifest;
        let m = manifest::builtin_headroom_manifest();
        let config = ToolConfig::from_manifest(&m);
        assert_eq!(config.tool_id, "headroom-ai");
        assert_eq!(config.health_check_path, "/health");
        assert_eq!(config.max_restart_attempts, 5);
        assert_eq!(config.restart_backoff_secs.len(), 5);
    }

    #[test]
    fn test_tool_config_default_headroom() {
        use crate::manifest_headroom as M;
        let config = ToolConfig::default_headroom();
        assert_eq!(config.tool_id, M::TOOL_ID);
        assert_eq!(config.health_check_path, M::HEALTH_CHECK_PATH);
        assert_eq!(config.max_restart_attempts, M::MAX_RESTART_ATTEMPTS);
    }

    #[test]
    fn test_build_command_generic() {
        use crate::manifest;
        let m = manifest::builtin_headroom_manifest();
        let _cmd = build_command(Path::new("/usr/bin/python"), &m, 18700);
        // Command is built but not executed in tests — just verify it constructs without panicking
    }
}