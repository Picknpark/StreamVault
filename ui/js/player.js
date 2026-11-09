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

let switchViewFn = null;
let hudIdleTimer = null;
let canvasClickTimer = null;
let lastViewportSig = "";
let lastTipSig = "";
let nativeTipActive = false;

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
  const btnOpenUrl = document.getElementById("btn-player-open-url-prompt");
  const btnHandoffExt = document.getElementById("btn-player-handoff-ext");
  const btnSavePerm = document.getElementById("btn-player-save-perm");

  const canvas = document.getElementById("player-video-canvas");
  const timeline = document.getElementById("player-timeline");
  const timelineTip = document.getElementById("player-timeline-tip");

  const btnPlay = document.getElementById("btn-player-play");
  const btnStop = document.getElementById("btn-player-stop");
  const btnSeekBack = document.getElementById("btn-player-seek-back");
  const btnSeekFwd = document.getElementById("btn-player-seek-fwd");
  const btnMute = document.getElementById("btn-player-mute");
  const volSlider = document.getElementById("player-volume-slider");

  const speedSelect = document.getElementById("player-speed-select");
  const audioSelect = document.getElementById("player-audio-select");
  const subSelect = document.getElementById("player-sub-select");
  const btnLoadSub = document.getElementById("btn-player-load-sub");
  const aspectSelect = document.getElementById("player-aspect-select");
  const btnPip = document.getElementById("btn-player-pip");
  const btnFullscreen = document.getElementById("btn-player-fullscreen");

  btnBackBrowser.addEventListener("click", () => switchViewFn("browser"));
  btnOpenUrl.addEventListener("click", () => promptDirectMediaUrlModal());

  btnHandoffExt.addEventListener("click", () => {
    const url = store.playerState?.original_url;
    if (!url) {
      showToast("No media currently loaded", "error");
      return;
    }
    onOpenExternal(url, store.playerState.title);
  });

  btnSavePerm.addEventListener("click", () => {
    const url = store.playerState?.original_url;
    if (!url) {
      showToast("No media currently loaded", "error");
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
      setVolume((store.playerState.volume || 100) + step);
    },
    { passive: false }
  );

  // Timeline hover & click-to-seek:
  // When native libmpv video surface is active, render via owned Win32 popup (`StreamVaultTimeTip`)
  // so the Time Tooltip sits cleanly OVER the video frame instead of underneath it!
  timeline.addEventListener("mousemove", (ev) => {
    const dur = store.playerState.duration_ms || 0;
    if (dur <= 0) return;
    const rect = timeline.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
    const label = formatTimeMs(Math.round(dur * ratio));

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
    } catch (e) {
      showToast(String(e), "error");
    }
  });

  btnSeekBack.addEventListener("click", () => seekByConfiguredInterval(-1));
  btnSeekFwd.addEventListener("click", () => seekByConfiguredInterval(1));

  btnMute.addEventListener("click", async () => {
    const snap = await invokeCmd("player_toggle_mute");
    updatePlayerState(snap);
  });

  volSlider.addEventListener("input", () => {
    setVolume(Number(volSlider.value));
  });

  speedSelect.addEventListener("change", async () => {
    const snap = await invokeCmd("player_set_speed", { speed: Number(speedSelect.value) });
    updatePlayerState(snap);
  });

  audioSelect.addEventListener("change", async () => {
    const id = Number(audioSelect.value);
    if (!id) return;
    const snap = await invokeCmd("player_set_audio_track", { trackId: id });
    updatePlayerState(snap);
  });

  subSelect.addEventListener("change", async () => {
    const val = subSelect.value;
    const trackId = val === "off" || val === "" ? null : Number(val);
    const snap = await invokeCmd("player_set_subtitle_track", { trackId });
    updatePlayerState(snap);
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

  aspectSelect.addEventListener("change", async () => {
    const snap = await invokeCmd("player_set_aspect_ratio", { ratio: aspectSelect.value });
    updatePlayerState(snap);
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

export async function initiateInAppPlayback(url, customTitle = null, forcePositionMs = null) {
  try {
    switchViewFn("player");
    document.getElementById("player-title-text").textContent =
      customTitle || "Connecting to media stream...";

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
            await initiateInAppPlayback(url, customTitle, pos.position_ms);
          };
          card.querySelector("#btn-resume-zero").onclick = async () => {
            closeModal();
            await initiateInAppPlayback(url, customTitle, 0);
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
    store.recentMedia = await invokeCmd("recent_media_list", { limit: 60 });
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
      card.querySelector("#modal-stream-cancel").onclick = closeModal;
      card.querySelector("#modal-stream-play").onclick = () => {
        const val = inp.value.trim();
        if (!val) return;
        closeModal();
        initiateInAppPlayback(val, null, null);
      };
    }
  );
}

export function updatePlayerState(snap) {
  if (!snap) return;
  const prevFullscreen = store.playerState.fullscreen;
  store.playerState = { ...store.playerState, ...snap, fullscreen: prevFullscreen };

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
    pill.classList.add("hidden");
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
  const curr = store.playerState.speed || 1.0;
  let idx = SPEED_STEPS.findIndex((s) => Math.abs(s - curr) < 0.03);
  if (idx === -1) idx = 3;
  const nextIdx = Math.max(0, Math.min(SPEED_STEPS.length - 1, idx + direction));
  const snap = await invokeCmd("player_set_speed", { speed: SPEED_STEPS[nextIdx] });
  updatePlayerState(snap);
  showToast(`Playback speed: ${SPEED_STEPS[nextIdx]}x`);
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

  const hasVideo =
    store.playerState.active &&
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
  lastViewportSig = visible ? "shown" : "hidden";
  invokeCmd("player_set_viewport", {
    visible,
    x: rect.left,
    y: rect.top,
    width: rect.width,
    height: rect.height,
  }).catch(() => {});
}

export function handlePlayerKeyShortcut(code) {
  if (store.currentView !== "player") return false;
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
    setVolume((store.playerState.volume || 100) + 5);
    return true;
  }
  if (code === (sc.volume_down || "ArrowDown")) {
    setVolume((store.playerState.volume || 100) - 5);
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
  if (code === (sc.exit_fullscreen || "Escape") && store.playerState.fullscreen) {
    toggleFullscreen(false);
    return true;
  }
  if (code === (sc.speed_down || "BracketLeft")) {
    stepPlaybackSpeed(-1);
    return true;
  }
  if (code === (sc.speed_up || "BracketRight")) {
    stepPlaybackSpeed(1);
    return true;
  }
  return false;
}
