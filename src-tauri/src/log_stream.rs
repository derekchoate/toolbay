//! Real-time log streaming via Tokio file tailing.
//!
//! Spawns a background task per tool that reads new content from the log file
//! on each cycle and pushes it to subscribers using Tauri's event system
//! (`app_handle.emit()`).
//!
//! ### Architecture
//! - `LogStreamManager` holds an `AppHandle` clone + home dir + active stream state
//! - Each stream is a Tokio task that uses std::fs (blocking) in spawn_blocking to read
//!   new bytes from the log file every 500ms
//! - Tasks are stopped explicitly via `stop_stream()` or `stop_all()`

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek};
use std::path::PathBuf;
use tokio::sync::{mpsc, Mutex};
use tauri::Emitter;

use crate::paths;

// ---------------------------------------------------------------------------
// Event payload — must be serializable for Tauri events
// ---------------------------------------------------------------------------

/// A log line emitted to the frontend as a real-time event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogLineEvent {
    /// The tool_id this log line belongs to.
    pub tool_id: String,
    /// The log line content (without trailing newline).
    pub line: String,
}

// ---------------------------------------------------------------------------
// StreamEntry — tracks a single tailer task's shutdown channel
// ---------------------------------------------------------------------------

struct StreamEntry {
    /// Channel sender to signal the tailer task to stop.
    shutdown_tx: mpsc::Sender<()>,
}

// ---------------------------------------------------------------------------
// LogStreamManager
// ---------------------------------------------------------------------------

/// Manages per-tool log streaming tasks.
///
/// Each tool_id can have at most one active stream task. When `stop_stream()`
/// is called, the tailer task receives a shutdown signal and exits cleanly.
pub struct LogStreamManager {
    /// Shared handle to emit events to the frontend webview.
    app_handle: tauri::AppHandle,
    /// Home directory path for resolving log file locations.
    home: PathBuf,
    /// Active streams keyed by tool_id.
    streams: Mutex<HashMap<String, StreamEntry>>,
}

impl LogStreamManager {
    /// Create a new stream manager.
    pub fn new(app_handle: tauri::AppHandle, home: PathBuf) -> Self {
        Self {
            app_handle,
            home,
            streams: Mutex::new(HashMap::new()),
        }
    }

    /// Start a log stream for the given tool_id.
    ///
    /// If a stream already exists for this tool, returns `false` (no-op).
    /// Otherwise spawns a new tailer task and returns `true`.
    pub async fn start_stream(&self, tool_id: &str) -> bool {
        let mut streams = self.streams.lock().await;

        // If stream already exists, nothing to do.
        if streams.contains_key(tool_id) {
            return false;
        }

        let home = self.home.clone();
        let log_path = paths::log_file(&home, tool_id);

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        // Spawn the tailer task.
        let app_handle = self.app_handle.clone();
        let tool_id_owned = tool_id.to_string();

        tokio::spawn(async move {
            Self::tail_log_task(&app_handle, &tool_id_owned, &log_path, shutdown_rx).await;
        });

        streams.insert(tool_id.to_string(), StreamEntry { shutdown_tx });
        true
    }

    /// Stop the log stream for a specific tool_id.
    pub async fn stop_stream(&self, tool_id: &str) {
        let mut streams = self.streams.lock().await;
        streams.remove(tool_id);
    }

    /// Stop ALL active log streams.
    pub async fn stop_all(&self) {
        let mut streams = self.streams.lock().await;
        streams.clear();
    }

    /// Tail a log file and emit new lines as Tauri events.
    ///
    /// This task runs in a blocking I/O loop:
    /// 1. Opens the log file for reading on first iteration
    /// 2. Tracks the last-read byte offset to avoid re-reading old content
    /// 3. On each cycle, reads only new bytes since last check
    /// 4. Emits complete lines via `app_handle.emit()` to all frontend listeners
    /// 5. Stops when shutdown is triggered or the file disappears permanently
    async fn tail_log_task(
        app_handle: &tauri::AppHandle,
        tool_id: &str,
        log_path: &PathBuf,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) {
        let event_name = "log-update";

        // Open the log file for reading synchronously.
        let mut file = match std::fs::File::open(log_path) {
            Ok(f) => f,
            Err(_) => return, // Log file doesn't exist yet — give up silently.
        };

        let metadata = match file.metadata() {
            Ok(m) => m,
            Err(_) => return,
        };
        let mut last_size: u64 = metadata.len();

        loop {
            tokio::select! {
                // Listen for shutdown signal.
                _ = shutdown_rx.recv() => {
                    break;
                }

                // Read new content from the log file.
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
                    // Re-check file metadata each cycle to get current size.
                    let current_metadata = match file.metadata() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    let current_size = current_metadata.len();

                    // If file was truncated/rotated (smaller than last read), reset.
                    if current_size < last_size {
                        match std::fs::File::open(log_path) {
                            Ok(f) => {
                                file = f;
                                last_size = 0;
                            }
                            Err(_) => continue,
                        }
                        continue;
                    }

                    // If no new bytes, loop again.
                    if current_size == last_size {
                        continue;
                    }

                    // Read the new portion of the file synchronously using BufReader.
                    let mut reader = BufReader::new(file.try_clone().unwrap_or_else(|_| {
                        // If try_clone fails, reopen the file.
                        std::fs::File::open(log_path).expect("failed to reopen log file")
                    }));

                    // Seek to last-read position.
                    if reader.seek(std::io::SeekFrom::Start(last_size)).is_err() {
                        continue;
                    }

                    let mut emitted_any = false;
                    loop {
                        let mut line_buffer = String::new();
                        let n = match reader.read_line(&mut line_buffer) {
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        if n == 0 {
                            break;
                        }
                        if !line_buffer.is_empty() {
                            let line = line_buffer.trim_end_matches(&['\r', '\n'][..]).to_string();
                            let event = LogLineEvent {
                                tool_id: tool_id.to_string(),
                                line,
                            };
                            let _ = app_handle.emit(event_name, event);
                            emitted_any = true;
                        }
                    }

                    // Update last_size to current position.
                    if emitted_any {
                        if let Ok(pos) = reader.seek(std::io::SeekFrom::Current(0)) {
                            last_size = pos;
                        }
                        // Re-acquire the file handle for next iteration since BufReader
                        // consumed some bytes from it.
                        match std::fs::File::open(log_path) {
                            Ok(f) => file = f,
                            Err(_) => break,
                        }
                    }
                }
            }
        }
    }
}