import { initBookmarksModule, refreshBookmarks, renderBookmarksPanel } from "./bookmarks.js";
import {
  ensureBrowserTabsInitialized,
  initBrowserModule,
  renderActiveMediaToolbarPill,
  renderBrowserChrome,
  showMediaDetectedBanner,
  syncBrowserWebviewViewport,
} from "./browser.js";
import {
  initDownloadsModule,
  renderDownloadsPanel,
  triggerPermanentDownload,
  updateSidebarDownloadBadge,
} from "./downloads.js";
import { initHistoryModule, refreshHistory } from "./history.js";
import { initHomeModule, renderHomePanel } from "./home.js";
import { invokeCmd, listenEvent } from "./ipc.js";
import {
  applyFullscreenDomState,
  handlePlayerKeyShortcut,
  initPlayerModule,
  initiateInAppPlayback,
  promptDirectMediaUrlModal,
  renderStreamCacheTelemetry,
  syncPlayerSurfaceViewport,
  updatePlayerState,
  wakeFullscreenHud,
} from "./player.js";
import {
  initRecentMediaModule,
  refreshRecentMedia,
  renderRecentMediaPanel,
} from "./recent-media.js";
import { initSettingsModule, renderSettingsPanel } from "./settings.js";
import { store } from "./state.js";
import {
  closeModal,
  escapeHtml,
  formatCompactNetRate,
  openModal,
  showToast,
} from "./utils.js";

function applyTheme(theme) {
  document.documentElement.setAttribute("data-theme", theme || "dark");
}

export function switchView(viewName) {
  store.currentView = viewName;
  document.querySelectorAll("#sidebar .nav-item").forEach((btn) => {
    btn.classList.toggle("active", btn.getAttribute("data-view") === viewName);
  });

  const views = [
    "home",
    "browser",
    "player",
    "recent",
    "bookmarks",
    "history",
    "downloads",
    "settings",
  ];
  views.forEach((v) => {
    const el = document.getElementById(`view-${v}`);
    if (el) el.classList.toggle("hidden", v !== viewName);
  });

  if (viewName === "home") renderHomePanel();
  else if (viewName === "browser") {
    renderBrowserChrome();
    ensureBrowserTabsInitialized();
  } else if (viewName === "recent") refreshRecentMedia();
  else if (viewName === "bookmarks") refreshBookmarks();
  else if (viewName === "history") refreshHistory();
  else if (viewName === "downloads") renderDownloadsPanel();
  else if (viewName === "settings") renderSettingsPanel();

  syncBrowserWebviewViewport();
  syncPlayerSurfaceViewport();
  requestAnimationFrame(() => {
    syncBrowserWebviewViewport();
    syncPlayerSurfaceViewport();
  });
}

