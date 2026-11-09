#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browser_tabs;
mod commands;

use crate::browser_tabs::BrowserTabManager;
use crate::commands::{persist_snapshot_position, AppState};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use streamvault_core::mpv_engine::SurfaceInputEvent;
use streamvault_core::portable::PortablePaths;
use streamvault_core::StreamVaultCore;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

fn main() {
    // 1. Resolve portable directory relative to StreamVault.exe BEFORE Tauri/WebView2 initializes!
    let early_paths = match PortablePaths::resolve(None) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Fatal portable initialization error: {err}");
            return;
        }
    };

    // Force WebView2 to store 100% of shell UI state inside `StreamVault/data/webview2/shell/`
    // (isolated from `data/webview2/tabs/` used by `tab-*` child WebView2 instances, and never `%LOCALAPPDATA%`)!
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &early_paths.webview2_shell_dir);
    if let Some(fixed_wv2) = early_paths.find_fixed_webview2_dir() {
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", fixed_wv2);
    }

    // Check Windows WebView2 runtime availability before Tauri creates windows
    #[cfg(windows)]
    {
        let deps = early_paths.inspect_dependencies();
        if !deps.webview2_available {
            streamvault_core::portable::show_missing_webview2_alert(&early_paths.root_dir);
            return;
        }
    }

    let core = match StreamVaultCore::initialize(Some(early_paths.root_dir.clone())) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Failed initializing StreamVault core: {err}");
            return;
        }
    };

    let tab_manager = BrowserTabManager::new(core.clone());
    let tabs_for_setup = tab_manager.clone();
    let core_for_setup = core.clone();
    let core_for_exit = core.clone();
    let shutdown_ran = Arc::new(AtomicBool::new(false));
    let shutdown_ran_close = shutdown_ran.clone();

    let app = tauri::Builder::default()
        .manage(AppState {
            core: core.clone(),
            tabs: tab_manager,
        })
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Attach embedded Win32 libmpv video child HWND to the main Tauri window
            if let Some(main_win) = app.get_window("main") {
                if let Ok(wh) = main_win.window_handle() {
                    let parent_hwnd: isize = match wh.as_raw() {
                        RawWindowHandle::Win32(h) => h.hwnd.get(),
                        _ => 0,
                    };
                    if parent_hwnd != 0 {
                        let cb_app = app_handle.clone();
                        let cb_core = core_for_setup.clone();
                        let cb_tabs = tabs_for_setup.clone();
                        let _ = core_for_setup.player.attach_to_parent_window(
                            parent_hwnd,
                            Arc::new(move |ev: SurfaceInputEvent| match ev {
                                SurfaceInputEvent::SingleClick => {
                                    if let Ok(snap) = cb_core.player.toggle_pause() {
                                        if snap.paused {
                                            persist_snapshot_position(&cb_core, &snap);
                                        }
                                        cb_tabs.broadcast_active_media_highlight(&cb_app);
                                        let _ = cb_app.emit("sv://player-state", snap);
                                    }
                                }
                                SurfaceInputEvent::DoubleClick => {
                                    if let Some(w) = cb_app.get_window("main") {
                                        let next = !w.is_fullscreen().unwrap_or(false);
                                        let _ = w.set_fullscreen(next);
                                        let _ = cb_app.emit("sv://fullscreen-changed", next);
                                    }
                                }
                                SurfaceInputEvent::WheelDelta { notches } => {
                                    let curr = cb_core.player.snapshot().volume;
                                    if let Ok(snap) = cb_core.player.set_volume(curr + (notches as i64 * 5)) {
                                        let _ = cb_core.db.update_single_setting(
                                            "default_volume",
                                            serde_json::json!(snap.volume),
                                        );
                                        let _ = cb_app.emit("sv://player-state", snap);
                                    }
                                }
                                SurfaceInputEvent::MouseMove => {
                                    let _ = cb_app.emit("sv://player-mouse-wake", true);
                                }
                                SurfaceInputEvent::KeyPress { vk_code } => {
                                    let _ = cb_app.emit("sv://player-surface-key", vk_code);
                                }
                                SurfaceInputEvent::MediaCommand { action } => {
                                    match action.as_str() {
                                        "play_pause" => {
                                            if cb_core.player.snapshot().active {
                                                if let Ok(snap) = cb_core.player.toggle_pause() {
                                                    if snap.paused {
                                                        persist_snapshot_position(&cb_core, &snap);
                                                    }
                                                    cb_tabs.broadcast_active_media_highlight(&cb_app);
                                                    let _ = cb_app.emit("sv://player-state", snap);
                                                }
                                            }
                                        }
                                        "play" => {
                                            if cb_core.player.snapshot().active {
                                                if let Ok(snap) = cb_core.player.set_pause(false) {
                                                    cb_tabs.broadcast_active_media_highlight(&cb_app);
                                                    let _ = cb_app.emit("sv://player-state", snap);
                                                }
                                            }
                                        }
                                        "pause" => {
                                            if cb_core.player.snapshot().active {
                                                if let Ok(snap) = cb_core.player.set_pause(true) {
                                                    persist_snapshot_position(&cb_core, &snap);
                                                    cb_tabs.broadcast_active_media_highlight(&cb_app);
                                                    let _ = cb_app.emit("sv://player-state", snap);
                                                }
                                            }
                                        }
                                        "fast_forward" => {
                                            if cb_core.player.snapshot().active {
                                                if let Ok(snap) =
                                                    cb_core.player.seek_relative_seconds(10.0)
                                                {
                                                    let _ = cb_app.emit("sv://player-state", snap);
                                                }
                                            }
                                        }
                                        "rewind" => {
                                            if cb_core.player.snapshot().active {
                                                if let Ok(snap) =
                                                    cb_core.player.seek_relative_seconds(-10.0)
                                                {
                                                    let _ = cb_app.emit("sv://player-state", snap);
                                                }
                                            }
                                        }
                                        "exit_pip" => {
                                            if let Ok(snap) = cb_core.player.set_pip_mode(false) {
                                                let _ = cb_app.emit("sv://player-state", snap);
                                            }
                                            let _ = cb_app.emit("sv://media-key-command", action);
                                        }
                                        _ => {
                                            let _ = cb_app.emit("sv://media-key-command", action);
                                        }
                                    }
                                }
                                SurfaceInputEvent::StateChanged => {
                                    let snap = cb_core.player.snapshot();
                                    let _ = cb_app.emit("sv://player-state", snap);
                                }
                                SurfaceInputEvent::PlaybackEnded {
                                    reason,
                                    error_message,
                                } => {
                                    let snap = cb_core.player.snapshot();
                                    persist_snapshot_position(&cb_core, &snap);
                                    cb_tabs.broadcast_active_media_highlight(&cb_app);
                                    let _ = cb_app.emit(
                                        "sv://player-ended",
                                        serde_json::json!({
                                            "reason": reason,
                                            "error": error_message
                                        }),
                                    );
                                }
                            }),
                        );
                        if let Ok(init_settings) = core_for_setup.db.get_settings() {
                            let _ = core_for_setup.player.set_volume(init_settings.default_volume);
                        }
                    }
                }
            }

            // Periodic background telemetry & playback-position persistence (every ~800ms UI / ~5s SQLite)
            let tick_app = app_handle.clone();
            let tick_core = core_for_setup.clone();
            let tick_tabs = tabs_for_setup.clone();
            tauri::async_runtime::spawn(async move {
                let mut ticks: u64 = 0;
                let mut net_sampler = streamvault_core::sys_utils::NetSpeedSampler::new();
                loop {
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    ticks = ticks.wrapping_add(1);

                    let net_snap = net_sampler.sample();
                    let _ = tick_app.emit("sv://net-speed", net_snap);

                    let player_snap = tick_core.player.snapshot();
                    let cache_status = tick_core.stream_cache.get_active_status();

                    if player_snap.active && !player_snap.paused && ticks % 6 == 0 {
                        persist_snapshot_position(&tick_core, &player_snap);
                        tick_tabs.broadcast_active_media_highlight(&tick_app);
                    }

                    if player_snap.active || cache_status.is_some() {
                        let _ = tick_app.emit(
                            "sv://stream-telemetry",
                            serde_json::json!({
                                "player": player_snap,
                                "cache": cache_status
                            }),
                        );
                    }

                    if let Ok(downloads) = tick_core.downloader.list_downloads_with_live_progress() {
                        let any_active = downloads
                            .iter()
                            .any(|d| d.status == "downloading" || d.status == "queued");
                        if any_active || ticks % 5 == 0 {
                            let _ = tick_app.emit("sv://downloads-updated", downloads);
                        }
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_bootstrap_state,
            commands::app_check_github_update,
            commands::get_dependency_status,
            commands::get_recent_logs,
            commands::clipboard_write_text,
            commands::tab_ensure_initialized,
            commands::tab_list,
            commands::tab_create,
            commands::tab_close,
            commands::tab_switch,
            commands::tab_navigate,
            commands::tab_go_back,
            commands::tab_go_forward,
            commands::tab_reload,
            commands::tab_go_home,
            commands::tab_get_page_media,
            commands::tab_request_page_media,
            commands::tab_find_in_page,
            commands::tab_scroll_to_active_media,
            commands::browser_set_viewport,
            commands::suggest_address_bar,
            commands::tab_context_bridge,
            commands::probe_media_link,
            commands::prepare_in_app_playback,
            commands::start_in_app_playback,
            commands::open_media_external,
            commands::player_get_state,
            commands::player_toggle_pause,
            commands::player_set_pause,
            commands::player_stop,
            commands::player_seek_relative,
            commands::player_seek_absolute,
            commands::player_set_volume,
            commands::player_toggle_mute,
            commands::player_set_speed,
            commands::player_set_audio_track,
            commands::player_set_subtitle_track,
            commands::player_browse_and_load_subtitle,
            commands::player_set_aspect_ratio,
            commands::player_set_fullscreen,
            commands::player_set_pip,
            commands::player_set_viewport,
            commands::player_set_time_tooltip,
            commands::player_open_local_file,
            commands::player_browse_and_open_local_media,
            commands::player_load_external_subtitle_path,
            commands::player_load_external_subtitle_url,
            commands::player_set_sub_delay,
            commands::player_set_audio_delay,
            commands::player_set_sub_scale,
            commands::player_set_dialogue_boost,
            commands::player_take_screenshot,
            commands::player_reveal_path,
            commands::player_sync_smtc,
            commands::stream_cache_status,
            commands::stream_cache_summary,
            commands::stream_cache_clear,
            commands::download_pick_batch_folder,
            commands::download_start_batch,
            commands::download_prepare_save_as,
            commands::download_prompt_save_as_and_start,
            commands::download_start_default_dir,
            commands::download_list,
            commands::download_pause,
            commands::download_resume,
            commands::download_retry,
            commands::download_cancel,
            commands::download_remove,
            commands::download_clear_completed,
            commands::download_open_file,
            commands::download_open_folder,
            commands::recent_media_list,
            commands::recent_media_remove,
            commands::recent_media_clear,
            commands::playback_position_get,
            commands::playback_position_reset,
            commands::playback_position_clear_all,
            commands::bookmarks_tree,
            commands::bookmarks_check_url,
            commands::bookmarks_add,
            commands::bookmarks_update,
            commands::bookmarks_move,
            commands::bookmarks_delete,
            commands::bookmark_folder_create,
            commands::bookmark_folder_rename,
            commands::bookmark_folder_delete,
            commands::bookmarks_search,
            commands::history_list,
            commands::history_delete,
            commands::history_clear_period,
            commands::external_players_detect,
            commands::external_player_browse_exe,
            commands::external_player_add_custom,
            commands::external_player_remove_custom,
            commands::external_player_test,
            commands::settings_get,
            commands::settings_save,
            commands::settings_update_key,
            commands::settings_browse_download_dir,
            commands::privacy_clear_all_data,
        ])
        .build(tauri::generate_context!())
        .expect("error while building StreamVault Tauri application");

    app.run(move |app_handle, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { .. },
            ..
        } if label == "main" => {
            if !shutdown_ran_close.swap(true, Ordering::SeqCst) {
                core_for_exit.graceful_shutdown();
            }
        }
        RunEvent::WindowEvent {
            event: WindowEvent::DragDrop(dnd_event),
            ..
        } => match dnd_event {
            tauri::DragDropEvent::Enter { paths, .. } => {
                let has_valid = paths.iter().any(|p| {
                    streamvault_core::media_detect::is_local_media_path(p)
                        || streamvault_core::media_detect::is_local_subtitle_path(p)
                });
                if has_valid {
                    let _ = app_handle.emit("sv://file-drop-hover", true);
                }
            }
            tauri::DragDropEvent::Leave => {
                let _ = app_handle.emit("sv://file-drop-hover", false);
            }
            tauri::DragDropEvent::Drop { paths, .. } => {
                let _ = app_handle.emit("sv://file-drop-hover", false);
                let mut media_paths = Vec::new();
                let mut subtitle_paths = Vec::new();
                for p in paths {
                    if streamvault_core::media_detect::is_local_media_path(&p) {
                        media_paths.push(p.to_string_lossy().to_string());
                    } else if streamvault_core::media_detect::is_local_subtitle_path(&p) {
                        subtitle_paths.push(p.to_string_lossy().to_string());
                    }
                }
                media_paths.sort();
                subtitle_paths.sort();
                if !media_paths.is_empty() || !subtitle_paths.is_empty() {
                    let _ = app_handle.emit(
                        "sv://files-dropped",
                        serde_json::json!({
                            "media_paths": media_paths,
                            "subtitle_paths": subtitle_paths
                        }),
                    );
                }
            }
            _ => {}
        },
        RunEvent::Exit => {
            if !shutdown_ran.swap(true, Ordering::SeqCst) {
                core_for_exit.graceful_shutdown();
            }
        }
        _ => {}
    });
}
