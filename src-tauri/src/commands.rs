use crate::browser_tabs::{BrowserTabInfo, BrowserTabManager, PageMediaCatalog, ViewportBounds};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
use streamvault_core::{ActiveMediaHighlight, ReleaseUpdateCheckResult, StreamVaultCore};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotCaptureResponse {
    pub path: String,
    pub filename: String,
    pub native_overlay_shown: bool,
}

// ---------------------------------------------------------------------------
// 1. Bootstrap & Diagnostics
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn app_check_github_update(
    state: State<'_, AppState>,
    repo_slug_override: Option<String>,
) -> CmdResult<ReleaseUpdateCheckResult> {
    state
        .core
        .check_github_release_update(repo_slug_override)
        .await
        .map_err(err_str)
}

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

#[tauri::command]
pub async fn tab_get_page_media(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<Option<PageMediaCatalog>> {
    if let Some(existing) = state.tabs.get_page_media_catalog(tab_id.as_deref()) {
        if !existing.items.is_empty() {
            return Ok(Some(existing));
        }
    }
    state
        .tabs
        .scan_page_media_live(&app, tab_id.as_deref())
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn tab_request_page_media(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<Option<PageMediaCatalog>> {
    state
        .tabs
        .scan_page_media_live(&app, tab_id.as_deref())
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn tab_find_in_page(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
    query: String,
    forward: Option<bool>,
    clear: Option<bool>,
    stay_on_current: Option<bool>,
) -> CmdResult<serde_json::Value> {
    state
        .tabs
        .find_in_page_live(
            &app,
            tab_id.as_deref(),
            &query,
            forward.unwrap_or(true),
            clear.unwrap_or(false),
            stay_on_current.unwrap_or(false),
        )
        .await
        .map_err(err_str)
}

#[tauri::command]
pub async fn tab_scroll_to_active_media(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: Option<String>,
) -> CmdResult<bool> {
    Ok(state
        .tabs
        .scroll_to_active_media(&app, tab_id.as_deref())
        .await)
}

/// Narrow, nonce-authenticated bridge called ONLY by the closure-isolated content script
/// inside `tab-*` WebView2 instances.
#[tauri::command]
pub async fn tab_context_bridge(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    tab_id: String,
    action: String,
    url: String,
    title: String,
    payload: Option<String>,
) -> CmdResult<()> {
    if !state.tabs.verify_nonce(&token) {
        return Err("Unauthorized tab context bridge invocation".to_string());
    }

    match action.as_str() {
        "page_media_catalog" => {
            if let Some(raw_json) = payload {
                if let Ok(catalog) = serde_json::from_str::<PageMediaCatalog>(&raw_json) {
                    state.tabs.store_page_media_catalog(&app, &tab_id, catalog);
                }
            }
            return Ok(());
        }
        "find_result" => {
            if let Some(raw_json) = payload {
                if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("tab_id".to_string(), serde_json::json!(tab_id));
                    }
                    let _ = app.emit("sv://find-result", val);
                }
            }
            return Ok(());
        }
        "open_find_bar" => {
            let _ = app.emit(
                "sv://open-find-bar",
                serde_json::json!({ "tab_id": tab_id }),
            );
            return Ok(());
        }
        _ => {}
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
        "open_background_tab" => {
            let tabs = state.tabs.clone();
            let app_bg = app.clone();
            let title_opt = (!title.trim().is_empty()).then(|| title.clone());
            tokio::task::spawn_blocking(move || {
                tabs.create_tab_allowing_media(&app_bg, &clean_url, title_opt.as_deref(), false)
                    .map_err(err_str)
            })
            .await
            .map_err(err_str)??;
            let _ = app.emit(
                "sv://toast",
                serde_json::json!({ "message": "Opened link in background tab", "kind": "info" }),
            );
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
        "dblclick_media_prompt" => {
            let _ = app.emit(
                "sv://media-link-prompt",
                serde_json::json!({
                    "url": clean_url,
                    "title": title,
                    "tab_id": tab_id
                }),
            );
        }
        "single_click_media" => {
            let auto_detect = state
                .core
                .db
                .get_settings()
                .map(|s| s.auto_detect_media)
                .unwrap_or(true);
            if auto_detect {
                state.tabs.dispatch_media_detected(app, tab_id, clean_url);
            } else {
                let tabs = state.tabs.clone();
                let tid = tab_id.clone();
                let u = clean_url.clone();
                tokio::task::spawn_blocking(move || {
                    tabs.navigate_tab(&app, Some(&tid), &u).map_err(err_str)
                })
                .await
                .map_err(err_str)??;
            }
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
    let trimmed = url.trim();
    let (launch_target, highlight_key) =
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let clean = preserve_or_sanitize_http_url(trimmed).map_err(err_str)?;
            (clean.clone(), clean)
        } else {
            let raw_local = trimmed.strip_prefix("local://").unwrap_or(trimmed).trim();
            let native_path =
                streamvault_core::portable::normalize_native_path(PathBuf::from(raw_local));
            if raw_local.is_empty() || raw_local.starts_with('-') || !native_path.is_file() {
                return Err(format!(
                    "Invalid media URL or local file not found: {trimmed}"
                ));
            }
            let native_str = native_path.to_string_lossy().to_string();
            let local_uri = format!("local://{}", native_str.replace('\\', "/"));
            (native_str, local_uri)
        };

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
                    "url": launch_target
                }));
            }
            settings.default_external_player
        }
    };

    let title = custom_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| extract_display_title_from_url(&highlight_key));

    launch_in_external_player(
        &state.core.db,
        &state.core.logger,
        &chosen_exe,
        &launch_target,
        Some(&title),
    )
    .map_err(err_str)?;

    state
        .core
        .set_active_media_highlight(Some(&highlight_key), Some("external"), Some(&title));
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
pub async fn player_toggle_pause(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<PlayerStateSnapshot> {
    let snap = state.core.player.toggle_pause().map_err(err_str)?;
    if snap.paused {
        persist_snapshot_position(&state.core, &snap);
    }
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(snap)
}

#[tauri::command]
pub async fn player_set_pause(
    app: AppHandle,
    state: State<'_, AppState>,
    paused: bool,
) -> CmdResult<PlayerStateSnapshot> {
    let snap = state.core.player.set_pause(paused).map_err(err_str)?;
    if snap.paused {
        persist_snapshot_position(&state.core, &snap);
    }
    state.tabs.broadcast_active_media_highlight(&app);
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
    let snap = state.core.player.set_volume(volume).map_err(err_str)?;
    let _ = state
        .core
        .db
        .update_single_setting("default_volume", serde_json::json!(snap.volume));
    Ok(snap)
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

#[tauri::command]
pub async fn player_open_local_file(
    app: AppHandle,
    state: State<'_, AppState>,
    file_path: String,
    start_position_ms: Option<i64>,
) -> CmdResult<PlayerStateSnapshot> {
    let raw_trimmed = file_path
        .trim()
        .strip_prefix("local://")
        .unwrap_or(file_path.trim())
        .trim();
    let path = streamvault_core::portable::normalize_native_path(PathBuf::from(raw_trimmed));
    if !path.is_file() {
        return Err(format!("Local file does not exist: {}", path.display()));
    }

    let prev = state.core.player.snapshot();
    if prev.active {
        persist_snapshot_position(&state.core, &prev);
    }
    state.core.stream_cache.stop_active_session();

    let settings = state.core.db.get_settings().map_err(err_str)?;
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local Media".to_string());
    let local_uri_key = format!("local://{}", path.to_string_lossy().replace('\\', "/"));

    if start_position_ms == Some(0) {
        let _ = state.core.db.reset_playback_position(&local_uri_key);
    }

    let resume_ms = match start_position_ms {
        Some(ms) => Some(ms),
        None => state
            .core
            .db
            .get_playback_position(&local_uri_key)
            .ok()
            .flatten()
            .filter(|p| p.should_offer_resume && settings.resume_playback_behavior != "always_beginning")
            .map(|p| p.position_ms),
    };

    let _ = state.core.db.record_recent_media_open(
        &local_uri_key,
        Some(&title),
        None,
        "in_app",
    );

    let snap = state
        .core
        .player
        .load_local_file(
            &path,
            &title,
            resume_ms,
            Some(settings.default_playback_speed),
            settings.auto_load_matching_subtitle,
        )
        .map_err(err_str)?;

    state
        .core
        .set_active_media_highlight(Some(&local_uri_key), Some("in_app"), Some(&title));
    state.tabs.broadcast_active_media_highlight(&app);

    Ok(snap)
}

#[tauri::command]
pub async fn player_browse_and_open_local_media() -> CmdResult<Vec<String>> {
    #[cfg(windows)]
    {
        let mut media_exts: Vec<&str> = Vec::new();
        media_exts.extend_from_slice(streamvault_core::media_detect::VIDEO_EXTENSIONS);
        media_exts.extend_from_slice(streamvault_core::media_detect::AUDIO_EXTENSIONS);

        let picked = rfd::AsyncFileDialog::new()
            .set_title("Open Local Video / Audio Files — StreamVault")
            .add_filter("Media Files", &media_exts)
            .add_filter("Subtitle Files", streamvault_core::media_detect::SUBTITLE_EXTENSIONS)
            .add_filter("All Files", &["*"])
            .pick_files()
            .await;

        let Some(files) = picked else {
            return Ok(Vec::new());
        };
        return Ok(files
            .into_iter()
            .map(|f| f.path().to_string_lossy().to_string())
            .collect());
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn player_load_external_subtitle_path(
    state: State<'_, AppState>,
    file_path: String,
) -> CmdResult<PlayerStateSnapshot> {
    let p = PathBuf::from(file_path.trim());
    state
        .core
        .player
        .add_external_subtitle_file(&p)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_load_external_subtitle_url(
    state: State<'_, AppState>,
    subtitle_url: String,
    select: Option<bool>,
) -> CmdResult<PlayerStateSnapshot> {
    let clean = preserve_or_sanitize_http_url(&subtitle_url).map_err(err_str)?;
    state
        .core
        .player
        .add_external_subtitle_url(&clean, select.unwrap_or(true))
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_set_sub_delay(
    state: State<'_, AppState>,
    delay_ms: i64,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .set_sub_delay_ms(delay_ms)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_set_audio_delay(
    state: State<'_, AppState>,
    delay_ms: i64,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .set_audio_delay_ms(delay_ms)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_set_sub_scale(
    state: State<'_, AppState>,
    scale: f64,
) -> CmdResult<PlayerStateSnapshot> {
    state.core.player.set_sub_scale(scale).map_err(err_str)
}

#[tauri::command]
pub async fn player_set_dialogue_boost(
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<PlayerStateSnapshot> {
    state
        .core
        .player
        .set_dialogue_boost(enabled)
        .map_err(err_str)
}

#[tauri::command]
pub async fn player_take_screenshot(
    app: AppHandle,
    state: State<'_, AppState>,
    include_subtitles: Option<bool>,
) -> CmdResult<ScreenshotCaptureResponse> {
    let settings = state.core.db.get_settings().map_err(err_str)?;
    let base_dl = state
        .core
        .paths
        .resolve_portable_path(&settings.default_download_dir);
    let screenshots_dir = base_dl.join("Screenshots");
    let include_subs = include_subtitles.unwrap_or(true);
    let player = state.core.player.clone();
    let saved_path = tokio::task::spawn_blocking(move || {
        player
            .take_screenshot_to_dir(&screenshots_dir, include_subs)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)??;

    let filename = saved_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "screenshot.png".to_string());

    let scale_factor = app
        .get_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let player_for_ui = state.core.player.clone();
    let fname_for_ui = filename.clone();
    let path_for_ui = saved_path.clone();
    let fallback_dir = state.core.paths.downloads_dir.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let native_overlay_shown = if app
        .run_on_main_thread(move || {
            let shown = player_for_ui.show_screenshot_toast(
                &fname_for_ui,
                &path_for_ui,
                &fallback_dir,
                scale_factor,
            );
            let _ = tx.send(shown);
        })
        .is_ok()
    {
        rx.await.unwrap_or(false)
    } else {
        false
    };

    Ok(ScreenshotCaptureResponse {
        path: saved_path.to_string_lossy().to_string(),
        filename,
        native_overlay_shown,
    })
}

#[tauri::command]
pub async fn player_reveal_path(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let target = PathBuf::from(path.trim());
    let fallback_dir = state.core.paths.downloads_dir.clone();
    tokio::task::spawn_blocking(move || {
        streamvault_core::downloader::reveal_in_file_manager(&target, None, &fallback_dir)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

#[tauri::command]
pub async fn player_sync_smtc(
    state: State<'_, AppState>,
    has_prev: bool,
    has_next: bool,
) -> CmdResult<()> {
    state.core.player.sync_smtc(has_prev, has_next);
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
pub async fn download_pick_batch_folder(
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    #[cfg(windows)]
    {
        let settings = state.core.db.get_settings().map_err(err_str)?;
        let start_dir = state
            .core
            .paths
            .resolve_portable_path(&settings.default_download_dir);
        let picked = rfd::AsyncFileDialog::new()
            .set_title("Choose Destination Folder for Batch Download — StreamVault")
            .set_directory(start_dir)
            .pick_folder()
            .await;
        return Ok(picked.map(|f| state.core.paths.to_portable_string(f.path())));
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Ok(None)
    }
}

#[tauri::command]
pub async fn download_start_batch(
    state: State<'_, AppState>,
    urls: Vec<String>,
    destination_dir: Option<String>,
) -> CmdResult<Vec<DownloadRecord>> {
    let resolved_dir = destination_dir
        .filter(|s| !s.trim().is_empty())
        .map(|s| state.core.paths.resolve_portable_path(&s));
    let downloader = state.core.downloader.clone();
    tokio::task::spawn_blocking(move || {
        downloader
            .enqueue_batch_downloads(urls, resolved_dir)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

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
pub async fn recent_media_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CmdResult<()> {
    state.core.db.remove_recent_media(id).map_err(err_str)?;
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(())
}

#[tauri::command]
pub async fn recent_media_clear(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<usize> {
    let n = state.core.db.clear_recent_media().map_err(err_str)?;
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(n)
}

#[tauri::command]
pub async fn playback_position_get(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<PlaybackPositionRecord>> {
    state.core.db.get_playback_position(&url).map_err(err_str)
}

#[tauri::command]
pub async fn playback_position_reset(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<()> {
    state
        .core
        .db
        .reset_playback_position(&url)
        .map_err(err_str)?;
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(())
}

#[tauri::command]
pub async fn playback_position_clear_all(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<usize> {
    let n = state
        .core
        .db
        .clear_all_playback_positions()
        .map_err(err_str)?;
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(n)
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
pub async fn privacy_clear_all_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<serde_json::Value> {
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
    state.tabs.broadcast_active_media_highlight(&app);
    Ok(serde_json::json!({
        "cleared_history": hist,
        "cleared_recent_media": recent,
        "cleared_positions": pos,
        "cleared_cache_files": cache.removed_files
    }))
}
