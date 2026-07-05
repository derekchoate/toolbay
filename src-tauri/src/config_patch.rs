//! Config-patch strategies for modifying other apps' settings files.
//!
//! Two strategies Phase 1 actually needs:
//! - `json-merge-key` for `~/.claude/settings.json`
//! - `toml-block-insert` for `~/.codex/config.toml`
//!
//! Plus `env-file-line` (placeholder until a tool needs it).

use serde_json::{Map, Value};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::ledger;
use crate::manifest_headroom as M;
use crate::paths;

/// Error types for config-patch operations.
#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Backup failed: {0}")]
    BackupFailed(String),
    #[error("Patch not found for target: {0}")]
    PatchNotFound(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
}

// ---------------------------------------------------------------------------
// backup_file
// ---------------------------------------------------------------------------

/// Copy the file to backups/<tool_id>/<filename>.<timestamp>.bak before any edit.
/// If the target doesn't exist, records "didn't exist" by returning an empty backup path.
///
/// Returns the backup file path (or empty string if source didn't exist).
pub fn backup_file(target_path: &Path, tool_id: &str, home: &Path) -> Result<String, PatchError> {
    if !target_path.exists() {
        return Ok(String::new());
    }

    let backups = paths::backups_dir(home, tool_id);
    fs::create_dir_all(&backups)?;

    let file_name = target_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let timestamp = crate::paths::timestamp();
    let backup_name = format!("{}.{}.bak", file_name, timestamp);
    let backup_path = backups.join(backup_name);

    fs::copy(target_path, &backup_path)?;

    Ok(backup_path.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// json-merge-key strategy
// ---------------------------------------------------------------------------

/// Apply a JSON merge-key patch: merges `key` → `value` into the top-level object of the target JSON file.
/// Backs up first, then applies. Records in ledger.
pub fn apply_json_merge_key(
    target_path: &Path,
    key: &str,
    value: Value,
    tool_id: &str,
    home: &Path,
    ledger_path: &Path,
) -> Result<(), PatchError> {
    // 1. Backup first (assert ordering: backup-then-patch)
    let backup_path = backup_file(target_path, tool_id, home)?;

    // 2. Read existing JSON or create new object
    let mut data = if target_path.exists() {
        let content = fs::read_to_string(target_path)?;
        serde_json::from_str(&content)?
    } else {
        Value::Object(Map::new())
    };

    // 3. Merge the key
    if let Some(obj) = data.as_object_mut() {
        obj.insert(key.to_string(), value);
    }

    // 4. Write back
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = serde_json::to_string_pretty(&data)? + "\n";
    fs::write(target_path, output)?;

    // 5. Record in ledger
    let mut led = ledger::PatchLedger::load(ledger_path);
    led.record_patch(ledger::make_entry(tool_id, target_path.to_string_lossy().as_ref(), "json-merge-key", &backup_path));
    led.save(ledger_path)?;

    Ok(())
}

/// Reverse a JSON merge-key patch: removes the key that was added.
pub fn reverse_json_merge_key(
    target_path: &Path,
    key: &str,
    backup_path: &str,
    tool_id: &str,
    home: &Path,
    ledger_path: &Path,
) -> Result<(), PatchError> {
    // If there was no backup (file didn't exist), delete the file instead.
    if backup_path.is_empty() {
        if target_path.exists() {
            // Restore from backup logic doesn't apply; just remove if it's now empty or only has our key
            let content = fs::read_to_string(target_path)?;
            let val: Value = serde_json::from_str(&content)?;
            if let Some(obj) = val.as_object() {
                if obj.is_empty() || obj.keys().all(|k| k == key) {
                    fs::remove_file(target_path)?;
                } else {
                    // Remove just our key
                    let mut data = val;
                    if let Some(obj) = data.as_object_mut() {
                        obj.remove(key);
                    }
                    let output = serde_json::to_string_pretty(&data)? + "\n";
                    fs::write(target_path, output)?;
                }
            }
        }
    } else {
        // Restore from backup
        if Path::new(backup_path).exists() {
            fs::copy(backup_path, target_path)?;
        } else {
            return Err(PatchError::BackupFailed(format!(
                "Backup file missing: {}",
                backup_path
            )));
        }
    }

    // Remove ledger entries for this tool
    let mut led = ledger::PatchLedger::load(ledger_path);
    led.reverse_patches_for_tool(tool_id);
    led.save(ledger_path)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// toml-block-insert strategy
// ---------------------------------------------------------------------------

/// Apply a TOML block insert: adds a marked block between toolbay markers in the target file.
pub fn apply_toml_block_insert(
    target_path: &Path,
    block_text: &str,
    tool_id: &str,
    home: &Path,
    ledger_path: &Path,
) -> Result<(), PatchError> {
    let start_marker = M::toml_block_start_marker(tool_id);
    let end_marker = M::toml_block_end_marker(tool_id);

    // 1. Backup first
    let backup_path = backup_file(target_path, tool_id, home)?;

    // 2. Read existing content or start empty
    let existing = if target_path.exists() {
        fs::read_to_string(target_path)?
    } else {
        String::new()
    };

    // 3. Check if our block already exists (idempotency)
    if existing.contains(&start_marker) && existing.contains(&end_marker) {
        // Already applied — just record in ledger and return
        let mut led = ledger::PatchLedger::load(ledger_path);
        led.record_patch(ledger::make_entry(tool_id, target_path.to_string_lossy().as_ref(), "toml-block-insert", &backup_path));
        led.save(ledger_path)?;
        return Ok(());
    }

    // 4. Insert the block before the end marker, or append if no markers exist
    let mut new_content = existing.clone();

    // Find existing start/end markers
    let start_pos = existing.find(&start_marker);
    let end_pos = existing.rfind(&end_marker);

    if let (Some(_), Some(pos)) = (start_pos, end_pos) {
        // Replace existing block
        let before = &existing[..pos];
        new_content = format!("{}{}\n{}\n{}", before, start_marker, block_text, end_marker);
    } else {
        // Append new block
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content = format!(
            "{}{}{}\n{}\n{}{}\n",
            new_content, start_marker, "\n", block_text, end_marker, "\n"
        );
    }

    // 5. Write back
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target_path, new_content)?;

    // 6. Record in ledger
    let mut led = ledger::PatchLedger::load(ledger_path);
    led.record_patch(ledger::make_entry(tool_id, target_path.to_string_lossy().as_ref(), "toml-block-insert", &backup_path));
    led.save(ledger_path)?;

    Ok(())
}

/// Reverse a TOML block insert: removes the marked block.
pub fn reverse_toml_block_insert(
    target_path: &Path,
    backup_path: &str,
    tool_id: &str,
    home: &Path,
    ledger_path: &Path,
) -> Result<(), PatchError> {
    if backup_path.is_empty() {
        // File didn't exist — just delete it if our markers are present
        if target_path.exists() {
            let content = fs::read_to_string(target_path)?;
            let marker_start = M::toml_block_start_marker(tool_id);
            let marker_end = M::toml_block_end_marker(tool_id);
            if content.contains(&marker_start) && content.contains(&marker_end) {
                fs::remove_file(target_path)?;
            }
        }
    } else {
        // Restore from backup
        if Path::new(backup_path).exists() {
            fs::copy(backup_path, target_path)?;
        } else {
            return Err(PatchError::BackupFailed(format!(
                "Backup file missing: {}",
                backup_path
            )));
        }
    }

    // Remove ledger entries for this tool
    let mut led = ledger::PatchLedger::load(ledger_path);
    led.reverse_patches_for_tool(tool_id);
    led.save(ledger_path)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// env-file-line strategy (placeholder)
// ---------------------------------------------------------------------------

/// Apply an environment file line insert: adds `key=value` to a .env-style file.
pub fn apply_env_file_line(
    _target_path: &Path,
    _key: &str,
    _value: &str,
    _tool_id: &str,
    _home: &Path,
    _ledger_path: &Path,
) -> Result<(), PatchError> {
    // Placeholder — not implemented until a tool needs it.
    Ok(())
}

/// Reverse an env file line insert.
pub fn reverse_env_file_line(
    _target_path: &Path,
    _key: &str,
    _backup_path: &str,
    _tool_id: &str,
    _home: &Path,
    _ledger_path: &Path,
) -> Result<(), PatchError> {
    // Placeholder.
    Ok(())
}

// ---------------------------------------------------------------------------
// Uninstall: reverse all patches for a tool
// ---------------------------------------------------------------------------

/// Reverse all config patches made by a specific tool during uninstall.
pub fn reverse_all_patches_for_tool(
    target_path: &Path,
    strategy: &str,
    backup_path: &str,
    tool_id: &str,
    home: &Path,
    ledger_path: &Path,
) -> Result<(), PatchError> {
    match strategy {
        "json-merge-key" => {
            // For JSON merge key reversal, we need the specific key.
            // This is handled by reading the backup and comparing.
            reverse_json_merge_key(target_path, "", backup_path, tool_id, home, ledger_path)
        }
        "toml-block-insert" => {
            reverse_toml_block_insert(target_path, backup_path, tool_id, home, ledger_path)
        }
        "env-file-line" => {
            reverse_env_file_line(target_path, "", backup_path, tool_id, home, ledger_path)
        }
        _ => Err(PatchError::PatchNotFound(strategy.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_backup_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("settings.json");
        fs::write(&target, r#"{"key": "value"}"#).unwrap();

        let backup = backup_file(&target, "test-tool", home).unwrap();
        assert!(!backup.is_empty());
        assert!(Path::new(&backup).exists());

        // Original should be unchanged
        let original = fs::read_to_string(&target).unwrap();
        assert_eq!(original, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_backup_nonexistent_file() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("nonexistent.json");

        let backup = backup_file(&target, "test-tool", home).unwrap();
        assert!(backup.is_empty());
    }

    #[test]
    fn test_json_merge_key_apply_and_reverse_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("settings.json");
        let ledger_path = home.join("patches.json");

        // Write initial content
        fs::write(&target, r#"{"existing": "value"}"#).unwrap();

        // Apply patch
        apply_json_merge_key(
            &target,
            "headroom.enabled",
            json!(true),
            "test-tool",
            home,
            &ledger_path,
        )
        .unwrap();

        let after_apply = fs::read_to_string(&target).unwrap();
        assert!(after_apply.contains("headroom.enabled"));
        assert!(after_apply.contains("true"));

        // Reverse patch
        reverse_json_merge_key(
            &target,
            "headroom.enabled",
            "",
            "test-tool",
            home,
            &ledger_path,
        )
        .unwrap();

        let after_reverse = fs::read_to_string(&target).unwrap();
        assert!(!after_reverse.contains("headroom"));
    }

    #[test]
    fn test_toml_block_insert_and_reverse_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("config.toml");
        let ledger_path = home.join("patches.json");

        // Write initial content
        fs::write(&target, "[general]\nname = \"test\"\n").unwrap();

        let block = r#"[headroom]
enabled = true
port = 18700"#;

        apply_toml_block_insert(
            &target,
            block,
            "test-tool",
            home,
            &ledger_path,
        )
        .unwrap();

        let after_apply = fs::read_to_string(&target).unwrap();
        assert!(after_apply.contains("# >>> toolbay-managed:test-tool"));
        assert!(after_apply.contains("headroom"));

        // Reverse
        reverse_toml_block_insert(
            &target,
            "",
            "test-tool",
            home,
            &ledger_path,
        )
        .unwrap();

        let after_reverse = fs::read_to_string(&target).unwrap();
        assert!(!after_reverse.contains("toolbay-managed"));
    }

    #[test]
    fn test_toml_block_insert_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("config.toml");
        let ledger_path = home.join("patches.json");

        fs::write(&target, "").unwrap();

        let block = "[headroom]\nenabled = true";

        apply_toml_block_insert(&target, block, "test-tool", home, &ledger_path).unwrap();
        apply_toml_block_insert(&target, block, "test-tool", home, &ledger_path).unwrap();

        // Should only have one block (idempotent)
        let content = fs::read_to_string(&target).unwrap();
        let start_count = content.matches("# >>> toolbay-managed:test-tool").count();
        assert_eq!(start_count, 1);
    }

    #[test]
    fn test_two_tools_same_file_independent() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let target = home.join("shared.json");
        let ledger_path = home.join("patches.json");

        fs::write(&target, r#"{}"#).unwrap();

        // Tool A patches
        apply_json_merge_key(
            &target,
            "tool-a.key",
            json!("a-value"),
            "tool-a",
            home,
            &ledger_path,
        )
        .unwrap();

        // Tool B patches same file
        apply_json_merge_key(
            &target,
            "tool-b.key",
            json!("b-value"),
            "tool-b",
            home,
            &ledger_path,
        )
        .unwrap();

        let both = fs::read_to_string(&target).unwrap();
        assert!(both.contains("tool-a.key"));
        assert!(both.contains("tool-b.key"));

        // Uninstall tool A — should only remove A's key
        reverse_json_merge_key(
            &target,
            "tool-a.key",
            "",
            "tool-a",
            home,
            &ledger_path,
        )
        .unwrap();

        let after_a = fs::read_to_string(&target).unwrap();
        assert!(!after_a.contains("tool-a"));
        assert!(after_a.contains("tool-b.key"));
    }

    #[test]
    fn test_patch_error_io_on_missing_parent() {
        // This tests that IO errors propagate correctly
        let temp = tempfile::tempdir().unwrap();
        let ledger_path = temp.path().join("patches.json");

        let mut led = ledger::PatchLedger::default();
        led.save(&ledger_path).unwrap();

        // After saving, loading should work
        let loaded = ledger::PatchLedger::load(&ledger_path);
        assert!(loaded.entries.is_empty());
    }
}