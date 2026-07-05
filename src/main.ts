/**
 * Toolbay — Frontend for macOS menu-bar app managing background CLI/AI tools.
 * Vanilla TS (no framework needed for this simple UI).
 */

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ToolStatusResponse {
  status: string;
  port: number | null;
  installed: boolean;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let currentStatus = "stopped";
let installBtn: HTMLButtonElement | null;
let startBtn: HTMLButtonElement | null;
let stopBtn: HTMLButtonElement | null;
let restartBtn: HTMLButtonElement | null;
let uninstallBtn: HTMLButtonElement | null;
let statusBadgeEl: HTMLElement | null;
let installMsgEl: HTMLElement | null;
let logOutputEl: HTMLElement | null;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function setStatus(status: string) {
  currentStatus = status.toLowerCase();
  if (!statusBadgeEl) return;

  // Remove all status classes
  statusBadgeEl.classList.remove("stopped", "starting", "running", "crashed", "restarting");
  statusBadgeEl.classList.add(currentStatus);

  // Update text
  const labels: Record<string, string> = {
    stopped: "● Stopped",
    starting: "◌ Starting…",
    running: "● Running",
    crashed: "✕ Crashed",
    restarting: "↻ Restarting…",
  };
  statusBadgeEl.textContent = labels[currentStatus] || currentStatus.toUpperCase();
}

function updateButtons() {
  const isInstalled = currentStatus !== "stopped" || installMsgEl?.dataset.installed === "true";
  const canControl = ["running", "crashed", "restarting"].includes(currentStatus);

  if (installBtn) installBtn.disabled = isInstalled;
  if (startBtn) startBtn.disabled = !isInstalled || currentStatus === "running" || currentStatus === "starting" || currentStatus === "restarting";
  if (stopBtn) stopBtn.disabled = !canControl;
  if (restartBtn) restartBtn.disabled = !canControl;
  if (uninstallBtn) uninstallBtn.disabled = !isInstalled;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async function getStatus() {
  try {
    const resp: ToolStatusResponse = await invoke("get_status");
    setStatus(resp.status);
    updateButtons();
    if (resp.installed) {
      if (installMsgEl) {
        installMsgEl.dataset.installed = "true";
        installMsgEl.textContent = `Installed — Port: ${resp.port ?? "—"}`;
      }
    } else {
      if (installMsgEl) {
        installMsgEl.dataset.installed = "false";
        installMsgEl.textContent = "Not installed";
      }
    }
  } catch (err) {
    console.error("Failed to get status:", err);
  }
}

async function installTool() {
  if (!installMsgEl) return;
  installMsgEl.textContent = "Installing… Please wait.";
  try {
    const result = await invoke("install_tool");
    installMsgEl.textContent = `Installation complete: ${result}`;
    updateButtons();
    // Poll for status after install
    setTimeout(getStatus, 1000);
  } catch (err) {
    installMsgEl.textContent = `Install failed: ${err}`;
  }
}

async function startTool() {
  try {
    await invoke("start_tool");
    setStatus("starting");
    updateButtons();
    // Poll for status updates
    pollStatus();
  } catch (err) {
    console.error("Failed to start:", err);
  }
}

async function stopTool() {
  try {
    await invoke("stop_tool");
    setStatus("stopped");
    updateButtons();
  } catch (err) {
    console.error("Failed to stop:", err);
  }
}

async function restartTool() {
  try {
    await invoke("restart_tool");
    setStatus("restarting");
    updateButtons();
    pollStatus();
  } catch (err) {
    console.error("Failed to restart:", err);
  }
}

async function uninstallTool() {
  if (!confirm(
    "Are you sure you want to uninstall headroom-ai?\n\n" +
    "This will:\n" +
    "  • Remove the runtime directory (~/.local/share/toolbay/runtimes/headroom-ai/)\n" +
    "  • Reverse config patches in ~/.claude/settings.json and ~/.codex/config.toml\n" +
    "  • Backups are preserved in ~/Library/Application Support/toolbay/backups/"
  )) {
    return;
  }

  if (!installMsgEl) return;
  installMsgEl.textContent = "Uninstalling… Please wait.";

  try {
    const result = await invoke("uninstall_tool");
    installMsgEl.textContent = String(result);
    setStatus("stopped");
    updateButtons();
  } catch (err) {
    installMsgEl.textContent = `Uninstall failed: ${err}`;
  }
}

async function refreshLogs() {
  if (!logOutputEl) return;
  try {
    const logContent: string = await invoke("tail_log", { lines: 100 });
    logOutputEl.textContent = logContent || "(No log entries yet)";
  } catch (err) {
    logOutputEl.textContent = `Failed to read logs: ${err}`;
  }
}

async function openLogsDir() {
  try {
    await invoke("open_logs_dir");
  } catch (err) {
    console.error("Failed to open logs dir:", err);
  }
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

let pollInterval: number | null = null;

function pollStatus() {
  if (pollInterval !== null) clearInterval(pollInterval);
  // Poll every 2 seconds for the first 30 seconds, then every 10 seconds
  let count = 0;
  const maxPolls = 150; // 30 seconds at 2s intervals

  pollInterval = window.setInterval(async () => {
    count++;
    await getStatus();

    // If running or stopped, slow down polling
    if (count > maxPolls || currentStatus === "stopped" || currentStatus === "running") {
      clearInterval(pollInterval!);
      pollInterval = null;
      // Do a final check
      if (currentStatus !== "starting" && currentStatus !== "restarting") {
        getStatus();
      }
    }
  }, 2000);
}

// ---------------------------------------------------------------------------
// Window close handler — hide instead of quit (menu-bar app)
// ---------------------------------------------------------------------------

async function handleWindowClose() {
  try {
    await getCurrentWindow().hide();
  } catch (err) {
    console.error("Failed to hide window:", err);
  }
}

// ---------------------------------------------------------------------------
// DOM Ready
// ---------------------------------------------------------------------------

window.addEventListener("DOMContentLoaded", () => {
  installBtn = document.querySelector("#install-btn");
  startBtn = document.querySelector("#start-btn");
  stopBtn = document.querySelector("#stop-btn");
  restartBtn = document.querySelector("#restart-btn");
  uninstallBtn = document.querySelector("#uninstall-btn");
  statusBadgeEl = document.querySelector("#status-badge");
  installMsgEl = document.querySelector("#install-msg");
  logOutputEl = document.querySelector("#log-output");

  // Button event listeners
  installBtn?.addEventListener("click", installTool);
  startBtn?.addEventListener("click", startTool);
  stopBtn?.addEventListener("click", stopTool);
  restartBtn?.addEventListener("click", restartTool);
  uninstallBtn?.addEventListener("click", uninstallTool);

  // Log refresh button
  document.querySelector("#refresh-logs-btn")?.addEventListener("click", () => {
    refreshLogs();
  });

  document.querySelector("#open-logs-dir-btn")?.addEventListener("click", openLogsDir);

  // Close button — hide instead of quit (menu-bar app)
  const closeBtn = document.querySelector("#close-btn");
  if (closeBtn) {
    closeBtn.addEventListener("click", handleWindowClose);
  }

  // Initial status check
  getStatus();

  // Refresh logs after a short delay
  setTimeout(refreshLogs, 2000);

  // Auto-refresh logs every 15 seconds
  setInterval(refreshLogs, 15000);
});