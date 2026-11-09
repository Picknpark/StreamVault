import { openBookmarkEditorModal, refreshBookmarks, urlsEquivalent } from "./bookmarks.js";
import { updateSidebarDownloadBadge } from "./downloads.js";
import { invokeCmd } from "./ipc.js";
import { setCustomPlaylistAndPlay } from "./player.js";
import { store } from "./state.js";
import {
  closeModal,
  copyTextToClipboard,
  escapeHtml,
  formatBytes,
  openModal,
  showToast,
} from "./utils.js";

let switchViewCallback = null;
let mediaActionHandlers = null;
let addressInputFocused = false;
let suggestionDebounce = null;
let findInputDebounce = null;
let browserInitPromise = null;

export async function ensureBrowserTabsInitialized() {
  if (store.tabs && store.tabs.length > 0) {
    syncBrowserWebviewViewport();
    return store.tabs;
  }
  if (browserInitPromise) return browserInitPromise;

  const slot = document.getElementById("browser-webview-Slot");
  if (slot && store.tabs.length === 0) {
    slot.innerHTML = `<div class="webview-placeholder-note">Initializing Microsoft Edge WebView2 browser tab...</div>`;
  }

  browserInitPromise = (async () => {
    try {
      const tabs = await invokeCmd("tab_ensure_initialized");
      if (Array.isArray(tabs)) {
        store.tabs = tabs;
      }
      if (slot && store.tabs.length > 0) {
        slot.innerHTML = `<div class="webview-placeholder-note">WebView2 Browser Viewport</div>`;
      }
      renderBrowserChrome();
      syncBrowserWebviewViewport();
      requestAnimationFrame(() => syncBrowserWebviewViewport());
      return store.tabs;
    } catch (err) {
      if (slot) {
        slot.innerHTML = `
          <div style="max-width:460px;padding:18px;border-radius:8px;background:var(--bg-surface);border:1px solid var(--border-strong);text-align:center;">
            <strong style="display:block;margin-bottom:6px;color:var(--text-primary);">Browser Tab Initialization Error</strong>
            <div style="font-size:12px;color:var(--text-secondary);margin-bottom:12px;word-break:break-word;">${escapeHtml(String(err))}</div>
            <button id="btn-retry-browser-init" class="btn btn-primary btn-sm">Retry Browser Initialization</button>
          </div>
        `;
        slot.querySelector("#btn-retry-browser-init")?.addEventListener("click", () => {
          ensureBrowserTabsInitialized();
        });
      }
      showToast(`Browser error: ${err}`, "error");
      return [];
    } finally {
      browserInitPromise = null;
    }
  })();

  return browserInitPromise;
}

