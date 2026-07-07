/**
 * Toolbay — Multi-tool frontend for macOS menu-bar app managing background CLI/AI tools.
 * Vanilla TS (no framework needed). Each registered tool gets its own card with controls.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ToolStatusResponse {
  status: string;
  port: number | null;
  installed: boolean;
}

interface ToolInfoResponse {
  tool_id: string;
  display_name: string;
  status: string;
  port: number;
  installed: boolean;
}

/** Shape of the LogLineEvent emitted by the backend during streaming. */
interface LogLineEvent {
  tool_id: string;
  line: string;
}

// ---------------------------------------------------------------------------
// State — per-tool tracking
// ---------------------------------------------------------------------------

/** Per-tool streaming state. */
interface ToolStreamState {
  isStreaming: boolean;
  listener: (() => void) | null;
  accumulatedLines: string[];
}

/** Map of tool_id → per-tool state. */
const streamStates: Map<string, ToolStreamState> = new Map();

// ---------------------------------------------------------------------------
// Helpers — Status
// ---------------------------------------------------------------------------

function updateStatusBadge(badge: HTMLElement, status: string) {
  const normalized = status.toLowerCase();
  badge.classList.remove("stopped", "starting", "running", "crashed", "restarting");
  badge.classList.add(normalized);

  const labels: Record<string, string> = {
    stopped: "● Stopped",
    starting: "◌ Starting…",
    running: "● Running",
    crashed: "✕ Crashed",
    restarting: "↻ Restarting…",
  };
  badge.textContent = labels[normalized] || normalized.toUpperCase();
}

function updateToolButtons(
  installBtn: HTMLButtonElement,
  startBtn: HTMLButtonElement,
  stopBtn: HTMLButtonElement,
  restartBtn: HTMLButtonElement,
  uninstallBtn: HTMLButtonElement,
  status: string,
  installed: boolean
) {
  const canControl = ["running", "crashed", "restarting"].includes(status);

  installBtn.disabled = installed;
  startBtn.disabled = !installed || status === "running" || status === "starting" || status === "restarting";
  stopBtn.disabled = !canControl;
  restartBtn.disabled = !canControl;
  uninstallBtn.disabled = !installed;
}

// ---------------------------------------------------------------------------
// Helpers — Log lines
// ---------------------------------------------------------------------------

function appendLogLine(toolId: string, line: string) {
  const state = streamStates.get(toolId);
  if (!state) return;

  state.accumulatedLines.push(line);

  // Keep only last 500 lines in memory
  if (state.accumulatedLines.length > 500) {
    state.accumulatedLines = state.accumulatedLines.slice(-500);
  }

  const el = document.querySelector(`.tool-card[data-tool-id="${toolId}"] .tool-log-output`);
  if (el) {
    el.textContent = state.accumulatedLines.join("\n");
    (el as HTMLElement).scrollTop = (el as HTMLElement).scrollHeight;
  }
}

function clearLogBuffer(toolId: string) {
  const state = streamStates.get(toolId);
  if (state) {
    state.accumulatedLines = [];
  }
}

// ---------------------------------------------------------------------------
// Commands — per tool_id
// ---------------------------------------------------------------------------

async function getToolStatus(toolId: string): Promise<ToolStatusResponse | null> {
  try {
    return await invoke<ToolStatusResponse>("get_status", { toolId });
  } catch (err) {
    console.error(`Failed to get status for ${toolId}:`, err);
    return null;
  }
}

async function installTool(toolId: string, cardEl: HTMLElement) {
  const msgEl = cardEl.querySelector(".install-msg") as HTMLElement | null;
  if (!msgEl) return;

  msgEl.textContent = "Installing…";

  try {
    const result = await invoke("install_tool", { toolId });
    msgEl.textContent = `Installed: ${result}`;
    updateCardFromStatus(toolId, cardEl);
  } catch (err) {
    msgEl.textContent = `Install failed: ${err}`;
  }
}

