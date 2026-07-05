# Session Log — 2026-07-05

## Status: BUILD PASSING ✅

`cargo check` completes successfully with only warnings (no errors).

---

## What Was Built

### Rust Modules (src-tauri/src/)

| File | Purpose |
|------|---------|
| `paths.rs` | Path resolution for toolbay directories (runtime, logs, state, ports file) |
| `manifest_headroom.rs` | Hardcoded manifest: download URLs, SHA-256 hashes, port range, restart backoff |
| `runtime_install.rs` | Download + verify Python standalone + headroom wheel, install into runtime dir |
| `supervisor.rs` | Process lifecycle: spawn, health-check loop, crash recovery, status states |
| `ledger.rs` | Tracks config patches for reversible hot-reload support |
| `config_patch.rs` | TOML patching with atomic write + backup/restore |
| `commands.rs` | All 8 Tauri command handlers (get_status, install_tool, start_tool, stop_tool, restart_tool, uninstall_tool, tail_log, open_logs_dir) |
| `lib.rs` | AppState struct, tauri::Builder setup, invoke_handler registration |
| `main.rs` | Entry point: `toolbay_lib::run()` |

### Frontend Files (src/)

| File | Purpose |
|------|---------|
| `main.ts` | IPC bridge to Tauri commands; renders install/start/stop/restart/uninstall UI |
| `styles.css` | Dark-mode macOS menu-bar styling with status indicators |
| `index.html` | Minimal shell page |

---

## Key Decisions & Gotchas

### 1. Duplicate Macro Errors — Fixed by Module Separation
**Problem:** Every `#[tauri::command]` defined directly in `lib.rs` produced duplicate macro errors:
```
error[E0255]: the name `__cmd__get_status` is defined multiple times
```
**Solution:** Moved ALL Tauri commands into a dedicated `commands.rs` module. The proc-macro generates unique macro names per file, and having them in a single non-root module avoids the collision with the build script's code generation.

### 2. Crate Type Change
Removed `staticlib` from `[lib] crate-type`. Only `cdylib` and `rlib` are needed for Tauri apps.

### 3. Restart Method Signature
`Supervisor::restart()` requires `&mut self` because it calls `start()` which takes `&mut self`. The Tauri command handles this by using scoped mutable locks:
```rust
{
    let mut sup = state.supervisor.lock().await;
    sup.as_mut().unwrap().restart(&ports_path).await?;
} // lock dropped before checking result
```

---

## Architecture Overview

```
┌─────────────────────────────────────────────┐
│              Tauri Frontend                 │
│         (main.ts + styles.css)              │
│  ┌───────────┐  IPC calls via invoke()     │
│  │ React-like │ ─────────────────────►      │
│  │ Vanilla JS │                             │
│  └───────────┘                             │
├─────────────────────────────────────────────┤
│              Tauri Backend (Rust)           │
│                                             │
│  commands.rs  ← 8 Tauri command handlers    │
│       │                                     │
│       ▼                                     │
│  lib.rs → AppState + tauri::Builder         │
│       │                                     │
│       ├─► paths.rs          (path helpers)  │
│       ├─► manifest_headroom.rs (constants)  │
│       ├─► runtime_install.rs  (download+install) │
│       ├─► supervisor.rs     (process mgmt)  │
│       ├─► ledger.rs           (patch tracking)│
│       └─► config_patch.rs   (TOML editing)  │
├─────────────────────────────────────────────┤
│              External Tools                 │
│  - Python standalone (downloaded)          │
│  - headroom-ai wheel (downloaded)          │
│  - macOS `open` command (for logs dir)     │
└─────────────────────────────────────────────┘
```

---

## Running the App

### Development Mode
```bash
npm run tauri dev
```

### Production Build
```bash
npm run tauri build
```

### Check Compilation Only
```bash
cd src-tauri && cargo check
```

---

## Remaining Warnings (Non-Critical)

33 warnings for unused code — these are reserved for Phase 2 features:
- `health_check()` in supervisor.rs — HTTP health polling (unused parameter currently)
- `status_channel()` in supervisor.rs — UI status broadcast channel
- `supervisor_start_tool()` / `supervisor_stop_tool()` in supervisor.rs — convenience functions replaced by direct Supervisor usage
- `get_port()` on PortsState — not yet used after allocation
- `build_launch_command()` in manifest_headroom.rs — utility function
- Various unused constants (PYTHON_INTERPRETER_NAME, SCRIPTS_DIR_NAME, etc.)

These can be prefixed with `_` or `#[allow(dead_code)]` to silence warnings.

---

## Next Steps for Future Sessions

### Phase 2: Complete the Implementation
1. **Wire up health check** — Use the `health_check()` function in supervisor's main loop
2. **Implement log streaming** — Connect tail_log to real-time log watching (use tokio::fs::read_to_string polling or file watchers)
3. **Add more tools** — Make the system generic beyond headroom-ai (currently hardcoded to TOOL_ID = "headroom-ai")
4. **Port conflict handling** — Implement proper port allocation with fallback when range is exhausted

### Phase 3: Polish & Distribution
1. **Code signing** for macOS notarization
2. **Auto-updater** integration (Tauri has built-in updater plugin)
3. **Multi-tool support** — UI should list all installed tools, not just headroom-ai
4. **Settings panel** — Configure port ranges, restart policies, log levels

### Documentation Needed
- User-facing README with installation instructions
- Architecture deep-dive document
- API reference for Tauri commands (what each returns on success/failure)

---

## File Structure Reference

```
ai-tools-tray-helper/
├── docs/
│   ├── PLAN.md              ← Original project plan
│   └── SESSION_LOG.md       ← This file
├── src/
│   ├── main.ts              ← Frontend IPC + rendering
│   ├── styles.css           ← Dark mode styling
│   └── index.html           ← Shell page (in root, not src/)
├── src-tauri/
│   ├── Cargo.toml           ← Dependencies
│   ├── tauri.conf.json      ← App config (bundle name, icon paths)
│   ├── build.rs             ← tauri-build invocation
│   └── src/
│       ├── main.rs          ← Entry point
│       ├── lib.rs           ← AppState + tauri::Builder
│       ├── commands.rs      ← All Tauri command handlers
│       ├── paths.rs         ← Path resolution
│       ├── manifest_headroom.rs  ← Hardcoded tool manifest
│       ├── runtime_install.rs    ← Download + install flow
│       ├── supervisor.rs       ← Process lifecycle
│       ├── ledger.rs           ← Config patch tracking
│       └── config_patch.rs     ← TOML editing utilities
├── package.json
└── vite.config.ts
```

---

## Dependencies Summary

| Crate | Version | Purpose |
|-------|---------|---------|
| tauri | 2.x | Framework |
| tauri-plugin-opener | 2.x | Open logs in Finder |
| tokio | 1.x | Async runtime, process management |
| reqwest | 0.12 | HTTP downloads |
| sha2 | 0.10 | SHA-256 verification |
| thiserror | 2.x | Error derives |
| futures | 0.3 | Stream extensions |
| zip | 8.x | Wheel unpacking (wheels are ZIPs) |
| dirs | 5.x | Home directory resolution |
| serde / serde_json | 1.x | JSON serialization |