async function openInExternalPlayerFlow(url, title = null) {
  try {
    const res = await invokeCmd("open_media_external", {
      url,
      executablePath: null,
      customTitle: title,
    });

    if (res && res.needs_player_selection) {
      const players = res.players || [];
      openModal(
        `
          <h3>Open with External Media Player</h3>
          <p style="margin:0;font-size:12px;color:var(--text-secondary);word-break:break-all;">
            ${escapeHtml(title || url)}
          </p>
          <div style="display:flex;flex-direction:column;gap:7px;max-height:240px;overflow-y:auto;">
            ${
              players.length === 0
                ? `<div style="padding:10px;color:var(--text-muted);font-size:12px;">
                    No external players were automatically detected yet. Click "Browse .exe..." below to pick PotPlayer, VLC, MPC-HC, or another player.
                   </div>`
                : players
                    .map(
                      (p) => `
                  <button class="btn" style="justify-content:space-between;" data-pick-exe="${escapeHtml(p.executable_path)}">
                    <span>▶ ${escapeHtml(p.name)}</span>
                    <span style="font-size:11px;color:var(--text-muted);">${escapeHtml(p.source)}</span>
                  </button>
                `
                    )
                    .join("")
            }
          </div>
          <label style="display:flex;align-items:center;gap:8px;font-size:12px;cursor:pointer;">
            <input id="ext-make-default" type="checkbox" />
            <span>Remember as Default External Player</span>
          </label>
          <div class="modal-actions">
            <button id="ext-browse-custom" class="btn" style="margin-right:auto;">Browse .exe...</button>
            <button id="ext-cancel" class="btn">Cancel</button>
          </div>
        `,
        (card) => {
          card.querySelector("#ext-cancel").onclick = closeModal;
          card.querySelector("#ext-browse-custom").onclick = async () => {
            const picked = await invokeCmd("external_player_browse_exe");
            if (!picked) return;
            await invokeCmd("external_player_add_custom", {
              name: "",
              executablePath: picked,
            });
            if (card.querySelector("#ext-make-default").checked) {
              store.settings = await invokeCmd("settings_update_key", {
                key: "default_external_player",
                value: picked,
              });
            }
            closeModal();
            await invokeCmd("open_media_external", {
              url,
              executablePath: picked,
              customTitle: title,
            });
            refreshRecentMedia().catch(() => {});
            showToast("Launched media in external player", "success");
          };

          card.querySelectorAll("[data-pick-exe]").forEach((btn) => {
            btn.onclick = async () => {
              const exe = btn.getAttribute("data-pick-exe");
              if (card.querySelector("#ext-make-default").checked) {
                store.settings = await invokeCmd("settings_update_key", {
                  key: "default_external_player",
                  value: exe,
                });
              }
              closeModal();
              await invokeCmd("open_media_external", {
                url,
                executablePath: exe,
                customTitle: title,
              });
              refreshRecentMedia().catch(() => {});
              showToast("Launched media in external player", "success");
            };
          });
        }
      );
      return;
    }

    refreshRecentMedia().catch(() => {});
    showToast("Opened media in external player", "success");
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function bootstrapApplication() {
  // Wire modal open/close events so child WebView2 & libmpv HWNDs never occlude HTML modals!
  document.addEventListener("sv:modal-visibility", (ev) => {
    store.modalOpen = Boolean(ev.detail?.open);
    syncBrowserWebviewViewport();
    syncPlayerSurfaceViewport();
  });

  // Sidebar collapse & navigation
  const sidebar = document.getElementById("sidebar");
  document.getElementById("btn-toggle-sidebar").addEventListener("click", async () => {
    const collapsed = sidebar.classList.toggle("collapsed");
    syncBrowserWebviewViewport();
    syncPlayerSurfaceViewport();
    try {
      store.settings = await invokeCmd("settings_update_key", {
        key: "sidebar_collapsed",
        value: collapsed,
      });
    } catch (_) {}
  });

  document.querySelectorAll("#sidebar .nav-item").forEach((btn) => {
    btn.addEventListener("click", () => {
      switchView(btn.getAttribute("data-view"));
    });
  });

  const mediaActions = {
    playInStreamVault: (url, title = null, posMs = null) =>
      initiateInAppPlayback(url, title, posMs),
    openInExternalPlayer: (url, title = null) => openInExternalPlayerFlow(url, title),
    downloadMediaPermanent: (url) => triggerPermanentDownload(url, false),
    promptDirectMediaUrlModal,
  };

  initHomeModule({
    onSwitchView: switchView,
    onPlayMedia: mediaActions.playInStreamVault,
  });
  initBrowserModule({
    onSwitchView: switchView,
    onMediaActions: mediaActions,
  });
  initPlayerModule({
    onSwitchView: switchView,
    onDownloadMedia: mediaActions.downloadMediaPermanent,
    onOpenExternal: mediaActions.openInExternalPlayer,
  });
  initDownloadsModule();
  initRecentMediaModule(mediaActions);
  initBookmarksModule({ onSwitchView: switchView });
  initHistoryModule({ onSwitchView: switchView });
  initSettingsModule({ onApplyTheme: applyTheme });

  // Global keyboard shortcuts
  window.addEventListener("keydown", (ev) => {
    const tag = (ev.target?.tagName || "").toUpperCase();
    const inInput = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";

    if (ev.ctrlKey && ev.code === "KeyT") {
      ev.preventDefault();
      switchView("browser");
      invokeCmd("tab_create", { url: null, active: true }).catch(() => {});
      return;
    }
    if (ev.ctrlKey && ev.code === "KeyL") {
      ev.preventDefault();
      switchView("browser");
      const inp = document.getElementById("address-input");
      inp?.focus();
      inp?.select();
      return;
    }
    if (!inInput && !store.modalOpen) {
      if (handlePlayerKeyShortcut(ev.code)) {
        ev.preventDefault();
      }
    }
  });

  // Subscribe to real-time backend Tauri events
  await listenEvent("sv://tabs-updated", (tabs) => {
    store.tabs = tabs || [];
    renderBrowserChrome();
  });

  await listenEvent("sv://media-detected", (payload) => {
    showMediaDetectedBanner(payload);
  });

  await listenEvent("sv://non-media-download-requested", (payload) => {
    if (payload?.url) {
      triggerPermanentDownload(payload.url, true);
    }
  });

  await listenEvent("sv://context-action", (payload) => {
    if (!payload) return;
    if (payload.action === "play_in_app") {
      initiateInAppPlayback(payload.url, payload.title || null, null);
    } else if (payload.action === "play_external") {
      openInExternalPlayerFlow(payload.url, payload.title || null);
    } else if (payload.action === "download") {
      triggerPermanentDownload(payload.url, false);
    }
  });

  await listenEvent("sv://active-media-changed", (hl) => {
    store.activeHighlight = hl || {};
    renderActiveMediaToolbarPill();
  });

  await listenEvent("sv://player-state", (snap) => {
    updatePlayerState(snap);
  });

  await listenEvent("sv://player-ended", () => {
    refreshRecentMedia().catch(() => {});
    showToast("Media playback finished", "info");
  });

  await listenEvent("sv://stream-telemetry", (payload) => {
    if (payload?.cache !== undefined) {
      store.streamCacheStatus = payload.cache;
      renderStreamCacheTelemetry();
    }
    if (payload?.player) {
      updatePlayerState(payload.player);
    }
  });

  await listenEvent("sv://net-speed", (snap) => {
    if (document.body.classList.contains("player-fullscreen")) {
      return;
    }
    const downBps = Number(snap?.down_bps) || 0;
    const upBps = Number(snap?.up_bps) || 0;
    const downEl = document.getElementById("net-speed-down");
    const upEl = document.getElementById("net-speed-up");
    const box = document.getElementById("sidebar-net-monitor");
    const downTxt = formatCompactNetRate(downBps);
    const upTxt = formatCompactNetRate(upBps);
    if (downEl) downEl.textContent = downTxt;
    if (upEl) upEl.textContent = upTxt;
    if (box) {
      box.setAttribute("data-active", downBps > 0 || upBps > 0 ? "true" : "false");
      box.title = `Live Network Throughput — Down: ${downTxt} | Up: ${upTxt}`;
    }
  });

  await listenEvent("sv://downloads-updated", (list) => {
    store.downloads = list || [];
    updateSidebarDownloadBadge();
    if (store.currentView === "downloads") {
      renderDownloadsPanel();
    }
  });

  await listenEvent("sv://fullscreen-changed", (isFs) => {
    applyFullscreenDomState(Boolean(isFs));
  });

  await listenEvent("sv://player-mouse-wake", () => {
    wakeFullscreenHud();
  });

  await listenEvent("sv://player-surface-key", (vkCode) => {
    // Map Win32 VK codes forwarded from embedded libmpv video HWND
    const vkMap = {
      0x20: "Space",
      0x25: "ArrowLeft",
      0x27: "ArrowRight",
      0x26: "ArrowUp",
      0x28: "ArrowDown",
      0x4d: "KeyM",
      0x46: "KeyF",
      0x1b: "Escape",
      0xdb: "BracketLeft",
      0xdd: "BracketRight",
    };
    const mapped = vkMap[vkCode];
    if (mapped) handlePlayerKeyShortcut(mapped);
  });

  await listenEvent("sv://toast", (t) => {
    if (t?.message) showToast(t.message, t.kind || "info");
  });

  // Load initial state from Rust portable backend
  try {
    const boot = await invokeCmd("app_bootstrap_state");
    store.settings = boot.settings;
    store.dependencies = boot.dependencies;
    store.tabs = boot.tabs || [];
    store.bookmarks = boot.bookmarks || { folders: [], bookmarks: [] };
    store.recentMedia = boot.recent_media || [];
    store.downloads = boot.downloads || [];
    store.externalPlayers = boot.external_players || [];
    store.activeHighlight = boot.active_highlight || {};

    applyTheme(store.settings.theme);
    if (store.settings.sidebar_collapsed) {
      sidebar.classList.add("collapsed");
    }

    if (boot.recovered_corrupt_db) {
      showToast(
        "Recovered portable SQLite database automatically from backup after corruption was detected.",
        "error"
      );
    }

    updateSidebarDownloadBadge();
    try {
      renderBookmarksPanel();
    } catch (_) {}
    try {
      renderRecentMediaPanel();
    } catch (_) {}

    const startView =
      store.settings.startup_behavior === "restore_session" ||
      store.settings.startup_behavior === "homepage"
        ? "browser"
        : "home";
    switchView(startView);

    // Warm up the first browser tab asynchronously off the Win32 IPC thread so
    // Settings, Bookmarks, and Home remain 100% instant and Browser is ready immediately.
    if (startView !== "browser") {
      setTimeout(() => {
        ensureBrowserTabsInitialized().catch(() => {});
      }, 60);
    }
  } catch (err) {
    showToast(`Initialization error: ${err}`, "error");
    switchView("home");
  }
}

window.addEventListener("DOMContentLoaded", bootstrapApplication);
