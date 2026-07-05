//! Generic tool manifest and registry system.
//!
//! Phase 2+: Replace hardcoded constants with a pluggable tool registry so
//! the supervisor can manage any number of tools, not just headroom-ai.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ToolManifest — per-tool configuration
// ---------------------------------------------------------------------------

/// Configuration for a single tool in the Toolbay system.
///
/// Each entry describes how to download, install, launch, and manage a tool's lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    /// Unique tool identifier (e.g., "headroom-ai", "codex-cli")
    pub tool_id: String,

    /// Display name shown in tray UI
    pub display_name: String,

    /// URL to download the Python standalone interpreter archive
    pub python_standalone_url: String,

    /// Expected SHA-256 of the Python standalone archive
    pub python_standalone_sha256: String,

    /// URL to download the tool's wheel file
    pub wheel_url: String,

    /// Expected SHA-256 of the tool wheel
    pub wheel_sha256: String,

    /// Port range for this tool: [start, end)
    pub port_range_start: u16,
    pub port_range_end: u16,

    /// Health check endpoint path (full URL = http://localhost:<port><path>)
    #[serde(default = "default_health_check_path")]
    pub health_check_path: String,

    /// Default health-check timeout in seconds
    #[serde(default = "default_health_check_timeout")]
    pub health_check_timeout_secs: u64,

    /// Maximum restart attempts before marking as Crashed
    #[serde(default = "default_max_restart_attempts_val")]
    pub max_restart_attempts: u32,

    /// Backoff delays between restarts (in seconds)
    #[serde(default = "default_restart_backoff_vec")]
    pub restart_backoff_secs: Vec<u64>,

    /// Config-patch targets for this tool
    #[serde(default)]
    pub config_patches: Vec<ConfigPatchTarget>,

    /// Command used to launch the tool (e.g., ["python", "-m", "headroom.ai"])
    #[serde(default = "default_launch_command")]
    pub launch_command: Vec<String>,
}

impl Default for ToolManifest {
    fn default() -> Self {
        Self {
            tool_id: String::new(),
            display_name: String::new(),
            python_standalone_url: String::new(),
            python_standalone_sha256: String::new(),
            wheel_url: String::new(),
            wheel_sha256: String::new(),
            port_range_start: 18700,
            port_range_end: 18799,
            health_check_path: default_health_check_path(),
            health_check_timeout_secs: default_health_check_timeout(),
            max_restart_attempts: default_max_restart_attempts_val(),
            restart_backoff_secs: default_restart_backoff_vec(),
            config_patches: Vec::new(),
            launch_command: default_launch_command(),
        }
    }
}

/// A single config-patch target that a tool can modify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPatchTarget {
    /// Target file path (relative to home directory)
    pub rel_path: String,

    /// Patch strategy: "json-merge-key", "toml-block-insert", or "env-file-line"
    pub strategy: String,

    /// For "json-merge-key": the key to merge into the top-level object
    #[serde(default)]
    pub json_key: Option<String>,

    /// For "json-merge-key": the value to set for `key`
    #[serde(default)]
    pub json_value: Option<serde_json::Value>,

    /// For "toml-block-insert": the TOML block text to insert
    #[serde(default)]
    pub toml_block: Option<String>,
}

