(function () {
  if (window.__SV_CONTENT_SCRIPT_INSTALLED__) return;
  Object.defineProperty(window, "__SV_CONTENT_SCRIPT_INSTALLED__", {
    value: true,
    configurable: false,
    writable: false,
  });

  const SECRET_TOKEN = "__SV_SECRET_TOKEN__";
  const TAB_ID = "__SV_TAB_ID__";

  const MEDIA_EXT_RE =
    /\.(mp4|mkv|webm|avi|mov|m4v|ts|m2ts|flv|wmv|mpg|mpeg|ogv|3gp|vob|divx|rmvb|asf|mp3|aac|flac|wav|ogg|opus|m4a|wma|ac3|eac3|dts|alac|m3u8|mpd)(?:$|[?#])/i;

  let activeNormalizedKey = null;
  let activePlaybackMode = null; // "in_app" | "external" | null
  let menuEl = null;
  let menuOpenedAt = 0;
  let pendingMediaClickTimer = null;

  function normalizeMediaKeyJs(rawHref) {
    try {
      const u = new URL(rawHref, window.location.href);
      if (u.protocol !== "http:" && u.protocol !== "https:") return null;
      u.hash = "";
      const scheme = u.protocol.replace(":", "").toLowerCase();
      const host = u.hostname.toLowerCase();
      let portPart = "";
      if (u.port) {
        const p = Number(u.port);
        if (!((scheme === "http" && p === 80) || (scheme === "https" && p === 443))) {
          portPart = ":" + p;
        }
      }
      const path = u.pathname || "/";
      return scheme + "://" + host + portPart + path + u.search;
    } catch (_) {
      return null;
    }
  }

  function resolveFullUrl(href) {
    try {
      const u = new URL(href, window.location.href);
      if (u.protocol === "http:" || u.protocol === "https:") {
        return u.href;
      }
    } catch (_) {}
    return null;
  }

  function looksLikeMediaHref(href) {
    if (!href) return false;
    try {
      const u = new URL(href, window.location.href);
      if (u.protocol !== "http:" && u.protocol !== "https:") return false;
      const p = u.pathname || "";
      if (MEDIA_EXT_RE.test(p)) return true;
      const q = u.search || "";
      if (/(?:mime=video|mime=audio|type=video|type=audio|format=mp4|format=mkv|format=m3u8)/i.test(q)) {
        return true;
      }
    } catch (_) {}
    return false;
  }

  function extractAnchorLabel(anchor, rawUrl) {
    const text = (anchor?.innerText || anchor?.textContent || "").trim();
    if (text) return text.slice(0, 220);
    const attrTitle = (anchor?.getAttribute?.("title") || anchor?.getAttribute?.("download") || "").trim();
    if (attrTitle) return attrTitle.slice(0, 220);
    try {
      const u = new URL(rawUrl);
      const last = u.pathname.split("/").filter(Boolean).pop() || "";
      if (last) return decodeURIComponent(last).slice(0, 220);
    } catch (_) {}
    return rawUrl;
  }

  function extractMediaLinkTarget(target) {
    if (!target || typeof target.closest !== "function") return null;

    const anchor = target.closest("a[href]");
    if (anchor) {
      const raw = resolveFullUrl(anchor.getAttribute("href"));
      if (!raw) return null;
      const dlAttr = anchor.getAttribute("download") || "";
      const isMedia = looksLikeMediaHref(raw) || MEDIA_EXT_RE.test(dlAttr);
      return {
        anchor,
        raw,
        label: extractAnchorLabel(anchor, raw),
        isMedia,
      };
    }

    // Also support directory-listing table rows / list items (Apache, Nginx, h5ai, Dhaka-Flix)
    // when double-clicking or right-clicking on the row adjacent to a single media link.
    const row = target.closest("tr, li, .item, [role='row']");
    if (row && typeof row.querySelectorAll === "function") {
      const anchors = Array.from(row.querySelectorAll("a[href]"));
      const mediaMatches = [];
      const seen = new Set();
      for (const a of anchors) {
        const raw = resolveFullUrl(a.getAttribute("href"));
        if (raw && looksLikeMediaHref(raw) && !seen.has(raw)) {
          seen.add(raw);
          mediaMatches.push({
            anchor: a,
            raw,
            label: extractAnchorLabel(a, raw),
            isMedia: true,
          });
        }
      }
      if (mediaMatches.length === 1) {
        return mediaMatches[0];
      }
    }

    return null;
  }

  function fallbackExecCopy(text) {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(text).catch(() => {});
      }
    } catch (_) {}
    try {
      const root = document.body || document.documentElement;
      if (!root) return;
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.setAttribute("readonly", "");
      ta.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0;pointer-events:none;";
      root.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      root.removeChild(ta);
    } catch (_) {}
  }

  function injectStyles() {
    if (document.getElementById("__sv_injected_styles")) return;
    const style = document.createElement("style");
    style.id = "__sv_injected_styles";
    style.textContent = `
      a.__sv-active-media-link {
        outline: 2px solid #38bdf8 !important;
        outline-offset: 2px !important;
        background-color: rgba(56, 189, 248, 0.14) !important;
        border-radius: 4px !important;
        transition: background-color 0.18s ease !important;
      }
      a.__sv-active-media-link::after {
        content: attr(data-sv-badge);
        display: inline-block;
        margin-left: 8px;
        padding: 1px 7px;
        font-size: 11px;
        font-weight: 600;
        font-family: "Segoe UI", system-ui, sans-serif;
        line-height: 1.45;
        color: #0b1120 !important;
        background: #38bdf8 !important;
        border-radius: 999px;
        vertical-align: middle;
        text-decoration: none !important;
      }
      #__sv_ctx_menu {
        position: fixed;
        z-index: 2147483647;
        min-width: 248px;
        max-width: 340px;
        background: #161922;
        color: #eef2f6;
        border: 1px solid #2b3245;
        border-radius: 8px;
        box-shadow: 0 14px 36px rgba(0, 0, 0, 0.58);
        padding: 5px;
        font-family: "Segoe UI", -apple-system, system-ui, sans-serif;
        font-size: 13px;
        user-select: none;
      }
      #__sv_ctx_menu .__sv_hdr {
        padding: 6px 10px 5px;
        font-size: 11px;
        font-weight: 600;
        color: #8b96ab;
        border-bottom: 1px solid #242a3a;
        margin-bottom: 4px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      #__sv_ctx_menu button.__sv_item {
        all: unset;
        box-sizing: border-box;
        width: 100%;
        display: flex;
        align-items: center;
        gap: 9px;
        padding: 7px 10px;
        border-radius: 5px;
        cursor: pointer;
        color: #e5ebf3;
        font-size: 13px;
        line-height: 1.3;
      }
      #__sv_ctx_menu button.__sv_item:hover {
        background: #2563eb;
        color: #ffffff;
      }
      #__sv_ctx_menu .__sv_sep {
        height: 1px;
        background: #242a3a;
        margin: 4px 2px;
      }
    `;
    (document.head || document.documentElement).appendChild(style);
  }

  function applyBlankNewTabSurfaceIfNeeded() {
    if (window.location.href !== "about:blank") return;
    const apply = () => {
      if (window.location.href !== "about:blank") return;
      if (document.getElementById("__sv_direct_media_card")) return;
      const docEl = document.documentElement;
      const body = document.body;
      if (docEl) docEl.style.background = "#0f1117";
      if (body && body.children.length === 0 && !(body.textContent || "").trim()) {
        body.style.cssText =
          "margin:0;height:100vh;background:#0f1117;color:#687387;display:flex;align-items:center;justify-content:center;font-family:'Segoe UI',system-ui,sans-serif;user-select:none;";
        body.innerHTML = `
          <div style="text-align:center;opacity:0.8;">
            <div style="font-size:15px;font-weight:600;color:#9aa4b8;margin-bottom:4px;">New Tab</div>
            <div style="font-size:12px;color:#687387;">Enter a website, LAN server IP, or direct media URL above</div>
          </div>
        `;
      }
    };
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", apply, { once: true });
    } else {
      apply();
    }
  }

  function refreshActiveLinkHighlight() {
    injectStyles();
    const anchors = document.querySelectorAll("a[href]");
    const badgeLabel =
      activePlaybackMode === "external"
        ? "Playing in External Player"
        : "Playing in StreamVault";

    for (let i = 0; i < anchors.length; i++) {
      const a = anchors[i];
      if (!activeNormalizedKey) {
        if (a.classList.contains("__sv-active-media-link")) {
          a.classList.remove("__sv-active-media-link");
          a.removeAttribute("data-sv-badge");
        }
        continue;
      }
      const norm = normalizeMediaKeyJs(a.getAttribute("href"));
      if (norm && norm === activeNormalizedKey) {
        a.classList.add("__sv-active-media-link");
        a.setAttribute("data-sv-badge", badgeLabel);
      } else if (a.classList.contains("__sv-active-media-link")) {
        a.classList.remove("__sv-active-media-link");
        a.removeAttribute("data-sv-badge");
      }
    }
  }

  Object.defineProperty(window, "__SV_SET_ACTIVE_MEDIA__", {
    value: function (normalizedKey, mode) {
      activeNormalizedKey = normalizedKey || null;
      activePlaybackMode = mode || null;
      refreshActiveLinkHighlight();
    },
    configurable: false,
    writable: false,
  });

  function sendBridgeAction(action, url, title) {
    try {
      const invoke =
        window.__TAURI_INTERNALS__ && typeof window.__TAURI_INTERNALS__.invoke === "function"
          ? window.__TAURI_INTERNALS__.invoke
          : null;
      if (!invoke) return;
      invoke("tab_context_bridge", {
        token: SECRET_TOKEN,
        tabId: TAB_ID,
        action: action,
        url: url,
        title: title || "",
      }).catch(() => {});
    } catch (_) {}
  }

  // Renders an interactive dark StreamVault Media Card inside a tab when the user chooses
  // "+ Open in New Tab" on a media container (such as .mkv / .avi / .ts) that WebView2
  // cannot play inline natively.
  Object.defineProperty(window, "__SV_RENDER_DIRECT_MEDIA_TAB__", {
    value: function (mediaUrl, mediaTitle) {
      const render = () => {
        injectStyles();
        const safeTitle = String(mediaTitle || mediaUrl || "Media Stream");
        const safeUrl = String(mediaUrl || "");
        if (document.documentElement) {
          document.documentElement.style.background = "#0f1117";
        }
        if (!document.body) return;
        try {
          document.title = safeTitle;
        } catch (_) {}
        document.body.style.cssText =
          "margin:0;min-height:100vh;background:#0f1117;color:#eef2f7;display:flex;align-items:center;justify-content:center;padding:24px;box-sizing:border-box;font-family:'Segoe UI',-apple-system,system-ui,sans-serif;";
        document.body.innerHTML = "";

        const card = document.createElement("div");
        card.id = "__sv_direct_media_card";
        card.style.cssText =
          "max-width:660px;width:100%;background:#171b26;border:1px solid #323b50;border-radius:10px;padding:22px 24px;box-shadow:0 16px 42px rgba(0,0,0,0.55);";

        const badge = document.createElement("div");
        badge.style.cssText =
          "display:inline-flex;align-items:center;gap:6px;padding:3px 9px;border-radius:999px;background:rgba(56,189,248,0.14);color:#38bdf8;font-size:11px;font-weight:600;margin-bottom:10px;";
        badge.textContent = "DIRECT MEDIA STREAM TAB";

        const titleEl = document.createElement("h2");
        titleEl.style.cssText =
          "margin:0 0 8px;font-size:17px;font-weight:600;color:#eef2f7;word-break:break-word;line-height:1.35;";
        titleEl.textContent = safeTitle;

        const urlEl = document.createElement("div");
        urlEl.style.cssText =
          "margin:0 0 16px;padding:8px 10px;border-radius:6px;background:#10131b;border:1px solid #232938;font-family:Consolas,monospace;font-size:11.5px;color:#9aa4b8;word-break:break-all;";
        urlEl.textContent = safeUrl;

        const actions = document.createElement("div");
        actions.style.cssText = "display:flex;flex-wrap:wrap;gap:9px;";

        const makeBtn = (label, primary, onClick) => {
          const b = document.createElement("button");
          b.type = "button";
          b.textContent = label;
          b.style.cssText = primary
            ? "height:34px;padding:0 14px;border-radius:6px;border:1px solid #2563eb;background:#2563eb;color:#fff;font-size:13px;font-weight:600;cursor:pointer;"
            : "height:34px;padding:0 13px;border-radius:6px;border:1px solid #323b50;background:#1e2433;color:#eef2f7;font-size:12.5px;font-weight:500;cursor:pointer;";
          b.addEventListener("click", onClick);
          return b;
        };

        actions.appendChild(
          makeBtn("▶ Play in StreamVault", true, () =>
            sendBridgeAction("play_in_app", safeUrl, safeTitle)
          )
        );
        actions.appendChild(
          makeBtn("↗ Open in External Player", false, () =>
            sendBridgeAction("play_external", safeUrl, safeTitle)
          )
        );
        actions.appendChild(
          makeBtn("⬇ Download Media", false, () =>
            sendBridgeAction("download", safeUrl, safeTitle)
          )
        );
        actions.appendChild(
          makeBtn("⎘ Copy Media URL", false, () => {
            fallbackExecCopy(safeUrl);
            sendBridgeAction("copy_url", safeUrl, safeTitle);
          })
        );

        card.appendChild(badge);
        card.appendChild(titleEl);
        card.appendChild(urlEl);
        card.appendChild(actions);
        document.body.appendChild(card);
      };

      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", render, { once: true });
      } else {
        render();
      }
    },
    configurable: false,
    writable: false,
  });

  function closeCustomMenu() {
    if (menuEl && menuEl.parentNode) {
      menuEl.parentNode.removeChild(menuEl);
    }
    menuEl = null;
  }

  function openMediaContextMenu(x, y, raw, label, isLikelyMedia) {
    closeCustomMenu();
    injectStyles();

    menuEl = document.createElement("div");
    menuEl.id = "__sv_ctx_menu";

    const hdr = document.createElement("div");
    hdr.className = "__sv_hdr";
    hdr.textContent = isLikelyMedia ? "StreamVault Media Link" : "StreamVault Link Options";
    hdr.title = raw;
    menuEl.appendChild(hdr);

    const items = isLikelyMedia
      ? [
          { icon: "▶", text: "Play in StreamVault", act: "play_in_app" },
          { icon: "↗", text: "Open in External Player", act: "play_external" },
          { icon: "⬇", text: "Download Media", act: "download" },
          { sep: true },
          { icon: "＋", text: "Open in New Tab", act: "open_new_tab" },
          { icon: "⎘", text: "Copy Media URL", act: "copy_url" },
        ]
      : [
          { icon: "＋", text: "Open Link in New Tab", act: "open_new_tab" },
          { icon: "▶", text: "Play as Media in StreamVault", act: "play_in_app" },
          { icon: "↗", text: "Open in External Player", act: "play_external" },
          { icon: "⬇", text: "Download with StreamVault", act: "download" },
          { sep: true },
          { icon: "⎘", text: "Copy Link URL", act: "copy_url" },
        ];

    for (const item of items) {
      if (item.sep) {
        const s = document.createElement("div");
        s.className = "__sv_sep";
        menuEl.appendChild(s);
        continue;
      }
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "__sv_item";
      btn.innerHTML = `<span>${item.icon}</span><span>${item.text}</span>`;
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        closeCustomMenu();
        if (item.act === "copy_url") {
          fallbackExecCopy(raw);
          sendBridgeAction("copy_url", raw, label);
        } else {
          sendBridgeAction(item.act, raw, label);
        }
      });
      menuEl.appendChild(btn);
    }

    (document.body || document.documentElement).appendChild(menuEl);
    menuOpenedAt = performance.now();

    const rect = menuEl.getBoundingClientRect();
    const maxX = Math.max(8, window.innerWidth - rect.width - 8);
    const maxY = Math.max(8, window.innerHeight - rect.height - 8);
    menuEl.style.left = Math.min(Math.max(8, x), maxX) + "px";
    menuEl.style.top = Math.min(Math.max(8, y), maxY) + "px";
  }

  // Intercept single-click on confirmed media links just long enough (~230ms) so Click #1
  // of a double-click does NOT navigate away before `dblclick` fires, while keeping normal
  // single-click media detection intact and leaving non-media webpage/folder links 100% instant!
  window.addEventListener(
    "click",
    (ev) => {
      if (menuEl && menuEl.contains(ev.target)) return;
      if (ev.button !== 0 || ev.ctrlKey || ev.metaKey || ev.shiftKey || ev.altKey) return;

      const anchor = ev.target && ev.target.closest ? ev.target.closest("a[href]") : null;
      if (!anchor) return;
      const raw = resolveFullUrl(anchor.getAttribute("href"));
      const dlAttr = anchor.getAttribute("download") || "";
      if (!raw || (!looksLikeMediaHref(raw) && !MEDIA_EXT_RE.test(dlAttr))) {
        return;
      }

      ev.preventDefault();
      ev.stopPropagation();

      if (pendingMediaClickTimer) {
        clearTimeout(pendingMediaClickTimer);
        pendingMediaClickTimer = null;
      }

      if (ev.detail === 1) {
        const label = extractAnchorLabel(anchor, raw);
        pendingMediaClickTimer = setTimeout(() => {
          pendingMediaClickTimer = null;
          sendBridgeAction("probe_and_inspect", raw, label);
        }, 230);
      }
    },
    true
  );

  // Double-clicking a media link opens the StreamVault Media Link options popup (same as right-click)
  window.addEventListener(
    "dblclick",
    (ev) => {
      if (menuEl && menuEl.contains(ev.target)) return;
      const found = extractMediaLinkTarget(ev.target);
      if (!found || !found.isMedia) return;

      if (pendingMediaClickTimer) {
        clearTimeout(pendingMediaClickTimer);
        pendingMediaClickTimer = null;
      }

      ev.preventDefault();
      ev.stopPropagation();
      try {
        const sel = window.getSelection && window.getSelection();
        if (sel && sel.removeAllRanges) sel.removeAllRanges();
      } catch (_) {}

      openMediaContextMenu(ev.clientX, ev.clientY, found.raw, found.label, true);
    },
    true
  );

  // Right-click context menu on links (and directory-listing rows containing a media link)
  window.addEventListener(
    "contextmenu",
    (ev) => {
      if (ev.shiftKey) return;
      const found = extractMediaLinkTarget(ev.target);
      if (!found) {
        closeCustomMenu();
        return;
      }

      if (pendingMediaClickTimer) {
        clearTimeout(pendingMediaClickTimer);
        pendingMediaClickTimer = null;
      }

      ev.preventDefault();
      ev.stopPropagation();
      openMediaContextMenu(ev.clientX, ev.clientY, found.raw, found.label, found.isMedia);
    },
    true
  );

  window.addEventListener(
    "mousedown",
    (ev) => {
      if (menuEl && !menuEl.contains(ev.target)) closeCustomMenu();
    },
    true
  );
  window.addEventListener(
    "scroll",
    () => {
      if (menuEl && performance.now() - menuOpenedAt > 140) closeCustomMenu();
    },
    true
  );
  window.addEventListener(
    "blur",
    () => {
      if (menuEl && performance.now() - menuOpenedAt > 140) closeCustomMenu();
    }
  );
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") closeCustomMenu();
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => {
      applyBlankNewTabSurfaceIfNeeded();
      refreshActiveLinkHighlight();
    });
  } else {
    applyBlankNewTabSurfaceIfNeeded();
    refreshActiveLinkHighlight();
  }

  const mo = new MutationObserver(() => {
    if (activeNormalizedKey) refreshActiveLinkHighlight();
  });
  mo.observe(document.documentElement, { childList: true, subtree: true });
})();
