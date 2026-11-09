import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import {
  closeModal,
  escapeHtml,
  formatBytes,
  formatSpeed,
  formatTimeMs,
  openModal,
  showToast,
} from "./utils.js";

const SPEED_STEPS = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
const SUB_EXTS = new Set(["srt", "ass", "ssa", "vtt", "sub"]);

let switchViewFn = null;
let hudIdleTimer = null;
let canvasClickTimer = null;
let lastViewportSig = "";
let lastTipSig = "";
let nativeTipActive = false;

function normalizeMediaUrlKey(raw) {
  if (!raw) return "";
  if (raw.startsWith("local://")) return raw;
  try {
    const u = new URL(raw);
    u.hash = "";
    return u.href.replace(/%[0-9a-f]{2}/gi, (m) => m.toUpperCase());
  } catch (_) {
    return String(raw).trim();
  }
}

function safeDecodeUri(s) {
  if (!s) return "";
  try {
    return decodeURIComponent(s);
  } catch (_) {
    return s;
  }
}

function extractDecodedStemAndExt(raw) {
  try {
    const clean = String(raw || "").trim();
    let lastSeg = "";
    if (/^https?:\/\//i.test(clean)) {
      const u = new URL(clean);
      const parts = u.pathname.split("/").filter(Boolean);
      lastSeg = parts.length ? safeDecodeUri(parts[parts.length - 1]) : "";
    } else {
      const parts = clean.split(/[\\/]/).filter(Boolean);
      lastSeg = parts.length ? safeDecodeUri(parts[parts.length - 1]) : "";
    }
    const dot = lastSeg.lastIndexOf(".");
    if (dot <= 0) return null;
    return {
      stem: lastSeg.slice(0, dot).trim().toLowerCase(),
      ext: lastSeg.slice(dot + 1).trim().toLowerCase(),
      filename: lastSeg,
    };
  } catch (_) {
    return null;
  }
}

function matchSubtitleItemsForMediaUrl(mediaUrl, subItems) {
  const info = extractDecodedStemAndExt(mediaUrl);
  if (!info || !info.stem) return [];
  const exact = [];
  const suffixed = [];
  for (const sub of subItems) {
    const sInfo = extractDecodedStemAndExt(sub.url || sub.filename || "");
    if (!sInfo || !SUB_EXTS.has(sInfo.ext)) continue;
    if (sInfo.stem === info.stem) {
      exact.push(sub.url);
    } else if (sInfo.stem.startsWith(info.stem)) {
      const rest = sInfo.stem.slice(info.stem.length);
      if (rest.startsWith(".") || rest.startsWith("_") || rest.startsWith("-")) {
        suffixed.push(sub.url);
      }
    }
  }
  return [...exact, ...suffixed];
}

function findChapterAtTimeMs(chapters, timeMs) {
  if (!Array.isArray(chapters) || chapters.length === 0) return null;
  let active = null;
  for (const ch of chapters) {
    if (timeMs >= (ch.time_ms || 0)) {
      active = ch;
    } else {
      break;
    }
  }
  return active;
}

function syncQueueControlsAndSmtc() {
  const q = store.playlistQueue || [];
  const idx = store.playlistIndex;
  const hasPrev = q.length > 1 && idx > 0;
  const hasNext = q.length > 1 && idx >= 0 && idx < q.length - 1;

  const btnPrev = document.getElementById("btn-player-prev");
  const btnNext = document.getElementById("btn-player-next");
  const btnQueue = document.getElementById("btn-player-queue");
  const pill = document.getElementById("player-queue-pill");

  if (btnPrev) btnPrev.disabled = !hasPrev;
  if (btnNext) btnNext.disabled = !hasNext;
  if (btnQueue) {
    btnQueue.textContent = q.length > 1 ? `☰ Queue (${q.length})` : "☰ Queue";
  }
  if (pill) {
    if (q.length > 1 && idx >= 0) {
      pill.textContent = `Ep ${idx + 1} / ${q.length}`;
      pill.classList.remove("hidden");
    } else {
      pill.classList.add("hidden");
    }
  }

  invokeCmd("player_sync_smtc", { hasPrev, hasNext }).catch(() => {});
}

async function tryPopulateSiblingQueueForUrl(targetUrl) {
  const targetKey = normalizeMediaUrlKey(targetUrl);
  // If targetUrl is already in current playlistQueue, just update playlistIndex!
  const existingIdx = (store.playlistQueue || []).findIndex(
    (item) => !item.isLocal && normalizeMediaUrlKey(item.url) === targetKey
  );
  if (existingIdx !== -1) {
    store.playlistIndex = existingIdx;
    syncQueueControlsAndSmtc();
    return store.playlistQueue[existingIdx];
  }

  // Otherwise check cached page media catalogs (active tab first, then other open tabs)
  const catalogs = [];
  const activeTab = (store.tabs || []).find((t) => t.is_active);
  if (activeTab && store.pageMediaByTab[activeTab.id]) {
    catalogs.push(store.pageMediaByTab[activeTab.id]);
  } else if (activeTab) {
    try {
      const fetched = await invokeCmd("tab_get_page_media", { tabId: activeTab.id });
      if (fetched) {
        store.pageMediaByTab[activeTab.id] = fetched;
        catalogs.push(fetched);
      }
    } catch (_) {}
  }
  for (const [tid, cat] of Object.entries(store.pageMediaByTab || {})) {
    if (!activeTab || tid !== activeTab.id) catalogs.push(cat);
  }

  for (const cat of catalogs) {
    const items = cat?.items || [];
    const mediaItems = items.filter((i) => i.kind === "video" || i.kind === "audio");
    const subItems = items.filter((i) => i.kind === "subtitle");
    const matchIdx = mediaItems.findIndex((m) => normalizeMediaUrlKey(m.url) === targetKey);
    if (matchIdx !== -1) {
      store.playlistQueue = mediaItems.map((m) => ({
        url: m.url,
        title: m.filename || m.title || m.url,
        isLocal: false,
        localPath: null,
        matchedSubtitles: matchSubtitleItemsForMediaUrl(m.url, subItems),
      }));
      store.playlistIndex = matchIdx;
      syncQueueControlsAndSmtc();
      return store.playlistQueue[matchIdx];
    }
  }

  // Single standalone stream item
  store.playlistQueue = [
    {
      url: targetUrl,
      title: extractDecodedStemAndExt(targetUrl)?.filename || targetUrl,
      isLocal: false,
      localPath: null,
      matchedSubtitles: [],
    },
  ];
  store.playlistIndex = 0;
  syncQueueControlsAndSmtc();
  return store.playlistQueue[0];
}

