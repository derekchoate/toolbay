# Session Log — Toolbay Project

## Current State

**Branch:** `phase2-tool-registry-integration`  
**Status:** BUILD CLEAN ✅ | 60 tests passing | 1 harmless warning  
**Last Updated:** 2026-07-05 (Third Session — Auto-restart on Health Check Failure)

### Quick Start for New Sessions
```bash
# Check compilation
cd src-tauri && cargo check

# Run tests
cd src-tauri && cargo test

# Run app in dev mode
npm run tauri dev
```

---

## Phase 1: Foundation ✅ COMPLETE

`cargo check` completed with zero warnings, zero errors. 58 tests passed.

### What Was Built

| File | Purpose |
|------|---------|
| `paths.rs` | Path resolution for toolbay directories (runtime, logs, state, ports file) |
| `manifest_headroom.rs` | Hardcoded manifest: download URLs, SHA-256 hashes, port range, restart backoff |
| `manifest.rs` | Generic ToolManifest struct + ToolRegistry for multi-tool support |
| `runtime_install.rs` | Download + verify Python standalone + headroom wheel, install into runtime dir |
| `supervisor.rs` (original) | Process lifecycle: spawn, health-check loop, crash recovery, status states |
| `ledger.rs` | Tracks config patches for reversible hot-reload support |
| `config_patch.rs` | TOML/JSON patching with atomic write + backup/restore |
| `commands.rs` (original) | 8 Tauri command handlers for headroom-ai only |
| `lib.rs` (original) | AppState struct, tauri::Builder setup |

### Key Decisions & Gotchas (Phase 1)

1. **Duplicate Macro Errors** — Fixed by moving all `#[tauri::command]` functions into `commands.rs`
2. **String Indexing** — Fixed with `.chars().nth()` for UTF-8 compatibility
3. **TOML Block Reversal** — Fixed to strip only managed content, preserving file
4. **Restart Method Signature** — Uses scoped mutable locks in Tauri commands
5. **Multi-Tool Manifest Pattern** — Created `ToolManifest`, `ConfigPatchTarget`, `ToolRegistry`

---

## Phase 2: Multi-Tool Generic System ✅ CORE ITEMS COMPLETE

### What Was Completed (Third Session — Auto-restart on Health Check Failure)

#### 1. Implemented auto-restart when health check fails
- **Problem:** Previously, when the background health-check task detected a failed health probe, it only broke out of its loop and logged a message. The process was not killed, and no restart was triggered.
- **Solution:** Rewrote `Supervisor::start()` to use an outer restart loop that:
  1. Spawns the initial process
  2. Races between natural process exit and health-monitor signals using `tokio::select!`
  3. On health failure: kills the unhealthy process, applies backoff delay, re-spawns
  4. On crash (non-zero exit): same backoff + restart cycle
  5. Exhausts up to `max_restart_attempts` before marking as Crashed

#### 2. New `wait_for_crash_or_health_failure()` method
- Uses `tokio::select!` to race between:
  - **Process exit:** Returns `Ok(false)` — caller checks `exit_status.success()`
  - **Health monitor signal:** Kills the process, waits for it, returns `Ok(true)`
- Health monitor is spawned as a background task that polls at configurable intervals

#### 3. Fixed hardcoded manifest reference in health monitor
- **Problem:** The old health monitor used `builtin_headroom_manifest().health_check_path.clone()` and a hardcoded `/health` path, ignoring the tool's actual config.
- **Solution:** Health monitor now receives `ToolConfig` values (tool_id, health_check_path, timeout) directly from the supervisor's own config — fully generic per-tool.

#### 4. Removed unused `handle_crash()` method
- The old `handle_crash()` used a hardcoded headroom manifest and was incomplete
- Restart logic is now inline in `start()`, using the actual manifest passed to it

---

### What Was Completed (Second Session)

#### 1. Wire AppState to use ToolRegistry (#5 from Phase 2 list)
- Added `tools_file()` path helper for registry persistence in `paths.rs`
- Wired `ToolRegistry` into `AppState` with `RwLock` for concurrent read/write access
- Registry loads from disk at startup (falls back to built-in defaults)

#### 2. Dynamic Tool Discovery (#1 from Phase 2 list)
- **`list_tools()`** Tauri command — enumerates all registered tools with status
- **`register_tool()`** Tauri command — accepts `RegisterToolRequest` struct
- Custom tools persisted to JSON file alongside patches.json

#### 3. Supervisor Generic over ToolConfig
- Created `ToolConfig` struct (with `Clone`) for per-tool configuration
- `Supervisor::new()` takes a `ToolConfig` instead of hardcoded constants
- `ToolConfig::from_manifest()` creates config from any registered manifest

#### 4. Health Check Integration (#2 from Phase 2 list)
- `health_check()` function polls HTTP endpoint with configurable timeout
- Background health-check task spawned via `tokio::spawn` in `Supervisor::start()`

#### 5. Incremental Log Reading (#3 from Phase 2 list)
- Added `LogOffsets` struct for tracking last-read offset per tool
- `tail_log()` command accepts optional `reset` parameter
- Uses `BufReader::split(b'\n')` to skip already-read bytes

### Current File States (Post-Phase 2)

| File | Purpose |
|------|---------|
| `paths.rs` | Added `tools_file(home)` for registry JSON persistence |
| `manifest.rs` | ToolManifest, ConfigPatchTarget, ToolRegistry structs + tests |
| `lib.rs` | AppState with registry + log_offsets; LogOffsets struct |
| `commands.rs` | 10 commands total: get_status, install_tool, start_tool, stop_tool, restart_tool, uninstall_tool, tail_log, open_logs_dir, list_tools, register_tool |
| `supervisor.rs` | Generic ToolConfig, health_check(), build_command from manifest, spawn, prepare_log_files, PortsState |

