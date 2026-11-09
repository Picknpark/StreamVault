use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use streamvault_core::downloader::resolve_non_colliding_destination;
use streamvault_core::external_player::build_external_player_command;
use streamvault_core::media_detect::{
    detect_media_from_url_only, extract_server_filename, probe_media_url, resolve_navigation_input,
};
use streamvault_core::StreamVaultCore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Spawns a realistic local HTTP media server (simulating `http://172.16.50.12/DHAKA-FLIX-12/...`)
/// supporting HTTP Range (`206 Partial Content`), `HEAD`, `Content-Disposition`, non-Range mode,
/// and transient connection drops.
async fn spawn_mock_media_server(
    payload: Arc<Vec<u8>>,
    support_range: bool,
    drop_first_midstream: bool,
) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let range_requests = Arc::new(AtomicUsize::new(0));
    let range_counter = range_requests.clone();
    let dropped_once = Arc::new(AtomicBool::new(false));

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let data = payload.clone();
            let rc = range_counter.clone();
            let dropped = dropped_once.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let Ok(n) = sock.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let method = first_line.split_whitespace().next().unwrap_or("GET");
                let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                let is_header_mime_endpoint = path.starts_with("/mime-stream");

                let mut range_hdr: Option<(usize, usize)> = None;
                for line in req.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("range") && support_range {
                            if let Some(spec) = v.trim().strip_prefix("bytes=") {
                                if let Some((s, e)) = spec.split_once('-') {
                                    if let Ok(start) = s.trim().parse::<usize>() {
                                        let end = e
                                            .trim()
                                            .parse::<usize>()
                                            .unwrap_or_else(|_| data.len().saturating_sub(1))
                                            .min(data.len().saturating_sub(1));
                                        if start < data.len() && end >= start {
                                            range_hdr = Some((start, end));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let extra_cd = if is_header_mime_endpoint {
                    "Content-Disposition: attachment; filename*=UTF-8''Cinema%20 Paradiso%20%E2%98%85%20Remastered.mkv\r\n"
                } else {
                    ""
                };

                if method == "HEAD" {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: {}\r\nContent-Length: {}\r\n{extra_cd}Connection: close\r\n\r\n",
                        if support_range { "bytes" } else { "none" },
                        data.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    return;
                }

                if let Some((start, end)) = range_hdr {
                    rc.fetch_add(1, Ordering::SeqCst);
                    let slice = &data[start..=end];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\n{extra_cd}Connection: close\r\n\r\n",
                        data.len(),
                        slice.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;

                    if drop_first_midstream
                        && slice.len() > 200_000
                        && !dropped.swap(true, Ordering::SeqCst)
                    {
                        let _ = sock.write_all(&slice[..131_072]).await;
                        // Simulate sudden LAN connection drop!
                        return;
                    }

                    for chunk in slice.chunks(65_536) {
                        if sock.write_all(chunk).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                } else {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: {}\r\nContent-Length: {}\r\n{extra_cd}Connection: close\r\n\r\n",
                        if support_range { "bytes" } else { "none" },
                        data.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    for chunk in data.chunks(65_536) {
                        if sock.write_all(chunk).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                }
            });
        }
    });

    (port, range_requests)
}

#[tokio::test]
async fn test_dhaka_flix_mkv_detection_streaming_seek_and_cache_isolation() {
    let temp_root = std::env::temp_dir().join(format!("sv_test_{}", uuid::Uuid::new_v4().simple()));
    let core = StreamVaultCore::initialize(Some(temp_root.clone())).expect("core init");
    assert!(core.paths.webview2_shell_dir.is_dir());
    assert!(core.paths.webview2_tabs_dir.is_dir());
    assert_ne!(core.paths.webview2_shell_dir, core.paths.webview2_tabs_dir);

    // Build deterministic 2.5 MiB MKV-like payload with recognizable byte pattern at every offset
    let total_size = 2_621_440usize;
    let mut payload = vec![0u8; total_size];
    payload[0..4].copy_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]); // Matroska EBML magic header
    for (i, b) in payload.iter_mut().enumerate().skip(4) {
        *b = ((i * 31 + (i >> 8)) & 0xFF) as u8;
    }

    let (port, range_hits) =
        spawn_mock_media_server(Arc::new(payload.clone()), true, true).await;

    // Exact Dhaka-Flix style encoded URL with Unicode `★`, `—`, parentheses, spaces
    let dhaka_url = format!(
        "http://127.0.0.1:{port}/DHAKA-FLIX-12/TV-WEB-Series/TV%20Series%20%E2%98%85%20%200%20%20%E2%80%94%20%209/2%20Broke%20Girls%20%28TV%20Series%202011%E2%80%932017%29%20720p/Season%201/2.Broke.Girls.S01E01.720p.AMZN.WEB-DL.x264.ESub-MsVerse.mkv"
    );

    // 1. Verify Media Detection & Exact Original Filename Extraction
    let url_det = detect_media_from_url_only(&dhaka_url);
    assert!(url_det.is_media);
    assert_eq!(
        url_det.suggested_filename,
        "2.Broke.Girls.S01E01.720p.AMZN.WEB-DL.x264.ESub-MsVerse.mkv"
    );

    let probed = probe_media_url(&core.http_client, &dhaka_url)
        .await
        .expect("probe ok");
    assert!(probed.is_media);
    assert!(probed.supports_range);
    assert_eq!(probed.content_length, Some(total_size as u64));

    // Also verify MIME + Content-Disposition detection on extensionless URL!
    let mime_url = format!("http://127.0.0.1:{port}/mime-stream?id=9912");
    let mime_probed = probe_media_url(&core.http_client, &mime_url)
        .await
        .expect("mime probe ok");
    assert!(mime_probed.is_media);
    assert_eq!(mime_probed.detection_source, "mime_header");
    assert_eq!(
        mime_probed.suggested_filename,
        "Cinema  Paradiso ★ Remastered.mkv"
    );

    // 2. Start Progressive Disk-Cached Streaming Session (even with simulated mid-stream LAN drop!)
    let status = core
        .stream_cache
        .start_stream_session(&dhaka_url)
        .await
        .expect("stream start");
    assert!(status.supports_range);
    assert_eq!(status.total_bytes, Some(total_size as u64));

    // Verify `.svcache` exists inside `StreamVault/data/cache/media/`
    let cache_info = core.stream_cache.inspect_cache().unwrap();
    assert!(cache_info.total_files >= 1);
    assert!(cache_info.active_session_id.is_some());

    // Verify active session is protected during manual `clear_inactive_cache`!
    let manual_clean = core.stream_cache.clear_inactive_cache().unwrap();
    assert!(manual_clean.skipped_active_files >= 1);

    // 3. Perform mid-file HTTP Range seek (`bytes=1800000-1865535`) through local proxy BEFORE full download!
    let seek_resp = core
        .http_client
        .get(&status.local_stream_url)
        .header("Range", "bytes=1800000-1865535")
        .send()
        .await
        .expect("range seek response");
    assert_eq!(seek_resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    let seek_bytes = seek_resp.bytes().await.unwrap();
    assert_eq!(seek_bytes.len(), 65_536);
    assert_eq!(&seek_bytes[..], &payload[1_800_000..=1_865_535]);
    assert!(range_hits.load(Ordering::SeqCst) >= 1);

    // 4. Test Permanent Download Manager (pause + resume + duplicate filename safety + cache independence)
    let dl1 = core
        .downloader
        .start_download(&dhaka_url, None, true)
        .await
        .expect("start permanent download");
    assert_eq!(
        dl1.filename,
        "2.Broke.Girls.S01E01.720p.AMZN.WEB-DL.x264.ESub-MsVerse.mkv"
    );

    // Pause quickly then resume to exercise HTTP Range resumable permanent transfer
    tokio::time::sleep(Duration::from_millis(25)).await;
    let _ = core.downloader.pause_download(&dl1.id);
    tokio::time::sleep(Duration::from_millis(60)).await;
    let _ = core.downloader.resume_download(&dl1.id).unwrap();

    // Wait for permanent download completion
    let mut completed_rec = None;
    for _ in 0..80 {
        let rec = core.downloader.get_download(&dl1.id).unwrap().unwrap();
        if rec.status == "completed" {
            completed_rec = Some(rec);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let completed_rec = completed_rec.expect("permanent download must complete");
    let perm_path = PathBuf::from(&completed_rec.destination_path);
    assert!(perm_path.is_file());
    let downloaded_data = fs::read(&perm_path).unwrap();
    assert_eq!(downloaded_data.len(), total_size);
    assert_eq!(downloaded_data, payload);

    // Duplicate filename safety check: downloading again must NOT overwrite `perm_path`!
    let non_colliding = resolve_non_colliding_destination(&perm_path);
    assert_ne!(non_colliding, perm_path);
    assert!(non_colliding
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("(1).mkv"));

    // 5. Shutdown cleanup: temporary `.svcache` MUST be deleted, permanent `.mkv` MUST remain intact!
    core.graceful_shutdown();
    let cache_after = core.stream_cache.inspect_cache().unwrap();
    assert_eq!(
        cache_after.total_files, 0,
        "Temporary streaming cache must be empty after application exit"
    );
    assert!(
        perm_path.is_file(),
        "Permanent downloaded MKV must remain intact after cache cleanup"
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_bookmarks_history_recent_media_resume_and_portability_relocation() {
    let dir_a = std::env::temp_dir().join(format!("sv_port_a_{}", uuid::Uuid::new_v4().simple()));
    let dir_b = std::env::temp_dir().join(format!("sv_port_b_{}", uuid::Uuid::new_v4().simple()));

    let mkv_url = "http://172.16.50.12/DHAKA-FLIX-12/Series/Episode%2001%20%281080p%29.mkv";

    {
        let core = StreamVaultCore::initialize(Some(dir_a.clone())).unwrap();

        // Address bar resolution checks (Facebook, YouTube, LAN IP, search)
        assert_eq!(
            resolve_navigation_input("facebook.com", "https://www.google.com/search?q={query}").unwrap(),
            "https://facebook.com/"
        );
        assert_eq!(
            resolve_navigation_input("172.16.50.12", "https://www.google.com/search?q={query}").unwrap(),
            "http://172.16.50.12/"
        );
        assert_eq!(
            resolve_navigation_input("http://192.168.1.10/movies/", "https://www.google.com/search?q={query}").unwrap(),
            "http://192.168.1.10/movies/"
        );

        // Bookmarks & Folders (completely separate from Recent Media!)
        let folder = core.db.create_bookmark_folder("BDIX Servers", None).unwrap();
        let bm = core
            .db
            .add_bookmark("Dhaka Flix", "http://172.16.50.12/", Some(folder.id), true)
            .unwrap();
        assert_eq!(bm.title, "Dhaka Flix");

        // Browsing History
        core.db
            .record_history_visit("Facebook", "https://facebook.com")
            .unwrap();
        core.db
            .record_history_visit("Dhaka Flix Index", "http://172.16.50.12/")
            .unwrap();
        assert_eq!(core.db.list_history(None, 50).unwrap().len(), 2);

        // Recent Media + 30:00 (1,800,000 ms) playback position out of 1 hour (3,600,000 ms)
        core.db
            .record_recent_media_open(mkv_url, Some("Episode 01 (1080p)"), Some("video/x-matroska"), "in_app")
            .unwrap();
        let pos = core
            .db
            .save_playback_position(mkv_url, 1_800_000, 3_600_000, Some(1), Some(2), Some(1.25), Some("16:9"))
            .unwrap();
        assert!(pos.should_offer_resume);
        assert_eq!(pos.formatted_position, "30:00");

        // Ensure playing media did NOT pollute Bookmarks!
        let tree = core.db.list_bookmark_tree().unwrap();
        assert_eq!(tree.bookmarks.len(), 1);
        assert_eq!(tree.bookmarks[0].url, "http://172.16.50.12/");

        // Browser session persistence
        core.db
            .save_browser_session(&[
                streamvault_core::db::session::SavedSessionTab {
                    tab_order: 0,
                    title: "Dhaka Flix".to_string(),
                    url: "http://172.16.50.12/".to_string(),
                    is_active: true,
                },
                streamvault_core::db::session::SavedSessionTab {
                    tab_order: 1,
                    title: "YouTube".to_string(),
                    url: "https://youtube.com/".to_string(),
                    is_active: false,
                },
            ])
            .unwrap();

        // Update setting
        core.db
            .update_single_setting("default_playback_speed", serde_json::json!(1.5))
            .unwrap();

        core.graceful_shutdown();
    }

    // PORTABILITY RELOCATION TEST: Move the entire portable folder `dir_a` -> `dir_b` (simulating copying to another PC!)
    fs::rename(&dir_a, &dir_b).expect("relocate portable folder");

    {
        let relocated = StreamVaultCore::initialize(Some(dir_b.clone())).unwrap();
        assert_eq!(relocated.db.list_bookmark_tree().unwrap().bookmarks.len(), 1);
        assert_eq!(relocated.db.list_recent_media(50).unwrap().len(), 1);
        assert_eq!(relocated.db.list_history(None, 50).unwrap().len(), 2);
        assert_eq!(relocated.db.load_browser_session().unwrap().len(), 2);
        assert_eq!(relocated.db.get_settings().unwrap().default_playback_speed, 1.5);

        let resume_check = relocated
            .db
            .get_playback_position(mkv_url)
            .unwrap()
            .expect("saved playback position survives relocation");
        assert!(resume_check.should_offer_resume);
        assert_eq!(resume_check.position_ms, 1_800_000);
        assert_eq!(resume_check.formatted_position, "30:00");

        // Verify completion threshold resets unfinished resume offer
        let finished = relocated
            .db
            .save_playback_position(mkv_url, 3_590_000, 3_600_000, None, None, None, None)
            .unwrap();
        assert!(finished.completed);
        assert!(!finished.should_offer_resume);

        relocated.graceful_shutdown();
    }

    let _ = fs::remove_dir_all(&dir_b);
}

#[test]
fn test_corrupted_sqlite_recovery_and_external_player_argument_preservation() {
    let temp_root = std::env::temp_dir().join(format!("sv_corrupt_{}", uuid::Uuid::new_v4().simple()));

    // 1. Initialize clean DB, seed legacy multi-backups, update data, and generate backups multiple times;
    //    verify `data/backups` strictly keeps ONLY a single overridden `streamvault.backup.db`!
    {
        let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();
        let backups_dir = temp_root.join("data").join("backups");
        fs::write(backups_dir.join("streamvault.backup.20260101_000001_000.db"), b"old1").unwrap();
        fs::write(backups_dir.join("streamvault.backup.20260102_000002_000.db"), b"old2").unwrap();

        core.db.create_rolling_backup().unwrap();
        core.db
            .add_bookmark("Saved Before Corruption", "https://example.org", None, true)
            .unwrap();
        let out_path = core.db.create_rolling_backup().unwrap();
        assert_eq!(
            out_path.file_name().unwrap().to_string_lossy(),
            "streamvault.backup.db"
        );

        let backup_files: Vec<String> = fs::read_dir(&backups_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(backup_files, vec!["streamvault.backup.db".to_string()]);
        core.graceful_shutdown();
    }

    // 2. Corrupt `streamvault.db` with garbage bytes!
    let db_file = temp_root.join("data").join("database").join("streamvault.db");
    fs::write(&db_file, b"CORRUPTED_GARBAGE_NOT_SQLITE_AT_ALL_1234567890").unwrap();

    // 3. Re-open StreamVaultCore: must NOT crash, must restore from rolling backup!
    {
        let recovered_core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();
        assert!(recovered_core.db.was_recovered_from_corruption());
        let bms = recovered_core.db.list_bookmark_tree().unwrap().bookmarks;
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].title, "Saved Before Corruption");
        recovered_core.graceful_shutdown();
    }

    // 4. Verify External Player argument handling preserves exact URL without decoding `%20` or mangling `()` / `★`
    let fake_player = temp_root.join("PotPlayerMini64.exe");
    fs::write(&fake_player, b"MZ_mock_exe").unwrap();
    let tricky_url = "http://172.16.50.12/DHAKA-FLIX-12/TV%20Series%20%E2%98%85/2%20Broke%20Girls%20(2011)/S01E01.mkv";
    let cmd = build_external_player_command(&fake_player.to_string_lossy(), tricky_url).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert_eq!(args, vec![tricky_url.to_string()]);

    // Verify filename extraction on Unicode & encoded paths
    let extracted = extract_server_filename(
        "http://172.16.50.12/Movies/Spirited%20Away%20%282001%29%20%E2%98%85%201080p.mkv",
        None,
        None,
    );
    assert_eq!(extracted, "Spirited Away (2001) ★ 1080p.mkv");

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_non_range_server_graceful_fallback_and_startup_crash_recovery() {
    let temp_root = std::env::temp_dir().join(format!("sv_norange_{}", uuid::Uuid::new_v4().simple()));
    let media_cache_dir = temp_root.join("data").join("cache").join("media");
    fs::create_dir_all(&media_cache_dir).unwrap();

    // Simulate an orphaned temporary `.svcache` left behind by a forced termination / crash
    let orphan_file = media_cache_dir.join("session_crashed123.svcache");
    fs::write(&orphan_file, vec![0xABu8; 32_768]).unwrap();
    assert!(orphan_file.is_file());

    // Initializing StreamVaultCore must automatically clean orphaned temporary cache files!
    let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();
    assert!(
        !orphan_file.exists(),
        "Startup crash recovery must remove orphaned .svcache files"
    );

    // Spawn a legacy HTTP server that does NOT support HTTP Range requests (`support_range = false`)
    let payload: Vec<u8> = (0..524_288).map(|i| (i % 251) as u8).collect();
    let (port, _) = spawn_mock_media_server(Arc::new(payload.clone()), false, false).await;
    let url = format!("http://127.0.0.1:{port}/legacy/OldServerMovie.mp4");

    let status = core.stream_cache.start_stream_session(&url).await.unwrap();
    assert!(!status.supports_range);
    assert!(
        status.range_notice.is_some(),
        "Non-range server must populate user-facing range limitation explanation"
    );

    // Wait briefly so progressive buffer caches bytes and verify local backward seek works!
    tokio::time::sleep(Duration::from_millis(180)).await;
    let resp = core
        .http_client
        .get(&status.local_stream_url)
        .header("Range", "bytes=0-4095")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT);
    let head_slice = resp.bytes().await.unwrap();
    assert_eq!(&head_slice[..4096], &payload[..4096]);

    core.graceful_shutdown();
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_blank_homepage_clipboard_viewmode_and_net_sampler() {
    let temp_root =
        std::env::temp_dir().join(format!("streamvault_test_ui6_{}", uuid::Uuid::new_v4()));
    let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();

    // 1. Blank homepage URL & about:blank resolution
    let tpl = "https://duckduckgo.com/?q={query}";
    assert_eq!(
        streamvault_core::media_detect::resolve_homepage_or_blank("", tpl),
        "about:blank"
    );
    assert_eq!(
        streamvault_core::media_detect::resolve_homepage_or_blank("   ", tpl),
        "about:blank"
    );
    assert_eq!(
        streamvault_core::media_detect::resolve_homepage_or_blank("about:blank", tpl),
        "about:blank"
    );
    assert_eq!(
        streamvault_core::media_detect::resolve_navigation_input("about:blank", tpl).unwrap(),
        "about:blank"
    );

    // 2. Exact Dhaka-Flix percent-encoded media URL preservation for clipboard & open-in-new-tab
    let dhaka_exact = "http://172.16.50.12/DHAKA-FLIX-12/TV-WEB-Series/TV%20Series%20%E2%98%85%20%200%20%20%E2%80%94%20%209/2%20Broke%20Girls%20%28TV%20Series%202011%E2%80%932017%29%20720p/Season%201/2.Broke.Girls.S01E01.720p.AMZN.WEB-DL.x264.ESub-MsVerse.mkv";
    let preserved = streamvault_core::media_detect::preserve_or_sanitize_http_url(dhaka_exact).unwrap();
    assert_eq!(preserved, dhaka_exact);
    let det = streamvault_core::media_detect::detect_media_from_url_only(&preserved);
    assert!(det.is_media);
    assert_eq!(
        det.suggested_filename,
        "2.Broke.Girls.S01E01.720p.AMZN.WEB-DL.x264.ESub-MsVerse.mkv"
    );
    streamvault_core::sys_utils::write_system_clipboard_text(&preserved).unwrap();

    // 3. Recent Media Grid/List view mode persistence in SQLite
    let default_settings = core.db.get_settings().unwrap();
    assert_eq!(default_settings.recent_media_view_mode, "grid");
    let updated = core
        .db
        .update_single_setting("recent_media_view_mode", serde_json::json!("list"))
        .unwrap();
    assert_eq!(updated.recent_media_view_mode, "list");
    assert_eq!(core.db.get_settings().unwrap().recent_media_view_mode, "list");

    // 4. NetSpeedSampler sampling
    let mut sampler = streamvault_core::sys_utils::NetSpeedSampler::new();
    std::thread::sleep(Duration::from_millis(80));
    let snap = sampler.sample();
    let _ = (snap.down_bps, snap.up_bps);

    core.graceful_shutdown();
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_subtitle_matching_local_sidecars_single_backup_and_semver() {
    let temp_root =
        std::env::temp_dir().join(format!("sv_tier123_unit_{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&temp_root).unwrap();

    // 1. Server subtitle stem matching (exact + language suffix, percent-encoded)
    let ep_url = "http://172.16.50.12/Series/Show%20S01E02%20%281080p%29.mkv";
    let candidates = vec![
        "http://172.16.50.12/Series/Show%20S01E01%20%281080p%29.srt".to_string(),
        "http://172.16.50.12/Series/Show%20S01E02%20%281080p%29.en.ass".to_string(),
        "http://172.16.50.12/Series/Show%20S01E02%20%281080p%29.srt".to_string(),
        "http://172.16.50.12/Series/Show%20S01E03%20%281080p%29.vtt".to_string(),
    ];
    let matched = streamvault_core::media_detect::match_subtitle_urls_for_media(ep_url, &candidates);
    assert_eq!(matched.len(), 2);
    assert_eq!(
        matched[0],
        "http://172.16.50.12/Series/Show%20S01E02%20%281080p%29.srt"
    );
    assert_eq!(
        matched[1],
        "http://172.16.50.12/Series/Show%20S01E02%20%281080p%29.en.ass"
    );

    // 2. Local sidecar subtitle discovery
    let local_dir = temp_root.join("local_movies");
    fs::create_dir_all(&local_dir).unwrap();
    let vid = local_dir.join("Dune.Part.Two.2024.mkv");
    let sub1 = local_dir.join("Dune.Part.Two.2024.srt");
    let sub2 = local_dir.join("Dune.Part.Two.2024.en.ass");
    let unrelated = local_dir.join("Other.Movie.srt");
    fs::write(&vid, b"mkv").unwrap();
    fs::write(&sub1, b"1\n00:00:01,000 --> 00:00:02,000\nHi\n").unwrap();
    fs::write(&sub2, b"[Script Info]\n").unwrap();
    fs::write(&unrelated, b"nope").unwrap();

    assert!(streamvault_core::media_detect::is_local_media_path(&vid));
    assert!(streamvault_core::media_detect::is_local_subtitle_path(&sub1));
    let sidecars = streamvault_core::mpv_engine::discover_local_sidecar_subtitles(&vid);
    assert_eq!(sidecars.len(), 2);

    // 3. SemVer GitHub update comparison
    assert!(streamvault_core::is_newer_semver("1.1.0", "1.2.0"));
    assert!(streamvault_core::is_newer_semver("1.1.0", "v2.0.0"));
    assert!(!streamvault_core::is_newer_semver("1.1.0", "1.1.0"));
    assert!(!streamvault_core::is_newer_semver("1.1.0", "1.0.9"));

    // 4. Tier 1–3 settings persistence + single rolling `streamvault.backup.db` invariant
    for _ in 0..3 {
        let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();
        core.db
            .update_single_setting("auto_play_next_episode", serde_json::json!(false))
            .unwrap();
        core.db
            .update_single_setting("download_speed_limit_kbps", serde_json::json!(2048))
            .unwrap();
        core.db
            .update_single_setting("github_repo_slug", serde_json::json!("streamvault/StreamVault"))
            .unwrap();
        let s = core.db.get_settings().unwrap();
        assert!(!s.auto_play_next_episode);
        assert_eq!(s.download_speed_limit_kbps, 2048);
        assert_eq!(s.github_repo_slug, "streamvault/StreamVault");
        core.graceful_shutdown();
    }

    let backup_files: Vec<_> = fs::read_dir(temp_root.join("data").join("backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(
        backup_files.len(),
        1,
        "data/backups must contain strictly ONE rolling backup file (streamvault.backup.db)"
    );
    assert_eq!(
        backup_files[0].file_name().to_string_lossy(),
        "streamvault.backup.db"
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_batch_media_grabber_queue_and_concurrency_cap() {
    let payload: Arc<Vec<u8>> = Arc::new((0..65_536).map(|i| (i % 251) as u8).collect());
    let (port, _srv) = spawn_mock_media_server(payload.clone(), true, false).await;
    let addr = format!("127.0.0.1:{port}");

    let temp_root =
        std::env::temp_dir().join(format!("sv_batch_dl_{}", uuid::Uuid::new_v4().simple()));
    let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();
    core.db
        .update_single_setting("max_concurrent_downloads", serde_json::json!(2))
        .unwrap();

    let batch_dir = temp_root.join("Season_01");
    let urls = vec![
        format!("http://{addr}/Series/Episode.S01E01.mkv"),
        format!("http://{addr}/Series/Episode.S01E02.mkv"),
        format!("http://{addr}/Series/Episode.S01E03.mkv"),
        format!("http://{addr}/Series/Episode.S01E04.mkv"),
    ];

    let queued = core
        .downloader
        .enqueue_batch_downloads(urls, Some(batch_dir.clone()))
        .unwrap();
    assert_eq!(queued.len(), 4);

    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        let all = core.downloader.list_downloads_with_live_progress().unwrap();
        if all.len() == 4 && all.iter().all(|r| r.status == "completed") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Batch downloads did not complete before timeout: {:?}",
            all.iter().map(|r| (&r.filename, &r.status)).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    for ep in 1..=4 {
        let f = batch_dir.join(format!("Episode.S01E0{ep}.mkv"));
        assert!(f.is_file(), "Expected batch file {}", f.display());
        assert_eq!(fs::read(&f).unwrap().len(), payload.len());
    }

    core.graceful_shutdown();
    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_audit_bugfix_regressions() {
    // 1. Synced media extensions + query-parameter & local:// filename extraction
    for ext in [
        "m2ts", "mts", "mpg", "mpeg", "ogv", "3gp", "divx", "alac", "aiff", "ape",
    ] {
        let u = format!("http://172.16.50.10/Archive/TestClip.{ext}");
        let det = detect_media_from_url_only(&u);
        assert!(det.is_media, "Expected .{ext} to be detected as direct media");
    }
    let query_url = "https://cdn.example.org/download?id=9981&filename=Blade%20Runner%202049.mkv";
    let query_det = detect_media_from_url_only(query_url);
    assert!(query_det.is_media);
    assert_eq!(query_det.suggested_filename, "Blade Runner 2049.mkv");

    let local_uri = r"local://D:\Media\Movies\My Local Episode (2024).mp4";
    assert_eq!(
        extract_server_filename(local_uri, None, None),
        "My Local Episode (2024).mp4"
    );

    // 2. Custom external player removal resets default_external_player & local:// launch support
    let temp_root =
        std::env::temp_dir().join(format!("sv_reg_fix_{}", uuid::Uuid::new_v4().simple()));
    let core = StreamVaultCore::initialize(Some(temp_root.clone())).unwrap();

    let custom_exe = temp_root.join("CustomMpcHc64.exe");
    fs::write(&custom_exe, b"MZ_custom_player").unwrap();
    let custom_exe_str = custom_exe.to_string_lossy().to_string();

    let added = streamvault_core::external_player::add_custom_external_player(
        &core.db,
        "MPC-HC Custom",
        &custom_exe_str,
    )
    .unwrap();
    let custom_id = added.custom_id.expect("custom_id present");
    core.db
        .update_single_setting("default_external_player", serde_json::json!(&custom_exe_str))
        .unwrap();
    assert_eq!(
        core.db.get_settings().unwrap().default_external_player,
        custom_exe_str
    );

    streamvault_core::external_player::remove_custom_external_player(&core.db, custom_id)
        .unwrap();
    assert_eq!(
        core.db.get_settings().unwrap().default_external_player,
        "ask",
        "Removing custom default player must reset default_external_player to 'ask'"
    );

    let local_media_file = temp_root.join("Sample Local Movie.mkv");
    fs::write(&local_media_file, vec![0x11u8; 4096]).unwrap();
    let local_prefixed = format!("local://{}", local_media_file.to_string_lossy());
    let local_cmd = build_external_player_command(&custom_exe_str, &local_prefixed).unwrap();
    let local_args: Vec<String> = local_cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert_eq!(
        local_args,
        vec![local_media_file.to_string_lossy().to_string()]
    );

    // 3. Inverted HTTP Range header (`bytes=5000-1000`) is safely ignored per RFC 9110 (200 OK, no u64 underflow)
    let payload: Arc<Vec<u8>> = Arc::new((0..32_768).map(|i| (i % 251) as u8).collect());
    let (port, _srv) = spawn_mock_media_server(payload.clone(), true, false).await;
    let src_url = format!("http://127.0.0.1:{port}/Movies/RangeCheck.mkv");
    let session = core.stream_cache.start_stream_session(&src_url).await.unwrap();
    let client = reqwest::Client::new();
    let bad_range_resp = client
        .get(&session.local_stream_url)
        .header("Range", "bytes=5000-1000")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_range_resp.status().as_u16(), 200);
    assert_eq!(bad_range_resp.bytes().await.unwrap().len(), payload.len());

    // 4. Removing an active/throttled download cancels cleanly without re-creating zombie DB row
    core.db
        .update_single_setting("download_speed_limit_kbps", serde_json::json!(8))
        .unwrap();
    let big_payload: Arc<Vec<u8>> = Arc::new(vec![0x5Au8; 262_144]);
    let (slow_port, _slow_srv) = spawn_mock_media_server(big_payload, true, false).await;
    let slow_url = format!("http://127.0.0.1:{slow_port}/Movies/SlowTransfer.mkv");
    let dl_rec = core
        .downloader
        .start_download(&slow_url, None, true)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    core.downloader.remove_download_entry(&dl_rec.id).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let remaining = core.downloader.list_downloads_with_live_progress().unwrap();
    assert!(
        remaining.iter().all(|r| r.id != dl_rec.id),
        "Deleted active download must not reappear as a zombie row"
    );

    core.graceful_shutdown();
    let _ = fs::remove_dir_all(&temp_root);
}