export function hidePlayerTimeTooltip() {
  const timelineTip = document.getElementById("player-timeline-tip");
  if (timelineTip) timelineTip.classList.add("hidden");
  if (nativeTipActive) {
    nativeTipActive = false;
    lastTipSig = "";
    invokeCmd("player_set_time_tooltip", {
      visible: false,
      x: 0,
      y: 0,
      text: "",
    }).catch(() => {});
  }
}

export function initPlayerModule({ onSwitchView, onDownloadMedia, onOpenExternal }) {
  switchViewFn = onSwitchView;

  const btnBackBrowser = document.getElementById("btn-player-back-browser");
  const btnOpenLocal = document.getElementById("btn-player-open-local");
  const btnOpenUrl = document.getElementById("btn-player-open-url-prompt");
  const btnHandoffExt = document.getElementById("btn-player-handoff-ext");
  const btnSavePerm = document.getElementById("btn-player-save-perm");
  const queuePill = document.getElementById("player-queue-pill");

  const canvas = document.getElementById("player-video-canvas");
  const timeline = document.getElementById("player-timeline");
  const timelineTip = document.getElementById("player-timeline-tip");

  const btnPrev = document.getElementById("btn-player-prev");
  const btnPlay = document.getElementById("btn-player-play");
  const btnNext = document.getElementById("btn-player-next");
  const btnStop = document.getElementById("btn-player-stop");
  const btnSeekBack = document.getElementById("btn-player-seek-back");
  const btnSeekFwd = document.getElementById("btn-player-seek-fwd");
  const btnQueue = document.getElementById("btn-player-queue");
  const btnMute = document.getElementById("btn-player-mute");
  const volSlider = document.getElementById("player-volume-slider");

  const speedSelect = document.getElementById("player-speed-select");
  const audioSelect = document.getElementById("player-audio-select");
  const subSelect = document.getElementById("player-sub-select");
  const btnLoadSub = document.getElementById("btn-player-load-sub");
  const btnSyncFx = document.getElementById("btn-player-sync-fx");
  const btnDialogueBoost = document.getElementById("btn-player-dialogue-boost");
  const btnScreenshot = document.getElementById("btn-player-screenshot");
  const aspectSelect = document.getElementById("player-aspect-select");
  const btnPip = document.getElementById("btn-player-pip");
  const btnFullscreen = document.getElementById("btn-player-fullscreen");

  btnBackBrowser.addEventListener("click", () => switchViewFn("browser"));
  btnOpenLocal.addEventListener("click", () => promptOpenLocalMediaFiles());
  btnOpenUrl.addEventListener("click", () => promptDirectMediaUrlModal());
  queuePill?.addEventListener("click", () => openPlaylistQueueModal());
  btnQueue?.addEventListener("click", () => openPlaylistQueueModal());
  btnPrev?.addEventListener("click", () => playPrevInQueue());
  btnNext?.addEventListener("click", () => playNextInQueue(false));

  btnHandoffExt.addEventListener("click", () => {
    const url = store.playerState?.original_url;
    if (!url) {
      showToast("No media currently loaded", "error");
      return;
    }
    const cleanTarget = url.startsWith("local://") ? url.slice("local://".length) : url;
    onOpenExternal(cleanTarget, store.playerState.title);
  });

  btnSavePerm.addEventListener("click", () => {
    const url = store.playerState?.original_url;
    if (!url) {
      showToast("No media currently loaded", "error");
      return;
    }
    if (url.startsWith("local://")) {
      showToast("This file is already stored locally on your computer", "info");
      return;
    }
    onDownloadMedia(url);
  });

  // Mouse interactions on video canvas (when audio card or empty canvas receives mouse events)
  canvas.addEventListener("click", () => {
    clearTimeout(canvasClickTimer);
    canvasClickTimer = setTimeout(() => {
      if (store.playerState.active) {
        togglePlayPause();
      }
    }, 210);
  });

  canvas.addEventListener("dblclick", (ev) => {
    ev.preventDefault();
    clearTimeout(canvasClickTimer);
    toggleFullscreen();
  });

  canvas.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      const step = ev.deltaY < 0 ? 5 : -5;
      setVolume((store.playerState.volume ?? 100) + step);
    },
    { passive: false }
  );

  // Timeline hover & click-to-seek with MKV Chapter title readout!
  timeline.addEventListener("mousemove", (ev) => {
    const dur = store.playerState.duration_ms || 0;
    if (dur <= 0) return;
    const rect = timeline.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
    const hoverMs = Math.round(dur * ratio);
    const chap = findChapterAtTimeMs(store.playerState.chapters, hoverMs);
    const label = chap
      ? `${formatTimeMs(hoverMs)} • ${chap.title}`
      : formatTimeMs(hoverMs);

    const useNativeTip =
      store.playerState.active &&
      !store.playerState.pip_active &&
      store.playerState.engine_ready;

    if (useNativeTip) {
      timelineTip.classList.add("hidden");
      const rx = Math.round(ev.clientX);
      const ry = Math.round(rect.top);
      const sig = `${rx}:${ry}:${label}`;
      if (sig !== lastTipSig) {
        lastTipSig = sig;
        nativeTipActive = true;
        invokeCmd("player_set_time_tooltip", {
          visible: true,
          x: rx,
          y: ry,
          text: label,
        }).catch(() => {});
      }
    } else {
      timelineTip.style.left = `${ratio * 100}%`;
      timelineTip.textContent = label;
      timelineTip.classList.remove("hidden");
    }
  });

  timeline.addEventListener("mouseleave", () => {
    hidePlayerTimeTooltip();
  });

  timeline.addEventListener("click", async (ev) => {
    const dur = store.playerState.duration_ms || 0;
    if (dur <= 0) return;
    const rect = timeline.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
    const targetMs = Math.round(dur * ratio);
    try {
      const snap = await invokeCmd("player_seek_absolute", { positionMs: targetMs });
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnPlay.addEventListener("click", togglePlayPause);
  btnStop.addEventListener("click", async () => {
    try {
      const snap = await invokeCmd("player_stop");
      store.streamCacheStatus = null;
      updatePlayerState(snap);
      syncQueueControlsAndSmtc();
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnSeekBack.addEventListener("click", () => seekByConfiguredInterval(-1));
  btnSeekFwd.addEventListener("click", () => seekByConfiguredInterval(1));

  btnMute.addEventListener("click", async () => {
    try {
      const snap = await invokeCmd("player_toggle_mute");
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  volSlider.addEventListener("input", () => {
    setVolume(Number(volSlider.value));
  });

  speedSelect.addEventListener("change", async () => {
    try {
      const snap = await invokeCmd("player_set_speed", { speed: Number(speedSelect.value) });
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  audioSelect.addEventListener("change", async () => {
    const id = Number(audioSelect.value);
    if (!id) return;
    try {
      const snap = await invokeCmd("player_set_audio_track", { trackId: id });
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  subSelect.addEventListener("change", async () => {
    const val = subSelect.value;
    const trackId = val === "off" || val === "" ? null : Number(val);
    try {
      const snap = await invokeCmd("player_set_subtitle_track", { trackId });
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnLoadSub.addEventListener("click", async () => {
    try {
      const maybeSnap = await invokeCmd("player_browse_and_load_subtitle");
      if (maybeSnap) {
        updatePlayerState(maybeSnap);
        showToast("Loaded external subtitle track", "success");
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnSyncFx?.addEventListener("click", () => openSyncAndSubtitleFxModal());
  btnDialogueBoost?.addEventListener("click", () => toggleDialogueBoost());
  btnScreenshot?.addEventListener("click", () => capturePlayerScreenshot());

  aspectSelect.addEventListener("change", async () => {
    try {
      const snap = await invokeCmd("player_set_aspect_ratio", { ratio: aspectSelect.value });
      updatePlayerState(snap);
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnPip.addEventListener("click", async () => {
    try {
      const next = !store.playerState.pip_active;
      const snap = await invokeCmd("player_set_pip", { enable: next });
      updatePlayerState(snap);
      syncPlayerSurfaceViewport();
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnFullscreen.addEventListener("click", () => toggleFullscreen());

  // Fullscreen mouse-move wake
  document.getElementById("view-player").addEventListener("mousemove", wakeFullscreenHud);

  // Sync Win32 libmpv child HWND bounds on layout resize
  if (typeof ResizeObserver !== "undefined") {
    const ro = new ResizeObserver(() => syncPlayerSurfaceViewport(true));
    ro.observe(canvas);
  }
  window.addEventListener("resize", () => {
    hidePlayerTimeTooltip();
    syncPlayerSurfaceViewport(true);
  });
}

export async function openLocalMediaFile(filePath, startPositionMs = null) {
  try {
    switchViewFn("player");
    const name = filePath.split(/[\\/]/).pop() || filePath;
    document.getElementById("player-title-text").textContent = name;
    store.streamCacheStatus = null;
    const snap = await invokeCmd("player_open_local_file", {
      filePath,
      startPositionMs,
    });
    updatePlayerState(snap);
    store.recentMedia = await invokeCmd("recent_media_list", { limit: 60 });
    syncQueueControlsAndSmtc();
    syncPlayerSurfaceViewport();
  } catch (e) {
    showToast(`Failed opening local file: ${e}`, "error");
  }
}

export async function promptOpenLocalMediaFiles() {
  try {
    const paths = await invokeCmd("player_browse_and_open_local_media");
    if (!paths || paths.length === 0) return;
    const mediaPaths = [];
    const subPaths = [];
    for (const p of paths) {
      const ext = (p.split(".").pop() || "").toLowerCase();
      if (SUB_EXTS.has(ext)) subPaths.push(p);
      else mediaPaths.push(p);
    }
    await handleDroppedOrPickedLocalFiles(mediaPaths, subPaths);
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function handleDroppedOrPickedLocalFiles(mediaPaths = [], subtitlePaths = []) {
  if (mediaPaths.length > 0) {
    store.playlistQueue = mediaPaths.map((p) => ({
      url: `local://${p.replace(/\\/g, "/")}`,
      title: p.split(/[\\/]/).pop() || p,
      isLocal: true,
      localPath: p,
      matchedSubtitles: [],
    }));
    store.playlistIndex = 0;
    await openLocalMediaFile(mediaPaths[0], null);
    for (const sp of subtitlePaths) {
      try {
        const snap = await invokeCmd("player_load_external_subtitle_path", { filePath: sp });
        updatePlayerState(snap);
      } catch (_) {}
    }
    if (mediaPaths.length > 1) {
      showToast(`Queued ${mediaPaths.length} local files in playlist`, "success");
    }
    return;
  }

  if (subtitlePaths.length > 0) {
    if (!store.playerState.active) {
      showToast("Start playing a video first before dropping a subtitle file", "error");
      return;
    }
    for (const sp of subtitlePaths) {
      try {
        const snap = await invokeCmd("player_load_external_subtitle_path", { filePath: sp });
        updatePlayerState(snap);
        const subName = sp.split(/[\\/]/).pop() || sp;
        showToast(`Loaded subtitle: ${subName}`, "success");
      } catch (e) {
        showToast(String(e), "error");
      }
    }
  }
}

export async function setCustomPlaylistAndPlay(items, startIndex = 0) {
  if (!Array.isArray(items) || items.length === 0) return;
  store.playlistQueue = items;
  store.playlistIndex = Math.max(0, Math.min(items.length - 1, startIndex));
  await playQueueAtIndex(store.playlistIndex, false);
}

export async function playQueueAtIndex(index, fromAutoAdvance = false) {
  const q = store.playlistQueue || [];
  if (index < 0 || index >= q.length) return false;
  store.playlistIndex = index;
  const entry = q[index];
  syncQueueControlsAndSmtc();

  if (entry.isLocal && entry.localPath) {
    await openLocalMediaFile(entry.localPath, fromAutoAdvance ? 0 : null);
  } else {
    await initiateInAppPlayback(entry.url, entry.title, fromAutoAdvance ? 0 : null, true);
  }
  return true;
}

export async function playNextInQueue(fromAutoAdvance = false) {
  const q = store.playlistQueue || [];
  const nextIdx = store.playlistIndex + 1;
  if (nextIdx >= 0 && nextIdx < q.length) {
    const nextItem = q[nextIdx];
    if (fromAutoAdvance) {
      wakeFullscreenHud();
      showToast(`▶ Next episode: ${nextItem.title}`, "info");
    }
    return playQueueAtIndex(nextIdx, fromAutoAdvance);
  }
  return false;
}

export async function playPrevInQueue() {
  const prevIdx = store.playlistIndex - 1;
  if (prevIdx >= 0 && prevIdx < (store.playlistQueue || []).length) {
    return playQueueAtIndex(prevIdx, false);
  }
  return false;
}

export async function handlePlayerPlaybackEnded(payload) {
  // MPV_END_FILE_REASON_EOF === 0 (natural end-of-file, not user stop/replace)
  const reason = payload?.reason;
  if (reason === 4 || payload?.error) {
    wakeFullscreenHud();
    showToast(
      `Playback error: ${payload?.error || "Failed decoding or streaming media"}`,
      "error"
    );
    return;
  }
  const autoNext = store.settings?.auto_play_next_episode !== false;
  if (reason === 0 && autoNext) {
    const advanced = await playNextInQueue(true);
    if (advanced) return;
  }
  if (reason === 0) {
    wakeFullscreenHud();
    store.recentMedia = await invokeCmd("recent_media_list", { limit: 60 }).catch(
      () => store.recentMedia
    );
    showToast("Media playback finished", "info");
  }
}

export async function handleMediaKeyCommand(action) {
  switch (action) {
    case "next":
      await playNextInQueue(false);
      break;
    case "previous":
      await playPrevInQueue();
      break;
    case "stop":
      try {
        const snap = await invokeCmd("player_stop");
        store.streamCacheStatus = null;
        updatePlayerState(snap);
        syncQueueControlsAndSmtc();
      } catch (_) {}
      break;
    case "play_pause":
      if (store.playerState.active) await togglePlayPause();
      break;
    case "play":
      if (store.playerState.active && store.playerState.paused) {
        try {
          const snap = await invokeCmd("player_set_pause", { paused: false });
          updatePlayerState(snap);
        } catch (_) {}
      }
      break;
    case "pause":
      if (store.playerState.active && !store.playerState.paused) {
        try {
          const snap = await invokeCmd("player_set_pause", { paused: true });
          updatePlayerState(snap);
        } catch (_) {}
      }
      break;
    case "fast_forward":
      if (store.playerState.active) await seekByConfiguredInterval(1);
      break;
    case "rewind":
      if (store.playerState.active) await seekByConfiguredInterval(-1);
      break;
    case "exit_pip":
      store.playerState.pip_active = false;
      syncPlayerSurfaceViewport();
      break;
    default:
      break;
  }
}

export async function initiateInAppPlayback(
  url,
  customTitle = null,
  forcePositionMs = null,
  skipQueueRebuild = false
) {
  try {
    if (typeof url === "string" && url.startsWith("local://")) {
      const localPath = url.slice("local://".length);
      await openLocalMediaFile(localPath, forcePositionMs);
      return;
    }

    switchViewFn("player");
    document.getElementById("player-title-text").textContent =
      customTitle || "Connecting to media stream...";

    let queueEntry = null;
    if (!skipQueueRebuild) {
      queueEntry = await tryPopulateSiblingQueueForUrl(url);
    } else {
      queueEntry = (store.playlistQueue || [])[store.playlistIndex] || null;
    }

    let res;
    if (forcePositionMs !== null) {
      res = await invokeCmd("start_in_app_playback", {
        url,
        customTitle,
        startPositionMs: forcePositionMs,
      });
    } else {
      res = await invokeCmd("prepare_in_app_playback", { url, customTitle });
    }

    if (res.needs_resume_dialog && res.saved_position) {
      const pos = res.saved_position;
      openModal(
        `
          <h3>Resume Playback?</h3>
          <p style="margin:0;color:var(--text-secondary);word-break:break-word;">
            <strong>${escapeHtml(customTitle || url)}</strong> was previously watched up to
            <strong style="color:var(--accent);">${escapeHtml(pos.formatted_position)}</strong>.
          </p>
          <div style="display:flex;flex-direction:column;gap:8px;margin-top:6px;">
            <button id="btn-resume-yes" class="btn btn-primary">
              ▶ Resume from ${escapeHtml(pos.formatted_position)}
            </button>
            <button id="btn-resume-zero" class="btn">
              ⟲ Start from beginning (00:00)
            </button>
          </div>
          <div class="modal-actions">
            <button id="btn-resume-cancel" class="btn">Cancel</button>
          </div>
        `,
        (card) => {
          card.querySelector("#btn-resume-cancel").onclick = closeModal;
          card.querySelector("#btn-resume-yes").onclick = async () => {
            closeModal();
            await initiateInAppPlayback(url, customTitle, pos.position_ms, true);
          };
          card.querySelector("#btn-resume-zero").onclick = async () => {
            closeModal();
            await initiateInAppPlayback(url, customTitle, 0, true);
          };
        }
      );
      return;
    }

    if (res.stream_status) {
      store.streamCacheStatus = res.stream_status;
    }
    if (res.player_state) {
      updatePlayerState(res.player_state);
    }

    // Auto-attach stem-matched server subtitles (.srt / .ass / .vtt) when enabled!
    if (
      store.settings?.auto_load_matching_subtitle !== false &&
      queueEntry &&
      Array.isArray(queueEntry.matchedSubtitles) &&
      queueEntry.matchedSubtitles.length > 0
    ) {
      for (let i = 0; i < queueEntry.matchedSubtitles.length; i++) {
        const subUrl = queueEntry.matchedSubtitles[i];
        try {
          const snap = await invokeCmd("player_load_external_subtitle_url", {
            subtitleUrl: subUrl,
            select: i === 0,
          });
          updatePlayerState(snap);
        } catch (_) {}
      }
      showToast("Auto-loaded matching server subtitle", "success");
    }

    store.recentMedia = await invokeCmd("recent_media_list", { limit: 60 });
    syncQueueControlsAndSmtc();
    syncPlayerSurfaceViewport();
  } catch (e) {
    showToast(`Playback failed: ${e}`, "error");
  }
}

export function promptDirectMediaUrlModal() {
  openModal(
    `
      <h3>Open Direct Media Stream URL</h3>
      <p style="margin:0;color:var(--text-secondary);font-size:12px;">
        Enter any HTTP/HTTPS video or audio link (MKV, MP4, AVI, WEBM, MP3, FLAC, local server stream).
      </p>
      <input
        id="modal-direct-stream-url"
        class="input-field"
        placeholder="http://172.16.50.12/.../Episode.S01E01.mkv"
      />
      <div class="modal-actions">
        <button id="modal-stream-cancel" class="btn">Cancel</button>
        <button id="modal-stream-play" class="btn btn-primary">▶ Play in StreamVault</button>
      </div>
    `,
    (card) => {
      const inp = card.querySelector("#modal-direct-stream-url");
      inp.focus();
      const submit = () => {
        const val = inp.value.trim();
        if (!val) return;
        closeModal();
        initiateInAppPlayback(val, null, null);
      };
      inp.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter") {
          ev.preventDefault();
          submit();
        }
      });
      card.querySelector("#modal-stream-cancel").onclick = closeModal;
      card.querySelector("#modal-stream-play").onclick = submit;
    }
  );
}

export function openPlaylistQueueModal() {
  const q = store.playlistQueue || [];
  const activeIdx = store.playlistIndex;
  openModal(
    `
      <h3>Episode &amp; Media Playlist Queue (${q.length})</h3>
      <p style="margin:0;font-size:12px;color:var(--text-secondary);">
        Sibling episodes from directory pages or multi-file selections queue automatically.
      </p>
      <div class="grabber-list-box" style="max-height:310px;">
        ${
          q.length === 0
            ? `<div style="padding:14px;color:var(--text-muted);font-size:12.5px;text-align:center;">
                 Queue is empty. Open a directory media link, use <strong>⬇ Grab</strong>, or click <strong>📂 Add Local Files</strong>.
               </div>`
            : q
                .map((item, idx) => {
                  const isCurr = idx === activeIdx;
                  const subBadge =
                    item.matchedSubtitles && item.matchedSubtitles.length > 0
                      ? `<span class="grabber-ext-tag subtitle">SUB</span>`
                      : "";
                  return `
                    <div class="grabber-item-row" data-q-idx="${idx}" style="${
                      isCurr
                        ? "background:rgba(56,189,248,0.16);border:1px solid rgba(56,189,248,0.4);"
                        : ""
                    }">
                      <span style="font-family:var(--font-mono);font-size:11px;color:var(--text-muted);min-width:26px;">
                        ${isCurr ? "▶" : idx + 1 + "."}
                      </span>
                      <span style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:${
                        isCurr ? "600" : "400"
                      };">
                        ${escapeHtml(item.title)}
                      </span>
                      ${subBadge}
                    </div>
                  `;
                })
                .join("")
        }
      </div>
      <div class="modal-actions">
        <button id="q-add-local" class="btn" style="margin-right:auto;">📂 Add Local Files…</button>
        ${q.length > 1 ? `<button id="q-clear" class="btn">Clear Queue</button>` : ""}
        <button id="q-close" class="btn btn-primary">Done</button>
      </div>
    `,
    (card) => {
      card.querySelector("#q-close").onclick = closeModal;
      card.querySelector("#q-add-local").onclick = async () => {
        closeModal();
        await promptOpenLocalMediaFiles();
      };
      const clearBtn = card.querySelector("#q-clear");
      if (clearBtn) {
        clearBtn.onclick = () => {
          const curr = (store.playlistQueue || [])[store.playlistIndex];
          store.playlistQueue = curr ? [curr] : [];
          store.playlistIndex = curr ? 0 : -1;
          syncQueueControlsAndSmtc();
          closeModal();
        };
      }
      card.querySelectorAll("[data-q-idx]").forEach((row) => {
        row.onclick = async () => {
          const idx = Number(row.getAttribute("data-q-idx"));
          closeModal();
          await playQueueAtIndex(idx, false);
        };
      });
    }
  );
}

export function openSyncAndSubtitleFxModal() {
  const st = store.playerState;
  const subDelay = st.sub_delay_ms || 0;
  const audDelay = st.audio_delay_ms || 0;
  const subScalePct = Math.round((st.sub_scale || 1.0) * 100);

  openModal(
    `
      <h3>Subtitle &amp; Audio Sync + Subtitle Size</h3>
      <div style="display:flex;flex-direction:column;gap:14px;">
        <div>
          <div style="display:flex;justify-content:space-between;font-size:12.5px;margin-bottom:6px;">
            <span>Subtitle Delay (Shortcuts: <strong>Z</strong> / <strong>X</strong>)</span>
            <strong id="fx-sub-delay-val" style="font-family:var(--font-mono);color:var(--accent);">
              ${subDelay >= 0 ? "+" : ""}${subDelay} ms
            </strong>
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <button class="btn btn-sm" data-sub-step="-500">-500 ms</button>
            <button class="btn btn-sm" data-sub-step="-100">-100 ms</button>
            <button class="btn btn-sm" data-sub-reset="1">Reset (0 ms)</button>
            <button class="btn btn-sm" data-sub-step="100">+100 ms</button>
            <button class="btn btn-sm" data-sub-step="500">+500 ms</button>
          </div>
        </div>

        <div>
          <div style="display:flex;justify-content:space-between;font-size:12.5px;margin-bottom:6px;">
            <span>Audio Track Delay (A/V Lip-Sync)</span>
            <strong id="fx-aud-delay-val" style="font-family:var(--font-mono);color:var(--accent);">
              ${audDelay >= 0 ? "+" : ""}${audDelay} ms
            </strong>
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <button class="btn btn-sm" data-aud-step="-500">-500 ms</button>
            <button class="btn btn-sm" data-aud-step="-100">-100 ms</button>
            <button class="btn btn-sm" data-aud-reset="1">Reset (0 ms)</button>
            <button class="btn btn-sm" data-aud-step="100">+100 ms</button>
            <button class="btn btn-sm" data-aud-step="500">+500 ms</button>
          </div>
        </div>

        <div>
          <div style="display:flex;justify-content:space-between;font-size:12.5px;margin-bottom:6px;">
            <span>Subtitle Scale</span>
            <strong id="fx-sub-scale-val" style="font-family:var(--font-mono);color:var(--accent);">
              ${subScalePct}%
            </strong>
          </div>
          <div style="display:flex;align-items:center;gap:10px;">
            <input id="fx-sub-scale-slider" type="range" min="50" max="240" step="5" value="${subScalePct}" style="flex:1;" />
            <button id="fx-sub-scale-reset" class="btn btn-sm">100%</button>
          </div>
        </div>
      </div>
      <div class="modal-actions">
        <button id="fx-close" class="btn btn-primary">Done</button>
      </div>
    `,
    (card) => {
      const subValEl = card.querySelector("#fx-sub-delay-val");
      const audValEl = card.querySelector("#fx-aud-delay-val");
      const scaleValEl = card.querySelector("#fx-sub-scale-val");
      const scaleSlider = card.querySelector("#fx-sub-scale-slider");

      const applySub = async (nextMs) => {
        try {
          const snap = await invokeCmd("player_set_sub_delay", { delayMs: nextMs });
          updatePlayerState(snap);
          subValEl.textContent = `${snap.sub_delay_ms >= 0 ? "+" : ""}${snap.sub_delay_ms} ms`;
        } catch (e) {
          showToast(String(e), "error");
        }
      };
      const applyAud = async (nextMs) => {
        try {
          const snap = await invokeCmd("player_set_audio_delay", { delayMs: nextMs });
          updatePlayerState(snap);
          audValEl.textContent = `${snap.audio_delay_ms >= 0 ? "+" : ""}${snap.audio_delay_ms} ms`;
        } catch (e) {
          showToast(String(e), "error");
        }
      };

      card.querySelectorAll("[data-sub-step]").forEach((btn) => {
        btn.onclick = () =>
          applySub((store.playerState.sub_delay_ms || 0) + Number(btn.getAttribute("data-sub-step")));
      });
      card.querySelector("[data-sub-reset]").onclick = () => applySub(0);

      card.querySelectorAll("[data-aud-step]").forEach((btn) => {
        btn.onclick = () =>
          applyAud(
            (store.playerState.audio_delay_ms || 0) + Number(btn.getAttribute("data-aud-step"))
          );
      });
      card.querySelector("[data-aud-reset]").onclick = () => applyAud(0);

      scaleSlider.oninput = async () => {
        const pct = Number(scaleSlider.value);
        scaleValEl.textContent = `${pct}%`;
        try {
          const snap = await invokeCmd("player_set_sub_scale", { scale: pct / 100 });
          updatePlayerState(snap);
        } catch (_) {}
      };
      card.querySelector("#fx-sub-scale-reset").onclick = async () => {
        scaleSlider.value = "100";
        scaleValEl.textContent = "100%";
        try {
          const snap = await invokeCmd("player_set_sub_scale", { scale: 1.0 });
          updatePlayerState(snap);
        } catch (e) {
          showToast(String(e), "error");
        }
      };
      card.querySelector("#fx-close").onclick = closeModal;
    }
  );
}

export async function adjustSubDelayMs(deltaMs) {
  try {
    const next = (store.playerState.sub_delay_ms || 0) + deltaMs;
    const snap = await invokeCmd("player_set_sub_delay", { delayMs: next });
    updatePlayerState(snap);
    wakeFullscreenHud();
    showToast(`Subtitle delay: ${snap.sub_delay_ms >= 0 ? "+" : ""}${snap.sub_delay_ms} ms`, "info");
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function toggleDialogueBoost() {
  try {
    const next = !store.playerState.dialogue_boost;
    const snap = await invokeCmd("player_set_dialogue_boost", { enabled: next });
    updatePlayerState(snap);
    wakeFullscreenHud();
    showToast(
      next
        ? "🌙 Dialogue Boost enabled (Night dynamic audio normalization)"
        : "Dialogue Boost disabled",
      "info"
    );
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function capturePlayerScreenshot() {
  if (!store.playerState.active) {
    showToast("Play a video first to capture a screenshot", "error");
    return;
  }
  try {
    const res = await invokeCmd("player_take_screenshot", { includeSubtitles: true });
    if (!res?.native_overlay_shown) {
      wakeFullscreenHud();
      showToast(`📷 Saved ${res.filename}`, "success", {
        label: "Reveal",
        onClick: () => {
          invokeCmd("player_reveal_path", { path: res.path }).catch(() => {});
        },
      });
    }
  } catch (e) {
    showToast(`Screenshot failed: ${e}`, "error");
  }
}

export async function jumpChapter(direction) {
  const chapters = store.playerState.chapters || [];
  if (chapters.length === 0) return false;
  const pos = store.playerState.position_ms || 0;
  let target = null;
  if (direction > 0) {
    target = chapters.find((c) => c.time_ms > pos + 900);
  } else {
    for (let i = chapters.length - 1; i >= 0; i--) {
      if (chapters[i].time_ms < pos - 2500) {
        target = chapters[i];
        break;
      }
    }
    if (!target) target = chapters[0];
  }
  if (!target) return false;
  try {
    const snap = await invokeCmd("player_seek_absolute", { positionMs: target.time_ms });
    updatePlayerState(snap);
    wakeFullscreenHud();
    showToast(`Chapter: ${target.title}`, "info");
    return true;
  } catch (_) {
    return false;
  }
}

function renderChapterMarkers(snap) {
  const layer = document.getElementById("player-chapter-layer");
  if (!layer) return;
  const dur = snap.duration_ms || 0;
  const chapters = snap.chapters || [];
  if (dur <= 0 || chapters.length === 0) {
    if (layer.innerHTML !== "") layer.innerHTML = "";
    return;
  }
  layer.innerHTML = chapters
    .filter((c) => c.time_ms > 0 && c.time_ms < dur)
    .map((c) => {
      const pct = ((c.time_ms / dur) * 100).toFixed(3);
      return `<div class="timeline-chapter-tick" style="left:${pct}%;"></div>`;
    })
    .join("");
}

export function updatePlayerState(snap) {
  if (!snap) return;
  const prevFullscreen = store.playerState.fullscreen;
  const prevPaused = store.playerState.paused;
  store.playerState = { ...store.playerState, ...snap, fullscreen: prevFullscreen };

  if (prevFullscreen) {
    if (!prevPaused && snap.paused) {
      document.body.classList.remove("player-hud-idle");
      clearTimeout(hudIdleTimer);
    } else if (prevPaused && !snap.paused) {
      wakeFullscreenHud();
    }
  }

  // While fullscreen HUD is hidden during active playback, skip mutating hidden WebView2 DOM
  // elements so WebView2 performs zero DirectComposition repaints (preventing DWM flicker).
  if (
    document.body.classList.contains("player-fullscreen") &&
    document.body.classList.contains("player-hud-idle") &&
    snap.active &&
    !snap.paused
  ) {
    syncPlayerSurfaceViewport();
    return;
  }

  const navBadge = document.getElementById("nav-playing-badge");
  if (navBadge) {
    navBadge.classList.toggle("hidden", !snap.active);
  }

  document.getElementById("player-title-text").textContent =
    snap.title || snap.original_url || "No Media Loaded";

  const btnPlay = document.getElementById("btn-player-play");
  btnPlay.textContent = snap.paused ? "▶ Play" : "⏸ Pause";

  const intervalSec = store.settings?.seek_interval_seconds || 10;
  document.getElementById("btn-player-seek-back").textContent = `⟲ ${intervalSec}s`;
  document.getElementById("btn-player-seek-fwd").textContent = `${intervalSec}s ⟳`;

  document.getElementById("player-time-current").textContent = formatTimeMs(snap.position_ms);
  document.getElementById("player-time-total").textContent = formatTimeMs(snap.duration_ms);

  const pct =
    snap.duration_ms > 0
      ? Math.max(0, Math.min(100, (snap.position_ms / snap.duration_ms) * 100))
      : 0;
  document.getElementById("player-timeline-played").style.width = `${pct}%`;

  renderChapterMarkers(snap);

  const volSlider = document.getElementById("player-volume-slider");
  if (document.activeElement !== volSlider) {
    volSlider.value = String(snap.volume ?? 100);
  }
  document.getElementById("player-volume-label").textContent = snap.muted
    ? "Muted"
    : `${snap.volume ?? 100}%`;
  document.getElementById("btn-player-mute").textContent = snap.muted ? "🔇" : "🔊";

  const speedSel = document.getElementById("player-speed-select");
  if (snap.speed) {
    const speedStr = String(Number(snap.speed.toFixed(2)));
    if (![...speedSel.options].some((o) => o.value === speedStr)) {
      const opt = document.createElement("option");
      opt.value = speedStr;
      opt.textContent = `${speedStr}x`;
      speedSel.appendChild(opt);
    }
    speedSel.value = speedStr;
  }

  // Highlight Sync & Dialogue Boost active states
  const btnSync = document.getElementById("btn-player-sync-fx");
  if (btnSync) {
    const hasOffset =
      (snap.sub_delay_ms || 0) !== 0 ||
      (snap.audio_delay_ms || 0) !== 0 ||
      Math.abs((snap.sub_scale || 1.0) - 1.0) > 0.04;
    btnSync.classList.toggle("active-toggle", hasOffset);
  }
  const btnBoost = document.getElementById("btn-player-dialogue-boost");
  if (btnBoost) {
    btnBoost.classList.toggle("active-toggle", Boolean(snap.dialogue_boost));
  }

  // Populate Audio & Subtitle Track selectors
  renderTrackSelectors(snap);

  // Overlay visibility when idle or audio-only
  const emptyOverlay = document.getElementById("player-empty-overlay");
  const hasVideoTrack = (snap.tracks || []).some((t) => t.type === "video" && t.selected);
  if (!snap.active) {
    emptyOverlay.classList.remove("hidden");
    document.getElementById("player-overlay-heading").textContent = snap.engine_ready
      ? "StreamVault Built-in Media Engine (libmpv Ready)"
      : "StreamVault Media Engine";
  } else if (snap.active && snap.tracks?.length > 0 && !hasVideoTrack) {
    emptyOverlay.classList.remove("hidden");
    document.getElementById("player-overlay-heading").textContent =
      `♪ Audio Stream: ${snap.title || ""}`;
  } else if (snap.engine_ready) {
    emptyOverlay.classList.add("hidden");
  }

  renderStreamCacheTelemetry();
  syncPlayerSurfaceViewport();
}

function renderTrackSelectors(snap) {
  const audioSel = document.getElementById("player-audio-select");
  const subSel = document.getElementById("player-sub-select");
  const tracks = snap.tracks || [];

  const audioTracks = tracks.filter((t) => t.type === "audio");
  if (audioTracks.length === 0) {
    audioSel.innerHTML = `<option value="">Audio: Default</option>`;
  } else {
    audioSel.innerHTML = audioTracks
      .map((t) => {
        const label = [
          `#${t.id}`,
          t.lang ? t.lang.toUpperCase() : null,
          t.title || null,
          t.codec ? `(${t.codec})` : null,
        ]
          .filter(Boolean)
          .join(" ");
        return `<option value="${t.id}" ${t.selected ? "selected" : ""}>Audio: ${escapeHtml(label)}</option>`;
      })
      .join("");
  }

  const subTracks = tracks.filter((t) => t.type === "sub");
  subSel.innerHTML = [
    `<option value="off" ${!snap.subtitles_visible || !snap.subtitle_track_id ? "selected" : ""}>Subtitles: Off</option>`,
    ...subTracks.map((t) => {
      const label = [
        `#${t.id}`,
        t.lang ? t.lang.toUpperCase() : null,
        t.title || null,
        t.external ? "[Ext]" : null,
      ]
        .filter(Boolean)
        .join(" ");
      const isSel = snap.subtitles_visible && t.selected;
      return `<option value="${t.id}" ${isSel ? "selected" : ""}>Sub: ${escapeHtml(label)}</option>`;
    }),
  ].join("");
}

export function renderStreamCacheTelemetry() {
  if (
    document.body.classList.contains("player-fullscreen") &&
    document.body.classList.contains("player-hud-idle") &&
    store.playerState.active &&
    !store.playerState.paused
  ) {
    return;
  }
  const pill = document.getElementById("player-cache-pill");
  const rangeBanner = document.getElementById("player-range-notice");
  const rangeText = document.getElementById("player-range-notice-text");
  const bufferLayer = document.getElementById("player-buffer-layer");

  const cs = store.streamCacheStatus;
  if (!cs) {
    if (store.playerState?.original_url?.startsWith("local://") && store.playerState.active) {
      pill.classList.remove("hidden");
      pill.classList.remove("warning");
      pill.textContent = "Local Direct Playback";
    } else {
      pill.classList.add("hidden");
    }
    rangeBanner.classList.add("hidden");
    bufferLayer.innerHTML = "";
    return;
  }

  pill.classList.remove("hidden");
  const pct = cs.cached_percent ? cs.cached_percent.toFixed(1) : "0.0";
  const speedPart =
    cs.network_state === "completed"
      ? "Cached 100%"
      : `Cached ${pct}% (${formatBytes(cs.cached_bytes)}) • ${formatSpeed(cs.download_speed_bps)}`;
  pill.textContent = cs.network_state === "retrying" ? `Reconnecting... • ${speedPart}` : speedPart;
  pill.classList.toggle("warning", !cs.supports_range || cs.network_state === "retrying");

  if (cs.range_notice) {
    rangeText.textContent = cs.range_notice;
    rangeBanner.classList.remove("hidden");
  } else {
    rangeBanner.classList.add("hidden");
  }

  // Render disk-cached byte intervals on timeline!
  bufferLayer.innerHTML = (cs.buffered_ranges || [])
    .map(([s, e]) => {
      const left = (s * 100).toFixed(2);
      const width = Math.max(0.2, (e - s) * 100).toFixed(2);
      return `<div class="timeline-buffer-seg" style="left:${left}%;width:${width}%;"></div>`;
    })
    .join("");
}

export async function togglePlayPause() {
  try {
    const snap = await invokeCmd("player_toggle_pause");
    updatePlayerState(snap);
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function seekByConfiguredInterval(direction) {
  const secs = (store.settings?.seek_interval_seconds || 10) * direction;
  try {
    const snap = await invokeCmd("player_seek_relative", { deltaSeconds: secs });
    updatePlayerState(snap);
  } catch (_) {}
}

export async function setVolume(vol) {
  const clamped = Math.max(0, Math.min(150, Math.round(vol)));
  try {
    const snap = await invokeCmd("player_set_volume", { volume: clamped });
    updatePlayerState(snap);
  } catch (_) {}
}

export async function stepPlaybackSpeed(direction) {
  try {
    const curr = store.playerState.speed || 1.0;
    let idx = SPEED_STEPS.findIndex((s) => Math.abs(s - curr) < 0.03);
    if (idx === -1) idx = 3;
    const nextIdx = Math.max(0, Math.min(SPEED_STEPS.length - 1, idx + direction));
    const snap = await invokeCmd("player_set_speed", { speed: SPEED_STEPS[nextIdx] });
    updatePlayerState(snap);
    showToast(`Playback speed: ${SPEED_STEPS[nextIdx]}x`);
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function toggleFullscreen(explicit = null) {
  try {
    const isFs = await invokeCmd("player_set_fullscreen", { fullscreen: explicit });
    applyFullscreenDomState(isFs);
  } catch (e) {
    showToast(String(e), "error");
  }
}

export function applyFullscreenDomState(isFs) {
  hidePlayerTimeTooltip();
  store.playerState.fullscreen = isFs;
  document.body.classList.toggle("player-fullscreen", isFs);
  if (isFs) {
    wakeFullscreenHud();
  } else {
    clearTimeout(hudIdleTimer);
    document.body.classList.remove("player-hud-idle");
  }
  setTimeout(() => syncPlayerSurfaceViewport(true), 35);
}

export function wakeFullscreenHud() {
  const wasIdle = document.body.classList.contains("player-hud-idle");
  if (wasIdle) {
    document.body.classList.remove("player-hud-idle");
    updatePlayerState(store.playerState);
  }
  clearTimeout(hudIdleTimer);
  if (store.playerState.fullscreen && !store.playerState.paused) {
    hudIdleTimer = setTimeout(() => {
      if (store.playerState.fullscreen && !store.playerState.paused && !store.modalOpen) {
        hidePlayerTimeTooltip();
        document.body.classList.add("player-hud-idle");
        syncPlayerSurfaceViewport();
      }
    }, 2800);
  }
}

export function syncPlayerSurfaceViewport() {
  const canvas = document.getElementById("player-video-canvas");
  if (!canvas) return;

  const tracks = store.playerState.tracks || [];
  const isAudioOnlyLoaded =
    tracks.length > 0 &&
    !tracks.some((t) => t.type === "video" && t.selected) &&
    !(store.playerState.video_width > 0 && store.playerState.video_height > 0);

  const hasVideo =
    store.playerState.active &&
    !isAudioOnlyLoaded &&
    !store.modalOpen &&
    store.currentView === "player" &&
    !store.playerState.pip_active;

  if (!hasVideo) {
    hidePlayerTimeTooltip();
    if (lastViewportSig !== "hidden") {
      lastViewportSig = "hidden";
      invokeCmd("player_set_viewport", {
        visible: false,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
      }).catch(() => {});
    }
    return;
  }

  const rect = canvas.getBoundingClientRect();
  const visible = rect.width > 16 && rect.height > 16;
  const sig = visible
    ? `${Math.round(rect.left)}:${Math.round(rect.top)}:${Math.round(rect.width)}:${Math.round(rect.height)}`
    : "hidden";
  if (sig === lastViewportSig) return;
  lastViewportSig = sig;
  invokeCmd("player_set_viewport", {
    visible,
    x: rect.left,
    y: rect.top,
    width: rect.width,
    height: rect.height,
  }).catch(() => {});
}

export function handlePlayerKeyShortcut(code, fromSurface = false) {
  if (store.currentView !== "player" && !fromSurface) return false;
  const sc = store.settings?.shortcuts || {};
  if (code === (sc.play_pause || "Space")) {
    togglePlayPause();
    return true;
  }
  if (code === (sc.seek_backward || "ArrowLeft")) {
    seekByConfiguredInterval(-1);
    return true;
  }
  if (code === (sc.seek_forward || "ArrowRight")) {
    seekByConfiguredInterval(1);
    return true;
  }
  if (code === (sc.volume_up || "ArrowUp")) {
    setVolume((store.playerState.volume ?? 100) + 5);
    return true;
  }
  if (code === (sc.volume_down || "ArrowDown")) {
    setVolume((store.playerState.volume ?? 100) - 5);
    return true;
  }
  if (code === (sc.toggle_mute || "KeyM")) {
    invokeCmd("player_toggle_mute").then(updatePlayerState).catch(() => {});
    return true;
  }
  if (code === (sc.toggle_fullscreen || "KeyF")) {
    toggleFullscreen();
    return true;
  }
  if (code === (sc.exit_fullscreen || "Escape")) {
    if (store.playerState.fullscreen) {
      toggleFullscreen(false);
      return true;
    }
    if (store.playerState.pip_active) {
      invokeCmd("player_set_pip", { enable: false })
        .then((s) => {
          updatePlayerState(s);
          syncPlayerSurfaceViewport();
        })
        .catch(() => {});
      return true;
    }
  }
  if (code === (sc.speed_down || "BracketLeft")) {
    stepPlaybackSpeed(-1);
    return true;
  }
  if (code === (sc.speed_up || "BracketRight")) {
    stepPlaybackSpeed(1);
    return true;
  }
  if (code === "KeyZ") {
    adjustSubDelayMs(-100);
    return true;
  }
  if (code === "KeyX") {
    adjustSubDelayMs(100);
    return true;
  }
  if (code === "KeyS") {
    capturePlayerScreenshot();
    return true;
  }
  if (code === "KeyN") {
    playNextInQueue(false);
    return true;
  }
  if (code === "KeyP") {
    playPrevInQueue();
    return true;
  }
  if (code === "PageDown") {
    jumpChapter(1);
    return true;
  }
  if (code === "PageUp") {
    jumpChapter(-1);
    return true;
  }
  return false;
}