### Build Status
- **60 tests passing** (2 new: `test_tool_config_from_manifest`, `test_tool_config_default_headroom`)
- 1 harmless warning: `install_headroom_ai` unused function in `runtime_install.rs`

---

## Architecture Overview (Post-Phase 2)

```
┌─────────────────────────────────────────────┐
│              Tauri Frontend                 │
│         (main.ts + styles.css)              │
│  ┌───────────┐  IPC calls via invoke()     │
│  │ Vanilla JS │ ─────────────────────►      │
│  └───────────┘                             │
├─────────────────────────────────────────────┤
│              Tauri Backend (Rust)           │
│                                             │
│  commands.rs  ← 10 Tauri command handlers   │
│       │                                     │
│       ▼                                     │
│  lib.rs → AppState + tauri::Builder         │
│       │                                     │
│       ├─► paths.rs         (path helpers)   │
│       ├─► manifest.rs      ← tool registry  │
│       ├─► manifest_headroom.rs (Phase 1)    │
│       ├─► runtime_install.rs (download+install) │
│       ├─► supervisor.rs    (generic mgmt)   │
│       ├─► ledger.rs          (patch tracking)│
│       └─► config_patch.rs  (TOML editing)   │
├─────────────────────────────────────────────┤
│              External Tools                 │
│  - Python standalone (downloaded)          │
│  - headroom-ai wheel (downloaded)          │
│  - Custom tools via register_tool()        │
│  - macOS `open` command (for logs dir)     │
└─────────────────────────────────────────────┘

AppState structure:
  - home: Option<PathBuf>
  - supervisor: Arc<Mutex<Option<Supervisor>>>
  - registry: Arc<RwLock<ToolRegistry>>
  - log_offsets: Arc<Mutex<LogOffsets>>
```

---

## Key Implementation Details (Post-Phase 2)

### Error Handling Pattern
All Tauri commands return `Result<T, String>`. Error conversions use `.map_err()` for non-String error types:
- `InstallError` → `.map_err(|e| format!("...: {}", e))`
- `PatchError` → `.map_err(|e| format!("...: {}", e))`

### Async + RwLock Pattern
RwLock guards cannot be held across `.await` points (not `Send`). Pattern used:
```rust
let data = {
    let guard = state.registry.read().unwrap();
    // Collect all needed data while holding lock
    vec![(tid, m.clone()) for ...]
}; // guard dropped here
// Now safe to await
let sup = state.supervisor.lock().await;
```

### Incremental Log Reading Pattern
1. Get file size via `std::fs::metadata()`
2. Check stored offset against current file size
3. If offset >= file_size: file was truncated/rotated, read from beginning
4. Otherwise: use `BufReader::split(b'\n')` to skip to offset and read new bytes only

---

## Remaining Work for Future Sessions

### Phase 2 (Remaining Items)
1. ~~**Auto-restart on health check failure**~~ ✅ **COMPLETE** — Implemented in third session. `Supervisor::start()` now uses an outer restart loop with `tokio::select!` to race between process exit and health-monitor signals. On health failure, the unhealthy process is killed, backoff delay applied, and re-spawn attempted.
2. **Real-time log streaming** — Incremental polling is done, but WebSocket/SSE for push-based updates not implemented

### Phase 3: Polish & Distribution
1. **Code signing** for macOS notarization
2. **Auto-updater** integration (Tauri has built-in updater plugin)
3. **Multi-tool UI** — Frontend should list all installed tools with individual controls
4. **Settings panel** — Configure port ranges, restart policies, log levels

### Documentation Needed
- User-facing README with installation instructions
- Architecture deep-dive document
- API reference for Tauri commands (what each returns on success/failure)

---

## Test Results Summary

| Metric | Count |
|--------|-------|
| Total tests | **60** (+2 from Phase 2 session) |
| Passed | 60 |
| Failed | 0 |
| Warnings | 1 (harmless: `install_headroom_ai` unused) |

Test modules:
- `paths` — 12 tests (path resolution, timestamp format)
- `manifest` — 13 tests (registry CRUD, save/load, TOML markers, defaults)
- `manifest_headroom` — 7 tests (constants, markers, port range validation)
- `supervisor` — 10 tests (+2 new: ToolConfig tests; ports state, health check URL, build command)
- `runtime_install` — 4 tests (command building, disk usage reporting)
- `ledger` — 5 tests (save/load/record/reverse for tool-specific entries)
- `config_patch` — 9 tests (backup, JSON merge, TOML block insert, idempotency, multi-tool independence)

---

## Dependencies Summary

| Crate | Version | Purpose |
|-------|---------|---------|
| tauri | 2.x | Framework |
| tauri-plugin-opener | 2.x | Open logs in Finder |
| tokio | 1.x | Async runtime, process management |
| reqwest | 0.12 | HTTP downloads + health checks |
| sha2 | 0.10 | SHA-256 verification |
| thiserror | 2.x | Error derives |
| futures | 0.3 | Stream extensions (wheel download) |
| zip | 8.x | Wheel unpacking (wheels are ZIPs) |
| dirs | 5.x | Home directory resolution |
| serde / serde_json | 1.x | JSON serialization (registry, ports, patches) |

---

## Git History (Current Branch)

```
phase2-tool-registry-integration
├── 49f2e5f phase2: wire ToolRegistry into AppState, add multi-tool commands
├── ee9c472 phase2: supervisor generic over ToolConfig, health checks...
└── 57b1a5c docs: update SESSION_LOG with Phase 2 completion summary
```

To see all commits: `git log --oneline`