(function () {
  if (window.__SV_TAB_BRIDGE_READY__) return;
  window.__SV_TAB_BRIDGE_READY__ = true;

  const TAB_ID = "__SV_TAB_ID__";
  const BRIDGE_TOKEN = "__SV_SECRET_TOKEN__";
  const IS_TOP_FRAME = (function () {
    try {
      return window.self === window.top;
    } catch (_) {
      return false;
    }
  })();
  let currentHighlightKey = __SV_INIT_HIGHLIGHT_KEY__;
  let currentHighlightMode = __SV_INIT_HIGHLIGHT_MODE__;
  let currentHighlightHistory = __SV_INIT_HIGHLIGHT_HISTORY__;

  function getRawInvoke() {
    if (
      window.__TAURI_INTERNALS__ &&
      typeof window.__TAURI_INTERNALS__.invoke === "function"
    ) {
      return window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
    }
    return null;
  }

  // Capture initial reference if already present, with lazy fallback if initialized slightly later
  const initialRawInvoke = getRawInvoke();

  const VIDEO_EXTS = new Set([
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v",
    "mpeg", "mpg", "ts", "m2ts", "mts", "ogv", "3gp", "vob",
    "divx", "rmvb", "asf", "iso", "m3u8", "mpd"
  ]);
  const AUDIO_EXTS = new Set([
    "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma",
    "ac3", "eac3", "dts", "alac", "aiff", "aif", "ape", "mka"
  ]);
  const SUBTITLE_EXTS = new Set(["srt", "ass", "ssa", "vtt", "sub"]);

  const QUERY_FILE_KEYS = [
    "file", "filename", "name", "path", "video", "media", "download", "src", "url"
  ];

  function safeDecodeURIComponent(str) {
    if (!str) return "";
    try {
      return decodeURIComponent(str);
    } catch (_) {
      try {
        return unescape(str);
      } catch (_2) {
        return str;
      }
    }
  }

  function sendBridge(action, url, title, payloadObj) {
    const inv = initialRawInvoke || getRawInvoke();
    if (!inv) return Promise.resolve();
    const safeUrl =
      url && (url.startsWith("http://") || url.startsWith("https://"))
        ? url
        : window.location.href &&
            (window.location.href.startsWith("http://") ||
              window.location.href.startsWith("https://"))
          ? window.location.href
          : "http://localhost/";
    return inv("tab_context_bridge", {
      token: BRIDGE_TOKEN,
      tabId: TAB_ID,
      action: action,
      url: safeUrl,
      title: title || "",
      payload: payloadObj ? JSON.stringify(payloadObj) : null
    }).catch(function () {});
  }

  function extractExtAndName(candidateStr) {
    if (!candidateStr) return null;
    const cleaned = safeDecodeURIComponent(candidateStr.split("#")[0].split("?")[0]).trim();
    if (!cleaned) return null;
    const parts = cleaned.split("/").filter(Boolean);
    const last = parts.length ? parts[parts.length - 1] : cleaned;
    const dot = last.lastIndexOf(".");
    if (dot <= 0 || dot === last.length - 1) return null;
    const ext = last.slice(dot + 1).toLowerCase();
    if (VIDEO_EXTS.has(ext)) return { kind: "video", ext: ext, filename: last };
    if (AUDIO_EXTS.has(ext)) return { kind: "audio", ext: ext, filename: last };
    if (SUBTITLE_EXTS.has(ext)) return { kind: "subtitle", ext: ext, filename: last };
    return null;
  }

  function classifyFileUrl(href, downloadAttr) {
    try {
      const u = new URL(href, window.location.href);
      if (u.protocol !== "http:" && u.protocol !== "https:") return null;

      // 1. Check URL pathname segment
      const segs = u.pathname.split("/").filter(Boolean);
      if (segs.length > 0) {
        const fromPath = extractExtAndName(segs[segs.length - 1]);
        if (fromPath) {
          return {
            kind: fromPath.kind,
            ext: fromPath.ext,
            filename: fromPath.filename,
            href: u.href
          };
        }
      }

      // 2. Check explicit HTML5 download="filename.mkv" attribute if present
      if (downloadAttr) {
        const fromDl = extractExtAndName(downloadAttr);
        if (fromDl) {
          return {
            kind: fromDl.kind,
            ext: fromDl.ext,
            filename: fromDl.filename,
            href: u.href
          };
        }
      }

      // 3. Check common query string parameters (?file=Movie.mkv, ?path=...)
      for (let i = 0; i < QUERY_FILE_KEYS.length; i++) {
        const val = u.searchParams.get(QUERY_FILE_KEYS[i]);
        if (val) {
          const fromQuery = extractExtAndName(val);
          if (fromQuery) {
            return {
              kind: fromQuery.kind,
              ext: fromQuery.ext,
              filename: fromQuery.filename,
              href: u.href
            };
          }
        }
      }
    } catch (_) {}
    return null;
  }

  function isMediaAnchor(anchor) {
    if (!anchor || !anchor.href) return false;
    const dl = anchor.getAttribute ? anchor.getAttribute("download") : null;
    const info = classifyFileUrl(anchor.href, dl);
    return !!(info && (info.kind === "video" || info.kind === "audio"));
  }

  function isMediaAnchorOrText(anchor) {
    if (isMediaAnchor(anchor)) return true;
    if (!anchor || !anchor.href) return false;
    const textInfo = extractExtAndName((anchor.textContent || "").trim());
    return !!(textInfo && (textInfo.kind === "video" || textInfo.kind === "audio"));
  }

  const GENERIC_MEDIA_FILENAMES = new Set([
    "video.mp4", "video.mkv", "video.webm", "stream.m3u8", "master.m3u8",
    "playlist.m3u8", "index.m3u8", "manifest.mpd", "audio.mp3", "download.mp4", "play.mp4"
  ]);

  function normalizeKey(href) {
    try {
      const u = new URL(href, window.location.href);
      u.hash = "";
      return u.href.replace(/%[0-9a-f]{2}/gi, function (m) {
        return m.toUpperCase();
      });
    } catch (_) {
      return (href || "").trim();
    }
  }

  function decodedComparableKey(normKey) {
    if (!normKey) return "";
    return safeDecodeURIComponent(normKey).replace(/\+/g, " ").trim().toLowerCase();
  }

  function extractHostAndFilenameKey(href) {
    try {
      const u = new URL(href, window.location.href);
      if (u.protocol !== "http:" && u.protocol !== "https:") return null;
      const info = classifyFileUrl(u.href, null);
      if (!info || (info.kind !== "video" && info.kind !== "audio")) return null;
      const fnLower = (info.filename || "").trim().toLowerCase();
      if (fnLower.length < 8 || GENERIC_MEDIA_FILENAMES.has(fnLower)) return null;
      return u.host.toLowerCase() + "::" + fnLower;
    } catch (_) {
      return null;
    }
  }

  function keysEquivalent(href, targetNormalizedKey) {
    if (!href || !targetNormalizedKey) return false;
    const normA = normalizeKey(href);
    if (normA === targetNormalizedKey) return true;
    if (decodedComparableKey(normA) === decodedComparableKey(targetNormalizedKey)) return true;
    const hfA = extractHostAndFilenameKey(normA);
    const hfB = extractHostAndFilenameKey(targetNormalizedKey);
    return Boolean(hfA && hfB && hfA === hfB);
  }

  // Inject clean, high-contrast styles for multi-state media link highlighting, Find-in-Page, and right-click context menu
  function ensureStyles() {
    if (document.getElementById("sv-injected-style")) return;
    const style = document.createElement("style");
    style.id = "sv-injected-style";
    style.textContent = `
      @keyframes sv-playing-pulse {
        0%, 100% { box-shadow: 0 0 0 0 rgba(14, 165, 233, 0.45), 0 1px 4px rgba(0, 0, 0, 0.25); }
        50% { box-shadow: 0 0 0 4px rgba(14, 165, 233, 0.0), 0 1px 6px rgba(14, 165, 233, 0.45); }
      }
      @keyframes sv-spotlight-ring {
        0% { outline-offset: 8px; box-shadow: 0 0 0 8px rgba(56, 189, 248, 0.55); }
        100% { outline-offset: 2px; box-shadow: 0 0 12px rgba(14, 165, 233, 0.28); }
      }

      /* Row-level highlight for HTML directory tables (Apache/Nginx/h5ai) and lists */
      tr[data-sv-row-hl="in_app"],
      tr[data-sv-row-hl="in_app"] > td,
      tr[data-sv-row-hl="in_app"] > th {
        background-color: rgba(14, 165, 233, 0.11) !important;
      }
      tr[data-sv-row-hl="in_app"] > td:first-child,
      tr[data-sv-row-hl="in_app"] > th:first-child,
      li[data-sv-row-hl="in_app"],
      [role="row"][data-sv-row-hl="in_app"] {
        background-color: rgba(14, 165, 233, 0.11) !important;
        box-shadow: inset 3.5px 0 0 #0284c7 !important;
      }

      tr[data-sv-row-hl="paused"],
      tr[data-sv-row-hl="paused"] > td,
      tr[data-sv-row-hl="paused"] > th {
        background-color: rgba(99, 102, 241, 0.10) !important;
      }
      tr[data-sv-row-hl="paused"] > td:first-child,
      tr[data-sv-row-hl="paused"] > th:first-child,
      li[data-sv-row-hl="paused"],
      [role="row"][data-sv-row-hl="paused"] {
        background-color: rgba(99, 102, 241, 0.10) !important;
        box-shadow: inset 3.5px 0 0 #4f46e5 !important;
      }

      tr[data-sv-row-hl="external"],
      tr[data-sv-row-hl="external"] > td,
      tr[data-sv-row-hl="external"] > th {
        background-color: rgba(245, 158, 11, 0.12) !important;
      }
      tr[data-sv-row-hl="external"] > td:first-child,
      tr[data-sv-row-hl="external"] > th:first-child,
      li[data-sv-row-hl="external"],
      [role="row"][data-sv-row-hl="external"] {
        background-color: rgba(245, 158, 11, 0.12) !important;
        box-shadow: inset 3.5px 0 0 #d97706 !important;
      }

      /* Base highlighted anchor — preserves native display (inline / flex / block) for zero layout shift */
      a[data-sv-hl] {
        border-radius: 5px !important;
        box-decoration-break: clone;
        -webkit-box-decoration-break: clone;
        transition: box-shadow 0.2s ease, background-color 0.2s ease, outline-color 0.2s ease !important;
      }

      a[data-sv-hl="in_app"]:not([data-sv-card="true"]):not([data-sv-icon-only="true"]) {
        background: linear-gradient(90deg, rgba(14, 165, 233, 0.18), rgba(16, 185, 129, 0.12)) !important;
        outline: 1.5px solid rgba(2, 132, 199, 0.88) !important;
        outline-offset: 2px !important;
        box-shadow: 0 0 12px rgba(14, 165, 233, 0.26) !important;
        font-weight: 600 !important;
      }
      a[data-sv-hl="paused"]:not([data-sv-card="true"]):not([data-sv-icon-only="true"]) {
        background: rgba(99, 102, 241, 0.16) !important;
        outline: 1.5px solid rgba(79, 70, 229, 0.85) !important;
        outline-offset: 2px !important;
        box-shadow: 0 0 10px rgba(99, 102, 241, 0.22) !important;
        font-weight: 600 !important;
      }
      a[data-sv-hl="external"]:not([data-sv-card="true"]):not([data-sv-icon-only="true"]) {
        background: linear-gradient(90deg, rgba(245, 158, 11, 0.20), rgba(234, 88, 12, 0.12)) !important;
        outline: 1.5px solid rgba(217, 119, 6, 0.90) !important;
        outline-offset: 2px !important;
        box-shadow: 0 0 12px rgba(245, 158, 11, 0.26) !important;
        font-weight: 600 !important;
      }
      a[data-sv-hl="progress"]:not([data-sv-card="true"]):not([data-sv-icon-only="true"]) {
        background: rgba(99, 102, 241, 0.08) !important;
        box-shadow: inset 0 -1.5px 0 rgba(99, 102, 241, 0.72) !important;
      }
      a[data-sv-hl="completed"]:not([data-sv-card="true"]):not([data-sv-icon-only="true"]) {
        background: rgba(16, 185, 129, 0.07) !important;
      }

      /* Spotlight flash when locating or switching active episode */
      a[data-sv-spotlight="true"] {
        animation: sv-spotlight-ring 1.15s cubic-bezier(0.16, 1, 0.3, 1) 1 !important;
      }

      /* Active badges (::before) — solid high-contrast pills legible on BOTH white and dark pages */
      a[data-sv-hl="in_app"][data-sv-badge]:not([data-sv-icon-only="true"])::before,
      a[data-sv-hl="paused"][data-sv-badge]:not([data-sv-icon-only="true"])::before,
      a[data-sv-hl="external"][data-sv-badge]:not([data-sv-icon-only="true"])::before {
        content: attr(data-sv-badge);
        display: inline-block;
        vertical-align: baseline;
        font: 700 10.5px/1.35 "Segoe UI", system-ui, -apple-system, sans-serif !important;
        letter-spacing: 0.03em;
        padding: 1.5px 6.5px !important;
        margin-right: 6px !important;
        border-radius: 999px !important;
        color: #ffffff !important;
        text-decoration: none !important;
        white-space: nowrap;
      }
      a[data-sv-hl="in_app"][data-sv-badge]:not([data-sv-icon-only="true"])::before {
        background: linear-gradient(135deg, #0284c7, #059669) !important;
        border: 1px solid rgba(125, 211, 252, 0.55) !important;
        animation: sv-playing-pulse 2.2s ease-in-out infinite !important;
      }
      a[data-sv-hl="paused"][data-sv-badge]:not([data-sv-icon-only="true"])::before {
        background: linear-gradient(135deg, #4338ca, #6366f1) !important;
        border: 1px solid rgba(199, 210, 254, 0.50) !important;
        box-shadow: 0 1px 4px rgba(0, 0, 0, 0.22) !important;
      }
      a[data-sv-hl="external"][data-sv-badge]:not([data-sv-icon-only="true"])::before {
        background: linear-gradient(135deg, #d97706, #ea580c) !important;
        border: 1px solid rgba(253, 230, 138, 0.55) !important;
        box-shadow: 0 1px 4px rgba(0, 0, 0, 0.24) !important;
      }

      /* History badges (::after) — keeps left filename column 100% aligned for episode scanning */
      a[data-sv-hl="progress"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after,
      a[data-sv-hl="completed"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after,
      a[data-sv-hl="played"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after {
        content: attr(data-sv-badge);
        display: inline-block;
        vertical-align: baseline;
        font: 600 10.5px/1.3 "Segoe UI", system-ui, -apple-system, sans-serif !important;
        padding: 1px 6px !important;
        margin-left: 6px !important;
        border-radius: 999px !important;
        text-decoration: none !important;
        white-space: nowrap;
      }
      a[data-sv-hl="progress"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after {
        background: #312e81 !important;
        color: #e0e7ff !important;
        border: 1px solid rgba(165, 180, 252, 0.50) !important;
      }
      a[data-sv-hl="completed"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after {
        background: #065f46 !important;
        color: #ecfdf5 !important;
        border: 1px solid rgba(110, 231, 183, 0.45) !important;
      }
      a[data-sv-hl="played"][data-sv-badge]:not([data-sv-card="true"]):not([data-sv-icon-only="true"])::after {
        background: #1e293b !important;
        color: #e2e8f0 !important;
        border: 1px solid rgba(148, 163, 184, 0.42) !important;
      }

      /* Poster / Thumbnail Card Mode — floats pill cleanly over top-left corner of poster cards */
      a[data-sv-card="true"][data-sv-hl] {
        position: relative !important;
      }
      a[data-sv-card="true"][data-sv-hl="in_app"] {
        outline: 2.5px solid #0ea5e9 !important;
        outline-offset: 2px !important;
        box-shadow: 0 0 18px rgba(14, 165, 233, 0.42) !important;
      }
      a[data-sv-card="true"][data-sv-hl="paused"] {
        outline: 2.5px solid #6366f1 !important;
        outline-offset: 2px !important;
      }
      a[data-sv-card="true"][data-sv-hl="external"] {
        outline: 2.5px solid #f59e0b !important;
        outline-offset: 2px !important;
        box-shadow: 0 0 18px rgba(245, 158, 11, 0.38) !important;
      }
      a[data-sv-card="true"][data-sv-hl="progress"][data-sv-badge]::before,
      a[data-sv-card="true"][data-sv-hl="completed"][data-sv-badge]::before,
      a[data-sv-card="true"][data-sv-hl="played"][data-sv-badge]::before {
        content: attr(data-sv-badge);
        font: 700 10.5px/1.35 "Segoe UI", system-ui, -apple-system, sans-serif !important;
        padding: 2px 7px !important;
        border-radius: 999px !important;
        color: #ffffff !important;
        white-space: nowrap;
      }
      a[data-sv-card="true"][data-sv-hl="progress"][data-sv-badge]::before {
        background: rgba(49, 46, 129, 0.94) !important;
        border: 1px solid rgba(165, 180, 252, 0.55) !important;
      }
      a[data-sv-card="true"][data-sv-hl="completed"][data-sv-badge]::before {
        background: rgba(6, 95, 70, 0.94) !important;
        border: 1px solid rgba(110, 231, 183, 0.55) !important;
      }
      a[data-sv-card="true"][data-sv-hl="played"][data-sv-badge]::before {
        background: rgba(30, 41, 59, 0.94) !important;
        border: 1px solid rgba(148, 163, 184, 0.50) !important;
      }
      a[data-sv-card="true"][data-sv-badge]::before {
        position: absolute !important;
        top: 8px !important;
        left: 8px !important;
        z-index: 30 !important;
        margin: 0 !important;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.55) !important;
        pointer-events: none !important;
      }

      ::highlight(sv-find-all) {
        background-color: rgba(250, 204, 21, 0.78);
        color: #000000;
      }
      ::highlight(sv-find-current) {
        background-color: #ea580c;
        color: #ffffff;
      }
      #sv-media-ctx-menu {
        position: fixed;
        z-index: 2147483647;
        min-width: 248px;
        max-width: 350px;
        background: #111622;
        color: #f1f5f9;
        border: 1px solid rgba(148, 163, 184, 0.28);
        border-radius: 10px;
        box-shadow: 0 18px 44px rgba(0, 0, 0, 0.68);
        padding: 6px;
        font: 13px/1.35 "Segoe UI", system-ui, -apple-system, sans-serif;
        user-select: none;
        text-align: left;
      }
      #sv-media-ctx-menu .sv-ctx-header {
        padding: 6px 10px 7px;
        font-size: 11px;
        font-weight: 600;
        color: #94a3b8;
        border-bottom: 1px solid rgba(148, 163, 184, 0.15);
        margin-bottom: 4px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      #sv-media-ctx-menu button.sv-ctx-item {
        all: unset;
        box-sizing: border-box;
        display: flex;
        align-items: center;
        gap: 9px;
        width: 100%;
        padding: 8px 10px;
        border-radius: 6px;
        cursor: pointer;
        color: #e2e8f0;
        font-size: 13px;
      }
      #sv-media-ctx-menu button.sv-ctx-item:hover {
        background: rgba(56, 189, 248, 0.18);
        color: #ffffff;
      }
      #sv-media-ctx-menu .sv-ctx-sep {
        height: 1px;
        background: rgba(148, 163, 184, 0.15);
        margin: 4px 2px;
      }
    `;
    (document.head || document.documentElement).appendChild(style);
  }

  // Fast O(1) lookup tables compiled whenever highlight state changes
  let hlLookup = {
    exact: new Map(),
    decoded: new Map(),
    hostFile: new Map()
  };

  function rebuildHighlightLookup() {
    const exact = new Map();
    const decoded = new Map();
    const hostFile = new Map();
    const AMBIGUOUS = Symbol("ambiguous");

    function registerEntry(rawKey, entry, isPrimaryActive) {
      if (!rawKey) return;
      const norm = normalizeKey(rawKey);
      if (!norm) return;
      if (isPrimaryActive || !exact.has(norm)) {
        exact.set(norm, entry);
      }
      const dec = decodedComparableKey(norm);
      if (dec && (isPrimaryActive || !decoded.has(dec))) {
        decoded.set(dec, entry);
      }
      const hf = extractHostAndFilenameKey(norm);
      if (hf) {
        if (isPrimaryActive) {
          hostFile.set(hf, entry);
        } else if (!hostFile.has(hf)) {
          hostFile.set(hf, entry);
        } else if (hostFile.get(hf) !== entry) {
          hostFile.set(hf, AMBIGUOUS);
        }
      }
    }

    if (currentHighlightKey) {
      const mode =
        currentHighlightMode === "external"
          ? "external"
          : currentHighlightMode === "paused"
            ? "paused"
            : "in_app";
      const badge =
        mode === "external"
          ? "↗ EXTERNAL"
          : mode === "paused"
            ? "⏸ PAUSED"
            : "▶ PLAYING";
      const tooltip =
        mode === "external"
          ? "Currently active in External Player"
          : mode === "paused"
            ? "Paused in StreamVault Player"
            : "Currently playing in StreamVault Player";
      registerEntry(
        currentHighlightKey,
        { state: mode, badge: badge, tooltip: tooltip, isActive: true },
        true
      );
    }

    if (Array.isArray(currentHighlightHistory)) {
      for (let i = 0; i < currentHighlightHistory.length; i++) {
        const h = currentHighlightHistory[i];
        if (!h || !h.key) continue;
        const st =
          h.state === "completed"
            ? "completed"
            : h.state === "progress"
              ? "progress"
              : "played";
        let badge = h.label || (st === "completed" ? "✓ Watched" : "✓ Played");
        if (h.is_last && !currentHighlightKey) {
          if (st === "progress" && badge.indexOf("◐ ") === 0) {
            badge = "↺ " + badge.slice(2);
          } else if (st === "played") {
            badge = "↺ Last Played";
          }
        }
        registerEntry(
          h.key,
          {
            state: st,
            badge: badge,
            tooltip: h.tooltip || badge,
            isActive: false
          },
          false
        );
      }
    }

    // Remove ambiguous host+filename entries
    hostFile.forEach(function (val, k) {
      if (val === AMBIGUOUS) hostFile.delete(k);
    });

    hlLookup = { exact: exact, decoded: decoded, hostFile: hostFile };
  }

  function matchAnchorHighlight(href, pageHostFileCounts) {
    if (!href || (!hlLookup.exact.size && !hlLookup.decoded.size)) return null;
    const norm = normalizeKey(href);
    const byExact = hlLookup.exact.get(norm);
    if (byExact) return byExact;

    const dec = decodedComparableKey(norm);
    const byDec = dec ? hlLookup.decoded.get(dec) : null;
    if (byDec) return byDec;

    if (hlLookup.hostFile.size > 0) {
      const hf = extractHostAndFilenameKey(norm);
      if (hf && (!pageHostFileCounts || (pageHostFileCounts.get(hf) || 0) <= 2)) {
        const byHf = hlLookup.hostFile.get(hf);
        if (byHf) return byHf;
      }
    }
    return null;
  }

  function isPosterCardAnchor(a) {
    if (!a || !a.querySelector) return false;
    const img = a.querySelector("img");
    if (!img) return false;
    const w = Number(img.naturalWidth || img.width || img.offsetWidth || 0);
    const h = Number(img.naturalHeight || img.height || img.offsetHeight || 0);
    return w >= 56 && h >= 56;
  }

  function findSafeRowContainer(a) {
    if (!a || !a.closest) return null;
    const row = a.closest("tr, li, [role='row']");
    if (!row || !row.querySelectorAll) return null;
    const linksInRow = row.querySelectorAll("a[href]");
    if (linksInRow.length > 12) return null;
    return row;
  }

  function clearAnchorHighlightAttrs(a) {
    if (a.hasAttribute("data-sv-hl")) a.removeAttribute("data-sv-hl");
    if (a.hasAttribute("data-sv-badge")) a.removeAttribute("data-sv-badge");
    if (a.hasAttribute("data-sv-active-media")) a.removeAttribute("data-sv-active-media");
    if (a.hasAttribute("data-sv-card")) a.removeAttribute("data-sv-card");
    if (a.hasAttribute("data-sv-icon-only")) a.removeAttribute("data-sv-icon-only");
    if (a.hasAttribute("data-sv-orig-title")) {
      const orig = a.getAttribute("data-sv-orig-title");
      if (orig) a.setAttribute("title", orig);
      else a.removeAttribute("title");
      a.removeAttribute("data-sv-orig-title");
    }
  }

  function triggerSpotlightOnAnchor(a) {
    if (!a || !a.setAttribute) return;
    a.removeAttribute("data-sv-spotlight");
    void a.offsetWidth;
    a.setAttribute("data-sv-spotlight", "true");
    setTimeout(function () {
      if (a && a.removeAttribute) a.removeAttribute("data-sv-spotlight");
    }, 1250);
  }

  function scrollAnchorIntoComfortBand(a, forceCenter) {
    if (!a || !a.scrollIntoView) return;
    try {
      const rect = a.getBoundingClientRect();
      const outOfBand =
        !rect || rect.top < 64 || rect.bottom > (window.innerHeight || 720) - 64;
      if (forceCenter || outOfBand) {
        a.scrollIntoView({ block: "center", behavior: "smooth" });
      }
      triggerSpotlightOnAnchor(a);
    } catch (_) {}
  }

  function applyHighlightToAnchors() {
    ensureStyles();

    // Clear stale row highlights first
    const prevRows = document.querySelectorAll("[data-sv-row-hl]");
    for (let r = 0; r < prevRows.length; r++) {
      prevRows[r].removeAttribute("data-sv-row-hl");
    }

    const anchors = document.querySelectorAll("a[href]");
    if (!hlLookup.exact.size && !hlLookup.decoded.size) {
      for (let i = 0; i < anchors.length; i++) {
        clearAnchorHighlightAttrs(anchors[i]);
      }
      return null;
    }

    // Pre-scan page for text-bearing anchors per normalized URL (suppresses duplicate badges on 16x16 row icons)
    // and count host+filename occurrences so Tier-3 fallback never matches ambiguous filenames.
    const textKeysOnPage = new Set();
    const pageHostFileCounts = new Map();
    for (let i = 0; i < anchors.length; i++) {
      const a = anchors[i];
      if (!a.href) continue;
      const norm = normalizeKey(a.href);
      if (((a.textContent || "").trim()).length > 0) {
        textKeysOnPage.add(norm);
      }
      if (hlLookup.hostFile.size > 0) {
        const hf = extractHostAndFilenameKey(norm);
        if (hf) pageHostFileCounts.set(hf, (pageHostFileCounts.get(hf) || 0) + 1);
      }
    }

    let firstActiveTextMatch = null;
    let firstActiveAnyMatch = null;

    for (let i = 0; i < anchors.length; i++) {
      const a = anchors[i];
      const matched = a.href ? matchAnchorHighlight(a.href, pageHostFileCounts) : null;
      if (!matched) {
        clearAnchorHighlightAttrs(a);
        continue;
      }

      const norm = normalizeKey(a.href);
      const hasOwnText = ((a.textContent || "").trim()).length > 0;
      const cardMode = isPosterCardAnchor(a);
      const iconOnly = !hasOwnText && !cardMode && textKeysOnPage.has(norm);

      if (a.getAttribute("data-sv-hl") !== matched.state) {
        a.setAttribute("data-sv-hl", matched.state);
      }
      if (a.getAttribute("data-sv-badge") !== matched.badge) {
        a.setAttribute("data-sv-badge", matched.badge);
      }

      if (cardMode) {
        a.setAttribute("data-sv-card", "true");
      } else if (a.hasAttribute("data-sv-card")) {
        a.removeAttribute("data-sv-card");
      }

      if (iconOnly) {
        a.setAttribute("data-sv-icon-only", "true");
      } else if (a.hasAttribute("data-sv-icon-only")) {
        a.removeAttribute("data-sv-icon-only");
      }

      if (!a.hasAttribute("data-sv-orig-title")) {
        a.setAttribute("data-sv-orig-title", a.getAttribute("title") || "");
      }
      const origTitle = a.getAttribute("data-sv-orig-title") || "";
      const nextTitle = origTitle
        ? matched.tooltip + " — " + origTitle
        : matched.tooltip;
      if (a.getAttribute("title") !== nextTitle) {
        a.setAttribute("title", nextTitle);
      }

      if (matched.isActive) {
        if (a.getAttribute("data-sv-active-media") !== matched.state) {
          a.setAttribute("data-sv-active-media", matched.state);
        }
        const row = findSafeRowContainer(a);
        if (row && row.getAttribute("data-sv-row-hl") !== matched.state) {
          row.setAttribute("data-sv-row-hl", matched.state);
        }
        if (!firstActiveAnyMatch) firstActiveAnyMatch = a;
        if (!iconOnly && !firstActiveTextMatch) firstActiveTextMatch = a;
      } else if (a.hasAttribute("data-sv-active-media")) {
        a.removeAttribute("data-sv-active-media");
      }
    }

    return firstActiveTextMatch || firstActiveAnyMatch;
  }

  let lastScrolledActiveKey = currentHighlightKey || null;

  // Full state updater pushed by browser_tabs.rs (active playing/paused/external + watched/resume history)
  window.__SV_SET_HIGHLIGHT_STATE__ = function (stateObj, scrollIntoViewIfNeeded) {
    if (!stateObj || typeof stateObj !== "object") return false;
    const prevActiveKey = currentHighlightKey;
    currentHighlightKey = stateObj.active_key || null;
    currentHighlightMode = stateObj.active_mode || (currentHighlightKey ? "in_app" : null);
    if (Array.isArray(stateObj.history)) {
      currentHighlightHistory = stateObj.history;
    }
    rebuildHighlightLookup();
    const match = applyHighlightToAnchors();

    const activeChangedToNew =
      Boolean(currentHighlightKey) && currentHighlightKey !== prevActiveKey;
    if (match && (scrollIntoViewIfNeeded || activeChangedToNew)) {
      lastScrolledActiveKey = currentHighlightKey;
      scrollAnchorIntoComfortBand(match, Boolean(scrollIntoViewIfNeeded));
    }
    return Boolean(match);
  };

  window.__SV_SCROLL_TO_ACTIVE_MEDIA__ = function () {
    const match = applyHighlightToAnchors();
    if (match) {
      scrollAnchorIntoComfortBand(match, true);
      return true;
    }
    return false;
  };

  // Backwards-compatible signature: window.__SV_SET_ACTIVE_MEDIA__(norm_json, mode_json, optionalScroll)
  window.__SV_SET_ACTIVE_MEDIA__ = function (normalizedKey, modeOrScroll, scrollIntoViewIfNeeded) {
    const prevActiveKey = currentHighlightKey;
    currentHighlightKey = normalizedKey || null;
    if (typeof modeOrScroll === "string" && modeOrScroll) {
      currentHighlightMode = modeOrScroll;
    } else if (!currentHighlightKey) {
      currentHighlightMode = null;
    } else if (!currentHighlightMode) {
      currentHighlightMode = "in_app";
    }
    const shouldScroll =
      typeof modeOrScroll === "boolean" ? modeOrScroll : Boolean(scrollIntoViewIfNeeded);
    rebuildHighlightLookup();
    const match = applyHighlightToAnchors();
    const activeChangedToNew =
      Boolean(currentHighlightKey) && currentHighlightKey !== prevActiveKey;
    if (match && (shouldScroll || activeChangedToNew)) {
      lastScrolledActiveKey = currentHighlightKey;
      scrollAnchorIntoComfortBand(match, shouldScroll);
    }
    return Boolean(match);
  };

  rebuildHighlightLookup();

  // Interactive fallback card when user explicitly opens a non-browser container (.mkv/.avi/.ts) in a new tab
  window.__SV_RENDER_DIRECT_MEDIA_TAB__ = function (mediaUrl, displayTitle) {
    ensureStyles();
    const safeTitle = displayTitle || safeDecodeURIComponent((mediaUrl || "").split("/").pop() || "Direct Media Stream");
    const root = document.body || document.documentElement;
    if (!root) return;
    document.title = safeTitle;
    root.innerHTML = "";
    root.style.margin = "0";
    root.style.minHeight = "100vh";
    root.style.background = "#0b0f19";
    root.style.color = "#f1f5f9";
    root.style.display = "flex";
    root.style.alignItems = "center";
    root.style.justifyContent = "center";
    root.style.fontFamily = '"Segoe UI", system-ui, sans-serif';

    const card = document.createElement("div");
    card.style.cssText =
      "max-width:560px;width:92%;background:#131a2a;border:1px solid rgba(56,189,248,0.28);border-radius:14px;padding:26px;box-shadow:0 22px 50px rgba(0,0,0,0.6);";

    const badge = document.createElement("div");
    badge.textContent = "DIRECT MEDIA STREAM";
    badge.style.cssText =
      "display:inline-block;padding:3px 9px;border-radius:999px;background:rgba(56,189,248,0.16);color:#38bdf8;font-size:11px;font-weight:700;letter-spacing:0.06em;margin-bottom:10px;";

    const h2 = document.createElement("h2");
    h2.textContent = safeTitle;
    h2.style.cssText = "margin:0 0 8px;font-size:18px;line-height:1.35;word-break:break-word;color:#f8fafc;";

    const urlBox = document.createElement("div");
    urlBox.textContent = mediaUrl;
    urlBox.style.cssText =
      "font-size:12px;color:#94a3b8;word-break:break-all;background:#0b0f19;padding:9px 11px;border-radius:8px;margin-bottom:18px;border:1px solid rgba(148,163,184,0.14);";

    const btnRow = document.createElement("div");
    btnRow.style.cssText = "display:flex;flex-wrap:wrap;gap:9px;";

    const specs = [
      { id: "play_in_app", label: "▶ Play in StreamVault", bg: "#0284c7", fg: "#ffffff" },
      { id: "play_external", label: "↗ External Player…", bg: "#1e293b", fg: "#e2e8f0" },
      { id: "download", label: "⬇ Download Media…", bg: "#1e293b", fg: "#e2e8f0" },
      { id: "copy_url", label: "📋 Copy URL", bg: "#1e293b", fg: "#e2e8f0" }
    ];

    specs.forEach(function (sp) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = sp.label;
      b.style.cssText =
        "border:1px solid rgba(148,163,184,0.24);border-radius:8px;padding:9px 14px;font-size:13px;font-weight:600;cursor:pointer;background:" +
        sp.bg +
        ";color:" +
        sp.fg +
        ";";
      b.addEventListener("click", function () {
        sendBridge(sp.id, mediaUrl, safeTitle);
      });
      btnRow.appendChild(b);
    });

    card.appendChild(badge);
    card.appendChild(h2);
    card.appendChild(urlBox);
    card.appendChild(btnRow);
    root.appendChild(card);
  };

  // Collect page media in exact DOM order (Video, Audio, Subtitle links)
  function collectPageMediaCatalog() {
    const seen = new Set();
    const items = [];
    const anchors = document.querySelectorAll("a[href]");
    for (let i = 0; i < anchors.length; i++) {
      const a = anchors[i];
      if (!a.href) continue;
      const text = (a.textContent || "").trim();
      if (text === "../" || text === ".." || /^parent directory$/i.test(text)) continue;
      const dlAttr = a.getAttribute("download");
      const info = classifyFileUrl(a.href, dlAttr);
      if (!info) continue;
      const key = normalizeKey(info.href);
      if (seen.has(key)) continue;
      seen.add(key);
      items.push({
        url: info.href,
        filename: info.filename,
        title: text && text.length >= 2 ? text.slice(0, 220) : info.filename,
        kind: info.kind,
        ext: info.ext
      });
      if (items.length >= 800) break;
    }

    const mediaEls = document.querySelectorAll(
      "video[src], audio[src], video source[src], audio source[src]"
    );
    for (let j = 0; j < mediaEls.length; j++) {
      const src = mediaEls[j].src;
      if (!src) continue;
      const info = classifyFileUrl(src, null);
      if (!info) continue;
      const key = normalizeKey(info.href);
      if (seen.has(key)) continue;
      seen.add(key);
      items.push({
        url: info.href,
        filename: info.filename,
        title: info.filename,
        kind: info.kind,
        ext: info.ext
      });
    }

    return {
      page_url: window.location.href || "",
      page_title: document.title || "",
      items: items
    };
  }

  let lastCatalogHash = "";
  function scanAndPublishPageMedia(forceSend) {
    if (!IS_TOP_FRAME) {
      return { page_url: "", page_title: "", items: [] };
    }
    const catalog = collectPageMediaCatalog();
    const items = catalog.items;
    const sig =
      catalog.page_url +
      "::" +
      items.length +
      "::" +
      (items[0] ? items[0].url : "") +
      "::" +
      (items[items.length - 1] ? items[items.length - 1].url : "");
    if (!forceSend && sig === lastCatalogHash) return catalog;
    lastCatalogHash = sig;

    sendBridge("page_media_catalog", catalog.page_url, catalog.page_title, catalog);
    return catalog;
  }

  window.__SV_COLLECT_PAGE_MEDIA__ = function () {
    return collectPageMediaCatalog();
  };

  window.__SV_REQUEST_PAGE_MEDIA__ = function () {
    return scanAndPublishPageMedia(true);
  };

  // Non-destructive Find-in-Page engine (CSS Custom Highlight API + smooth scroll, never steals focus)
  let findState = {
    query: "",
    ranges: [],
    index: -1
  };

  function clearFindHighlights() {
    findState.ranges = [];
    findState.index = -1;
    findState.query = "";
    try {
      if (window.CSS && CSS.highlights) {
        CSS.highlights.delete("sv-find-all");
        CSS.highlights.delete("sv-find-current");
      }
    } catch (_) {}
  }

  function collectTextRanges(needleLower) {
    const results = [];
    const root = document.body || document.documentElement;
    if (!root || !needleLower) return results;
    const walker = document.createTreeWalker(
      root,
      NodeFilter.SHOW_TEXT,
      {
        acceptNode: function (node) {
          const parent = node.parentElement;
          if (!parent) return NodeFilter.FILTER_REJECT;
          const tag = parent.tagName;
          if (
            tag === "SCRIPT" ||
            tag === "STYLE" ||
            tag === "NOSCRIPT" ||
            tag === "TEMPLATE"
          ) {
            return NodeFilter.FILTER_REJECT;
          }
          if (parent.closest("#sv-media-ctx-menu")) return NodeFilter.FILTER_REJECT;
          const style = window.getComputedStyle(parent);
          if (style.display === "none" || style.visibility === "hidden") {
            return NodeFilter.FILTER_REJECT;
          }
          return NodeFilter.FILTER_ACCEPT;
        }
      }
    );

    let node;
    while ((node = walker.nextNode())) {
      const val = node.nodeValue;
      if (!val) continue;
      const lower = val.toLowerCase();
      let pos = 0;
      while (pos <= lower.length - needleLower.length) {
        const idx = lower.indexOf(needleLower, pos);
        if (idx === -1) break;
        const r = document.createRange();
        r.setStart(node, idx);
        r.setEnd(node, idx + needleLower.length);
        results.push(r);
        pos = idx + needleLower.length;
        if (results.length >= 1500) return results;
      }
    }
    return results;
  }

  function rangesStillConnected(ranges) {
    if (!ranges || !ranges.length) return false;
    const first = ranges[0];
    const last = ranges[ranges.length - 1];
    return Boolean(
      first &&
        first.startContainer &&
        first.startContainer.isConnected &&
        last &&
        last.startContainer &&
        last.startContainer.isConnected
    );
  }

  function applyFindVisuals() {
    ensureStyles();
    const total = findState.ranges.length;
    if (!total || findState.index < 0) {
      try {
        if (window.CSS && CSS.highlights) {
          CSS.highlights.delete("sv-find-all");
          CSS.highlights.delete("sv-find-current");
        }
      } catch (_) {}
      return;
    }

    const activeRange = findState.ranges[findState.index];
    try {
      if (window.CSS && CSS.highlights && typeof Highlight === "function") {
        const others = findState.ranges.filter(function (_, idx) {
          return idx !== findState.index;
        });
        CSS.highlights.set("sv-find-all", new Highlight(...others));
        CSS.highlights.set("sv-find-current", new Highlight(activeRange));
      } else {
        const sel = window.getSelection();
        if (sel) {
          sel.removeAllRanges();
          sel.addRange(activeRange);
        }
      }
    } catch (_) {}

    try {
      const rect = activeRange.getBoundingClientRect();
      if (rect && (rect.top < 48 || rect.bottom > window.innerHeight - 48)) {
        const el = activeRange.startContainer.parentElement;
        if (el && el.scrollIntoView) {
          el.scrollIntoView({ block: "center", behavior: "smooth" });
        }
      }
    } catch (_) {}
  }

  window.__SV_FIND_IN_PAGE__ = function (query, forward, clearOnly, stayOnCurrentIfSame) {
    if (!IS_TOP_FRAME) {
      return { query: "", current: 0, total: 0 };
    }
    const clean = (query || "").trim();
    if (clearOnly || !clean) {
      clearFindHighlights();
      const emptyRes = { query: "", current: 0, total: 0 };
      sendBridge("find_result", window.location.href, "", emptyRes);
      return emptyRes;
    }

    const lower = clean.toLowerCase();
    if (findState.query !== lower || !rangesStillConnected(findState.ranges)) {
      findState.query = lower;
      findState.ranges = collectTextRanges(lower);
      findState.index = findState.ranges.length > 0 ? 0 : -1;
    } else if (findState.ranges.length > 0 && !stayOnCurrentIfSame) {
      const step = forward === false ? -1 : 1;
      findState.index =
        (findState.index + step + findState.ranges.length) % findState.ranges.length;
    }

    applyFindVisuals();
    const res = {
      query: clean,
      current: findState.index >= 0 ? findState.index + 1 : 0,
      total: findState.ranges.length
    };
    sendBridge("find_result", window.location.href, "", res);
    return res;
  };

  function removeContextMenu() {
    const existing = document.getElementById("sv-media-ctx-menu");
    if (existing) existing.remove();
  }

  function findHttpAnchor(target) {
    const el =
      target && target.nodeType === 3 ? target.parentElement : target;
    const anchor = el && el.closest ? el.closest("a[href]") : null;
    if (!anchor || !anchor.href) return null;
    try {
      const u = new URL(anchor.href, window.location.href);
      if (u.protocol === "http:" || u.protocol === "https:") return anchor;
    } catch (_) {}
    return null;
  }

  function getAnchorLabel(anchor) {
    if (!anchor) return "";
    const text = (anchor.textContent || "").trim().replace(/\s+/g, " ");
    if (text) return text.slice(0, 180);
    const dl = anchor.getAttribute ? anchor.getAttribute("download") : "";
    if (dl) return dl.slice(0, 180);
    const info = classifyFileUrl(anchor.href, dl);
    return info ? info.filename : safeDecodeURIComponent((anchor.href || "").split("/").pop() || "");
  }

  // Middle-click (button === 1) -> open link in background tab
  document.addEventListener(
    "auxclick",
    function (e) {
      if (e.button !== 1) return;
      const anchor = findHttpAnchor(e.target);
      if (!anchor) return;
      if (!(initialRawInvoke || getRawInvoke())) return;
      e.preventDefault();
      e.stopPropagation();
      sendBridge("open_background_tab", anchor.href, getAnchorLabel(anchor));
    },
    true
  );

  // Coordinated Single-Click vs Double-Click on Media Links:
  // - Ctrl/Cmd + Left-click opens any HTTP(S) link in a background tab.
  // - Plain Left-click on a direct media link debounces 220ms so click #1 does not trigger
  //   navigation before click #2 of a double-click arrives!
  // - Double-clicking a media link opens StreamVault's Media Link Action Prompt modal.
  let pendingMediaClickTimer = null;
  let pendingMediaHref = null;

  function clearPendingMediaClick() {
    if (pendingMediaClickTimer) {
      clearTimeout(pendingMediaClickTimer);
      pendingMediaClickTimer = null;
    }
    pendingMediaHref = null;
  }

  document.addEventListener(
    "click",
    function (e) {
      const menu = document.getElementById("sv-media-ctx-menu");
      if (menu && !menu.contains(e.target)) {
        removeContextMenu();
      }

      if (e.button !== 0) return;
      const anchor = findHttpAnchor(e.target);
      if (!anchor) return;
      if (!(initialRawInvoke || getRawInvoke())) return;

      // Ctrl/Cmd + Left-Click -> Background tab
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        e.stopPropagation();
        clearPendingMediaClick();
        sendBridge("open_background_tab", anchor.href, getAnchorLabel(anchor));
        return;
      }

      // Plain unmodified Left-Click on a direct media link
      if (!e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey && isMediaAnchor(anchor)) {
        e.preventDefault();
        e.stopPropagation();
        const href = anchor.href;
        const label = getAnchorLabel(anchor);

        if (e.detail >= 2 || (pendingMediaHref && pendingMediaHref === href)) {
          clearPendingMediaClick();
          sendBridge("dblclick_media_prompt", href, label);
          return;
        }

        clearPendingMediaClick();
        pendingMediaHref = href;
        pendingMediaClickTimer = setTimeout(function () {
          pendingMediaClickTimer = null;
          pendingMediaHref = null;
          sendBridge("single_click_media", href, label);
        }, 220);
      }
    },
    true
  );

  document.addEventListener(
    "dblclick",
    function (e) {
      if (e.button !== 0) return;
      const anchor = findHttpAnchor(e.target);
      if (!anchor) return;
      if (!isMediaAnchorOrText(anchor)) return;
      if (!(initialRawInvoke || getRawInvoke())) return;

      e.preventDefault();
      e.stopPropagation();
      clearPendingMediaClick();
      sendBridge("dblclick_media_prompt", anchor.href, getAnchorLabel(anchor));
    },
    true
  );

  // Intercept Ctrl+F inside child webpages so StreamVault's Find-in-Page bar opens reliably
  document.addEventListener(
    "keydown",
    function (e) {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        e.stopPropagation();
        sendBridge("open_find_bar", window.location.href, "");
        return;
      }
      if (e.key === "Escape") {
        removeContextMenu();
      }
    },
    true
  );

  // Right-click context menu on links inside browsing tabs
  document.addEventListener(
    "contextmenu",
    function (e) {
      const anchor = findHttpAnchor(e.target);
      if (!anchor || !(initialRawInvoke || getRawInvoke())) {
        removeContextMenu();
        return;
      }

      e.preventDefault();
      e.stopPropagation();
      removeContextMenu();
      ensureStyles();

      const href = anchor.href;
      const linkText = getAnchorLabel(anchor);
      const media = isMediaAnchorOrText(anchor);

      const menu = document.createElement("div");
      menu.id = "sv-media-ctx-menu";

      const header = document.createElement("div");
      header.className = "sv-ctx-header";
      header.textContent = linkText || safeDecodeURIComponent(href.split("/").pop() || href);
      menu.appendChild(header);

      const actions = media
        ? [
            { id: "play_in_app", label: "▶  Play in StreamVault" },
            { id: "play_external", label: "↗  Play in External Player…" },
            { id: "download", label: "⬇  Download Media…" },
            { sep: true },
            { id: "open_background_tab", label: "⧉  Open Link in Background Tab" },
            { id: "open_new_tab", label: "⊞  Open Link in New Tab" },
            { id: "copy_url", label: "📋  Copy Media URL" }
          ]
        : [
            { id: "open_background_tab", label: "⧉  Open Link in Background Tab" },
            { id: "open_new_tab", label: "⊞  Open Link in New Tab" },
            { id: "probe_and_inspect", label: "🎬  Inspect / Open as Media…" },
            { id: "play_in_app", label: "▶  Play in StreamVault" },
            { id: "play_external", label: "↗  Play in External Player…" },
            { id: "download", label: "⬇  Download File…" },
            { sep: true },
            { id: "copy_url", label: "📋  Copy Link URL" }
          ];

      actions.forEach(function (item) {
        if (item.sep) {
          const sep = document.createElement("div");
          sep.className = "sv-ctx-sep";
          menu.appendChild(sep);
          return;
        }
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "sv-ctx-item";
        btn.textContent = item.label;
        btn.addEventListener("click", function (ev) {
          ev.preventDefault();
          ev.stopPropagation();
          removeContextMenu();
          sendBridge(item.id, href, linkText);
        });
        menu.appendChild(btn);
      });

      (document.body || document.documentElement).appendChild(menu);
      const rect = menu.getBoundingClientRect();
      const maxX = Math.max(8, window.innerWidth - rect.width - 10);
      const maxY = Math.max(8, window.innerHeight - rect.height - 10);
      menu.style.left = Math.min(e.clientX, maxX) + "px";
      menu.style.top = Math.min(e.clientY, maxY) + "px";
    },
    true
  );

  window.addEventListener("scroll", removeContextMenu, { passive: true });

  // Observe dynamic DOM updates so directory listings & SPA pages keep active + watched/progress highlights & media catalog fresh
  let scanTimer = null;
  let hlTimer = null;
  const obs = new MutationObserver(function () {
    if (hlLookup.exact.size || hlLookup.decoded.size) {
      if (hlTimer) clearTimeout(hlTimer);
      hlTimer = setTimeout(function () {
        applyHighlightToAnchors();
      }, 65);
    }
    if (scanTimer) clearTimeout(scanTimer);
    scanTimer = setTimeout(function () {
      scanAndPublishPageMedia(false);
    }, 320);
  });

  function startObserver() {
    ensureStyles();
    if (document.documentElement) {
      obs.observe(document.documentElement, { childList: true, subtree: true });
    }
    if (hlLookup.exact.size || hlLookup.decoded.size) {
      applyHighlightToAnchors();
    }
    scanAndPublishPageMedia(true);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", startObserver, { once: true });
  } else {
    startObserver();
  }
})();
