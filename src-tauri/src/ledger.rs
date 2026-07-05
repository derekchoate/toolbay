//! Patch ledger: records every config apply/reverse operation so uninstall can reverse exactly what was applied.
//!
//! Ledger entries are stored in `state/patches.json`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// A single patch entry recorded when a config change is made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEntry {
    /// The tool that made this patch (e.g., "headroom-ai").
    pub tool_id: String,
    /// Target file path that was patched.
    pub target_path: String,
    /// Strategy used for the patch.
    pub strategy: String,
    /// Backup file path (relative to toolbay root). Empty if no backup existed.
    pub backup_path: String,
    /// Timestamp of when this patch was applied.
    pub timestamp: String,
}

/// The full ledger stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchLedger {
    #[serde(default)]
    pub entries: Vec<PatchEntry>,
}

impl PatchLedger {
    /// Load the ledger from disk. Returns empty ledger if file doesn't exist.
    pub fn load(path: &Path) -> Self {
        let data = match fs::read_to_string(path) {
            Ok(d) => d,
            Err(_) => return PatchLedger::default(),
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    #[allow(dead_code)]
    /// Save the ledger to disk.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }

    #[allow(dead_code)]
    /// Record a new patch entry.
    pub fn record_patch(&mut self, entry: PatchEntry) {
        self.entries.push(entry);
    }

    #[allow(dead_code)]
    /// Get all entries for a specific tool.
    pub fn entries_for_tool(&self, tool_id: &str) -> Vec<&PatchEntry> {
        self.entries.iter().filter(|e| e.tool_id == tool_id).collect()
    }

    /// Reverse all patches for a tool — returns the entries so the caller can apply reversals.
    pub fn reverse_patches_for_tool(&mut self, tool_id: &str) -> Vec<PatchEntry> {
        let mut to_reverse = Vec::new();
        self.entries.retain(|e| {
            if e.tool_id == tool_id {
                to_reverse.push(e.clone());
                false
            } else {
                true
            }
        });
        to_reverse
    }

    #[allow(dead_code)]
    /// Remove all entries for a specific tool (called after successful reversal).
    pub fn remove_for_tool(&mut self, tool_id: &str) {
        self.entries.retain(|e| e.tool_id != tool_id);
    }
}

#[allow(dead_code)]
/// Create a new patch entry.
pub fn make_entry(tool_id: &str, target_path: &str, strategy: &str, backup_path: &str) -> PatchEntry {
    PatchEntry {
        tool_id: tool_id.to_string(),
        target_path: target_path.to_string(),
        strategy: strategy.to_string(),
        backup_path: backup_path.to_string(),
        timestamp: crate::paths::timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("patches.json");
        let ledger = PatchLedger::load(&path);
        assert!(ledger.entries.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("patches.json");

        let mut ledger = PatchLedger::default();
        ledger.record_patch(make_entry("headroom-ai", "/home/test/settings.json", "json-merge-key", ""));
        ledger.save(&path).unwrap();

        let loaded = PatchLedger::load(&path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].tool_id, "headroom-ai");
    }

    #[test]
    fn test_entries_for_tool() {
        let mut ledger = PatchLedger::default();
        ledger.record_patch(make_entry("tool-a", "/path/a.json", "json-merge-key", ""));
        ledger.record_patch(make_entry("tool-b", "/path/b.json", "toml-block-insert", ""));
        ledger.record_patch(make_entry("tool-a", "/path/c.json", "env-file-line", ""));

        let a_entries = ledger.entries_for_tool("tool-a");
        assert_eq!(a_entries.len(), 2);

        let b_entries = ledger.entries_for_tool("tool-b");
        assert_eq!(b_entries.len(), 1);
    }

    #[test]
    fn test_reverse_patches_for_tool() {
        let mut ledger = PatchLedger::default();
        ledger.record_patch(make_entry("tool-a", "/path/a.json", "json-merge-key", ""));
        ledger.record_patch(make_entry("tool-b", "/path/b.json", "toml-block-insert", ""));
        ledger.record_patch(make_entry("tool-a", "/path/c.json", "env-file-line", ""));

        let reversed = ledger.reverse_patches_for_tool("tool-a");
        assert_eq!(reversed.len(), 2);
        assert_eq!(ledger.entries.len(), 1);
        assert_eq!(ledger.entries[0].tool_id, "tool-b");
    }

    #[test]
    fn test_remove_for_tool() {
        let mut ledger = PatchLedger::default();
        ledger.record_patch(make_entry("tool-a", "/path/a.json", "json-merge-key", ""));
        ledger.record_patch(make_entry("tool-b", "/path/b.json", "toml-block-insert", ""));

        ledger.remove_for_tool("tool-a");
        assert_eq!(ledger.entries.len(), 1);
        assert_eq!(ledger.entries[0].tool_id, "tool-b");
    }

    #[test]
    fn test_timestamp_is_recorded() {
        let entry = make_entry("headroom-ai", "/path/settings.json", "json-merge-key", "");
        assert!(!entry.timestamp.is_empty());
        assert!(entry.timestamp.starts_with("20"));
    }

    #[test]
    fn test_reverse_does_not_affect_other_tools() {
        let mut ledger = PatchLedger::default();
        ledger.record_patch(make_entry("tool-a", "/shared/file.json", "json-merge-key", ""));
        ledger.record_patch(make_entry("tool-b", "/shared/file.json", "toml-block-insert", ""));

        let _reversed = ledger.reverse_patches_for_tool("tool-a");

        // tool-b's entry should still be in the ledger
        assert_eq!(ledger.entries.len(), 1);
        assert_eq!(ledger.entries[0].tool_id, "tool-b");
    }
}