async function startTool(toolId: string, cardEl: HTMLElement) {
  try {
    await invoke("start_tool", { toolId });
    const badge = cardEl.querySelector(".status-badge") as HTMLElement | null;
    if (badge) updateStatusBadge(badge, "starting");
    // Poll for status updates
    pollToolStatus(toolId, cardEl);
  } catch (err) {
    console.error(`Failed to start ${toolId}:`, err);
  }
}

async function stopTool(toolId: string, cardEl: HTMLElement) {
  try {
    await invoke("stop_tool", { toolId });

    // Stop streaming if active
    const state = streamStates.get(toolId);
    if (state?.isStreaming) {
      await toggleLogStream(toolId, false);
    }

    updateCardFromStatus(toolId, cardEl);
  } catch (err) {
    console.error(`Failed to stop ${toolId}:`, err);
  }
}

async function restartTool(toolId: string, cardEl: HTMLElement) {
  try {
    await invoke("restart_tool", { toolId });
    const badge = cardEl.querySelector(".status-badge") as HTMLElement | null;
    if (badge) updateStatusBadge(badge, "restarting");
    pollToolStatus(toolId, cardEl);
  } catch (err) {
    console.error(`Failed to restart ${toolId}:`, err);
  }
}

async function uninstallTool(toolId: string, displayName: string, cardEl: HTMLElement) {
  if (!confirm(
    `Are you sure you want to uninstall ${displayName}?

This will:
  • Remove the runtime directory
  • Reverse config patches
  • Backups are preserved`
  )) {
    return;
  }

  // Stop streaming if active
  const state = streamStates.get(toolId);
  if (state?.isStreaming) {
    await toggleLogStream(toolId, false);
  }

  try {
    const result = await invoke("uninstall_tool", { toolId });
    console.log(result);
    // Remove the card from DOM
    cardEl.remove();
    // Check if list is empty
    checkEmptyState();
  } catch (err) {
    alert(`Uninstall failed: ${err}`);
  }
}

async function refreshLogs(toolId: string, cardEl: HTMLElement) {
  const logOutput = cardEl.querySelector(".tool-log-output") as HTMLElement | null;
  if (!logOutput) return;

  try {
    // Stop streaming temporarily for a full refresh
    const wasStreaming = streamStates.get(toolId)?.isStreaming ?? false;
    if (wasStreaming) {
      await toggleLogStream(toolId, false);
    }

    clearLogBuffer(toolId);

    const logContent: string = await invoke("tail_log", { toolId, reset: true, lines: 100 });
    if (logContent) {
      const lines = logContent.split("\n");
      for (const line of lines) {
        appendLogLine(toolId, line);
      }
    } else {
      appendLogLine(toolId, "(No log entries yet)");
    }

    // Restart streaming if it was active
    if (wasStreaming) {
      await toggleLogStream(toolId, true);
    }
  } catch (err) {
    appendLogLine(toolId, `Failed to read logs: ${err}`);
  }
}

// ---------------------------------------------------------------------------
// Log streaming — per tool_id
// ---------------------------------------------------------------------------

