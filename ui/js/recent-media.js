import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import {
  copyTextToClipboard,
  escapeHtml,
  formatTimeMs,
  formatTimestamp,
  showToast,
} from "./utils.js";

let recentFilter = "";
let recentViewMode = null; // "grid" | "list"
let mediaHandlers = null;

function getEffectiveRecentViewMode() {
  if (recentViewMode === "grid" || recentViewMode === "list") {
    return recentViewMode;
  }
  const saved = store.settings?.recent_media_view_mode;
  return saved === "list" ? "list" : "grid";
}

function syncViewModeToggleButtons(mode) {
  const btnGrid = document.getElementById("btn-recent-view-grid");
  const btnList = document.getElementById("btn-recent-view-list");
  if (btnGrid) btnGrid.classList.toggle("active", mode === "grid");
  if (btnList) btnList.classList.toggle("active", mode === "list");
}

async function setRecentViewMode(mode) {
  const normalized = mode === "list" ? "list" : "grid";
  recentViewMode = normalized;
  if (store.settings) {
    store.settings.recent_media_view_mode = normalized;
  }
  syncViewModeToggleButtons(normalized);
  renderRecentMediaPanel();

  try {
    const updated = await invokeCmd("settings_update_key", {
      key: "recent_media_view_mode",
      value: normalized,
    });
    if (updated && typeof updated === "object") {
      store.settings = updated;
    }
  } catch (_) {}
}

export function initRecentMediaModule(handlers) {
  mediaHandlers = handlers;

  const btnGrid = document.getElementById("btn-recent-view-grid");
  const btnList = document.getElementById("btn-recent-view-list");

  btnGrid?.addEventListener("click", () => setRecentViewMode("grid"));
  btnList?.addEventListener("click", () => setRecentViewMode("list"));

  document.getElementById("recent-search-input")?.addEventListener("input", (ev) => {
    recentFilter = ev.target.value.trim().toLowerCase();
    renderRecentMediaPanel();
  });

  document.getElementById("btn-clear-recent-media")?.addEventListener("click", async () => {
    try {
      await invokeCmd("recent_media_clear");
      store.recentMedia = [];
      renderRecentMediaPanel();
      showToast("Cleared Recent Media (Bookmarks were untouched)", "info");
    } catch (e) {
      showToast(String(e), "error");
    }
  });
}

export async function refreshRecentMedia() {
  try {
    const rows = await invokeCmd("recent_media_list", { limit: 60 });
    store.recentMedia = Array.isArray(rows) ? rows : [];
  } catch (e) {
    showToast(String(e), "error");
  }
  renderRecentMediaPanel();
}

function getItemUrl(m) {
  return String(m?.original_url || m?.url || "");
}

function getItemPositionMs(m) {
  return Number(m?.last_position_ms ?? m?.position_ms ?? 0) || 0;
}

