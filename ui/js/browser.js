import { openBookmarkEditorModal, refreshBookmarks, urlsEquivalent } from "./bookmarks.js";
import { invokeCmd } from "./ipc.js";
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
  const btnMenu = document.getElementById("btn-browser-menu");
  const activeMediaPill = document.getElementById("toolbar-active-media-pill");

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

  activeMediaPill.addEventListener("click", () => {
    if (store.activeHighlight?.mode === "in_app") {
      switchViewCallback("player");
    } else if (store.activeHighlight?.original_url) {
      showToast(`Playing externally: ${store.activeHighlight.title || store.activeHighlight.original_url}`);
    }
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
      btn.addEventListener("click", (ev) => {
        if (ev.ctrlKey || ev.metaKey) {
          invokeCmd("tab_create", { url, active: true }).catch((e) =>
            showToast(String(e), "error")
          );
          return;
        }
        invokeCmd("tab_navigate", { tabId: null, input: url }).catch((e) =>
          showToast(String(e), "error")
        );
      });
      btn.addEventListener("auxclick", (ev) => {
        if (ev.button === 1) {
          ev.preventDefault();
          invokeCmd("tab_create", { url, active: true }).catch(() => {});
        }
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
    pill.textContent = hl.mode === "external" ? "↗ External Active" : "▶ Playing in StreamVault";
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

function openBookmarkCurrentTabModal() {
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
          if (act === "new-tab") invokeCmd("tab_create", { url: null, active: true });
          else if (act === "play-url") mediaActionHandlers.promptDirectMediaUrlModal();
          else if (act === "copy-curr" && active?.url) copyTextToClipboard(active.url);
          else switchViewCallback(act);
        };
      });
    }
  );
}
