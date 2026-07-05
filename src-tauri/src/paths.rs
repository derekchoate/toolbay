//! Single source of truth for every on-disk location used by toolbay.
//!
//! All paths are built from `dirs::home_dir()` rather than Tauri's identifier-suffixed helpers,
//! so the on-disk layout matches the spec exactly:
//! - App root: `~/Library/Application Support/toolbay/`
//! - Logs:     `~/Library/Logs/toolbay/`
//!
//! Every other module takes these paths as plain arguments — nothing else calls path helpers directly.
//! This makes everything unit-testable with `tempfile::TempDir` standing in for the real home directory.

use std::path::{Path, PathBuf};

/// Base directory: ~/Library/Application Support/toolbay/
pub fn toolbay_root(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("toolbay")
}

/// Runtime install directory for a specific tool: ~/Library/Application Support/toolbay/runtimes/<tool_id>/
pub fn runtime_dir(home: &Path, tool_id: &str) -> PathBuf {
    toolbay_root(home).join("runtimes").join(tool_id)
}

/// State directory: ~/Library/Application Support/toolbay/state/
pub fn state_dir(home: &Path) -> PathBuf {
    toolbay_root(home).join("state")
}

/// Ports state file: ~/Library/Application Support/toolbay/state/ports.json
pub fn ports_file(home: &Path) -> PathBuf {
    state_dir(home).join("ports.json")
}

/// Patches ledger file: ~/Library/Application Support/toolbay/state/patches.json
pub fn patches_file(home: &Path) -> PathBuf {
    state_dir(home).join("patches.json")
}

/// Tool registry file: ~/Library/Application Support/toolbay/state/tools.json
pub fn tools_file(home: &Path) -> PathBuf {
    state_dir(home).join("tools.json")
}

/// Backups directory for a specific tool: ~/Library/Application Support/toolbay/backups/<tool_id>/
pub fn backups_dir(home: &Path, tool_id: &str) -> PathBuf {
    toolbay_root(home).join("backups").join(tool_id)
}

/// Base log directory: ~/Library/Logs/toolbay/
pub fn toolbay_log_dir(home: &Path) -> PathBuf {
    home.join("Library").join("Logs").join("toolbay")
}

/// Log file for a specific tool: ~/Library/Logs/toolbay/<tool_id>.log
pub fn log_file(home: &Path, tool_id: &str) -> PathBuf {
    toolbay_log_dir(home).join(format!("{}.log", tool_id))
}

/// Error log file for a specific tool: ~/Library/Logs/toolbay/<tool_id>.err.log
pub fn err_log_file(home: &Path, tool_id: &str) -> PathBuf {
    toolbay_log_dir(home).join(format!("{}.err.log", tool_id))
}

/// Resolve the user's home directory. Returns None if it cannot be determined.
pub fn resolve_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Get a timestamp string suitable for use in backup filenames.
/// Format: YYYY-MM-DDTHH:MM:SS (UTC)
pub fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    let days_since_epoch: u64 = secs / 86400;
    let time_in_day: u32 = (secs % 86400) as u32;

    let hours = time_in_day / 3600;
    let minutes = (time_in_day % 3600) / 60;
    let seconds = time_in_day % 60;

    // Calculate year/month/day from days since epoch
    let mut current_days: i64 = days_since_epoch as i64;
    let mut year: i64 = 1970;
    loop {
        let leap = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366
        } else {
            365
        };
        if current_days < leap as i64 {
            break;
        }
        current_days -= leap as i64;
        year += 1;
    }

    let days_in_month = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;

    let mut month = 1;
    let mut day = current_days + 1; // days are 1-indexed
    for m in 1..=12 {
        let max_day = if m == 2 && is_leap {
            29
        } else {
            *days_in_month.get(m).unwrap_or(&31)
        };
        if day <= max_day as i64 {
            month = m;
            break;
        }
        day -= max_day as i64;
    }

    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolbay_root_path() {
        let home = PathBuf::from("/home/testuser");
        let root = toolbay_root(&home);
        assert_eq!(
            root,
            PathBuf::from("/home/testuser/Library/Application Support/toolbay")
        );
    }

    #[test]
    fn test_runtime_dir_path() {
        let home = PathBuf::from("/home/testuser");
        let dir = runtime_dir(&home, "headroom-ai");
        assert_eq!(
            dir,
            PathBuf::from(
                "/home/testuser/Library/Application Support/toolbay/runtimes/headroom-ai"
            )
        );
    }

    #[test]
    fn test_state_dir_path() {
        let home = PathBuf::from("/home/testuser");
        assert_eq!(state_dir(&home), toolbay_root(&home).join("state"));
    }

    #[test]
    fn test_ports_file_path() {
        let home = PathBuf::from("/home/testuser");
        assert_eq!(ports_file(&home), state_dir(&home).join("ports.json"));
    }

    #[test]
    fn test_patches_file_path() {
        let home = PathBuf::from("/home/testuser");
        assert_eq!(patches_file(&home), state_dir(&home).join("patches.json"));
    }

    #[test]
    fn test_backups_dir_path() {
        let home = PathBuf::from("/home/testuser");
        let dir = backups_dir(&home, "headroom-ai");
        assert_eq!(
            dir,
            PathBuf::from(
                "/home/testuser/Library/Application Support/toolbay/backups/headroom-ai"
            )
        );
    }

    #[test]
    fn test_log_file_path() {
        let home = PathBuf::from("/home/testuser");
        let log = log_file(&home, "headroom-ai");
        assert_eq!(log, PathBuf::from("/home/testuser/Library/Logs/toolbay/headroom-ai.log"));
    }

    #[test]
    fn test_err_log_file_path() {
        let home = PathBuf::from("/home/testuser");
        let log = err_log_file(&home, "headroom-ai");
        assert_eq!(log, PathBuf::from("/home/testuser/Library/Logs/toolbay/headroom-ai.err.log"));
    }

    #[test]
    fn test_timestamp_format() {
        let ts = timestamp();
        // Should match YYYY-MM-DDTHH:MM:SS pattern
        assert!(ts.starts_with("20"));
        assert_eq!(ts.len(), 19);
        assert_eq!(ts.chars().nth(4).unwrap(), '-');
        assert_eq!(ts.chars().nth(7).unwrap(), '-');
        assert_eq!(ts.chars().nth(10).unwrap(), 'T');
    }

    #[test]
    fn test_all_paths_share_root() {
        let home = PathBuf::from("/fake/home");
        let root = toolbay_root(&home);
        let state = state_dir(&home);
        let log_dir = toolbay_log_dir(&home);

        // All paths should start with the same root
        assert!(state.starts_with(root));
        // Logs are in a different base (~/Library/Logs/) but under the same home
        assert!(log_dir.starts_with(home.join("Library")));
    }

    #[test]
    fn test_paths_with_tempdir() {
        let temp_home = tempfile::tempdir().unwrap();
        let root = toolbay_root(temp_home.path());
        // The directory shouldn't exist yet — that's expected, modules create dirs on demand
        assert!(!root.exists());

        // But the computed path should be correct
        assert_eq!(root.file_name().unwrap(), "toolbay");
    }
}