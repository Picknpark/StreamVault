use crate::browser_tabs::{BrowserTabInfo, BrowserTabManager, ViewportBounds};
use serde::{Deserialize, Serialize};
use streamvault_core::db::bookmarks::{Bookmark, BookmarkFolder, BookmarkTree};
use streamvault_core::db::downloads::DownloadRecord;
use streamvault_core::db::history::{AddressSuggestion, HistoryEntry};
use streamvault_core::db::recent_media::{PlaybackPositionRecord, RecentMediaItem};
use streamvault_core::db::settings::AppSettings;
use streamvault_core::downloader::SaveAsSuggestion;
use streamvault_core::external_player::{
    add_custom_external_player, detect_external_players, launch_in_external_player,
    remove_custom_external_player, test_external_player, ExternalPlayerInfo, PlayerTestResult,
};
use streamvault_core::logging::LogEntry;
use streamvault_core::media_detect::{
    extract_display_title_from_url, preserve_or_sanitize_http_url, probe_media_url,
    MediaDetectionResult,
};
use streamvault_core::mpv_engine::PlayerStateSnapshot;
use streamvault_core::portable::DependencyStatus;
use streamvault_core::stream_cache::{CacheCleanupReport, CacheSummary, StreamCacheStatus};
use streamvault_core::{ActiveMediaHighlight, StreamVaultCore};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone)]
pub struct AppState {
    pub core: StreamVaultCore,
    pub tabs: BrowserTabManager,
}