impl Default for ConfigPatchTarget {
    fn default() -> Self {
        Self {
            rel_path: String::new(),
            strategy: String::new(),
            json_key: None,
            json_value: None,
            toml_block: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Default value helpers (private)
// ---------------------------------------------------------------------------

fn default_health_check_path() -> String {
    "/health".to_string()
}

fn default_health_check_timeout() -> u64 {
    5
}

fn default_max_restart_attempts_val() -> u32 {
    5
}

fn default_restart_backoff_vec() -> Vec<u64> {
    vec![1, 2, 5, 10, 30]
}

fn default_launch_command() -> Vec<String> {
    vec!["python".to_string(), "-m".to_string()]
}

// ---------------------------------------------------------------------------
// Built-in headroom-ai manifest
// ---------------------------------------------------------------------------

/// Create the built-in headroom-ai manifest (Phase 1 defaults).
pub fn builtin_headroom_manifest() -> ToolManifest {
    use crate::manifest_headroom as M;

    ToolManifest {
        tool_id: M::TOOL_ID.to_string(),
        display_name: M::TOOL_DISPLAY_NAME.to_string(),
        python_standalone_url: M::PYTHON_STANDALONE_URL.to_string(),
        python_standalone_sha256: M::PYTHON_STANDALONE_SHA256.to_string(),
        wheel_url: M::WHEEL_URL.to_string(),
        wheel_sha256: M::WHEEL_SHA256.to_string(),
        port_range_start: M::PORT_RANGE_START,
        port_range_end: M::PORT_RANGE_END,
        health_check_path: M::HEALTH_CHECK_PATH.to_string(),
        health_check_timeout_secs: M::HEALTH_CHECK_TIMEOUT_SECS,
        max_restart_attempts: M::MAX_RESTART_ATTEMPTS,
        restart_backoff_secs: M::RESTART_BACKOFF_SECS.to_vec(),
        config_patches: vec![
            // Claude Desktop settings patch
            ConfigPatchTarget {
                rel_path: M::CLAUDE_SETTINGS_REL_PATH.to_string(),
                strategy: "json-merge-key".to_string(),
                json_key: Some("headroom.enabled".to_string()),
                json_value: Some(serde_json::json!(true)),
                toml_block: None,
            },
            // Codex config patch
            ConfigPatchTarget {
                rel_path: M::CODEX_CONFIG_REL_PATH.to_string(),
                strategy: "toml-block-insert".to_string(),
                json_key: None,
                json_value: None,
                toml_block: Some(format!(
                    "[headroom]\nenabled = true\nport = {}",
                    M::PORT_RANGE_START
                )),
            },
        ],
        launch_command: vec![
            "python".to_string(),
            "-m".to_string(),
            "headroom.ai".to_string(),
        ],
    }
}

/// Get the TOML start marker for a tool ID.
#[allow(dead_code)]
pub fn toml_block_start_marker(tool_id: &str) -> String {
    format!("# >>> toolbay-managed:{} >>>", tool_id)
}

/// Get the TOML end marker for a tool ID.
#[allow(dead_code)]
pub fn toml_block_end_marker(tool_id: &str) -> String {
    format!("# <<< toolbay-managed:{} <<<", tool_id)
}

// ---------------------------------------------------------------------------
// ToolRegistry — in-memory + optional disk-backed store
// ---------------------------------------------------------------------------

/// Registry of known tools, loaded from the state directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistry {
    /// Built-in manifests that ship with toolbay (cannot be removed)
    #[serde(default)]
    pub builtins: HashMap<String, ToolManifest>,

    /// User-added or dynamically registered tools
    #[serde(default)]
    pub custom: HashMap<String, ToolManifest>,
}

impl ToolRegistry {
    #[allow(dead_code)]
    /// Create a new registry with the built-in headroom-ai manifest.
    pub fn new() -> Self {
        let mut registry = Self {
            builtins: HashMap::new(),
            custom: HashMap::new(),
        };

        // Register the built-in headroom manifest
        let headroom = builtin_headroom_manifest();
        registry.builtins.insert(headroom.tool_id.clone(), headroom);

        registry
    }

    #[allow(dead_code)]
    /// Load from disk. Creates a fresh registry with built-ins if file doesn't exist.
    pub fn load(path: &std::path::Path) -> Result<Self, std::io::Error> {
        use std::fs;

        if path.exists() {
            let data = fs::read_to_string(path)?;
            let mut loaded: Self = serde_json::from_str(&data).unwrap_or_default();

            // Merge built-ins (preserve user custom, always include builtins)
            for (id, manifest) in builtin_registry().builtins {
                if !loaded.builtins.contains_key(&id) {
                    loaded.builtins.insert(id, manifest);
                }
            }

            Ok(loaded)
        } else {
            Ok(Self::new())
        }
    }

    #[allow(dead_code)]
    /// Save to disk.
    pub fn save(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{}\n", data))?;
        Ok(())
    }

    #[allow(dead_code)]
    /// Look up a tool manifest by ID. Checks custom first, then builtins.
    pub fn get(&self, tool_id: &str) -> Option<&ToolManifest> {
        self.custom.get(tool_id).or_else(|| self.builtins.get(tool_id))
    }

    #[allow(dead_code)]
    /// Check if a tool is known (custom or builtin).
    pub fn contains(&self, tool_id: &str) -> bool {
        self.custom.contains_key(tool_id) || self.builtins.contains_key(tool_id)
    }

    #[allow(dead_code)]
    /// Register a new custom tool. Returns true if the tool was added (not already present).
    pub fn register(&mut self, manifest: ToolManifest) -> bool {
        let id = &manifest.tool_id;
        // Cannot override builtins
        if self.builtins.contains_key(id) {
            return false;
        }
        // Allow replacing custom tools with same ID
        let existed = self.custom.contains_key(id);
        self.custom.insert(id.clone(), manifest);
        !existed
    }

    #[allow(dead_code)]
    /// Unregister a custom tool. Returns true if the tool was removed.
    pub fn unregister(&mut self, tool_id: &str) -> bool {
        self.custom.remove(tool_id).is_some()
    }

    #[allow(dead_code)]
    /// Get all known tool IDs (custom first, then builtins).
    pub fn tool_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        // Custom tools first
        for id in self.custom.keys() {
            ids.push(id.clone());
        }
        // Then builtins
        for id in self.builtins.keys() {
            if !ids.contains(&id.to_string()) {
                ids.push(id.clone());
            }
        }
        ids
    }

    #[allow(dead_code)]
    /// Get all manifests (custom + builtin).
    pub fn all_manifests(&self) -> Vec<ToolManifest> {
        let mut manifests = Vec::new();
        for m in self.custom.values() {
            manifests.push(m.clone());
        }
        for m in self.builtins.values() {
            if !manifests.iter().any(|m2| m2.tool_id == m.tool_id) {
                manifests.push(m.clone());
            }
        }
        manifests
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a fresh registry with only built-ins (used during load merging).
#[allow(dead_code)]
fn builtin_registry() -> ToolRegistry {
    let mut registry = ToolRegistry {
        builtins: HashMap::new(),
        custom: HashMap::new(),
    };
    let headroom = builtin_headroom_manifest();
    registry.builtins.insert(headroom.tool_id.clone(), headroom);
    registry
}

// ---------------------------------------------------------------------------
// Convenience helpers (backward-compatible with manifest_headroom module)
// ---------------------------------------------------------------------------

/// Get the default tool ID (headroom-ai).
#[allow(dead_code)]
pub fn default_tool_id() -> &'static str {
    use crate::manifest_headroom;
    manifest_headroom::TOOL_ID
}

/// Get the default port range start.
#[allow(dead_code)]
pub fn default_port_range_start() -> u16 {
    use crate::manifest_headroom;
    manifest_headroom::PORT_RANGE_START
}

/// Get the default max restart attempts (from built-in headroom).
#[allow(dead_code)]
pub fn default_max_restart_attempts() -> u32 {
    use crate::manifest_headroom;
    manifest_headroom::MAX_RESTART_ATTEMPTS
}

/// Get the default restart backoff array.
#[allow(dead_code)]
pub fn default_restart_backoff_secs() -> &'static [u64] {
    use crate::manifest_headroom;
    manifest_headroom::RESTART_BACKOFF_SECS
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_headroom_manifest_has_required_fields() {
        let m = builtin_headroom_manifest();
        assert!(!m.tool_id.is_empty());
        assert!(!m.display_name.is_empty());
        assert!(!m.python_standalone_url.is_empty());
        assert!(!m.wheel_url.is_empty());
        assert!(m.port_range_start < m.port_range_end);
        assert!(m.max_restart_attempts > 0);
    }

    #[test]
    fn test_registry_creates_with_headroom_builtin() {
        let reg = ToolRegistry::new();
        assert!(reg.contains("headroom-ai"));
        assert_eq!(reg.tool_ids().len(), 1);

        let manifest = reg.get("headroom-ai").unwrap();
        assert_eq!(manifest.tool_id, "headroom-ai");
        assert_eq!(manifest.display_name, "Headroom AI");
    }

    #[test]
    fn test_registry_register_custom_tool() {
        let mut reg = ToolRegistry::new();
        let custom = ToolManifest {
            tool_id: "my-tool".to_string(),
            display_name: "My Custom Tool".to_string(),
            python_standalone_url: "https://example.com/python.tar.gz".to_string(),
            python_standalone_sha256: "abc123".to_string(),
            wheel_url: "https://example.com/my-tool.whl".to_string(),
            wheel_sha256: "def456".to_string(),
            port_range_start: 9000,
            port_range_end: 9099,
            health_check_path: "/ping".to_string(),
            health_check_timeout_secs: 3,
            max_restart_attempts: 3,
            restart_backoff_secs: vec![1, 2, 5],
            config_patches: vec![],
            launch_command: vec!["python".to_string(), "-m".to_string(), "my_tool".to_string()],
        };

        assert!(reg.register(custom));
        assert_eq!(reg.tool_ids().len(), 2);
        assert!(reg.contains("my-tool"));

        // Registering again should return false (already exists)
        let custom2 = ToolManifest {
            tool_id: "my-tool".to_string(),
            display_name: "My Tool v2".to_string(),
            ..ToolManifest::default()
        };
        assert!(!reg.register(custom2)); // returns false, but still updates

        // Cannot override builtins
        let headroom_clone = builtin_headroom_manifest();
        assert!(!reg.register(headroom_clone));
    }

    #[test]
    fn test_registry_unregister_custom_tool() {
        let mut reg = ToolRegistry::new();
        let custom = ToolManifest {
            tool_id: "removable".to_string(),
            display_name: "Removable".to_string(),
            python_standalone_url: String::new(),
            python_standalone_sha256: String::new(),
            wheel_url: String::new(),
            wheel_sha256: String::new(),
            port_range_start: 10000,
            port_range_end: 10099,
            health_check_path: "/health".to_string(),
            health_check_timeout_secs: 5,
            max_restart_attempts: 5,
            restart_backoff_secs: vec![1, 2, 5, 10, 30],
            config_patches: vec![],
            launch_command: vec!["python".to_string()],
        };
        reg.register(custom);

        assert!(reg.unregister("removable"));
        assert!(!reg.contains("removable"));

        // Cannot unregister builtins
        assert!(!reg.unregister("headroom-ai"));
    }

    #[test]
    fn test_registry_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tools.json");

        let mut reg = ToolRegistry::new();
        let custom = ToolManifest {
            tool_id: "saved-tool".to_string(),
            display_name: "Saved Tool".to_string(),
            python_standalone_url: "https://example.com/python.tar.gz".to_string(),
            python_standalone_sha256: "hash1".to_string(),
            wheel_url: "https://example.com/saved.whl".to_string(),
            wheel_sha256: "hash2".to_string(),
            port_range_start: 11000,
            port_range_end: 11099,
            health_check_path: "/health".to_string(),
            health_check_timeout_secs: 5,
            max_restart_attempts: 5,
            restart_backoff_secs: vec![1, 2, 5, 10, 30],
            config_patches: vec![],
            launch_command: vec!["python".to_string()],
        };
        reg.register(custom);

        reg.save(&path).unwrap();

        let loaded = ToolRegistry::load(&path).unwrap();
        assert!(loaded.contains("headroom-ai"));
        assert!(loaded.contains("saved-tool"));

        let saved_tool = loaded.get("saved-tool").unwrap();
        assert_eq!(saved_tool.port_range_start, 11000);
    }