export function initBrowserModule({ onSwitchView, onMediaActions }) {
  switchViewCallback = onSwitchView;
  mediaActionHandlers = onMediaActions;

  const btnNewTab = document.getElementById("btn-new-tab");
  const btnBack = document.getElementById("btn-nav-back");
  const btnFwd = document.getElementById("btn-nav-forward");
  const btnReload = document.getElementById("btn-nav-reload");
  const btnHome = document.getElementById("btn-nav-home");
  const addrForm = document.getElementById("address-bar-form");
  const addrInput = document.getElementById("address-input");
  const suggestionsBox = document.getElementById("address-suggestions");
  const btnStar = document.getElementById("btn-bookmark-star");
  const btnInspectMedia = document.getElementById("btn-inspect-url-media");
  const btnFind = document.getElementById("btn-browser-find");
  const btnGrabMedia = document.getElementById("btn-browser-grab-media");
  const btnMenu = document.getElementById("btn-browser-menu");
  const activeMediaPill = document.getElementById("toolbar-active-media-pill");
  const findInput = document.getElementById("browser-find-input");
  const btnFindPrev = document.getElementById("btn-find-prev");
  const btnFindNext = document.getElementById("btn-find-next");
  const btnFindClose = document.getElementById("btn-find-close");

  btnNewTab.addEventListener("click", async () => {
    try {
      const created = await invokeCmd("tab_create", { url: null, active: true });
      if (created && !store.tabs.some((t) => t.id === created.id)) {
        store.tabs = [...store.tabs.map((t) => ({ ...t, is_active: false })), created];
      }
      renderBrowserChrome();
      syncBrowserWebviewViewport();
      if (created?.url === "about:blank") {
        addrInput?.focus();
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnBack.addEventListener("click", () => invokeCmd("tab_go_back", { tabId: null }).catch(() => {}));
  btnFwd.addEventListener("click", () => invokeCmd("tab_go_forward", { tabId: null }).catch(() => {}));
  btnReload.addEventListener("click", () => invokeCmd("tab_reload", { tabId: null }).catch(() => {}));
  btnHome.addEventListener("click", async () => {
    try {
      await invokeCmd("tab_go_home", { tabId: null });
      syncBrowserWebviewViewport();
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  addrInput.addEventListener("focus", () => {
    addressInputFocused = true;
    addrInput.select();
  });

  addrInput.addEventListener("blur", () => {
    addressInputFocused = false;
    setTimeout(() => hideSuggestions(), 180);
  });

  addrInput.addEventListener("input", () => {
    clearTimeout(suggestionDebounce);
    const val = addrInput.value.trim();
    if (!val) {
      hideSuggestions();
      return;
    }
    suggestionDebounce = setTimeout(async () => {
      try {
        const list = await invokeCmd("suggest_address_bar", { query: val });
        renderSuggestions(list);
      } catch (_) {}
    }, 110);
  });

  addrForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    hideSuggestions();
    const input = addrInput.value.trim();
    if (!input) return;
    addrInput.blur();
    try {
      const updated = await invokeCmd("tab_navigate", { tabId: null, input });
      if (updated && !store.tabs.some((t) => t.id === updated.id)) {
        store.tabs = [...store.tabs.map((t) => ({ ...t, is_active: false })), updated];
      }
      renderBrowserChrome();
      syncBrowserWebviewViewport();
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnStar.addEventListener("click", () => openBookmarkCurrentTabModal());

  btnInspectMedia.addEventListener("click", async () => {
    const currentUrl = addrInput.value.trim() || getActiveTab()?.url;
    if (!currentUrl) return;
    try {
      const det = await invokeCmd("probe_media_link", { url: currentUrl });
      showMediaDetectedBanner({
        tab_id: getActiveTab()?.id || "manual",
        detection: { ...det, is_media: true },
        preferred_method: "ask",
      });
    } catch (e) {
      showToast(`Media inspection error: ${e}`, "error");
    }
  });

  btnMenu.addEventListener("click", () => openBrowserQuickMenu());

  btnFind?.addEventListener("click", () => {
    if (store.findBarOpen) {
      closeBrowserFindBar();
    } else {
      openBrowserFindBar();
    }
  });

  btnGrabMedia?.addEventListener("click", () => {
    openBatchMediaGrabberModal();
  });

  findInput?.addEventListener("input", () => {
    clearTimeout(findInputDebounce);
    findInputDebounce = setTimeout(() => {
      runBrowserFind({ forward: true, stayOnCurrent: true });
    }, 35);
  });

  findInput?.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      clearTimeout(findInputDebounce);
      runBrowserFind({ forward: !ev.shiftKey, stayOnCurrent: false });
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      closeBrowserFindBar();
    }
  });

  btnFindPrev?.addEventListener("click", () => {
    runBrowserFind({ forward: false, stayOnCurrent: false });
    findInput?.focus();
  });

  btnFindNext?.addEventListener("click", () => {
    runBrowserFind({ forward: true, stayOnCurrent: false });
    findInput?.focus();
  });

  btnFindClose?.addEventListener("click", () => {
    closeBrowserFindBar();
  });

  activeMediaPill.addEventListener("click", async () => {
    const mode = store.activeHighlight?.mode;
    if (mode === "in_app" || mode === "paused") {
      switchViewCallback("player");
    } else if (store.activeHighlight?.original_url) {
      const found = await invokeCmd("tab_scroll_to_active_media", { tabId: null }).catch(
        () => false
      );
      showToast(
        found
          ? `Located active external media: ${store.activeHighlight.title || store.activeHighlight.original_url}`
          : `Playing externally: ${store.activeHighlight.title || store.activeHighlight.original_url}`
      );
    }
  });

  activeMediaPill.addEventListener("contextmenu", async (ev) => {
    ev.preventDefault();
    if (!store.activeHighlight?.original_url) return;
    const found = await invokeCmd("tab_scroll_to_active_media", { tabId: null }).catch(
      () => false
    );
    showToast(
      found
        ? "Scrolled to highlighted media link on current tab"
        : "Active media link is not on this webpage",
      found ? "info" : "info"
    );
  });

  // Detected Media Banner buttons
  document.getElementById("btn-detected-play-app").addEventListener("click", () => {
    const det = store.detectedMediaBanner;
    if (!det) return;
    dismissMediaDetectedBanner();
    mediaActionHandlers.playInStreamVault(det.original_url, det.suggested_filename);
  });

  document.getElementById("btn-detected-play-ext").addEventListener("click", () => {
    const det = store.detectedMediaBanner;
    if (!det) return;
    dismissMediaDetectedBanner();
    mediaActionHandlers.openInExternalPlayer(det.original_url, det.suggested_filename);
  });

  document.getElementById("btn-detected-download").addEventListener("click", () => {
    const det = store.detectedMediaBanner;
    if (!det) return;
    dismissMediaDetectedBanner();
    mediaActionHandlers.downloadMediaPermanent(det.original_url);
  });

  document.getElementById("btn-detected-copy").addEventListener("click", () => {
    const det = store.detectedMediaBanner;
    if (!det) return;
    copyTextToClipboard(det.original_url);
  });

  document.getElementById("btn-detected-dismiss").addEventListener("click", () => {
    dismissMediaDetectedBanner();
  });

  // Observe Browser WebView2 slot geometry changes
  const slot = document.getElementById("browser-webview-Slot");
  if (slot && typeof ResizeObserver !== "undefined") {
    const ro = new ResizeObserver(() => syncBrowserWebviewViewport());
    ro.observe(slot);
  }
  window.addEventListener("resize", () => syncBrowserWebviewViewport());
}

export function getActiveTab() {
  return store.tabs.find((t) => t.is_active) || store.tabs[0] || null;
}

export function renderBrowserChrome() {
  const tabsContainer = document.getElementById("browser-tabs-list");
  const navTabBadge = document.getElementById("nav-tab-badge");
  if (navTabBadge) {
    navTabBadge.textContent = String(store.tabs.length || 0);
    navTabBadge.classList.toggle("hidden", !store.tabs.length);
  }

  tabsContainer.innerHTML = store.tabs
    .map(
      (t) => `
      <div class="browser-tab ${t.is_active ? "active" : ""}" data-tab-id="${escapeHtml(t.id)}" title="${escapeHtml(t.title)} — ${escapeHtml(t.url)}">
        ${t.is_loading ? '<span class="tab-spinner"></span>' : '<span style="font-size:12px;">🌐</span>'}
        <span class="tab-title">${escapeHtml(t.title || "New Tab")}</span>
        <button class="tab-close-btn" data-close-tab="${escapeHtml(t.id)}" title="Close Tab">✕</button>
      </div>
    `
    )
    .join("");

  tabsContainer.querySelectorAll(".browser-tab").forEach((el) => {
    const tid = el.getAttribute("data-tab-id");
    el.addEventListener("click", (ev) => {
      if (ev.target.closest("[data-close-tab]")) return;
      invokeCmd("tab_switch", { tabId: tid }).catch(() => {});
    });
    el.addEventListener("auxclick", (ev) => {
      if (ev.button === 1) {
        ev.preventDefault();
        invokeCmd("tab_close", { tabId: tid }).catch(() => {});
      }
    });
  });

  tabsContainer.querySelectorAll("[data-close-tab]").forEach((btn) => {
    btn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      const tid = btn.getAttribute("data-close-tab");
      invokeCmd("tab_close", { tabId: tid }).catch(() => {});
    });
  });

  const active = getActiveTab();
  const addrInput = document.getElementById("address-input");
  const protoBadge = document.getElementById("address-protocol-badge");
  const loadingLine = document.getElementById("browser-loading-line");
  const btnBack = document.getElementById("btn-nav-back");
  const btnFwd = document.getElementById("btn-nav-forward");
  const btnStar = document.getElementById("btn-bookmark-star");

  if (active) {
    const isBlank = !active.url || active.url === "about:blank";
    if (!addressInputFocused) {
      addrInput.value = isBlank ? "" : active.url;
    }
    const isHttps = !isBlank && (active.url || "").startsWith("https://");
    protoBadge.textContent = isBlank ? "NEW" : isHttps ? "HTTPS" : "HTTP";
    protoBadge.classList.toggle("secure", isHttps);
    loadingLine.classList.toggle("hidden", !active.is_loading || isBlank);
    btnBack.disabled = false;
    btnFwd.disabled = false;

    const isBookmarked =
      !isBlank &&
      (store.bookmarks?.bookmarks || []).some((b) => urlsEquivalent(b.url, active.url));
    btnStar.textContent = isBookmarked ? "★" : "☆";
    btnStar.classList.toggle("bookmarked", isBookmarked);
  } else {
    btnStar.textContent = "☆";
    btnStar.classList.remove("bookmarked");
  }

  renderBookmarkBar();
  renderActiveMediaToolbarPill();
  renderGrabberToolbarBadge();
}

export function renderBookmarkBar() {
  const bar = document.getElementById("browser-bookmark-bar");
  if (!bar) return;
  const showBar = store.settings?.show_bookmark_bar !== false;
  bar.classList.toggle("hidden", !showBar);
  if (!showBar) {
    syncBrowserWebviewViewport();
    return;
  }

  const barBookmarks = (store.bookmarks?.bookmarks || []).filter((b) => b.show_on_bar);
  if (barBookmarks.length === 0) {
    bar.innerHTML = `
      <button type="button" id="bm-bar-quick-add" class="bm-bar-item" title="Bookmark the current webpage or add a URL">
        <span style="color:var(--accent);">＋</span>
        <span>Bookmark Page</span>
      </button>
      <span style="font-size:11.5px;color:var(--text-muted);padding-left:4px;">Pinned bookmarks appear here</span>
    `;
    bar.querySelector("#bm-bar-quick-add")?.addEventListener("click", () => {
      openBookmarkCurrentTabModal();
    });
  } else {
    bar.innerHTML =
      barBookmarks
        .map(
          (b) => `
          <button class="bm-bar-item" data-bm-url="${escapeHtml(b.url)}" data-bm-id="${b.id}" title="${escapeHtml(b.title)}\n${escapeHtml(b.url)}">
            <span style="color:var(--accent);">★</span>
            <span>${escapeHtml(b.title)}</span>
          </button>
        `
        )
        .join("") +
      `
        <button type="button" id="bm-bar-quick-add" class="bm-bar-item" title="Add Bookmark" style="color:var(--text-muted);">
          <span>＋</span>
        </button>
      `;

    bar.querySelector("#bm-bar-quick-add")?.addEventListener("click", () => {
      openBookmarkCurrentTabModal();
    });

    bar.querySelectorAll(".bm-bar-item[data-bm-url]").forEach((btn) => {
      const url = btn.getAttribute("data-bm-url");
      btn.addEventListener("auxclick", (ev) => {
        if (ev.button === 1) {
          ev.preventDefault();
          invokeCmd("tab_create", { url, active: false }).catch((e) =>
            showToast(String(e), "error")
          );
        }
      });
      btn.addEventListener("click", (ev) => {
        if (ev.ctrlKey || ev.metaKey) {
          invokeCmd("tab_create", { url, active: false }).catch((e) =>
            showToast(String(e), "error")
          );
          return;
        }
        invokeCmd("tab_navigate", { tabId: null, input: url }).catch((e) =>
          showToast(String(e), "error")
        );
      });
      btn.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        openBookmarkBarContextModal(Number(btn.getAttribute("data-bm-id")));
      });
    });
  }
  syncBrowserWebviewViewport();
}

export function renderActiveMediaToolbarPill() {
  const pill = document.getElementById("toolbar-active-media-pill");
  if (!pill) return;
  const hl = store.activeHighlight;
  if (hl && hl.original_url) {
    pill.classList.remove("hidden");
    const labelTitle = hl.title || hl.original_url;
    if (hl.mode === "external") {
      pill.textContent = "↗ External Active";
      pill.title = `${labelTitle}\nClick to locate link on page`;
    } else if (hl.mode === "paused") {
      pill.textContent = "⏸ Paused in StreamVault";
      pill.title = `${labelTitle}\nClick to open Player • Right-click to locate link on page`;
    } else {
      pill.textContent = "▶ Playing in StreamVault";
      pill.title = `${labelTitle}\nClick to open Player • Right-click to locate link on page`;
    }
  } else {
    pill.classList.add("hidden");
  }
}

export function showMediaDetectedBanner(payload) {
  const { detection, preferred_method } = payload;
  if (!detection) return;

  // Respect automatic playback method setting if user explicitly configured one
  if (preferred_method === "in_app") {
    mediaActionHandlers.playInStreamVault(detection.original_url, detection.suggested_filename);
    return;
  }
  if (preferred_method === "external") {
    mediaActionHandlers.openInExternalPlayer(detection.original_url, detection.suggested_filename);
    return;
  }

  store.detectedMediaBanner = detection;
  if (store.currentView !== "browser") {
    switchViewCallback("browser");
  }

  const banner = document.getElementById("media-detected-banner");
  const badge = document.getElementById("detected-media-badge");
  const name = document.getElementById("detected-media-name");
  const meta = document.getElementById("detected-media-meta");

  const extLabel = (detection.extension || detection.media_kind || "MEDIA").toUpperCase();
  badge.textContent = extLabel;
  name.textContent = detection.suggested_filename || detection.original_url;

  const metaParts = [];
  if (detection.content_length) metaParts.push(formatBytes(detection.content_length));
  if (detection.mime_type) metaParts.push(detection.mime_type.split(";")[0]);
  metaParts.push(detection.supports_range ? "HTTP Range Ready" : "Progressive Stream");
  meta.textContent = metaParts.join(" • ");

  banner.classList.remove("hidden");
  syncBrowserWebviewViewport();
}

export function dismissMediaDetectedBanner() {
  store.detectedMediaBanner = null;
  document.getElementById("media-detected-banner")?.classList.add("hidden");
  syncBrowserWebviewViewport();
}

function renderSuggestions(list) {
  const box = document.getElementById("address-suggestions");
  if (!list || list.length === 0) {
    hideSuggestions();
    return;
  }
  store.suggestionsOpen = true;
  box.innerHTML = list
    .map(
      (item) => `
      <button type="button" class="suggestion-row" data-sug-url="${escapeHtml(item.url)}">
        <span class="suggestion-tag">${escapeHtml(item.source)}</span>
        <span class="suggestion-title">${escapeHtml(item.title)}</span>
        <span class="suggestion-url">${escapeHtml(item.url)}</span>
      </button>
    `
    )
    .join("");
  box.classList.remove("hidden");
  syncBrowserWebviewViewport();

  box.querySelectorAll(".suggestion-row").forEach((row) => {
    row.addEventListener("mousedown", (ev) => {
      ev.preventDefault();
      const targetUrl = row.getAttribute("data-sug-url");
      document.getElementById("address-input").value = targetUrl;
      hideSuggestions();
      invokeCmd("tab_navigate", { tabId: null, input: targetUrl }).catch((e) =>
        showToast(String(e), "error")
      );
    });
  });
}

function hideSuggestions() {
  const box = document.getElementById("address-suggestions");
  if (!box) return;
  store.suggestionsOpen = false;
  box.classList.add("hidden");
  syncBrowserWebviewViewport();
}

export function syncBrowserWebviewViewport() {
  const slot = document.getElementById("browser-webview-Slot");
  if (!slot) return;
  // Hide native child WebView2 whenever user is on another view OR an HTML modal/suggestions popup is open!
  const shouldShow =
    store.currentView === "browser" && !store.modalOpen && !store.suggestionsOpen;

  if (!shouldShow) {
    invokeCmd("browser_set_viewport", {
      bounds: { visible: false, x: 0, y: 0, width: 0, height: 0 },
    }).catch(() => {});
    return;
  }

  const rect = slot.getBoundingClientRect();
  invokeCmd("browser_set_viewport", {
    bounds: {
      visible: rect.width > 20 && rect.height > 20,
      x: Math.round(rect.left),
      y: Math.round(rect.top),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    },
  }).catch(() => {});
}

export function openBookmarkCurrentTabModal() {
  const active = getActiveTab();
  const typedUrl = document.getElementById("address-input")?.value?.trim() || "";
  const activeUrl = active?.url && active.url !== "about:blank" ? active.url : "";
  const activeTitle =
    active?.title && active.title !== "New Tab" && active.title !== "about:blank"
      ? active.title
      : "";
  const candidateUrl = activeUrl || typedUrl || "https://";
  const candidateTitle = activeTitle || typedUrl || "";

  const existing =
    candidateUrl && candidateUrl !== "https://"
      ? (store.bookmarks?.bookmarks || []).find((b) => urlsEquivalent(b.url, candidateUrl))
      : null;

  openBookmarkEditorModal(existing || null, {
    title: candidateTitle,
    url: candidateUrl,
  });
}

function openBookmarkBarContextModal(bmId) {
  const bm = (store.bookmarks?.bookmarks || []).find((b) => b.id === bmId);
  if (!bm) return;
  openModal(
    `
      <h3>${escapeHtml(bm.title)}</h3>
      <div style="font-size:12px;color:var(--text-secondary);word-break:break-all;">${escapeHtml(bm.url)}</div>
      <div style="display:flex;flex-direction:column;gap:7px;margin-top:4px;">
        <button id="ctx-bm-open" class="btn">Open in Current Tab</button>
        <button id="ctx-bm-newtab" class="btn">Open in New Tab</button>
        <button id="ctx-bm-edit" class="btn">Edit / Move Bookmark...</button>
        <button id="ctx-bm-copy" class="btn">Copy URL</button>
        <button id="ctx-bm-delete" class="btn btn-danger">Delete Bookmark</button>
      </div>
      <div class="modal-actions">
        <button id="ctx-bm-close" class="btn">Close</button>
      </div>
    `,
    (card) => {
      card.querySelector("#ctx-bm-close").onclick = closeModal;
      card.querySelector("#ctx-bm-open").onclick = () => {
        closeModal();
        invokeCmd("tab_navigate", { tabId: null, input: bm.url });
      };
      card.querySelector("#ctx-bm-newtab").onclick = () => {
        closeModal();
        invokeCmd("tab_create", { url: bm.url, active: true });
      };
      card.querySelector("#ctx-bm-edit").onclick = () => {
        closeModal();
        openBookmarkEditorModal(bm);
      };
      card.querySelector("#ctx-bm-copy").onclick = () => {
        closeModal();
        copyTextToClipboard(bm.url);
      };
      card.querySelector("#ctx-bm-delete").onclick = async () => {
        await invokeCmd("bookmarks_delete", { id: bm.id });
        closeModal();
        await refreshBookmarks();
        renderBrowserChrome();
        showToast("Bookmark removed");
      };
    }
  );
}

function openBrowserQuickMenu() {
  const active = getActiveTab();
  openModal(
    `
      <h3>StreamVault Browser Menu</h3>
      <div style="display:flex;flex-direction:column;gap:7px;">
        <button data-m="new-tab" class="btn">＋ New Tab</button>
        <button data-m="find-in-page" class="btn">🔍 Find in Page (Ctrl+F)</button>
        <button data-m="batch-grab" class="btn btn-primary">⬇ Batch Media Grabber (Page Links)…</button>
        <button data-m="play-url" class="btn btn-accent">▶ Open / Stream Direct Media URL...</button>
        <button data-m="bookmarks" class="btn">★ Bookmark Manager</button>
        <button data-m="recent" class="btn">⏱ Recent Media</button>
        <button data-m="history" class="btn">🕘 Browsing History</button>
        <button data-m="downloads" class="btn">⬇ Permanent Downloads</button>
        <button data-m="copy-curr" class="btn">⧉ Copy Current Tab URL</button>
        <button data-m="settings" class="btn">⚙ Settings</button>
      </div>
      <div class="modal-actions">
        <button id="menu-close" class="btn">Close</button>
      </div>
    `,
    (card) => {
      card.querySelector("#menu-close").onclick = closeModal;
      card.querySelectorAll("[data-m]").forEach((b) => {
        b.onclick = () => {
          const act = b.getAttribute("data-m");
          closeModal();
          if (act === "new-tab") invokeCmd("tab_create", { url: null, active: true }).catch(() => {});
          else if (act === "find-in-page") openBrowserFindBar();
          else if (act === "batch-grab") openBatchMediaGrabberModal();
          else if (act === "play-url") mediaActionHandlers.promptDirectMediaUrlModal();
          else if (act === "copy-curr") {
            if (active?.url && active.url !== "about:blank") {
              copyTextToClipboard(active.url);
            } else {
              showToast("No active page URL to copy", "info");
            }
          } else switchViewCallback(act);
        };
      });
    }
  );
}

export async function runBrowserFind({ forward = true, stayOnCurrent = false } = {}) {
  const inp = document.getElementById("browser-find-input");
  const countEl = document.getElementById("browser-find-count");
  if (!inp) return;

  await ensureBrowserTabsInitialized();
  const active = getActiveTab();
  const query = inp.value.trim();

  if (!query) {
    if (countEl) countEl.textContent = "0 / 0";
    invokeCmd("tab_find_in_page", {
      tabId: active?.id || null,
      query: "",
      forward: true,
      clear: true,
      stayOnCurrent: false,
    }).catch(() => {});
    return;
  }

  try {
    const res = await invokeCmd("tab_find_in_page", {
      tabId: active?.id || null,
      query,
      forward,
      clear: false,
      stayOnCurrent,
    });
    if (res && typeof res === "object") {
      handleFindResultEvent(res);
    }
  } catch (_) {}
}

export function openBrowserFindBar() {
  const bar = document.getElementById("browser-find-bar");
  const inp = document.getElementById("browser-find-input");
  if (!bar || !inp) return;
  store.findBarOpen = true;
  bar.classList.remove("hidden");
  syncBrowserWebviewViewport();
  setTimeout(() => {
    inp.focus();
    inp.select();
    if (inp.value.trim()) {
      runBrowserFind({ forward: true, stayOnCurrent: true });
    }
  }, 15);
}

export function closeBrowserFindBar() {
  const bar = document.getElementById("browser-find-bar");
  const countEl = document.getElementById("browser-find-count");
  if (!bar) return;
  clearTimeout(findInputDebounce);
  store.findBarOpen = false;
  bar.classList.add("hidden");
  if (countEl) countEl.textContent = "0 / 0";
  const active = getActiveTab();
  invokeCmd("tab_find_in_page", {
    tabId: active?.id || null,
    query: "",
    forward: true,
    clear: true,
    stayOnCurrent: false,
  }).catch(() => {});
  syncBrowserWebviewViewport();
}

export function handleFindResultEvent(res) {
  const countEl = document.getElementById("browser-find-count");
  if (!countEl || !res) return;
  const active = getActiveTab();
  if (active && res.tab_id && res.tab_id !== active.id) return;
  const total = Number(res.total) || 0;
  const curr = Number(res.current) || 0;
  countEl.textContent = `${curr} / ${total}`;
}

function safeDecodeSegment(seg) {
  if (!seg) return "";
  try {
    return decodeURIComponent(seg);
  } catch (_) {
    return seg;
  }
}

export function openMediaLinkPromptModal(payload) {
  if (!payload || !payload.url) return;
  const url = payload.url;
  let filename = "";
  let extBadge = "MEDIA";
  try {
    const parsed = new URL(url);
    const segs = parsed.pathname.split("/").filter(Boolean);
    if (segs.length) {
      filename = safeDecodeSegment(segs[segs.length - 1]);
      const dot = filename.lastIndexOf(".");
      if (dot > 0 && dot < filename.length - 1) {
        extBadge = filename.slice(dot + 1).toUpperCase();
      }
    }
  } catch (_) {}

  const displayTitle = (payload.title || "").trim() || filename || url;

  openModal(
    `
      <div style="display:flex;align-items:center;gap:8px;">
        <span class="grabber-ext-tag" style="font-size:11px;padding:3px 8px;">${escapeHtml(extBadge)}</span>
        <h3 style="margin:0;">Media Link Selected</h3>
      </div>
      <div style="font-size:13.5px;font-weight:600;color:var(--text-primary);word-break:break-word;margin-top:2px;">
        ${escapeHtml(displayTitle)}
      </div>
      <div style="font-size:11.5px;color:var(--text-muted);word-break:break-all;background:var(--bg-elevated, rgba(15,23,42,0.55));padding:8px 10px;border-radius:6px;border:1px solid var(--border-subtle, rgba(148,163,184,0.16));">
        ${escapeHtml(url)}
      </div>
      <div style="display:flex;flex-direction:column;gap:8px;margin-top:6px;">
        <button type="button" id="mdl-media-play-app" class="btn btn-primary" style="justify-content:flex-start;padding:10px 14px;font-weight:600;">
          ▶ &nbsp;Play in StreamVault Player
        </button>
        <button type="button" id="mdl-media-play-ext" class="btn btn-accent" style="justify-content:flex-start;padding:10px 14px;">
          ↗ &nbsp;Play in External Player…
        </button>
        <button type="button" id="mdl-media-download" class="btn" style="justify-content:flex-start;padding:10px 14px;">
          ⬇ &nbsp;Download Media…
        </button>
        <div style="display:flex;gap:8px;">
          <button type="button" id="mdl-media-newtab" class="btn" style="flex:1;">⊞ Open in New Tab</button>
          <button type="button" id="mdl-media-copy" class="btn" style="flex:1;">📋 Copy URL</button>
        </div>
      </div>
      <div class="modal-actions">
        <button type="button" id="mdl-media-cancel" class="btn">Cancel</button>
      </div>
    `,
    (card) => {
      const btnPlayApp = card.querySelector("#mdl-media-play-app");
      btnPlayApp.onclick = () => {
        closeModal();
        mediaActionHandlers.playInStreamVault(url, displayTitle);
      };
      card.querySelector("#mdl-media-play-ext").onclick = () => {
        closeModal();
        mediaActionHandlers.openInExternalPlayer(url, displayTitle);
      };
      card.querySelector("#mdl-media-download").onclick = () => {
        closeModal();
        mediaActionHandlers.downloadMediaPermanent(url);
      };
      card.querySelector("#mdl-media-newtab").onclick = () => {
        closeModal();
        invokeCmd("tab_create", { url, active: true }).catch((e) =>
          showToast(String(e), "error")
        );
      };
      card.querySelector("#mdl-media-copy").onclick = () => {
        closeModal();
        copyTextToClipboard(url);
      };
      card.querySelector("#mdl-media-cancel").onclick = closeModal;
      setTimeout(() => btnPlayApp.focus(), 15);
    }
  );
}

export function renderGrabberToolbarBadge() {
  const badge = document.getElementById("grab-media-count-badge");
  if (!badge) return;
  const active = getActiveTab();
  const cat = active ? store.pageMediaByTab?.[active.id] : null;
  const count = (cat?.items || []).filter(
    (i) => i.kind === "video" || i.kind === "audio"
  ).length;
  if (count > 0) {
    badge.textContent = String(count);
    badge.classList.remove("hidden");
  } else {
    badge.classList.add("hidden");
  }
}

function extractStemForSubMatch(rawUrl) {
  try {
    const u = new URL(rawUrl);
    const segs = u.pathname.split("/").filter(Boolean);
    if (!segs.length) return "";
    const last = safeDecodeSegment(segs[segs.length - 1]);
    const dot = last.lastIndexOf(".");
    return dot > 0 ? last.slice(0, dot).toLowerCase() : "";
  } catch (_) {
    return "";
  }
}

export async function openBatchMediaGrabberModal() {
  await ensureBrowserTabsInitialized();
  const active = getActiveTab();
  if (active) {
    try {
      const liveCatalog = await invokeCmd("tab_request_page_media", { tabId: active.id });
      if (liveCatalog && Array.isArray(liveCatalog.items)) {
        store.pageMediaByTab[active.id] = liveCatalog;
      } else {
        const fresh = await invokeCmd("tab_get_page_media", { tabId: active.id });
        if (fresh) store.pageMediaByTab[active.id] = fresh;
      }
      renderGrabberToolbarBadge();
    } catch (_) {}
  }

  let catalog = (active && store.pageMediaByTab?.[active.id]) || { items: [] };
  let allItems = catalog.items || [];
  const defaultDir = store.settings?.default_download_dir || "data/downloads";

  openModal(
    `
      <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
        <h3 id="grab-modal-heading" style="margin:0;">Batch Media Grabber — Page Links (${allItems.length})</h3>
        <button type="button" id="grab-rescan-btn" class="btn btn-sm" title="Rescan current webpage for media & subtitle links">↻ Rescan</button>
      </div>
      <p style="margin:0;font-size:12px;color:var(--text-secondary);">
        Select episodes, movies, audio tracks, or subtitles found on this page to batch-download or play as a continuous playlist.
      </p>

      <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-top:4px;">
        <input
          id="grab-filter-input"
          class="input-field"
          style="flex:1;min-width:170px;"
          placeholder="Filter by filename (e.g. 1080p, S01, .mkv)…"
        />
        <label style="display:flex;align-items:center;gap:5px;font-size:12px;cursor:pointer;">
          <input id="grab-inc-media" type="checkbox" checked />
          <span>Media</span>
        </label>
        <label style="display:flex;align-items:center;gap:5px;font-size:12px;cursor:pointer;">
          <input id="grab-inc-subs" type="checkbox" checked />
          <span>Subtitles</span>
        </label>
      </div>

      <div style="display:flex;justify-content:space-between;align-items:center;font-size:12px;">
        <div style="display:flex;gap:6px;">
          <button type="button" id="grab-sel-all" class="btn btn-sm">Select Visible</button>
          <button type="button" id="grab-sel-none" class="btn btn-sm">Deselect All</button>
        </div>
        <span id="grab-selected-summary" style="color:var(--accent);font-weight:600;">0 selected</span>
      </div>

      <div id="grab-items-container" class="grabber-list-box"></div>

      <div style="display:flex;align-items:center;gap:8px;font-size:12px;">
        <span style="color:var(--text-secondary);white-space:nowrap;">Save Folder:</span>
        <input id="grab-dest-dir" class="input-field" style="flex:1;" value="${escapeHtml(defaultDir)}" />
        <button type="button" id="grab-pick-dir" class="btn btn-sm">Folder…</button>
      </div>

      <div class="modal-actions" style="flex-wrap:wrap;gap:8px;">
        <button type="button" id="grab-cancel" class="btn" style="margin-right:auto;">Cancel</button>
        <button type="button" id="grab-play-playlist" class="btn btn-accent">▶ Play Selected as Playlist</button>
        <button type="button" id="grab-start-dl" class="btn btn-primary">⬇ Queue Selected Downloads</button>
      </div>
    `,
    (card) => {
      const selectedUrls = new Set(
        allItems.filter((i) => i.kind === "video" || i.kind === "audio").map((i) => i.url)
      );
      // Also auto-select subtitles that match media on page
      for (const s of allItems.filter((i) => i.kind === "subtitle")) {
        selectedUrls.add(s.url);
      }

      const filterInput = card.querySelector("#grab-filter-input");
      const incMedia = card.querySelector("#grab-inc-media");
      const incSubs = card.querySelector("#grab-inc-subs");
      const listBox = card.querySelector("#grab-items-container");
      const summaryEl = card.querySelector("#grab-selected-summary");
      const destDirInput = card.querySelector("#grab-dest-dir");

      const getFiltered = () => {
        const needle = filterInput.value.trim().toLowerCase();
        return allItems.filter((item) => {
          if (item.kind === "subtitle" && !incSubs.checked) return false;
          if ((item.kind === "video" || item.kind === "audio") && !incMedia.checked) return false;
          if (!needle) return true;
          return (
            (item.filename || "").toLowerCase().includes(needle) ||
            (item.title || "").toLowerCase().includes(needle) ||
            (item.ext || "").toLowerCase().includes(needle)
          );
        });
      };

      const renderList = () => {
        const visible = getFiltered();
        summaryEl.textContent = `${selectedUrls.size} of ${allItems.length} selected`;
        if (visible.length === 0) {
          listBox.innerHTML = `<div style="padding:18px;text-align:center;color:var(--text-muted);font-size:12.5px;">
            ${
              allItems.length === 0
                ? "No direct MKV/MP4/MP3/SRT links detected on the current tab yet."
                : "No links match the current filter."
            }
          </div>`;
          return;
        }
        listBox.innerHTML = visible
          .map((item, idx) => {
            const checked = selectedUrls.has(item.url) ? "checked" : "";
            const isSub = item.kind === "subtitle";
            return `
              <label class="grabber-item-row">
                <input type="checkbox" data-grab-idx="${idx}" ${checked} />
                <span class="grabber-ext-tag ${isSub ? "subtitle" : ""}">${escapeHtml(
              item.ext || item.kind
            )}</span>
                <span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(
                  item.url
                )}">
                  ${escapeHtml(item.filename || item.title)}
                </span>
              </label>
            `;
          })
          .join("");

        listBox.querySelectorAll("[data-grab-idx]").forEach((cb) => {
          cb.onchange = () => {
            const item = visible[Number(cb.getAttribute("data-grab-idx"))];
            if (!item) return;
            if (cb.checked) selectedUrls.add(item.url);
            else selectedUrls.delete(item.url);
            summaryEl.textContent = `${selectedUrls.size} of ${allItems.length} selected`;
          };
        });
      };

      filterInput.oninput = renderList;
      incMedia.onchange = renderList;
      incSubs.onchange = renderList;

      card.querySelector("#grab-sel-all").onclick = () => {
        getFiltered().forEach((i) => selectedUrls.add(i.url));
        renderList();
      };
      card.querySelector("#grab-sel-none").onclick = () => {
        selectedUrls.clear();
        renderList();
      };

      card.querySelector("#grab-rescan-btn").onclick = async () => {
        const curTab = getActiveTab();
        if (!curTab) return;
        try {
          const refreshed = await invokeCmd("tab_request_page_media", { tabId: curTab.id });
          if (refreshed && Array.isArray(refreshed.items)) {
            store.pageMediaByTab[curTab.id] = refreshed;
            catalog = refreshed;
            allItems = refreshed.items || [];
            selectedUrls.clear();
            allItems.forEach((i) => selectedUrls.add(i.url));
            const h = card.querySelector("#grab-modal-heading");
            if (h) h.textContent = `Batch Media Grabber — Page Links (${allItems.length})`;
            renderGrabberToolbarBadge();
            renderList();
            showToast(`Found ${allItems.length} media/subtitle link${allItems.length === 1 ? "" : "s"}`, "info");
          }
        } catch (e) {
          showToast(String(e), "error");
        }
      };

      card.querySelector("#grab-pick-dir").onclick = async () => {
        try {
          const picked = await invokeCmd("download_pick_batch_folder");
          if (picked) destDirInput.value = picked;
        } catch (e) {
          showToast(String(e), "error");
        }
      };

      card.querySelector("#grab-cancel").onclick = closeModal;

      card.querySelector("#grab-start-dl").onclick = async () => {
        const chosen = allItems.filter((i) => selectedUrls.has(i.url)).map((i) => i.url);
        if (chosen.length === 0) {
          showToast("Select at least one file to download", "error");
          return;
        }
        try {
          const created = await invokeCmd("download_start_batch", {
            urls: chosen,
            destinationDir: destDirInput.value.trim() || null,
          });
          store.downloads = await invokeCmd("download_list");
          updateSidebarDownloadBadge();
          closeModal();
          showToast(
            `Queued ${created.length} file${created.length === 1 ? "" : "s"} in Download Manager`,
            "success"
          );
          switchViewCallback("downloads");
        } catch (e) {
          showToast(String(e), "error");
        }
      };

      card.querySelector("#grab-play-playlist").onclick = async () => {
        const chosenMedia = allItems.filter(
          (i) => selectedUrls.has(i.url) && (i.kind === "video" || i.kind === "audio")
        );
        if (chosenMedia.length === 0) {
          showToast("Select at least one video or audio file to play", "error");
          return;
        }
        const pageSubs = allItems.filter((i) => i.kind === "subtitle");
        const playlist = chosenMedia.map((m) => {
          const mStem = extractStemForSubMatch(m.url);
          const matchedSubs = pageSubs
            .filter((s) => {
              const sStem = extractStemForSubMatch(s.url);
              return (
                sStem &&
                mStem &&
                (sStem === mStem ||
                  sStem.startsWith(mStem + ".") ||
                  sStem.startsWith(mStem + "_") ||
                  sStem.startsWith(mStem + "-"))
              );
            })
            .map((s) => s.url);
          return {
            url: m.url,
            title: m.filename || m.title || m.url,
            isLocal: false,
            localPath: null,
            matchedSubtitles: matchedSubs,
          };
        });
        closeModal();
        await setCustomPlaylistAndPlay(playlist, 0);
      };

      renderList();
      filterInput.focus();
    }
  );
}