async function toggleLogStream(toolId: string, forceState?: boolean) {
  let state = streamStates.get(toolId);
  if (!state) {
    state = { isStreaming: false, listener: null, accumulatedLines: [] };
    streamStates.set(toolId, state);
  }

  const shouldStart = forceState !== undefined ? forceState : !state.isStreaming;

  if (shouldStart) {
    try {
      await invoke("start_log_stream", { toolId });
      state.isStreaming = true;

      // Set up Tauri event listener for this tool
      let unlistenFn: (() => void) | null = null;
      const setupListener = async () => {
        unlistenFn = await listen<LogLineEvent>("log-update", (event) => {
          if (state?.isStreaming && event.payload?.tool_id === toolId) {
            appendLogLine(toolId, event.payload.line);
          }
        });
      };
      void setupListener(); // Fire and forget — listener will be set up async
      state.listener = () => { unlistenFn?.(); };

      // Update button text
      const streamBtn = document.querySelector(`.tool-card[data-tool-id="${toolId}"] .tool-stream-btn`);
      if (streamBtn) {
        (streamBtn as HTMLButtonElement).textContent = "⏸ Pause";
        (streamBtn as HTMLButtonElement).classList.add("streaming");
      }
    } catch (err) {
      console.error(`Failed to start log streaming for ${toolId}:`, err);
    }
  } else {
    try {
      await invoke("stop_log_stream", { toolId });
      state.isStreaming = false;

      if (state.listener) {
        state.listener();
        state.listener = null;
      }

      const streamBtn = document.querySelector(`.tool-card[data-tool-id="${toolId}"] .tool-stream-btn`);
      if (streamBtn) {
        (streamBtn as HTMLButtonElement).textContent = "▶ Stream";
        (streamBtn as HTMLButtonElement).classList.remove("streaming");
      }
    } catch (err) {
      console.error(`Failed to stop log streaming for ${toolId}:`, err);
    }
  }
}

// ---------------------------------------------------------------------------
// Polling — per tool_id
// ---------------------------------------------------------------------------

let pollIntervals: Map<string, number> = new Map();

function pollToolStatus(toolId: string, cardEl: HTMLElement) {
  // Clear existing interval if any
  const existing = pollIntervals.get(toolId);
  if (existing !== undefined) clearInterval(existing);

  let count = 0;
  const maxPolls = 150; // 30 seconds at 2s intervals

  const interval = window.setInterval(async () => {
    count++;
    const status = await getToolStatus(toolId);
    if (status) {
      updateCardUI(toolId, cardEl, status);
    }

    if (count >= maxPolls || status?.status === "stopped" || status?.status === "running") {
      clearInterval(interval);
      pollIntervals.delete(toolId);
      // Final check
      if (status && status.status !== "starting" && status.status !== "restarting") {
        updateCardUI(toolId, cardEl, status);
      }
    }
  }, 2000);

  pollIntervals.set(toolId, interval);
}

// ---------------------------------------------------------------------------
// Card rendering / updates
// ---------------------------------------------------------------------------

/** Update a tool card's UI elements from a ToolStatusResponse. */
function updateCardUI(
  toolId: string,
  cardEl: HTMLElement,
  status: ToolStatusResponse
) {
  const badge = cardEl.querySelector(".status-badge") as HTMLElement | null;
  const msgEl = cardEl.querySelector(".install-msg") as HTMLElement | null;
  const installBtn = cardEl.querySelector(".tool-install-btn") as HTMLButtonElement | null;
  const startBtn = cardEl.querySelector(".tool-start-btn") as HTMLButtonElement | null;
  const stopBtn = cardEl.querySelector(".tool-stop-btn") as HTMLButtonElement | null;
  const restartBtn = cardEl.querySelector(".tool-restart-btn") as HTMLButtonElement | null;
  const uninstallBtn = cardEl.querySelector(".tool-uninstall-btn") as HTMLButtonElement | null;

  if (badge) updateStatusBadge(badge, status.status);
  if (msgEl) {
    msgEl.dataset.installed = String(status.installed);
    msgEl.textContent = status.installed
      ? `Installed — Port: ${status.port ?? "—"}`
      : "Not installed";
  }

  if (installBtn && startBtn && stopBtn && restartBtn && uninstallBtn) {
    updateToolButtons(installBtn, startBtn, stopBtn, restartBtn, uninstallBtn, status.status, status.installed);
  }
}

/** Fetch current status and update the card. */
async function updateCardFromStatus(toolId: string, cardEl: HTMLElement) {
  const status = await getToolStatus(toolId);
  if (status) {
    updateCardUI(toolId, cardEl, status);
  }
}