    #[test]
    fn test_registry_load_creates_builtins_if_missing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nonexistent.json");

        let reg = ToolRegistry::load(&path).unwrap_or_default();
        // Should have built-in headroom even though file doesn't exist
        assert!(reg.contains("headroom-ai"));
    }

    #[test]
    fn test_registry_all_manifests_returns_both() {
        let mut reg = ToolRegistry::new();
        let custom = ToolManifest {
            tool_id: "tool-a".to_string(),
            display_name: "Tool A".to_string(),
            python_standalone_url: String::new(),
            python_standalone_sha256: String::new(),
            wheel_url: String::new(),
            wheel_sha256: String::new(),
            port_range_start: 12000,
            port_range_end: 12099,
            health_check_path: "/health".to_string(),
            health_check_timeout_secs: 5,
            max_restart_attempts: 5,
            restart_backoff_secs: vec![1, 2, 5, 10, 30],
            config_patches: vec![],
            launch_command: vec!["python".to_string()],
        };
        reg.register(custom);

        let manifests = reg.all_manifests();
        assert_eq!(manifests.len(), 2); // headroom + tool-a
    }

    #[test]
    fn test_toml_markers_include_tool_id() {
        let start = toml_block_start_marker("my-tool");
        let end = toml_block_end_marker("my-tool");
        assert!(start.contains("my-tool"));
        assert!(end.contains("my-tool"));
        assert!(start.starts_with("# >>>"));
        assert!(end.starts_with("# <<<"));
    }

    #[test]
    fn test_config_patch_target_defaults() {
        let target = ConfigPatchTarget {
            rel_path: ".claude/settings.json".to_string(),
            strategy: "json-merge-key".to_string(),
            json_key: Some("key".to_string()),
            json_value: Some(serde_json::json!(true)),
            toml_block: None,
        };

        assert_eq!(target.rel_path, ".claude/settings.json");
        assert_eq!(target.strategy, "json-merge-key");
        assert_eq!(target.json_key.as_deref(), Some("key"));
    }

    #[test]
    fn test_registry_tool_ids_order_custom_first() {
        let mut reg = ToolRegistry::new();
        let tool_a = ToolManifest {
            tool_id: "custom-tool".to_string(),
            display_name: "Custom".to_string(),
            python_standalone_url: String::new(),
            python_standalone_sha256: String::new(),
            wheel_url: String::new(),
            wheel_sha256: String::new(),
            port_range_start: 13000,
            port_range_end: 13099,
            health_check_path: "/health".to_string(),
            health_check_timeout_secs: 5,
            max_restart_attempts: 5,
            restart_backoff_secs: vec![1, 2, 5, 10, 30],
            config_patches: vec![],
            launch_command: vec!["python".to_string()],
        };
        reg.register(tool_a);

        let ids = reg.tool_ids();
        // Custom should come first
        assert_eq!(ids[0], "custom-tool");
        // Builtin second
        assert_eq!(ids[1], "headroom-ai");
    }

    #[test]
    fn test_builtin_manifest_restart_backoff_matches_max_attempts() {
        let m = builtin_headroom_manifest();
        assert_eq!(m.restart_backoff_secs.len(), m.max_restart_attempts as usize);
    }

    #[test]
    fn test_tool_manifest_default() {
        let m = ToolManifest::default();
        assert!(m.tool_id.is_empty());
        assert_eq!(m.health_check_path, "/health");
        assert_eq!(m.max_restart_attempts, 5);
        assert_eq!(m.restart_backoff_secs, vec![1, 2, 5, 10, 30]);
    }

    #[test]
    fn test_config_patch_target_default() {
        let t = ConfigPatchTarget::default();
        assert!(t.rel_path.is_empty());
        assert!(t.strategy.is_empty());
        assert!(t.json_key.is_none());
        assert!(t.json_value.is_none());
        assert!(t.toml_block.is_none());
    }
}