type CmdResult<T> = Result<T, String>;

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapPayload {
    pub settings: AppSettings,
    pub dependencies: DependencyStatus,
    pub tabs: Vec<BrowserTabInfo>,
    pub bookmarks: BookmarkTree,
    pub recent_media: Vec<RecentMediaItem>,
    pub downloads: Vec<DownloadRecord>,
    pub external_players: Vec<ExternalPlayerInfo>,
    pub active_highlight: ActiveMediaHighlight,
    pub recovered_corrupt_db: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackPreparationResponse {
    pub needs_resume_dialog: bool,
    pub saved_position: Option<PlaybackPositionRecord>,
    pub started: bool,
    pub player_state: Option<PlayerStateSnapshot>,
    pub stream_status: Option<StreamCacheStatus>,
}

// ---------------------------------------------------------------------------
// 1. Bootstrap & Diagnostics
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn app_bootstrap_state(
    state: State<'_, AppState>,
) -> CmdResult<BootstrapPayload> {
    // Decoupled from WebView2 tab creation so Settings, Bookmarks, Home, Recent Media,
    // and Downloads load immediately in <2ms even before any child WebView2 initializes.
    let tabs = state.tabs.list_tabs();
    let settings = state.core.db.get_settings().unwrap_or_default();
    let dependencies = state.core.paths.inspect_dependencies();
    let bookmarks = state
        .core
        .db
        .list_bookmark_tree()
        .unwrap_or(BookmarkTree {
            folders: Vec::new(),
            bookmarks: Vec::new(),
        });
    let recent_media = state.core.db.list_recent_media(50).unwrap_or_default();
    let downloads = state
        .core
        .downloader
        .list_downloads_with_live_progress()
        .unwrap_or_default();
    let external_players = detect_external_players(&state.core.db).unwrap_or_default();
    let active_highlight = state.core.active_highlight.read().clone();
    let recovered_corrupt_db = state.core.db.was_recovered_from_corruption();

    Ok(BootstrapPayload {
        settings,
        dependencies,
        tabs,
        bookmarks,
        recent_media,
        downloads,
        external_players,
        active_highlight,
        recovered_corrupt_db,
    })
}

#[tauri::command]
pub async fn get_dependency_status(state: State<'_, AppState>) -> CmdResult<DependencyStatus> {
    let paths = state.core.paths.clone();
    tokio::task::spawn_blocking(move || paths.inspect_dependencies())
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn get_recent_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<LogEntry>> {
    Ok(state.core.logger.recent_entries(limit.unwrap_or(150)))
}

#[tauri::command]
pub async fn clipboard_write_text(text: String) -> CmdResult<()> {
    tokio::task::spawn_blocking(move || {
        streamvault_core::sys_utils::write_system_clipboard_text(&text).map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

// ---------------------------------------------------------------------------
// 2. Multi-Tab WebView2 Browser & Secure Tab Context Bridge
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn tab_ensure_initialized(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Vec<BrowserTabInfo>> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.initialize_startup_tabs(&app).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_list(state: State<'_, AppState>) -> CmdResult<Vec<BrowserTabInfo>> {
    Ok(state.tabs.list_tabs())
}

#[tauri::command]
pub async fn tab_create(
    app: AppHandle,
    state: State<'_, AppState>,
    url: Option<String>,
    active: Option<bool>,
) -> CmdResult<BrowserTabInfo> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || {
        tabs.create_tab(&app, url.as_deref(), active.unwrap_or(true))
            .map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_close(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: String,
) -> CmdResult<Vec<BrowserTabInfo>> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.close_tab(&app, &tab_id).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_switch(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: String,
) -> CmdResult<Vec<BrowserTabInfo>> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.switch_tab(&app, &tab_id).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_navigate(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
    input: String,
) -> CmdResult<BrowserTabInfo> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || {
        tabs.navigate_tab(&app, tab_id.as_deref(), &input)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_go_back(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<()> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.go_back(&app, tab_id.as_deref()).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_go_forward(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<()> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.go_forward(&app, tab_id.as_deref()).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_reload(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<()> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || tabs.reload(&app, tab_id.as_deref()).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn tab_go_home(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<BrowserTabInfo> {
    let settings = state.core.db.get_settings().map_err(err_str)?;
    let target_home = streamvault_core::media_detect::resolve_homepage_or_blank(
        &settings.homepage_url,
        &settings.search_engine_template,
    );
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || {
        tabs.navigate_tab(&app, tab_id.as_deref(), &target_home)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

#[tauri::command]
pub async fn browser_set_viewport(
    app: AppHandle,
    state: State<'_, AppState>,
    bounds: ViewportBounds,
) -> CmdResult<()> {
    let tabs = state.tabs.clone();
    tokio::task::spawn_blocking(move || {
        tabs.set_viewport(&app, bounds);
    })
    .await
    .map_err(err_str)
}

#[tauri::command]
pub async fn suggest_address_bar(
    state: State<'_, AppState>,
    query: String,
) -> CmdResult<Vec<AddressSuggestion>> {
    state
        .core
        .db
        .suggest_addresses(&query, 10)
        .map_err(err_str)
}

/// Narrow, nonce-authenticated bridge called ONLY by the closure-isolated content script
/// inside `tab-*` WebView2 instances when the user clicks an item in the right-click media menu.
#[tauri::command]
pub async fn tab_context_bridge(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    tab_id: String,
    action: String,
    url: String,
    title: String,
) -> CmdResult<()> {
    if !state.tabs.verify_nonce(&token) {
        return Err("Unauthorized tab context bridge invocation".to_string());
    }
    let clean_url = preserve_or_sanitize_http_url(&url).map_err(err_str)?;

    match action.as_str() {
        "open_new_tab" => {
            let tabs = state.tabs.clone();
            let title_opt = (!title.trim().is_empty()).then(|| title.clone());
            tokio::task::spawn_blocking(move || {
                tabs.create_tab_allowing_media(&app, &clean_url, title_opt.as_deref(), true)
                    .map_err(err_str)
            })
            .await
            .map_err(err_str)??;
        }
        "play_in_app" => {
            let _ = app.emit(
                "sv://context-action",
                serde_json::json!({
                    "action": "play_in_app",
                    "url": clean_url,
                    "title": title,
                    "tab_id": tab_id
                }),
            );
        }
        "play_external" => {
            let _ = app.emit(
                "sv://context-action",
                serde_json::json!({
                    "action": "play_external",
                    "url": clean_url,
                    "title": title,
                    "tab_id": tab_id
                }),
            );
        }
        "download" => {
            let _ = app.emit(
                "sv://context-action",
                serde_json::json!({
                    "action": "download",
                    "url": clean_url,
                    "title": title,
                    "tab_id": tab_id
                }),
            );
        }
        "probe_and_inspect" => {
            state.tabs.dispatch_media_detected(app, tab_id, clean_url);
        }
        "copy_url" => {
            let url_to_copy = clean_url.clone();
            let copy_res = tokio::task::spawn_blocking(move || {
                streamvault_core::sys_utils::write_system_clipboard_text(&url_to_copy)
                    .map_err(err_str)
            })
            .await
            .map_err(err_str)?;
            match copy_res {
                Ok(()) => {
                    let _ = app.emit(
                        "sv://toast",
                        serde_json::json!({ "message": "Copied media URL to clipboard", "kind": "success" }),
                    );
                }
                Err(err) => {
                    let _ = app.emit(
                        "sv://toast",
                        serde_json::json!({ "message": format!("Clipboard error: {err}"), "kind": "error" }),
                    );
                    return Err(err);
                }
            }
        }
        _ => return Err("Unsupported context action".to_string()),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Media Detection, Resume Check & Playback Orchestration
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn probe_media_link(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<MediaDetectionResult> {
    probe_media_url(&state.core.http_client, &url)
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn prepare_in_app_playback(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    custom_title: Option<String>,
) -> CmdResult<PlaybackPreparationResponse> {
    let clean_url = preserve_or_sanitize_http_url(&url).map_err(err_str)?;
    let settings = state.core.db.get_settings().map_err(err_str)?;
    let saved_pos = state
        .core
        .db
        .get_playback_position(&clean_url)
        .map_err(err_str)?;

    if let Some(ref pos) = saved_pos {
        if pos.should_offer_resume {
            match settings.resume_playback_behavior.as_str() {
                "always_resume" => {
                    return execute_in_app_playback(
                        &app,
                        &state,
                        &clean_url,
                        custom_title.as_deref(),
                        Some(pos.position_ms),
                    )
                    .await;
                }
                "always_beginning" => {
                    return execute_in_app_playback(
                        &app,
                        &state,
                        &clean_url,
                        custom_title.as_deref(),
                        Some(0),
                    )
                    .await;
                }
                _ => {
                    // Offer Resume from MM:SS vs Start from Beginning dialog!
                    return Ok(PlaybackPreparationResponse {
                        needs_resume_dialog: true,
                        saved_position: Some(pos.clone()),
                        started: false,
                        player_state: None,
                        stream_status: None,
                    });
                }
            }
        }
    }

    execute_in_app_playback(&app, &state, &clean_url, custom_title.as_deref(), Some(0)).await
}

#[tauri::command]
pub async fn start_in_app_playback(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    custom_title: Option<String>,
    start_position_ms: Option<i64>,
) -> CmdResult<PlaybackPreparationResponse> {
    let clean_url = preserve_or_sanitize_http_url(&url).map_err(err_str)?;
    if start_position_ms == Some(0) {
        let _ = state.core.db.reset_playback_position(&clean_url);
    }
    execute_in_app_playback(
        &app,
        &state,
        &clean_url,
        custom_title.as_deref(),
        start_position_ms,
    )
    .await
}

async fn execute_in_app_playback(
    app: &AppHandle,
    state: &State<'_, AppState>,
    clean_url: &str,
    custom_title: Option<&str>,
    start_position_ms: Option<i64>,
) -> CmdResult<PlaybackPreparationResponse> {
    // Save outgoing playback position if another file was playing
    let prev = state.core.player.snapshot();
    if prev.active {
        if let Some(ref prev_url) = prev.original_url {
            let _ = state.core.db.save_playback_position(
                prev_url,
                prev.position_ms,
                prev.duration_ms,
                prev.audio_track_id,
                prev.subtitle_track_id,
                Some(prev.speed),
                Some(&prev.aspect_ratio),
            );
        }
    }

    let settings = state.core.db.get_settings().map_err(err_str)?;
    let stream_status = state
        .core
        .stream_cache
        .start_stream_session(clean_url)
        .await
        .map_err(err_str)?;

    let display_title = custom_title
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| extract_display_title_from_url(clean_url));

    let _ = state.core.db.record_recent_media_open(
        clean_url,
        Some(&display_title),
        Some(&stream_status.mime_type),
        "in_app",
    );

    let player_snap = state
        .core
        .player
        .load_stream(
            clean_url,
            &stream_status.local_stream_url,
            &display_title,
            start_position_ms,
            Some(settings.default_playback_speed),
        )
        .map_err(err_str)?;

    let _ = state.core.player.set_volume(settings.default_volume);

    state
        .core
        .set_active_media_highlight(Some(clean_url), Some("in_app"), Some(&display_title));
    state.tabs.broadcast_active_media_highlight(app);

    Ok(PlaybackPreparationResponse {
        needs_resume_dialog: false,
        saved_position: None,
        started: true,
        player_state: Some(player_snap),
        stream_status: Some(stream_status),
    })
}

#[tauri::command]
pub async fn open_media_external(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    executable_path: Option<String>,
    custom_title: Option<String>,
) -> CmdResult<serde_json::Value> {
    let clean_url = preserve_or_sanitize_http_url(&url).map_err(err_str)?;
    let chosen_exe = match executable_path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(exe) => exe,
        None => {
            let settings = state.core.db.get_settings().map_err(err_str)?;
            if settings.default_external_player == "ask"
                || settings.default_external_player.trim().is_empty()
            {
                let players = detect_external_players(&state.core.db).map_err(err_str)?;
                return Ok(serde_json::json!({
                    "needs_player_selection": true,
                    "players": players,
                    "url": clean_url
                }));
            }
            settings.default_external_player
        }
    };

    let title = custom_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| extract_display_title_from_url(&clean_url));

    launch_in_external_player(
        &state.core.db,
        &state.core.logger,
        &chosen_exe,
        &clean_url,
        Some(&title),
    )
    .map_err(err_str)?;

    state
        .core
        .set_active_media_highlight(Some(&clean_url), Some("external"), Some(&title));
    state.tabs.broadcast_active_media_highlight(&app);

    Ok(serde_json::json!({
        "needs_player_selection": false,
        "launched": true,
        "executable": chosen_exe
    }))
}

// ---------------------------------------------------------------------------
// 4. Built-in libmpv Media Player Controls
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn player_get_state(state: State<'_, AppState>) -> CmdResult<PlayerStateSnapshot> {
    Ok(state.core.player.snapshot())
}

#[tauri::command]
pub async fn player_toggle_pause(state: State<'_, AppState>) -> CmdResult<PlayerStateSnapshot> {
    let snap = state.core.player.toggle_pause().map_err(err_str)?;
    if snap.paused {
        persist_snapshot_position(&state.core, &snap);
    }
    Ok(snap)
}

#[tauri::command]
pub async fn player_set_pause(state: State<'_, AppState>, paused: bool) -> CmdResult<PlayerStateSnapshot> {
    let snap = state.core.player.set_pause(paused).map_err(err_str)?;
    if snap.paused {
        persist_snapshot_position(&state.core, &snap);
    }
    Ok(snap)
}

#[tauri::command]
pub async fn player_stop(app: AppHandle, state: State<'_, AppState>) -> CmdResult<PlayerStateSnapshot> {
    let before = state.core.player.snapshot();
    if before.active {
        persist_snapshot_position(&state.core, &before);
    }
    let after = state.core.player.stop().map_err(err_str)?;
    state.core.stream_cache.stop_active_session();
    state.core.set_active_media_highlight(None, None, None);
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(after)
}

#[tauri::command]
pub async fn player_seek_relative(
    state: State<'_, AppState>,
    delta_seconds: f64,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .seek_relative_seconds(delta_seconds)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_seek_absolute(
    state: State<'_, AppState>,
    position_ms: i64,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .seek_absolute_ms(position_ms)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_set_volume(
    state: State<'_, AppState>,
    volume: i64,
) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_volume(volume).map_err(err_str)
}

#[tauri::command]
pub async fn player_toggle_mute(state: State<'_, AppState>) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.toggle_mute().map_err(err_str)
}

#[tauri::command]
pub async fn player_set_speed(state: State<'_, AppState>, speed: f64) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_speed(speed).map_err(err_str)
}

#[tauri::command]
pub async fn player_set_audio_track(
    state: State<'_, AppState>,
    track_id: i64,
) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_audio_track(track_id).map_err(err_str)
}

#[tauri::command]
pub async fn player_set_subtitle_track(
    state: State<'_, AppState>,
    track_id: Option<i64>,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .set_subtitle_track(track_id)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_browse_and_load_subtitle(
    state: State<'_, AppState>,
) -> CmdResult<Option<PlayerStateSnapshot>> {
    #[cfg(windows)]
    {
        let picked = rfd::AsyncFileDialog::new()
            .set_title("Load External Subtitle File — StreamVault")
            .add_filter("Subtitle Files", &["srt", "ass", "ssa", "vtt", "sub", "idx"])
            .pick_file()
            .await;
        if let Some(file) = picked {
            let snap = state
                .core
                .player
                .add_external_subtitle_file(file.path())
                .map_err(err_str)?;
            return Ok(Some(snap));
        }
        return Ok(None);
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Ok(None)
    }
}

#[tauri::command]
pub async fn player_set_aspect_ratio(
    state: State<'_, AppState>,
    ratio: String,
) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_aspect_ratio(&ratio).map_err(err_str)
}

#[tauri::command]
pub async fn player_set_fullscreen(
    app: AppHandle,
    state: State<'_, AppState>,
    fullscreen: Option<bool>,
) -> CmdResult<bool> {
    state.core.player.set_time_tooltip(false, 0, 0, 1.0, "");
    let win = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;
    let current = win.is_fullscreen().unwrap_or(false);
    let target = fullscreen.unwrap_or(!current);
    win.set_fullscreen(target).map_err(err_str)?;
    Ok(target)
}

#[tauri::command]
pub async fn player_set_pip(
    state: State<'_, AppState>,
    enable: bool,
) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_pip_mode(enable).map_err(err_str)
}

#[tauri::command]
pub async fn player_set_viewport(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> CmdResult<()> {
    let scale = app
        .get_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let px = (x * scale).round() as i32;
    let py = (y * scale).round() as i32;
    let pw = (width * scale).round() as i32;
    let ph = (height * scale).round() as i32;
    state
        .core
        .player
        .set_video_viewport(visible, px, py, pw, ph);
    Ok(())
}

#[tauri::command]
pub async fn player_set_time_tooltip(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
    x: f64,
    y: f64,
    text: String,
) -> CmdResult<()> {
    let scale = app
        .get_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let px = (x * scale).round() as i32;
    let py = (y * scale).round() as i32;
    state
        .core
        .player
        .set_time_tooltip(visible, px, py, scale, &text);
    Ok(())
}

pub fn persist_snapshot_position(core: &StreamVaultCore, snap: &PlayerStateSnapshot) {
    if let Some(ref url) = snap.original_url {
        if snap.position_ms > 0 || snap.duration_ms > 0 {
            let _ = core.db.save_playback_position(
                url,
                snap.position_ms,
                snap.duration_ms,
                snap.audio_track_id,
                snap.subtitle_track_id,
                Some(snap.speed),
                Some(&snap.aspect_ratio),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Temporary Streaming Cache Management
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn stream_cache_status(state: State<'_, AppState>) -> CmdResult<Option<StreamCacheStatus>> {
    Ok(state.core.stream_cache.get_active_status())
}

#[tauri::command]
pub async fn stream_cache_summary(state: State<'_, AppState>) -> CmdResult<CacheSummary> {
    state.core.stream_cache.inspect_cache().map_err(err_str)
}

#[tauri::command]
pub async fn stream_cache_clear(state: State<'_, AppState>) -> CmdResult<CacheCleanupReport> {
    state
        .core
        .stream_cache
        .clear_inactive_cache()
        .map_err(err_str)
}

// ---------------------------------------------------------------------------
// 6. Permanent Media Download Manager
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn download_prepare_save_as(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<SaveAsSuggestion> {
    state
        .core
        .downloader
        .prepare_save_as(&url)
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn download_prompt_save_as_and_start(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<DownloadRecord>> {
    let picked = state
        .core
        .downloader
        .prompt_native_save_as(&url)
        .await
        .map_err(err_str)?;
    let Some((_suggestion, dest_path)) = picked else {
        return Ok(None);
    };
    let rec = state
        .core
        .downloader
        .start_download(&url, Some(dest_path), false)
        .await
        .map_err(err_str)?;
    Ok(Some(rec))
}

#[tauri::command]
pub async fn download_start_default_dir(
    state: State<'_, AppState>,
    url: String,
    custom_filename: Option<String>,
) -> CmdResult<DownloadRecord> {
    let dest = if let Some(fname) = custom_filename.filter(|s| !s.trim().is_empty()) {
        let settings = state.core.db.get_settings().map_err(err_str)?;
        let dir = state
            .core
            .paths
            .resolve_portable_path(&settings.default_download_dir);
        let safe_name = streamvault_core::media_detect::sanitize_windows_filename(&fname);
        Some(dir.join(safe_name))
    } else {
        None
    };

    state
        .core
        .downloader
        .start_download(&url, dest, true)
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn download_list(state: State<'_, AppState>) -> CmdResult<Vec<DownloadRecord>> {
    state
        .core
        .downloader
        .list_downloads_with_live_progress()
        .map_err(err_str)
}

#[tauri::command]
pub async fn download_pause(state: State<'_, AppState>, id: String) -> CmdResult<DownloadRecord> {
    state.core.downloader.pause_download(&id).map_err(err_str)
}

#[tauri::command]
pub async fn download_resume(state: State<'_, AppState>, id: String) -> CmdResult<DownloadRecord> {
    state.core.downloader.resume_download(&id).map_err(err_str)
}

#[tauri::command]
pub async fn download_retry(state: State<'_, AppState>, id: String) -> CmdResult<DownloadRecord> {
    state.core.downloader.retry_download(&id).map_err(err_str)
}

#[tauri::command]
pub async fn download_cancel(state: State<'_, AppState>, id: String) -> CmdResult<DownloadRecord> {
    state.core.downloader.cancel_download(&id).map_err(err_str)
}

#[tauri::command]
pub async fn download_remove(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state
        .core
        .downloader
        .remove_download_entry(&id)
        .map_err(err_str)
}

#[tauri::command]
pub async fn download_clear_completed(state: State<'_, AppState>) -> CmdResult<usize> {
    state
        .core
        .db
        .clear_completed_download_records()
        .map_err(err_str)
}

#[tauri::command]
pub async fn download_open_file(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let downloader = state.core.downloader.clone();
    tokio::task::spawn_blocking(move || downloader.open_downloaded_file(&id).map_err(err_str))
        .await
        .map_err(err_str)?
}

#[tauri::command]
pub async fn download_open_folder(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let downloader = state.core.downloader.clone();
    tokio::task::spawn_blocking(move || downloader.open_download_folder(&id).map_err(err_str))
        .await
        .map_err(err_str)?
}

// ---------------------------------------------------------------------------
// 7. Recent Media & Playback Positions (Strictly Separate from Bookmarks!)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn recent_media_list(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<RecentMediaItem>> {
    state
        .core
        .db
        .list_recent_media(limit.unwrap_or(100))
        .map_err(err_str)
}

#[tauri::command]
pub async fn recent_media_remove(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.core.db.remove_recent_media(id).map_err(err_str)
}

#[tauri::command]
pub async fn recent_media_clear(state: State<'_, AppState>) -> CmdResult<usize> {
    state.core.db.clear_recent_media().map_err(err_str)
}

#[tauri::command]
pub async fn playback_position_get(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<PlaybackPositionRecord>> {
    state.core.db.get_playback_position(&url).map_err(err_str)
}

#[tauri::command]
pub async fn playback_position_reset(state: State<'_, AppState>, url: String) -> CmdResult<()> {
    state
        .core
        .db
        .reset_playback_position(&url)
        .map_err(err_str)
}

#[tauri::command]
pub async fn playback_position_clear_all(state: State<'_, AppState>) -> CmdResult<usize> {
    state
        .core
        .db
        .clear_all_playback_positions()
        .map_err(err_str)
}

// ---------------------------------------------------------------------------
// 8. Bookmarks & Folders (Strictly Separate from Recent Media!)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn bookmarks_tree(state: State<'_, AppState>) -> CmdResult<BookmarkTree> {
    state.core.db.list_bookmark_tree().map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_check_url(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<Bookmark>> {
    state.core.db.find_bookmark_by_url(&url).map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_add(
    state: State<'_, AppState>,
    title: String,
    url: String,
    folder_id: Option<i64>,
    show_on_bar: Option<bool>,
) -> CmdResult<Bookmark> {
    state
        .core
        .db
        .add_bookmark(&title, &url, folder_id, show_on_bar.unwrap_or(true))
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_update(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    url: String,
    folder_id: Option<i64>,
    show_on_bar: bool,
) -> CmdResult<Bookmark> {
    state
        .core
        .db
        .update_bookmark(id, &title, &url, folder_id, show_on_bar)
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_move(
    state: State<'_, AppState>,
    id: i64,
    folder_id: Option<i64>,
    position: Option<i64>,
) -> CmdResult<()> {
    state
        .core
        .db
        .move_bookmark(id, folder_id, position)
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_delete(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.core.db.delete_bookmark(id).map_err(err_str)
}

#[tauri::command]
pub async fn bookmark_folder_create(
    state: State<'_, AppState>,
    name: String,
    parent_id: Option<i64>,
) -> CmdResult<BookmarkFolder> {
    state
        .core
        .db
        .create_bookmark_folder(&name, parent_id)
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmark_folder_rename(
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> CmdResult<()> {
    state
        .core
        .db
        .rename_bookmark_folder(id, &name)
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmark_folder_delete(
    state: State<'_, AppState>,
    id: i64,
    delete_bookmarks: Option<bool>,
) -> CmdResult<()> {
    state
        .core
        .db
        .delete_bookmark_folder(id, delete_bookmarks.unwrap_or(false))
        .map_err(err_str)
}

#[tauri::command]
pub async fn bookmarks_search(state: State<'_, AppState>, query: String) -> CmdResult<Vec<Bookmark>> {
    state.core.db.search_bookmarks(&query).map_err(err_str)
}

// ---------------------------------------------------------------------------
// 9. Browsing History
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn history_list(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: Option<usize>,
) -> CmdResult<Vec<HistoryEntry>> {
    state
        .core
        .db
        .list_history(query.as_deref(), limit.unwrap_or(500))
        .map_err(err_str)
}

#[tauri::command]
pub async fn history_delete(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.core.db.delete_history_entry(id).map_err(err_str)
}

#[tauri::command]
pub async fn history_clear_period(state: State<'_, AppState>, period: String) -> CmdResult<usize> {
    state
        .core
        .db
        .clear_history_period(&period)
        .map_err(err_str)
}

// ---------------------------------------------------------------------------
// 10. External Media Players
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn external_players_detect(state: State<'_, AppState>) -> CmdResult<Vec<ExternalPlayerInfo>> {
    detect_external_players(&state.core.db).map_err(err_str)
}

#[tauri::command]
pub async fn external_player_browse_exe() -> CmdResult<Option<String>> {
    #[cfg(windows)]
    {
        let picked = rfd::AsyncFileDialog::new()
            .set_title("Select External Media Player Executable — StreamVault")
            .add_filter("Windows Executable (*.exe)", &["exe"])
            .pick_file()
            .await;
        return Ok(picked.map(|h| h.path().to_string_lossy().to_string()));
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[tauri::command]
pub async fn external_player_add_custom(
    state: State<'_, AppState>,
    name: String,
    executable_path: String,
) -> CmdResult<ExternalPlayerInfo> {
    add_custom_external_player(&state.core.db, &name, &executable_path).map_err(err_str)
}

#[tauri::command]
pub async fn external_player_remove_custom(
    state: State<'_, AppState>,
    custom_id: i64,
) -> CmdResult<()> {
    remove_custom_external_player(&state.core.db, custom_id).map_err(err_str)
}

#[tauri::command]
pub async fn external_player_test(
    state: State<'_, AppState>,
    executable_path: String,
) -> CmdResult<PlayerTestResult> {
    test_external_player(&executable_path, &state.core.logger).map_err(err_str)
}

// ---------------------------------------------------------------------------
// 11. Settings & Privacy
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    state.core.db.get_settings().map_err(err_str)
}

#[tauri::command]
pub async fn settings_save(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> CmdResult<AppSettings> {
    state.core.db.save_settings(&settings).map_err(err_str)?;
    state.core.db.get_settings().map_err(err_str)
}

#[tauri::command]
pub async fn settings_update_key(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> CmdResult<AppSettings> {
    state
        .core
        .db
        .update_single_setting(&key, value)
        .map_err(err_str)
}

#[tauri::command]
pub async fn settings_browse_download_dir(
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    #[cfg(windows)]
    {
        let current_settings = state.core.db.get_settings().map_err(err_str)?;
        let start_dir = state
            .core
            .paths
            .resolve_portable_path(&current_settings.default_download_dir);
        let picked = rfd::AsyncFileDialog::new()
            .set_title("Choose Default Download Folder — StreamVault")
            .set_directory(start_dir)
            .pick_folder()
            .await;
        if let Some(folder) = picked {
            let portable_str = state.core.paths.to_portable_string(folder.path());
            let _ = state
                .core
                .db
                .update_single_setting("default_download_dir", serde_json::json!(portable_str));
            return Ok(Some(portable_str));
        }
        return Ok(None);
    }
    #[cfg(not(windows))]
    {
        let _ = PathBuf::new();
        let _ = state;
        Ok(None)
    }
}

#[tauri::command]
pub async fn privacy_clear_all_data(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    let hist = state.core.db.clear_history_period("all").map_err(err_str)?;
    let recent = state.core.db.clear_recent_media().map_err(err_str)?;
    let pos = state
        .core
        .db
        .clear_all_playback_positions()
        .map_err(err_str)?;
    let cache = state
        .core
        .stream_cache
        .clear_inactive_cache()
        .map_err(err_str)?;
    Ok(serde_json::json!({
        "cleared_history": hist,
        "cleared_recent_media": recent,
        "cleared_positions": pos,
        "cleared_cache_files": cache.removed_files
    }))
}
