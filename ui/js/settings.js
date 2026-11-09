import { renderBookmarkBar } from "./browser.js";
import { invokeCmd } from "./ipc.js";
import { store } from "./state.js";
import { copyTextToClipboard, escapeHtml, formatBytes, showToast } from "./utils.js";

let applyThemeFn = null;
let switchViewFn = null;
let settingsRenderSeq = 0;

export function initSettingsModule({ onApplyTheme, onSwitchView }) {
  applyThemeFn = onApplyTheme;
  switchViewFn = onSwitchView;

  document.querySelectorAll("#settings-category-tabs .settings-tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document
        .querySelectorAll("#settings-category-tabs .settings-tab-btn")
        .forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      store.activeSettingsTab = btn.getAttribute("data-stab");
      renderSettingsPanel();
    });
  });
}

async function updateSetting(key, value) {
  try {
    store.settings = await invokeCmd("settings_update_key", { key, value });
    if (key === "theme" && applyThemeFn) {
      applyThemeFn(store.settings.theme);
    }
    if (key === "show_bookmark_bar") {
      renderBookmarkBar();
    }
    showToast("Setting saved", "success");
  } catch (e) {
    showToast(String(e), "error");
  }
}

export async function renderSettingsPanel() {
  const pane = document.getElementById("settings-content-pane");
  if (!pane) return;
  const seq = ++settingsRenderSeq;

  if (!store.settings) {
    pane.innerHTML = `<div class="settings-group"><p style="margin:0;color:var(--text-secondary);">Loading application settings...</p></div>`;
    try {
      store.settings = await invokeCmd("settings_get");
      if (seq !== settingsRenderSeq) return;
      if (store.settings?.theme && applyThemeFn) {
        applyThemeFn(store.settings.theme);
      }
    } catch (err) {
      if (seq !== settingsRenderSeq) return;
      pane.innerHTML = `
        <div class="settings-group">
          <h3>Unable to Load Settings</h3>
          <p style="color:var(--text-secondary);font-size:12.5px;">${escapeHtml(String(err))}</p>
          <div>
            <button id="btn-retry-settings-load" class="btn btn-primary">Retry Loading Settings</button>
          </div>
        </div>
      `;
      const retryBtn = pane.querySelector("#btn-retry-settings-load");
      if (retryBtn) retryBtn.onclick = () => renderSettingsPanel();
      return;
    }
  }

  if (!store.settings) return;
  const s = store.settings;
  const tab = store.activeSettingsTab || "general";

  if (tab === "general") {
    pane.innerHTML = `
      <div class="settings-group">
        <h3>General Browser &amp; Appearance</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Startup View</strong>
            <span>Choose what opens when StreamVault launches</span>
          </div>
          <select id="set-startup" class="select-field">
            <option value="home" ${s.startup_behavior === "home" ? "selected" : ""}>StreamVault Home</option>
            <option value="restore_session" ${s.startup_behavior === "restore_session" ? "selected" : ""}>Browser (Restore Session)</option>
            <option value="homepage" ${s.startup_behavior === "homepage" ? "selected" : ""}>Browser (Open Homepage)</option>
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Homepage URL</strong>
            <span>Opened on new tabs and Home button</span>
          </div>
          <div style="display:flex;gap:6px;">
            <input id="set-homepage" class="input-field" style="width:270px;" value="${escapeHtml(s.homepage_url)}" />
            <button id="btn-save-homepage" class="btn btn-primary btn-sm">Save</button>
          </div>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Restore Previous Browser Tabs</strong>
            <span>Reopen your previous tabs automatically on startup</span>
          </div>
          <input id="set-restore-tabs" type="checkbox" ${s.restore_previous_session ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Show Bookmark Bar</strong>
            <span>Display pinned bookmarks directly below the browser address bar</span>
          </div>
          <input id="set-bm-bar" type="checkbox" ${s.show_bookmark_bar ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Application Theme</strong>
            <span>Dark charcoal theme is default</span>
          </div>
          <select id="set-theme" class="select-field">
            <option value="dark" ${s.theme === "dark" ? "selected" : ""}>Dark Charcoal (Default)</option>
            <option value="midnight" ${s.theme === "midnight" ? "selected" : ""}>Midnight OLED</option>
            <option value="light" ${s.theme === "light" ? "selected" : ""}>Light</option>
          </select>
        </div>
      </div>
    `;

    pane.querySelector("#set-startup").onchange = (e) =>
      updateSetting("startup_behavior", e.target.value);
    const saveHomepage = () =>
      updateSetting("homepage_url", pane.querySelector("#set-homepage").value.trim());
    pane.querySelector("#btn-save-homepage").onclick = saveHomepage;
    pane.querySelector("#set-homepage").addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        saveHomepage();
      }
    });
    pane.querySelector("#set-restore-tabs").onchange = (e) =>
      updateSetting("restore_previous_session", e.target.checked);
    pane.querySelector("#set-bm-bar").onchange = (e) =>
      updateSetting("show_bookmark_bar", e.target.checked);
    pane.querySelector("#set-theme").onchange = (e) =>
      updateSetting("theme", e.target.value);
    return;
  }

  if (tab === "media") {
    try {
      store.externalPlayers = await invokeCmd("external_players_detect");
    } catch (_) {
      store.externalPlayers = store.externalPlayers || [];
    }
    if (seq !== settingsRenderSeq) return;
    const players = store.externalPlayers || [];

    const playerOptions = [
      `<option value="ask" ${s.default_external_player === "ask" ? "selected" : ""}>Choose player each time</option>`,
      ...players.map(
        (p) =>
          `<option value="${escapeHtml(p.executable_path)}" ${s.default_external_player === p.executable_path ? "selected" : ""}>${escapeHtml(p.name)} (${escapeHtml(p.executable_path)})</option>`
      ),
    ].join("");

    pane.innerHTML = `
      <div class="settings-group">
        <h3>Playback &amp; Media Detection</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Automatic Media Link Detection</strong>
            <span>Detect MKV, MP4, AVI, MP3, FLAC &amp; MIME media headers while browsing</span>
          </div>
          <input id="set-auto-detect" type="checkbox" ${s.auto_detect_media ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>When Direct Media URL Is Opened</strong>
            <span>Offer all 3 choices or launch automatically</span>
          </div>
          <select id="set-play-method" class="select-field">
            <option value="ask" ${s.default_playback_method === "ask" ? "selected" : ""}>Show Action Bar (Play / External / Download)</option>
            <option value="in_app" ${s.default_playback_method === "in_app" ? "selected" : ""}>Play in StreamVault Automatically</option>
            <option value="external" ${s.default_playback_method === "external" ? "selected" : ""}>Open in External Player Automatically</option>
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Resume Playback Behavior</strong>
            <span>How previously watched media resumes</span>
          </div>
          <select id="set-resume-mode" class="select-field">
            <option value="ask" ${s.resume_playback_behavior === "ask" ? "selected" : ""}>Ask with Resume / Start-from-Beginning Dialog</option>
            <option value="always_resume" ${s.resume_playback_behavior === "always_resume" ? "selected" : ""}>Always Resume Automatically</option>
            <option value="always_beginning" ${s.resume_playback_behavior === "always_beginning" ? "selected" : ""}>Always Start from Beginning</option>
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Default Playback Speed</strong>
            <span>Initial speed for built-in libmpv player</span>
          </div>
          <select id="set-def-speed" class="select-field">
            ${[0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0]
              .map(
                (v) =>
                  `<option value="${v}" ${Math.abs(s.default_playback_speed - v) < 0.01 ? "selected" : ""}>${v}x</option>`
              )
              .join("")}
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Seek Interval (Seconds)</strong>
            <span>Left / Right arrow seek jump</span>
          </div>
          <select id="set-seek-sec" class="select-field">
            ${[5, 10, 15, 30]
              .map(
                (v) =>
                  `<option value="${v}" ${s.seek_interval_seconds === v ? "selected" : ""}>${v} seconds</option>`
              )
              .join("")}
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Auto-Play Next Episode in Directory / Playlist</strong>
            <span>Automatically advance to the next sibling episode when playback finishes cleanly</span>
          </div>
          <input id="set-autoplay-next" type="checkbox" ${s.auto_play_next_episode !== false ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Auto-Load Matching Subtitle (.srt / .ass / .vtt)</strong>
            <span>Automatically attach stem-matching server or local sidecar subtitles</span>
          </div>
          <input id="set-autoload-sub" type="checkbox" ${s.auto_load_matching_subtitle !== false ? "checked" : ""} />
        </div>
      </div>

      <div class="settings-group">
        <h3>Default External Player &amp; Installed Players</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Default External Player</strong>
            <span>Used when clicking "Open in External Player"</span>
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <select id="set-ext-player" class="select-field" style="max-width:340px;">${playerOptions}</select>
            <button id="btn-test-sel-player" class="btn btn-sm">Test Player</button>
          </div>
        </div>

        <div style="margin-top:12px;">
          <div style="font-size:12px;color:var(--text-secondary);margin-bottom:8px;">
            Add Custom External Player Executable (.exe):
          </div>
          <div style="display:flex;gap:7px;flex-wrap:wrap;">
            <input id="cust-player-name" class="input-field" style="width:150px;" placeholder="Player Name" />
            <input id="cust-player-path" class="input-field" style="flex:1;min-width:220px;" placeholder="C:\\Program Files\\DAUM\\PotPlayer\\PotPlayerMini64.exe" />
            <button id="btn-browse-player-exe" class="btn">Browse...</button>
            <button id="btn-add-cust-player" class="btn btn-primary">Add Player</button>
          </div>
        </div>

        <div style="margin-top:14px;">
          ${
            players.length === 0
              ? `<div style="font-size:12px;color:var(--text-muted);">No external players auto-detected yet. Add PotPlayer, VLC, MPC-HC, or any player above.</div>`
              : `<div class="data-list">
                  ${players
                    .map(
                      (p) => `
                    <div class="data-row">
                      <div class="data-row-main">
                        <div class="data-row-title">${escapeHtml(p.name)} <span style="font-size:11px;color:var(--accent);">[${escapeHtml(p.source)}]</span></div>
                        <div class="data-row-url">${escapeHtml(p.executable_path)}</div>
                      </div>
                      <div class="panel-toolbar">
                        <button class="btn btn-sm" data-test-exe="${escapeHtml(p.executable_path)}">Test</button>
                        ${
                          p.custom_id
                            ? `<button class="btn btn-danger btn-sm" data-rm-cust="${p.custom_id}">Remove</button>`
                            : ""
                        }
                      </div>
                    </div>
                  `
                    )
                    .join("")}
                 </div>`
          }
        </div>
      </div>
    `;

    pane.querySelector("#set-auto-detect").onchange = (e) =>
      updateSetting("auto_detect_media", e.target.checked);
    pane.querySelector("#set-play-method").onchange = (e) =>
      updateSetting("default_playback_method", e.target.value);
    pane.querySelector("#set-resume-mode").onchange = (e) =>
      updateSetting("resume_playback_behavior", e.target.value);
    pane.querySelector("#set-def-speed").onchange = (e) =>
      updateSetting("default_playback_speed", Number(e.target.value));
    pane.querySelector("#set-seek-sec").onchange = (e) =>
      updateSetting("seek_interval_seconds", Number(e.target.value));
    pane.querySelector("#set-autoplay-next").onchange = (e) =>
      updateSetting("auto_play_next_episode", e.target.checked);
    pane.querySelector("#set-autoload-sub").onchange = (e) =>
      updateSetting("auto_load_matching_subtitle", e.target.checked);
    pane.querySelector("#set-ext-player").onchange = (e) =>
      updateSetting("default_external_player", e.target.value);

    pane.querySelector("#btn-test-sel-player").onclick = async () => {
      const exe = pane.querySelector("#set-ext-player").value;
      if (!exe || exe === "ask") {
        showToast("Select a specific player first to test", "error");
        return;
      }
      try {
        const res = await invokeCmd("external_player_test", { executablePath: exe });
        showToast(res.message, res.success ? "success" : "error");
      } catch (e) {
        showToast(String(e), "error");
      }
    };

    pane.querySelector("#btn-browse-player-exe").onclick = async () => {
      try {
        const picked = await invokeCmd("external_player_browse_exe");
        if (picked) {
          pane.querySelector("#cust-player-path").value = picked;
        }
      } catch (e) {
        showToast(String(e), "error");
      }
    };

    const addCustomPlayer = async () => {
      const name = pane.querySelector("#cust-player-name").value.trim();
      const executablePath = pane.querySelector("#cust-player-path").value.trim();
      if (!executablePath) return;
      try {
        await invokeCmd("external_player_add_custom", { name, executablePath });
        showToast("Added custom external player", "success");
        renderSettingsPanel();
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    pane.querySelector("#btn-add-cust-player").onclick = addCustomPlayer;
    [pane.querySelector("#cust-player-name"), pane.querySelector("#cust-player-path")].forEach(
      (inp) => {
        inp?.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") {
            ev.preventDefault();
            addCustomPlayer();
          }
        });
      }
    );

    pane.querySelectorAll("[data-test-exe]").forEach((btn) => {
      btn.onclick = async () => {
        try {
          const res = await invokeCmd("external_player_test", {
            executablePath: btn.getAttribute("data-test-exe"),
          });
          showToast(res.message, res.success ? "success" : "error");
        } catch (e) {
          showToast(String(e), "error");
        }
      };
    });

    pane.querySelectorAll("[data-rm-cust]").forEach((btn) => {
      btn.onclick = async () => {
        try {
          await invokeCmd("external_player_remove_custom", {
            customId: Number(btn.getAttribute("data-rm-cust")),
          });
          store.settings = await invokeCmd("settings_get");
          renderSettingsPanel();
        } catch (e) {
          showToast(String(e), "error");
        }
      };
    });
    return;
  }

  if (tab === "downloads") {
    pane.innerHTML = `
      <div class="settings-group">
        <h3>Permanent Downloads</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Default Download Directory</strong>
            <span>Portable relative path <code>data/downloads</code> or any custom folder</span>
          </div>
          <div style="display:flex;gap:6px;">
            <input id="set-dl-dir" class="input-field" style="width:260px;" value="${escapeHtml(s.default_download_dir)}" />
            <button id="btn-browse-dl-dir" class="btn btn-sm">Browse...</button>
            <button id="btn-save-dl-dir" class="btn btn-primary btn-sm">Save</button>
          </div>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Ask Where to Save Before Every Download (Save As Dialog)</strong>
            <span>Pre-fills the exact original server filename. "Save As..." always remains available in Download Manager even when unchecked.</span>
          </div>
          <input id="set-ask-saveas" type="checkbox" ${s.ask_save_as_every_time ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Download Notifications</strong>
            <span>Show status notifications when downloads start or finish</span>
          </div>
          <input id="set-dl-notif" type="checkbox" ${s.download_notifications ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Max Concurrent Downloads</strong>
            <span>Active parallel downloads in Download Manager &amp; Batch Media Grabber (excess items queue automatically)</span>
          </div>
          <select id="set-max-conc" class="select-field">
            ${[1, 2, 3, 4, 6, 8]
              .map(
                (v) =>
                  `<option value="${v}" ${(s.max_concurrent_downloads || 3) === v ? "selected" : ""}>${v} simultaneous</option>`
              )
              .join("")}
          </select>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Per-Download Speed Limit (KB/s)</strong>
            <span>Set <code>0</code> for unlimited full LAN/broadband speed</span>
          </div>
          <div style="display:flex;gap:6px;align-items:center;">
            <input id="set-dl-speed-limit" class="input-field" type="number" min="0" step="256" style="width:130px;" value="${Number(s.download_speed_limit_kbps) || 0}" />
            <button id="btn-save-speed-limit" class="btn btn-primary btn-sm">Apply</button>
          </div>
        </div>
      </div>
    `;

    pane.querySelector("#btn-browse-dl-dir").onclick = async () => {
      try {
        const chosen = await invokeCmd("settings_browse_download_dir");
        if (chosen) {
          store.settings = await invokeCmd("settings_get");
          renderSettingsPanel();
        }
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    const saveDlDir = () =>
      updateSetting("default_download_dir", pane.querySelector("#set-dl-dir").value.trim());
    pane.querySelector("#btn-save-dl-dir").onclick = saveDlDir;
    pane.querySelector("#set-dl-dir").addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        saveDlDir();
      }
    });
    pane.querySelector("#set-ask-saveas").onchange = (e) =>
      updateSetting("ask_save_as_every_time", e.target.checked);
    pane.querySelector("#set-dl-notif").onchange = (e) =>
      updateSetting("download_notifications", e.target.checked);
    pane.querySelector("#set-max-conc").onchange = (e) =>
      updateSetting("max_concurrent_downloads", Number(e.target.value));
    const saveSpeedLimit = () => {
      const val = Math.max(
        0,
        Math.round(Number(pane.querySelector("#set-dl-speed-limit").value) || 0)
      );
      updateSetting("download_speed_limit_kbps", val);
    };
    pane.querySelector("#btn-save-speed-limit").onclick = saveSpeedLimit;
    pane.querySelector("#set-dl-speed-limit").addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        saveSpeedLimit();
      }
    });
    return;
  }

  if (tab === "cache") {
    let summary = { cache_dir: "data/cache/media", total_files: 0, total_bytes: 0 };
    try {
      summary = await invokeCmd("stream_cache_summary");
    } catch (_) {}
    if (seq !== settingsRenderSeq) return;
    pane.innerHTML = `
      <div class="settings-group">
        <h3>Temporary Streaming Media Cache</h3>
        <p style="font-size:12px;color:var(--text-secondary);margin-top:0;">
          Temporary cache files are used exclusively for progressive in-app streaming and seeking. They are completely separate from Permanent Downloads.
        </p>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Portable Temporary Cache Location</strong>
            <span><code>${escapeHtml(summary.cache_dir)}</code></span>
          </div>
          <span class="status-pill">${summary.total_files} file(s) • ${formatBytes(summary.total_bytes)}</span>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Automatically Clear Temporary Media on Exit</strong>
            <span>Removes temporary streaming buffers when StreamVault closes</span>
          </div>
          <input id="set-clear-exit" type="checkbox" ${s.clear_temp_cache_on_exit ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Startup Crash Recovery Cleanup</strong>
            <span>Clean orphaned temporary streaming files left behind after unexpected shutdowns</span>
          </div>
          <input id="set-crash-rec" type="checkbox" ${s.startup_cache_recovery ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Clear Temporary Streaming Cache Now</strong>
            <span>Safely deletes inactive cache files without touching active playback or permanent downloads</span>
          </div>
          <button id="btn-clear-cache-now" class="btn btn-danger">Clear Temporary Cache</button>
        </div>
      </div>
    `;

    pane.querySelector("#set-clear-exit").onchange = (e) =>
      updateSetting("clear_temp_cache_on_exit", e.target.checked);
    pane.querySelector("#set-crash-rec").onchange = (e) =>
      updateSetting("startup_cache_recovery", e.target.checked);
    pane.querySelector("#btn-clear-cache-now").onclick = async () => {
      try {
        const rep = await invokeCmd("stream_cache_clear");
        showToast(
          `Cleared ${rep.removed_files} temporary file(s) (${formatBytes(rep.freed_bytes)})${
            rep.skipped_active_files > 0 ? " — active stream protected" : ""
          }`,
          "success"
        );
        renderSettingsPanel();
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    return;
  }

  if (tab === "privacy") {
    pane.innerHTML = `
      <div class="settings-group">
        <h3>Privacy &amp; Data Controls</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Record Browsing History</strong>
            <span>Save visited webpage URLs and titles locally in portable SQLite</span>
          </div>
          <input id="set-hist-en" type="checkbox" ${s.browsing_history_enabled ? "checked" : ""} />
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Clear Browsing History</strong>
            <span>Deletes all recorded webpage history</span>
          </div>
          <button id="priv-clear-hist" class="btn">Clear History</button>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Clear Recent Media &amp; Saved Playback Positions</strong>
            <span>Does not delete Bookmarks or Permanent Downloads</span>
          </div>
          <button id="priv-clear-recent" class="btn">Clear Recent Media &amp; Positions</button>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Clear All Activity Data</strong>
            <span>Clears History, Recent Media, Resume Positions, and Temporary Cache (preserves Bookmarks &amp; Permanent Downloads)</span>
          </div>
          <button id="priv-clear-all" class="btn btn-danger">Clear Activity Data</button>
        </div>
      </div>
    `;

    pane.querySelector("#set-hist-en").onchange = (e) =>
      updateSetting("browsing_history_enabled", e.target.checked);
    pane.querySelector("#priv-clear-hist").onclick = async () => {
      try {
        await invokeCmd("history_clear_period", { period: "all" });
        store.history = [];
        showToast("Browsing history cleared");
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    pane.querySelector("#priv-clear-recent").onclick = async () => {
      try {
        await invokeCmd("recent_media_clear");
        await invokeCmd("playback_position_clear_all");
        store.recentMedia = [];
        showToast("Recent Media and resume positions cleared");
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    pane.querySelector("#priv-clear-all").onclick = async () => {
      try {
        await invokeCmd("privacy_clear_all_data");
        store.recentMedia = [];
        store.history = [];
        showToast("Activity data cleared", "success");
      } catch (e) {
        showToast(String(e), "error");
      }
    };
    return;
  }

  if (tab === "diagnostics") {
    let deps = store.dependencies || {
      portable_root: ".",
      portable_writable: true,
      free_disk_bytes: null,
      webview2_available: true,
      webview2_source: "Microsoft Edge WebView2",
      webview2_version: null,
      libmpv_available: false,
      libmpv_path: "runtime/media-engine/libmpv-2.dll",
      libmpv_api_version: null,
    };
    let logs = [];
    try {
      deps = await invokeCmd("get_dependency_status");
      store.dependencies = deps;
    } catch (_) {}
    try {
      logs = await invokeCmd("get_recent_logs", { limit: 80 });
    } catch (_) {}
    if (seq !== settingsRenderSeq) return;
    pane.innerHTML = `
      <div class="settings-group">
        <h3>Portable Architecture &amp; Windows Runtime Diagnostics</h3>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Portable Application Root</strong>
            <span><code>${escapeHtml(deps.portable_root)}</code></span>
          </div>
          <span class="status-pill ${deps.portable_writable ? "completed" : "failed"}">
            ${deps.portable_writable ? "Writable" : "Read-Only"}
          </span>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>AppData Isolation</strong>
            <span>Zero persistent reliance on <code>%APPDATA%</code> or <code>%LOCALAPPDATA%</code></span>
          </div>
          <span class="status-pill completed">100% Self-Contained</span>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Available Portable Drive Space</strong>
            <span>Checked before permanent downloads &amp; streaming cache writes</span>
          </div>
          <span>${deps.free_disk_bytes ? formatBytes(deps.free_disk_bytes) : "Available"}</span>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>Windows WebView2 Browser Engine</strong>
            <span>${escapeHtml(deps.webview2_source)} ${deps.webview2_version ? `(${escapeHtml(deps.webview2_version)})` : ""}</span>
          </div>
          <span class="status-pill ${deps.webview2_available ? "completed" : "failed"}">
            ${deps.webview2_available ? "Detected" : "Missing"}
          </span>
        </div>

        <div class="setting-row">
          <div class="setting-label">
            <strong>libmpv Built-in Media Engine</strong>
            <span>${escapeHtml(deps.libmpv_path || "runtime/media-engine/libmpv-2.dll")}</span>
          </div>
          <span class="status-pill ${deps.libmpv_available ? "completed" : "paused"}">
            ${escapeHtml(deps.libmpv_api_version || "Not Found")}
          </span>
        </div>
      </div>

      <div class="settings-group">
        <h3>Portable-Safe GitHub Release Update Notifier (v1.1.0)</h3>
        <p style="font-size:12px;color:var(--text-secondary);margin-top:0;">
          Checks GitHub Releases on demand without auto-overwriting your files. When upgrading, replace <code>StreamVault.exe</code> (and <code>runtime\\</code> if updated) while keeping your <code>data\\</code> folder intact — your SQLite database and single rolling backup (<code>data\\backups\\streamvault.backup.db</code>) remain 100% preserved.
        </p>

        <div class="setting-row">
          <div class="setting-label">
            <strong>GitHub Repository Slug</strong>
            <span>Format: <code>owner/StreamVault</code></span>
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <input id="set-gh-repo" class="input-field" style="width:230px;" placeholder="owner/StreamVault" value="${escapeHtml(s.github_repo_slug || "")}" />
            <button id="btn-save-gh-repo" class="btn btn-sm">Save Repo</button>
            <button id="btn-check-gh-update" class="btn btn-primary btn-sm">Check for Updates</button>
          </div>
        </div>

        <div id="gh-update-result-box" style="margin-top:10px;font-size:12.5px;color:var(--text-secondary);"></div>
      </div>

      <div class="settings-group">
        <h3>Structured Application Logs (<code>data/logs/streamvault.log</code>)</h3>
        <div class="log-viewer">
          ${
            logs.length === 0
              ? "No log entries."
              : logs
                  .map(
                    (l) =>
                      `<div><span style="color:var(--text-muted);">${escapeHtml(l.timestamp)}</span> <strong style="color:var(--accent);">[${escapeHtml(l.level)}/${escapeHtml(l.component)}]</strong> ${escapeHtml(l.message)}</div>`
                  )
                  .join("")
          }
        </div>
      </div>
    `;

    const ghInput = pane.querySelector("#set-gh-repo");
    const ghResBox = pane.querySelector("#gh-update-result-box");

    const saveGhRepo = () => {
      updateSetting("github_repo_slug", ghInput.value.trim());
    };
    pane.querySelector("#btn-save-gh-repo").onclick = saveGhRepo;
    ghInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        saveGhRepo();
      }
    });

    pane.querySelector("#btn-check-gh-update").onclick = async () => {
      const slug = ghInput.value.trim();
      if (!slug || !slug.includes("/")) {
        showToast("Enter your GitHub repository slug first (e.g. owner/StreamVault)", "error");
        return;
      }
      ghResBox.textContent = "Checking GitHub Releases…";
      try {
        if (slug !== (s.github_repo_slug || "")) {
          store.settings = await invokeCmd("settings_update_key", {
            key: "github_repo_slug",
            value: slug,
          });
        }
        const check = await invokeCmd("app_check_github_update", {
          repoSlugOverride: slug,
        });
        const badgeHtml = check.update_available
          ? `<span class="status-pill completed" style="margin-right:8px;">Update Available: v${escapeHtml(
              check.latest_version || ""
            )}</span>`
          : `<span class="status-pill" style="margin-right:8px;">Up to Date (v${escapeHtml(
              check.current_version
            )})</span>`;
        ghResBox.innerHTML = `
          <div style="padding:10px 12px;border-radius:8px;background:var(--bg-app);border:1px solid var(--border-subtle);display:flex;flex-direction:column;gap:8px;">
            <div style="display:flex;align-items:center;justify-content:space-between;flex-wrap:wrap;gap:8px;">
              <div>${badgeHtml} <strong>${escapeHtml(check.release_name || "")}</strong></div>
              ${
                check.release_url
                  ? `<div style="display:flex;gap:6px;">
                       <button id="gh-open-rel" class="btn btn-primary btn-sm">Open Release Page</button>
                       <button id="gh-copy-rel" class="btn btn-sm">Copy URL</button>
                     </div>`
                  : ""
              }
            </div>
            ${
              check.notes_excerpt
                ? `<div style="font-size:12px;color:var(--text-secondary);white-space:pre-wrap;">${escapeHtml(
                    check.notes_excerpt
                  )}</div>`
                : ""
            }
            <div style="font-size:11.5px;color:var(--text-muted);">${escapeHtml(
              check.portable_upgrade_note
            )}</div>
          </div>
        `;
        if (check.release_url) {
          ghResBox.querySelector("#gh-open-rel").onclick = () => {
            if (switchViewFn) switchViewFn("browser");
            invokeCmd("tab_create", { url: check.release_url, active: true }).catch(() => {});
          };
          ghResBox.querySelector("#gh-copy-rel").onclick = () => {
            copyTextToClipboard(check.release_url);
          };
        }
      } catch (e) {
        ghResBox.textContent = `Update check error: ${e}`;
        showToast(String(e), "error");
      }
    };
  }
}
