//! Pinned configuration constants for headroom-ai (Phase 1 — hardcoded, not a manifest loader yet).
//!
//! More realistic placeholder values — the spec calls out that real URLs/hashes
//! must be resolved before shipping. These are structured so Phase 2 can swap them
//! in from JSON without code changes.

/// Unique tool identifier
pub const TOOL_ID: &str = "headroom-ai";

/// Display name shown in tray UI
#[allow(dead_code)]
pub const TOOL_DISPLAY_NAME: &str = "Headroom AI";

// ---------------------------------------------------------------------------
// Standalone Python interpreter (python-build-standalone)
// ---------------------------------------------------------------------------

/// Pinned python-build-standalone release for macOS arm64.
/// URL: https://github.com/indygreg/python-build-standalone/releases/download/20241016/cpython-3.12.7+20241016-macos-aarch64-none-install-only.tar.gz
pub const PYTHON_STANDALONE_URL: &str =
    "https://github.com/indygreg/python-build-standalone/releases/download/20241016/cpython-3.12.7%2B20241016-macos-aarch64-none-install-only.tar.gz";

/// Expected SHA-256 of the python-build-standalone archive.
/// Placeholder — replace with actual hash before shipping.
pub const PYTHON_STANDALONE_SHA256: &str =
    " PLACEHOLDER_REPLACE_WITH_ACTUAL_HASH ";

// ---------------------------------------------------------------------------
// headroom-ai wheel
// ---------------------------------------------------------------------------

/// Pinned headroom-ai wheel URL (arm64 macOS).
/// Placeholder — replace with the real version and hash before shipping.
pub const WHEEL_URL: &str =
    "https://pypi.org/packages/headroom-ai-0.1.0-cp312-cp312-macosx_14_0_arm64.whl";

/// Expected SHA-256 of the headroom-ai wheel.
/// Placeholder — replace with actual hash before shipping.
pub const WHEEL_SHA256: &str =
    " PLACEHOLDER_REPLACE_WITH_ACTUAL_HASH ";

// ---------------------------------------------------------------------------
// Runtime configuration
// ---------------------------------------------------------------------------

/// Port range for tool allocation: 18700–18799
pub const PORT_RANGE_START: u16 = 18700;
pub const PORT_RANGE_END: u16 = 18799;

/// Health check endpoint path (the full URL is `http://localhost:<port><HEALTH_CHECK_PATH>`)
#[allow(dead_code)]
pub const HEALTH_CHECK_PATH: &str = "/health";

/// Default health-check timeout in seconds
#[allow(dead_code)]
pub const HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Maximum number of restart attempts before marking as Crashed
pub const MAX_RESTART_ATTEMPTS: u32 = 5;

/// Backoff delays between restarts (in seconds): [1, 2, 5, 10, 30]
pub const RESTART_BACKOFF_SECS: &[u64] = &[1, 2, 5, 10, 30];

// ---------------------------------------------------------------------------
// Config-patch targets
// ---------------------------------------------------------------------------

/// Path to Claude Desktop settings file (relative to home directory)
#[allow(dead_code)]
pub const CLAUDE_SETTINGS_REL_PATH: &str = ".claude/settings.json";

/// Path to Codex config file (relative to home directory)
#[allow(dead_code)]
pub const CODEX_CONFIG_REL_PATH: &str = ".codex/config.toml";

/// Marker prefix for TOML block inserts — start marker: # >>> toolbay-managed:<toolId> >>>
#[allow(dead_code)]
pub fn toml_block_start_marker(tool_id: &str) -> String {
    format!("# >>> toolbay-managed:{} >>>", tool_id)
}

/// Marker suffix for TOML block inserts — end marker: # <<< toolbay-managed:<toolId> <<<
#[allow(dead_code)]
pub fn toml_block_end_marker(tool_id: &str) -> String {
    format!("# <<< toolbay-managed:{} <<<", tool_id)
}

// ---------------------------------------------------------------------------
// Launch configuration
// ---------------------------------------------------------------------------

/// The Python interpreter executable name (within the installed runtime dir).
#[allow(dead_code)]
pub const PYTHON_INTERPRETER_NAME: &str = "python";

/// Relative path inside the install directory where the wheel's entry-point script lives.
/// On Unix, pip-installed scripts go to `bin/`.
#[allow(dead_code)]
pub const SCRIPTS_DIR_NAME: &str = "Scripts";

/// The command used to launch headroom-ai after installation.
/// More realistic: uses the vendored interpreter path + a module invocation.
#[allow(dead_code)]
pub fn build_launch_command(python_path: impl AsRef<str>) -> Vec<String> {
    vec![python_path.as_ref().to_string(), "-m".to_string(), "headroom.ai".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_id_is_non_empty() {
        assert!(!TOOL_ID.is_empty());
    }

    #[test]
    fn test_display_name_is_non_empty() {
        assert!(!TOOL_DISPLAY_NAME.is_empty());
    }

    #[test]
    fn test_port_range_valid() {
        assert!(PORT_RANGE_START < PORT_RANGE_END);
    }

    #[test]
    fn test_max_restart_attempts_positive() {
        assert!(MAX_RESTART_ATTEMPTS > 0);
    }

    #[test]
    fn test_backoff_has_correct_length() {
        // BACKOFF_SECS length should match MAX_RESTART_ATTEMPTS
        assert_eq!(RESTART_BACKOFF_SECS.len(), MAX_RESTART_ATTEMPTS as usize);
    }

    #[test]
    fn test_toml_markers_include_tool_id() {
        let start = toml_block_start_marker(TOOL_ID);
        let end = toml_block_end_marker(TOOL_ID);
        assert!(start.contains(TOOL_ID));
        assert!(end.contains(TOOL_ID));
        assert!(start.starts_with("# >>>"));
        assert!(end.starts_with("# <<<"));
    }

    #[test]
    fn test_build_launch_command() {
        let cmd = build_launch_command("/path/to/python");
        assert_eq!(cmd, vec!["/path/to/python", "-m", "headroom.ai"]);
    }

    #[test]
    fn test_hashes_are_placeholders() {
        // The placeholders should be obvious so we don't ship with them accidentally.
        assert!(PYTHON_STANDALONE_SHA256.contains("PLACEHOLDER"));
        assert!(WHEEL_SHA256.contains("PLACEHOLDER"));
    }
}