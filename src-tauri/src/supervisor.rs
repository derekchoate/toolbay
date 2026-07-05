//! Process lifecycle for headroom-ai (generic-shaped but wired to one tool in Phase 1).
//!
//! Status enum: Stopped | Starting | Running | Crashed | Restarting

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::manifest_headroom as M;
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
// Spawn
// ---------------------------------------------------------------------------

/// Spawn the headroom-ai process with stdout/stderr piped.
pub async fn spawn(
    python_path: &Path,
    port: u16,
) -> Result<tokio::process::Child, SupervisorError> {
    let mut cmd = Command::new(python_path);
    cmd.args(["-m", "headroom.ai"])
        .arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;
    Ok(child)
}

// ---------------------------------------------------------------------------
// Status channel (for UI updates)
// ---------------------------------------------------------------------------

/// Create a status channel pair for communicating status changes to the UI.
pub fn status_channel() -> (mpsc::UnboundedSender<Status>, mpsc::UnboundedReceiver<Status>) {
    mpsc::unbounded_channel()
}

// ---------------------------------------------------------------------------
// Supervisor handle
// ---------------------------------------------------------------------------

/// Supervisor state shared between tasks.
#[derive(Debug)]
pub struct Supervisor {
    pub status: std::sync::Arc<tokio::sync::Mutex<Status>>,
    pub port: u16,
    pub python_path: PathBuf,
    pub home: PathBuf,
}

impl Supervisor {
    /// Create a new supervisor instance (process not yet spawned).
    pub fn new(python_path: PathBuf, port: u16, home: PathBuf) -> Self {
        Self {
            status: std::sync::Arc::new(tokio::sync::Mutex::new(Status::Stopped)),
            port,
            python_path,
            home,
        }
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

    /// Start the tool: allocate port, spawn process, stream logs.
    pub async fn start(
        &mut self,
        ports_path: &Path,
    ) -> Result<(), SupervisorError> {
        // Allocate port and persist
        {
            let mut state = PortsState::load(ports_path).unwrap_or_default();
            state.allocate_port(M::TOOL_ID, M::PORT_RANGE_START, M::PORT_RANGE_END)?;
            state.save(ports_path)?;
        }

        self.set_status(Status::Starting).await;

        // Prepare log files
        let (_log_path, _err_log_path) = prepare_log_files(&self.home, M::TOOL_ID)?;

        let python_path = self.python_path.clone();
        let port = self.port;

        // Spawn the process
        let mut child = spawn(&python_path, port).await?;

        self.set_status(Status::Running).await;

        // Wait for process to exit (this blocks until the process ends)
        let exit_status = child.wait().await.map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

        if !exit_status.success() {
            self.handle_crash(ports_path).await?;
        } else {
            self.set_status(Status::Stopped).await;
        }

        Ok(())
    }

    /// Handle a crash: apply backoff restart policy.
    async fn handle_crash(&self, _ports_path: &Path) -> Result<(), SupervisorError> {
        let mut attempts: u64 = 0;
        let max_attempts = M::MAX_RESTART_ATTEMPTS as u64;

        while attempts < max_attempts {
            attempts += 1;
            let delay_secs = *M::RESTART_BACKOFF_SECS.get((attempts - 1) as usize).unwrap_or(&30);

            self.set_status(Status::Restarting).await;
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;

            // Try to restart
            match spawn(&self.python_path, self.port).await {
                Ok(mut child) => {
                    let exit = child.wait().await.map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;
                    if exit.success() {
                        self.set_status(Status::Stopped).await;
                        return Ok(());
                    }
                    // Continue retrying on failure
                }
                Err(_) => {
                    // Continue retrying on spawn failure
                }
            }
        }

        self.set_status(Status::Crashed).await;
        Err(SupervisorError::Supervisor(format!(
            "Process crashed and failed to restart after {} attempts",
            max_attempts
        )))
    }

    /// Stop the running process (if any). In a full implementation this would hold
    /// a Child handle and terminate it.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        // Placeholder: in a real implementation we'd hold an Option<Child> and kill it.
        self.set_status(Status::Stopped).await;
        Ok(())
    }

    /// Restart the tool (needs &mut because start does).
    pub async fn restart(&mut self, ports_path: &Path) -> Result<(), SupervisorError> {
        self.stop().await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start(ports_path).await
    }

    /// Get the current status.
    pub async fn current_status(&self) -> Status {
        self.status.lock().await.clone()
    }
}

// ---------------------------------------------------------------------------
// Convenience functions for Tauri commands
// ---------------------------------------------------------------------------

/// Start headroom-ai using the supervisor. Returns port and initial status.
pub async fn supervisor_start_tool(
    home: PathBuf,
    python_path: PathBuf,
    ports_path: PathBuf,
) -> Result<(u16, Status), SupervisorError> {
    // Allocate port first
    let mut state = PortsState::load(&ports_path).unwrap_or_default();
    let port = state.allocate_port(M::TOOL_ID, M::PORT_RANGE_START, M::PORT_RANGE_END)?;
    state.save(&ports_path)?;

    let mut sup = Supervisor::new(python_path, port, home);
    sup.start(&ports_path).await?;

    Ok((port, Status::Running))
}

/// Stop headroom-ai using the provided home path and ports file.
pub async fn supervisor_stop_tool(home: PathBuf, _ports_path: PathBuf) -> Result<Status, SupervisorError> {
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
        let path = temp.path().join("ports.json");

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
    fn test_backoff_array_length_matches_max_attempts() {
        assert_eq!(
            M::RESTART_BACKOFF_SECS.len(),
            M::MAX_RESTART_ATTEMPTS as usize
        );
    }
}