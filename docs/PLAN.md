# toolbay — Build Plan (Phase 1 detailed, Phases 2–3 outlined) + Test Plan

## Context

`.claude/CLAUDE.md` is the full product spec for a macOS menu-bar app that installs/runs/monitors/uninstalls background CLI tools (first: `headroom-ai`), config-patching other apps' settings files along the way. The repo is currently an untouched `create-tauri-app` scaffold (vanilla TS + Vite frontend, one `greet` Rust command, no tray code, deps limited to `tauri`/`tauri-plugin-opener`/`serde`/`serde_json`). Nothing has been built yet — this plan bootstraps Phase 1 (§7 of the spec: hardcode headroom-ai end-to-end, no manifest/trait abstraction yet) in concrete, file-level detail, then outlines Phases 2–3, then gives a full test plan.

Three spec open questions (§8) are resolved:
1. **Runtime install**: vendored pinned wheel URL + SHA-256 is the Phase 1 mechanism. Name Phase-1 functions so a second `pip`/`pipx`-based install path can be slotted in later without a rewrite (a real enum/trait comes in Phase 2, per the build order — don't build it early).
2. **Distribution**: plan for eventual Developer-ID-signed, notarized direct download (no App Store). Keep the bundle identifier stable now; actual signing/notarization config is a later task gated on having an Apple Developer certificate.
3. **Architecture**: Apple Silicon (arm64) only for v1.

A validated-technical-details pass (a Plan subagent read the actual installed Tauri v2 API surface, `Cargo.lock`, and ACL manifests in this repo) surfaced two things the spec doesn't fully account for, both folded into Phase 1 below:
- **Path mismatch**: Tauri's `app_data_dir()`/`app_log_dir()` auto-suffix the bundle identifier (`uk.me.dcb.toolbay.app`), not the plain `toolbay` the spec hardcodes everywhere. Phase 1 builds its own `paths.rs` that ignores those helpers for this purpose and hardcodes `~/Library/Application Support/toolbay/...` / `~/Library/Logs/toolbay/...` directly, so on-disk layout matches the spec exactly.
- **Missing Python interpreter**: the manifest's `launchCommand: ["python", ...]` and `runtime.kind: "python-wheel"` only describe installing the *wheel* — there's no system Python to run it with (§3.1 explicitly forbids using the system Python). Phase 1 must also vendor a pinned, hashed, portable CPython build for macOS arm64 (a `python-build-standalone` release), install it once, and resolve the wheel's `launchCommand` against that interpreter's absolute path — never against `$PATH`.

---

## Phase 1 — Hardcode headroom-ai end to end

### 1. Dependencies (`src-tauri/Cargo.toml`)

Add directly (all already resolve cleanly against the existing `Cargo.lock` graph — `tauri` itself pulls `reqwest` and `tokio` transitively, so no version conflicts):
- `reqwest` (with `stream` feature) — async HTTPS download
- `tokio` (features: `process`, `io-util`, `time`, `macros`, `rt-multi-thread`) — process supervision, async runtime
- `sha2` — streaming SHA-256 verification
- `zip` — unpack the wheel (a wheel is a zip file); run extraction via `tokio::task::spawn_blocking` since `zip` is sync
- `tempfile` (dev-dependency) — test fixtures for installer/patcher tests
- `httpmock` or a small `hyper`-based test server (dev-dependency) — mock the wheel/interpreter download in tests, no real network calls in CI

### 2. Module layout (new files under `src-tauri/src/`)

- **`paths.rs`** — single source of truth for every on-disk location: app root (`~/Library/Application Support/toolbay`), `runtimes/<toolId>/`, `state/ports.json`, `state/patches.json`, `backups/<toolId>/`, and logs (`~/Library/Logs/toolbay/<toolId>.log`/`.err.log`), built from `dirs::home_dir()` (already in the dependency graph) rather than Tauri's identifier-suffixed path helpers. Every other module takes these paths as plain arguments — nothing else calls the path helpers directly. This is what makes everything below unit-testable with `tempfile::TempDir` standing in for the real home directory.
- **`runtime_install.rs`** — Phase-1-hardcoded install flow for headroom-ai specifically:
  - `download_and_verify(url, expected_sha256, dest_path) -> Result<PathBuf>`: streams the download through both a file writer and a `Sha256` hasher chunk-by-chunk, compares digest before returning success, deletes the partial file and errors out on mismatch (never leaves an unverified artifact usable).
  - `install_standalone_python(install_dir) -> Result<PythonHandle>`: downloads+verifies the pinned `python-build-standalone` arm64 build, unpacks it, returns the resolved absolute interpreter path.
  - `install_headroom_wheel(python: &PythonHandle, install_dir) -> Result<()>`: downloads+verifies the pinned headroom-ai wheel, unpacks (or `pip install --target` using the vendored interpreter — prefer manual unpack unless headroom-ai's transitive deps make that impractical; see Open Items).
  - `report_install_size(install_dir) -> u64`: walks the dir, sums file sizes — used by the UI's "before you install" disclosure.
- **`supervisor.rs`** — process lifecycle for one tool (generic-shaped but only wired to headroom-ai in Phase 1):
  - `Status` enum: `Stopped | Starting | Running | Crashed | Restarting`.
  - `spawn(python_path, port, log_paths) -> Child` using `tokio::process::Command`, stdout/stderr piped and streamed line-by-line into the two log files via `AsyncBufReadExt::lines()`.
  - `supervise(child, restart_policy)`: a tokio task using `tokio::select!` over `child.wait()` and a shutdown channel; on unexpected exit, applies the capped exponential backoff from the spec (`[1,2,5,10,30]`s, max 5 attempts) before respawning, then sets `Crashed` and stops retrying.
  - `health_check(url) -> bool`: polls the HTTP health endpoint with a short timeout.
  - `allocate_port(ports_state_path) -> u16`: picks the first free port in `18700–18799` by attempting a real `TcpListener::bind`, records `{toolId: port}` into `ports.json`.
- **`config_patch.rs`** — the two strategies Phase 1 actually needs (`json-merge-key` for `~/.claude/settings.json`, `toml-block-insert` for `~/.codex/config.toml`; `env-file-line` waits until a tool needs it):
  - `backup_file(target_path, tool_id) -> Result<PathBuf>`: copies the untouched file to `backups/<toolId>/<filename>.<timestamp>.bak` before any edit (records "didn't exist" if the target is missing, so uninstall knows to delete rather than restore).
  - `apply_json_merge_key(...)` / `reverse_json_merge_key(...)`.
  - `apply_toml_block_insert(target_path, marker, block_text)` / `reverse_toml_block_insert(...)` — pure text between `# >>> toolbay-managed:<toolId> >>>` / `# <<< toolbay-managed:<toolId> <<<` markers (include the tool id in the marker so two tools' blocks in the same file are distinguishable and independently reversible — no `toml` crate needed, this is line-based).
  - **`ledger.rs`**: `record_patch(entry)` / `read_ledger()` / `reverse_patches_for_tool(tool_id)` against `state/patches.json` — every apply function above writes one ledger entry; uninstall reads only entries matching its `tool_id` and reverses exactly those, leaving other tools' entries untouched.
- **`manifest_headroom.rs`** (or plain constants) — the pinned URL/SHA-256/launch command/health-check URL/config-patch targets for this one tool, as Rust constants — **not** a JSON manifest loader yet (that's Phase 2).

### 3. Tray UI (`src-tauri/src/lib.rs`)

- Empty out `tauri.conf.json`'s `app.windows` (drop the default 800×600 window); create the tool-list window lazily in `.setup()` with `WebviewWindowBuilder`, initially hidden.
- Build the tray via `tauri::tray::TrayIconBuilder` + `tauri::menu::MenuBuilder` (status text, Start/Stop/Restart, "Show Logs", "Uninstall", Quit); use a template tray icon (`iconAsTemplate: true`) separate from the app's dock icon asset.
- Call the dock-hiding API in `.setup()` (`app_handle.set_dock_visibility(false)`, confirmed present in the installed `@tauri-apps/api` 2.11.x); as a belt-and-suspenders fix for the brief dock-flash this can leave, also try to bake `LSUIElement` into the bundled `Info.plist` — confirm the exact `tauri.conf.json`/`Info.plist` merge convention against current docs at implementation time (flagged, not blocking).
- Register `#[tauri::command]`s the popover window calls: `get_status`, `install_tool`, `start_tool`, `stop_tool`, `restart_tool`, `uninstall_tool`, `tail_log(lines)`. These call straight into the plain-Rust modules above — no plugin ACL entries needed (custom commands aren't gated by the capabilities system; this repo's own `greet` command already proves that). If a second window label (e.g. `"tools"`) is added, extend `capabilities/default.json`'s `windows` array, or that webview silently won't get even `core:default` grants.

### 4. Frontend (`src/`)

Keep it vanilla TS (matches the existing scaffold — one status card, a few buttons, and a log-tail pane doesn't need a framework): status badge for headroom-ai, start/stop/restart buttons, "view logs" tail pane, "uninstall" with a confirmation dialog listing exactly what will be removed/reversed (full-disclosure requirement), polling `get_status`/`tail_log` or listening for a Rust-emitted event.

### 5. Signing/notarization placeholder

No certificate exists yet — nothing to configure now beyond keeping the identifier stable. Follow-up once an Apple Developer ID exists: `tauri.conf.json`'s `bundle.macOS.signingIdentity` + notarization credentials, plus a signed/notarized smoke test before any real distribution.

---

## Phase 2 — Pull out the four subsystems (outline)

Once Phase 1 works end-to-end: introduce a `Manifest` serde struct matching spec §4, plus a loader/validator; convert `runtime_install.rs` into a `RuntimeInstaller` trait with `PythonWheel` as the first implementor (this is where the pip/pipx alternate path becomes a second implementor); convert `supervisor.rs`/`config_patch.rs` into subsystems parameterized by `Manifest` instead of hardcoded constants; add the registry that enumerates bundled manifests. Reference implementation = the Phase 1 code, so this is mostly "generalize the working thing," not new design.

## Phase 3 — Second tool via manifest only (outline)

Pick or wait for a concrete second CLI tool; write only its manifest JSON; verify zero code changes were needed outside the manifest file. This is the forcing function that proves the Phase 2 abstraction actually holds.

---

## Testing Plan

This project's working method (per `.claude/CLAUDE.md`) is strict TDD: every bug fix starts with a failing test that reproduces it, every new feature starts with its test before its implementation. Apply that per-module below, not as an afterthought pass at the end.

### Unit tests (`cargo test`, no GUI/network/real filesystem outside `tempfile::TempDir`)

All of the following are pure-Rust-testable per the module design above (paths passed in as arguments, no `AppHandle` reached into) — this is the bulk of the safety net and should run in CI on every change with zero external dependencies:

- **`paths.rs`**: given a fake home dir, every path helper returns the expected `toolbay`-rooted path (catches any regression back to identifier-suffixed paths).
- **`runtime_install.rs`**:
  - `download_and_verify` against a local mock HTTP server: correct hash → success + file present; tampered/wrong hash → error + **no file left on disk** (the security-critical assertion — check the artifact is actually deleted, not just that an error was returned); truncated body → error, not silent partial success; connection refused → clean error, no panic.
  - `install_standalone_python` / `install_headroom_wheel`: unpack a small fixture zip into a `TempDir`, assert resolved paths exist and are absolute.
  - `report_install_size`: known fixture directory → exact byte count.
- **`supervisor.rs`** (use a tiny fixture script — e.g. one that binds a port and responds `200` on `/health` — never depend on real headroom-ai in unit tests):
  - `allocate_port`: two sequential calls get two different free ports; a port already recorded as bound-and-still-open is skipped; `ports.json` reflects the assignment.
  - `spawn`+`supervise`: a process that exits immediately gets restarted following the exact declared backoff sequence; after 5 attempts, status becomes `Crashed` and no 6th attempt happens (inject the backoff schedule so the test isn't slow — don't sleep 30s for real).
  - stdout/stderr from the fixture process end up in the two expected log files, line-by-line, in order.
  - `health_check`: true/false correctly against a mock server returning 200 vs. 500 vs. connection-refused.
- **`config_patch.rs`** + ledger — the highest-trust subsystem, test most thoroughly:
  - `backup_file`: existing file → byte-identical timestamped copy; missing file → recorded as "didn't exist" so reversal deletes rather than restores.
  - `apply_json_merge_key` → `reverse_json_merge_key` round-trips to byte-identical original.
  - `apply_toml_block_insert` → `reverse_toml_block_insert` round-trips to byte-identical original; applying twice for the same tool doesn't duplicate the block.
  - **Multi-tool safety** (the spec's core promise): two different `tool_id`s patch the *same* target file; uninstalling tool A removes only A's block/key and leaves B's fully intact — assert exact remaining file content, not just "no error."
  - Ledger: `reverse_patches_for_tool` only touches entries whose `toolId` matches; a hand-edited `patches.json` entry pointing at a missing backup fails loudly rather than silently deleting the live config.

### Integration tests

- Full Phase-1 flow driven directly through the command-wrapped functions (call the Rust functions in a `#[tokio::test]`, bypassing the webview entirely): install → start → health check passes → stop → uninstall, against a fixture "fake headroom" script and scratch copies of `~/.claude/settings.json`/`~/.codex/config.toml`, asserting the post-uninstall file state exactly matches pre-install (diff the files).
- Crash-and-restart integration test: fixture process exits non-zero after N seconds, assert status transitions match `starting → running → restarting → running → ... → crashed` in order once attempts are exhausted.

### Security / adversarial tests

- Tampered wheel/interpreter hash at every stage (during download, and swapped on disk after install but before launch) is rejected before execution.
- Config patcher never writes if the backup step fails (assert ordering: backup-then-patch, not patch-then-backup).
- Ledger reversal refuses (rather than guesses) when its recorded backup path is missing.

### Manual QA checklist (native tray/menu and dock-hiding are outside WebDriver's reach — these stay manual)

Run on a real Apple Silicon Mac before calling Phase 1 "done," and again before any distributed release:
1. Launch app — dock icon never appears (not even briefly), tray icon renders correctly in light/dark menu bars.
2. Click tray icon → menu shows correct status; "Install" discloses exact download URLs/sizes/hashes/destination paths before proceeding — verify against `find ~/Library/Application\ Support/toolbay` afterward.
3. Start the real tool → health check goes green in the UI; cross-check by `curl`-ing the real health endpoint manually.
4. Kill the tool's process externally (`kill -9`) → UI shows `restarting` then `running` again without user action.
5. Repeatedly kill past the attempt cap → UI shows `crashed` and stops auto-restarting; manual "Restart" still works.
6. Diff `~/.claude/settings.json` / `~/.codex/config.toml` before/after install → exactly the expected block/key was added, nothing else touched, backup file matches pre-install content.
7. Uninstall → confirmation dialog lists real paths; afterward confirm runtime dir is gone, config files match pre-install exactly (diff against backup), and the ledger has no lingering entries for this tool.
8. Quit the tray app while the tool is running → confirm the child process is actually terminated (`ps aux | grep headroom`), not orphaned.
9. (Once a Developer ID cert exists) repeat 1–8 against a signed, notarized build on a clean-ish account — specifically re-check step 6, since non-sandboxed dotfile writes are expected to need no extra prompt but Apple has expanded TCC coverage over time.

### CI notes

`cargo test` (all unit + integration tests above) can run on any macOS arm64 CI runner with zero network access — every download in tests goes through a local mock server. Full app E2E (tray clicks, dock visibility) is not CI-automatable per the WebDriver limitation and stays in the manual checklist; keep it out of any CI gate rather than faking coverage for it.

---

## Open items to resolve at Phase 1 kickoff

- Pin the exact `headroom-ai` version to ship; fetch its real wheel URL + SHA-256 (the spec's manifest example has a placeholder hash).
- Pick and pin the exact `python-build-standalone` macOS-arm64 release to vendor as the interpreter, with its SHA-256.
- Check `headroom-ai`'s transitive PyPI dependency count: if small, hash-pin each one manually; if large, generate a hash-locked requirements file (`pip-compile --generate-hashes`) and vendor it — decide before writing `install_headroom_wheel`, since it changes that function's shape.
- Confirm the current `tauri.conf.json`/`Info.plist` mechanism for baking in `LSUIElement`, and `tauri-driver`'s current release/macOS-support status if DOM-level E2E for the popover window is wanted later.