function computeItemDisplayMeta(m) {
  const mediaUrl = getItemUrl(m);
  const posMs = getItemPositionMs(m);
  const durMs = Number(m?.duration_ms ?? 0) || 0;
  const pct = durMs > 0 ? Math.min(100, Math.round((posMs / durMs) * 100)) : 0;
  const resumable = posMs > 5000 && !m?.completed;

  const cleanPath = mediaUrl.split(/[?#]/)[0] || "";
  const lastSeg = cleanPath.split("/").pop() || "";
  const rawExt = lastSeg.includes(".") ? lastSeg.split(".").pop() || "" : "";
  const extBadge = (rawExt && rawExt.length <= 6 ? rawExt : "MEDIA").slice(0, 4).toUpperCase();

  const statusLabel = resumable
    ? `Resume at ${formatTimeMs(posMs)}${durMs > 0 ? " / " + formatTimeMs(durMs) : ""}`
    : m?.completed
    ? "Completed"
    : m?.playback_method === "external"
    ? "Played Externally"
    : "Played";

  return { mediaUrl, posMs, durMs, pct, resumable, extBadge, statusLabel };
}

export function renderRecentMediaPanel() {
  const container = document.getElementById("recent-media-container");
  if (!container) return;

  const mode = getEffectiveRecentViewMode();
  syncViewModeToggleButtons(mode);
  container.className = mode === "list" ? "recent-media-list" : "media-grid";

  const items = (store.recentMedia || []).filter((m) => {
    if (!m) return false;
    if (!recentFilter) return true;
    const title = String(m.title || "").toLowerCase();
    const mediaUrl = getItemUrl(m).toLowerCase();
    return title.includes(recentFilter) || mediaUrl.includes(recentFilter);
  });

  if (items.length === 0) {
    container.className = "media-grid";
    container.innerHTML = `<div class="empty-state">No recent media streams recorded yet.</div>`;
    return;
  }

  if (mode === "list") {
    container.innerHTML = items
      .map((m) => {
        const { mediaUrl, pct, resumable, extBadge, statusLabel } = computeItemDisplayMeta(m);
        const title = m.title || mediaUrl || "Untitled Media";
        return `
          <div class="recent-list-row" data-rm-id="${m.id}">
            <div class="recent-list-thumb">${escapeHtml(extBadge)}</div>
            <div class="recent-list-main">
              <div class="recent-list-title-line">
                <span class="recent-list-title" title="${escapeHtml(title)}">${escapeHtml(title)}</span>
                <span class="badge">${escapeHtml(statusLabel)}</span>
              </div>
              <div class="recent-list-meta">
                <span>${escapeHtml(formatTimestamp(m.last_played_at))}</span>
                <span>•</span>
                <span class="recent-list-url" title="${escapeHtml(mediaUrl)}">${escapeHtml(mediaUrl)}</span>
                ${
                  pct > 0
                    ? `<div class="recent-list-progress" title="${pct}% watched"><div class="progress-bar-fill" style="width:${pct}%;"></div></div>`
                    : ""
                }
              </div>
            </div>
            <div class="recent-list-actions">
              <button class="btn btn-primary btn-sm" data-rm-act="play" data-rm-id="${m.id}">
                ▶ ${resumable ? "Resume" : "Play"}
              </button>
              ${
                resumable
                  ? `<button class="btn btn-sm" data-rm-act="restart" data-rm-id="${m.id}" title="Restart from Beginning">↺</button>`
                  : ""
              }
              <button class="btn btn-sm" data-rm-act="external" data-rm-id="${m.id}" title="Open in External Player">↗ External</button>
              <button class="btn btn-sm" data-rm-act="copy" data-rm-id="${m.id}" title="Copy Media URL">⎘ Copy</button>
              <button class="btn btn-sm" data-rm-act="remove" data-rm-id="${m.id}" title="Remove from Recent Media">✕</button>
            </div>
          </div>
        `;
      })
      .join("");
  } else {
    container.innerHTML = items
      .map((m) => {
        const { mediaUrl, pct, resumable, extBadge, statusLabel } = computeItemDisplayMeta(m);
        const title = m.title || mediaUrl || "Untitled Media";
        return `
          <div class="media-card">
            <div class="media-card-top">
              <div class="media-thumb">${escapeHtml(extBadge)}</div>
              <div style="min-width:0;flex:1;">
                <div class="media-card-title" title="${escapeHtml(title)}">${escapeHtml(title)}</div>
                <div class="media-card-sub" title="${escapeHtml(mediaUrl)}">${escapeHtml(mediaUrl)}</div>
              </div>
            </div>

            ${
              pct > 0
                ? `<div class="progress-bar-bg"><div class="progress-bar-fill" style="width:${pct}%;"></div></div>`
                : ""
            }

            <div style="display:flex;align-items:center;justify-content:space-between;font-size:11.5px;color:var(--text-secondary);">
              <span class="badge">${escapeHtml(statusLabel)}</span>
              <span>${escapeHtml(formatTimestamp(m.last_played_at))}</span>
            </div>

            <div class="media-card-actions">
              <button class="btn btn-primary btn-sm" data-rm-act="play" data-rm-id="${m.id}">
                ▶ ${resumable ? "Resume" : "Play"}
              </button>
              ${
                resumable
                  ? `<button class="btn btn-sm" data-rm-act="restart" data-rm-id="${m.id}">↺ Restart</button>`
                  : ""
              }
              <button class="btn btn-sm" data-rm-act="external" data-rm-id="${m.id}">↗ External</button>
              <button class="btn btn-sm" data-rm-act="copy" data-rm-id="${m.id}" title="Copy Media URL">⎘ Copy</button>
              <button class="btn btn-sm" style="margin-left:auto;" data-rm-act="remove" data-rm-id="${m.id}" title="Remove from Recent Media">✕</button>
            </div>
          </div>
        `;
      })
      .join("");
  }

  container.querySelectorAll("[data-rm-act]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const act = btn.getAttribute("data-rm-act");
      const id = Number(btn.getAttribute("data-rm-id"));
      const item = (store.recentMedia || []).find((x) => x.id === id);
      if (!item) return;
      const itemUrl = getItemUrl(item);

      try {
        if (act === "play" && itemUrl) {
          mediaHandlers?.playInStreamVault(itemUrl, item.title, null);
        } else if (act === "restart" && itemUrl) {
          await invokeCmd("playback_position_reset", { url: itemUrl });
          mediaHandlers?.playInStreamVault(itemUrl, item.title, 0);
        } else if (act === "external" && itemUrl) {
          const cleanExtTarget = itemUrl.startsWith("local://")
            ? itemUrl.slice("local://".length)
            : itemUrl;
          mediaHandlers?.openInExternalPlayer(cleanExtTarget, item.title);
        } else if (act === "copy" && itemUrl) {
          const cleanCopy = itemUrl.startsWith("local://")
            ? itemUrl.slice("local://".length)
            : itemUrl;
          await copyTextToClipboard(cleanCopy);
        } else if (act === "remove") {
          await invokeCmd("recent_media_remove", { id });
          await refreshRecentMedia();
        }
      } catch (e) {
        showToast(String(e), "error");
      }
    });
  });
}