/** Create a tool card DOM element from the template and populate it. */
function createToolCard(toolInfo: ToolInfoResponse): HTMLElement | null {
  const template = document.querySelector<HTMLTemplateElement>("#tool-card-template");
  if (!template) return null;

  const clone = template.content.cloneNode(true) as DocumentFragment;
  const card = clone.children[0] as HTMLElement;

  // Set data attributes and content
  card.dataset.toolId = toolInfo.tool_id;
  card.querySelector(".tool-display-name")!.textContent = toolInfo.display_name;
  card.querySelector(".tool-id-badge")!.textContent = toolInfo.tool_id;

  const installMsg = card.querySelector(".install-msg") as HTMLElement | null;
  if (installMsg) {
    installMsg.dataset.installed = String(toolInfo.installed);
    installMsg.textContent = toolInfo.installed
      ? `Installed — Port: ${toolInfo.port || "—"}`
      : "Not installed";
  }

  // Initialize per-tool streaming state
  streamStates.set(toolInfo.tool_id, {
    isStreaming: false,
    listener: null,
    accumulatedLines: [],
  });

  // Wire up button event listeners using closure over toolId and card
  const installBtn = card.querySelector(".tool-install-btn") as HTMLButtonElement | null;
  const startBtn = card.querySelector(".tool-start-btn") as HTMLButtonElement | null;
  const stopBtn = card.querySelector(".tool-stop-btn") as HTMLButtonElement | null;
  const restartBtn = card.querySelector(".tool-restart-btn") as HTMLButtonElement | null;
  const uninstallBtn = card.querySelector(".tool-uninstall-btn") as HTMLButtonElement | null;
  const streamBtn = card.querySelector(".tool-stream-btn") as HTMLButtonElement | null;
  const refreshBtn = card.querySelector(".tool-refresh-btn") as HTMLButtonElement | null;

  installBtn?.addEventListener("click", () => installTool(toolInfo.tool_id, card));
  startBtn?.addEventListener("click", () => startTool(toolInfo.tool_id, card));
  stopBtn?.addEventListener("click", () => stopTool(toolInfo.tool_id, card));
  restartBtn?.addEventListener("click", () => restartTool(toolInfo.tool_id, card));
  uninstallBtn?.addEventListener("click", () => uninstallTool(toolInfo.tool_id, toolInfo.display_name, card));

  // Log stream toggle
  streamBtn?.addEventListener("click", () => {
    toggleLogStream(toolInfo.tool_id);
  });

  // Log refresh
  refreshBtn?.addEventListener("click", () => {
    refreshLogs(toolInfo.tool_id, card);
  });

  return card;
}

/** Refresh the full tool list from backend. */
async function renderToolList() {
  const container = document.getElementById("tool-list");
  if (!container) return;

  try {
    const tools: ToolInfoResponse[] = await invoke("list_tools");

    // Clear existing cards (but keep empty-state message)
    const existingCards = container.querySelectorAll(".tool-card");
    existingCards.forEach(c => c.remove());

    // Create and append a card for each tool
    for (const tool of tools) {
      const cardEl = createToolCard(tool);
      if (cardEl) {
        container.appendChild(cardEl);
        // Update UI from current status
        updateCardUI(tool.tool_id, cardEl, {
          status: tool.status,
          port: tool.port || null,
          installed: tool.installed,
        });

        // Refresh logs for this tool after a short delay
        setTimeout(() => refreshLogs(tool.tool_id, cardEl), 500);
      }
    }

    checkEmptyState();
  } catch (err) {
    console.error("Failed to list tools:", err);
  }
}

/** Show/hide the empty-state message. */
function checkEmptyState() {
  const container = document.getElementById("tool-list");
  const emptyMsg = document.getElementById("empty-tools-msg");
  if (!container || !emptyMsg) return;

  const hasCards = container.querySelectorAll(".tool-card").length > 0;
  emptyMsg.style.display = hasCards ? "none" : "block";
}

