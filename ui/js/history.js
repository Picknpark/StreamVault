import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import { escapeHtml, formatTimestamp, showToast } from "./utils.js";

let switchViewFn = null;
let searchTimer = null;

export function initHistoryModule({ onSwitchView }) {
  switchViewFn = onSwitchView;

  const searchInp = document.getElementById("history-search-input");
  const periodSel = document.getElementById("history-clear-period-select");
  const btnClear = document.getElementById("btn-clear-history-period");

  searchInp.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      refreshHistory(searchInp.value.trim());
    }, 140);
  });

  btnClear.addEventListener("click", async () => {
    try {
      const period = periodSel.value;
      const count = await invokeCmd("history_clear_period", { period });
      await refreshHistory(searchInp.value.trim());
      showToast(`Removed ${count} history entries`);
    } catch (e) {
      showToast(String(e), "error");
    }
  });
}

export async function refreshHistory(query = "") {
  store.history = await invokeCmd("history_list", {
    query: query || null,
    limit: 500,
  });
  renderHistoryPanel();
}

export function renderHistoryPanel() {
  const container = document.getElementById("history-list-container");
  if (!container) return;

  const historyEnabled = store.settings?.browsing_history_enabled !== false;
  const disabledNotice = !historyEnabled
    ? `<div style="margin-bottom:12px;padding:10px 14px;border-radius:6px;background:rgba(245,158,11,0.14);border:1px solid rgba(245,158,11,0.4);color:#fcd34d;">
        Browsing History recording is currently disabled in Settings → Privacy.
       </div>`
    : "";

  const items = store.history || [];
  if (items.length === 0) {
    container.innerHTML = `
      ${disabledNotice}
      <div class="empty-state">
        <strong>No Browsing History Recorded</strong>
        <p style="margin:6px 0 0;">Visited webpages appear here automatically when browsing history is enabled.</p>
      </div>
    `;
    return;
  }

  container.innerHTML = `
    ${disabledNotice}
    <div class="data-list">
      ${items
        .map(
          (h) => `
          <div class="data-row" data-hist-id="${h.id}">
            <div class="data-row-main">
              <div class="data-row-title">${escapeHtml(h.title)}</div>
              <div class="data-row-url">${escapeHtml(h.url)} • ${escapeHtml(formatTimestamp(h.visited_at))}</div>
            </div>
            <div class="panel-toolbar">
              <button class="btn btn-primary btn-sm" data-ha="open">Open</button>
              <button class="btn btn-sm" data-ha="newtab">New Tab</button>
              <button class="btn btn-danger btn-sm" data-ha="del">Delete</button>
            </div>
          </div>
        `
        )
        .join("")}
    </div>
  `;

  container.querySelectorAll("[data-hist-id]").forEach((row) => {
    const id = Number(row.getAttribute("data-hist-id"));
    const item = items.find((x) => x.id === id);
    if (!item) return;

    row.querySelectorAll("[data-ha]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const act = btn.getAttribute("data-ha");
        if (act === "open") {
          switchViewFn("browser");
          await invokeCmd("tab_navigate", { tabId: null, input: item.url });
        } else if (act === "newtab") {
          switchViewFn("browser");
          await invokeCmd("tab_create", { url: item.url, active: true });
        } else if (act === "del") {
          await invokeCmd("history_delete", { id: item.id });
          await refreshHistory(document.getElementById("history-search-input")?.value || "");
        }
      });
    });
  });
}