// ---------------------------------------------------------------------------
// Register Tool Modal
// ---------------------------------------------------------------------------

function showRegisterModal() {
  // Remove existing modal if any
  removeRegisterModal();

  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.innerHTML = `
    <div class="modal">
      <h2>Register New Tool</h2>
      <div class="form-group">
        <label for="reg-tool-id">Tool ID</label>
        <input type="text" id="reg-tool-id" placeholder="e.g. my-cli-tool" />
      </div>
      <div class="form-group">
        <label for="reg-display-name">Display Name</label>
        <input type="text" id="reg-display-name" placeholder="e.g. My CLI Tool" />
      </div>
      <div class="form-group">
        <label for="reg-python-url">Python Standalone URL</label>
        <input type="text" id="reg-python-url" placeholder="https://example.com/python-standalone.tar.gz" />
      </div>
      <div class="form-group">
        <label for="reg-python-sha256">Python SHA-256</label>
        <input type="text" id="reg-python-sha256" placeholder="abc123..." />
      </div>
      <div class="form-group">
        <label for="reg-wheel-url">Wheel URL</label>
        <input type="text" id="reg-wheel-url" placeholder="https://example.com/tool-0.1-py3-none-any.whl" />
      </div>
      <div class="form-group">
        <label for="reg-wheel-sha256">Wheel SHA-256</label>
        <input type="text" id="reg-wheel-sha256" placeholder="def456..." />
      </div>
      <div class="modal-actions">
        <button class="btn btn-cancel" id="reg-cancel-btn">Cancel</button>
        <button class="btn btn-primary" id="reg-confirm-btn">Register</button>
      </div>
    </div>
  `;

  document.body.appendChild(overlay);

  // Cancel handler
  overlay.querySelector("#reg-cancel-btn")!.addEventListener("click", removeRegisterModal);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) removeRegisterModal();
  });

  // Confirm handler
  overlay.querySelector("#reg-confirm-btn")!.addEventListener("click", async () => {
    const toolId = (overlay.querySelector("#reg-tool-id") as HTMLInputElement).value.trim();
    const displayName = (overlay.querySelector("#reg-display-name") as HTMLInputElement).value.trim();
    const pythonUrl = (overlay.querySelector("#reg-python-url") as HTMLInputElement).value.trim();
    const pythonSha256 = (overlay.querySelector("#reg-python-sha256") as HTMLInputElement).value.trim();
    const wheelUrl = (overlay.querySelector("#reg-wheel-url") as HTMLInputElement).value.trim();
    const wheelSha256 = (overlay.querySelector("#reg-wheel-sha256") as HTMLInputElement).value.trim();

    if (!toolId || !displayName || !pythonUrl || !pythonSha256 || !wheelUrl || !wheelSha256) {
      alert("All fields are required.");
      return;
    }

    try {
      await invoke("register_tool", {
        req: {
          tool_id: toolId,
          display_name: displayName,
          python_standalone_url: pythonUrl,
          python_standalone_sha256: pythonSha256,
          wheel_url: wheelUrl,
          wheel_sha256: wheelSha256,
        },
      });
      removeRegisterModal();
      renderToolList();
    } catch (err) {
      alert(`Registration failed: ${err}`);
    }
  });
}

function removeRegisterModal() {
  const existing = document.querySelector(".modal-overlay");
  if (existing) existing.remove();
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
  // Register tool button
  document.querySelector("#register-tool-btn")?.addEventListener("click", showRegisterModal);

  // Close button — hide instead of quit (menu-bar app)
  const closeBtn = document.querySelector("#close-btn");
  if (closeBtn) {
    closeBtn.addEventListener("click", handleWindowClose);
  }

  // Initial render: load tool list from backend
  renderToolList();

  // Auto-refresh tool list every 30 seconds
  setInterval(renderToolList, 30000);
});