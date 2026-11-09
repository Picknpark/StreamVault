use crate::db::downloads::DownloadRecord;
use crate::db::Database;
use crate::logging::StructuredLogger;
use crate::media_detect::{
    extract_server_filename, preserve_or_sanitize_http_url, probe_media_url, sanitize_windows_filename,
};
use crate::portable::PortablePaths;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use parking_lot::RwLock;
use reqwest::header::{ACCEPT_RANGES, CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveAsSuggestion {
    pub original_url: String,
    pub suggested_filename: String,
    pub extension: Option<String>,
    pub default_directory: String,
    pub mime_type: Option<String>,
    pub content_length: Option<u64>,
    pub supports_range: bool,
}

struct ActiveDownloadControl {
    pause_requested: AtomicBool,
    cancel_requested: AtomicBool,
    removed: AtomicBool,
    shutdown_requested: AtomicBool,
    speed_bps: AtomicU64,
    live_bytes: AtomicU64,
}

#[derive(Clone)]
pub struct DownloadManager {
    paths: PortablePaths,
    db: Database,
    client: reqwest::Client,
    logger: StructuredLogger,
    active_controls: Arc<RwLock<HashMap<String, Arc<ActiveDownloadControl>>>>,
    running_transfers: Arc<AtomicUsize>,
}

impl DownloadManager {
    pub fn new(
        paths: PortablePaths,
        db: Database,
        client: reqwest::Client,
        logger: StructuredLogger,
    ) -> Self {
        Self {
            paths,
            db,
            client,
            logger,
            active_controls: Arc::new(RwLock::new(HashMap::new())),
            running_transfers: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Probes the server to extract the exact original filename, extension, file size,
    /// and default destination directory for the Windows Save As dialog.
    pub async fn prepare_save_as(&self, raw_url: &str) -> Result<SaveAsSuggestion> {
        let preserved = preserve_or_sanitize_http_url(raw_url)?;
        let probe = probe_media_url(&self.client, &preserved)
            .await
            .unwrap_or_else(|_| crate::media_detect::detect_media_from_url_only(&preserved));

        let settings = self.db.get_settings()?;
        let default_dir_path = self.paths.resolve_portable_path(&settings.default_download_dir);
        let _ = fs::create_dir_all(&default_dir_path);

        let filename = if probe.suggested_filename.trim().is_empty() {
            extract_server_filename(&preserved, None, probe.mime_type.as_deref())
        } else {
            probe.suggested_filename
        };

        let ext = filename
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .or(probe.extension);

        Ok(SaveAsSuggestion {
            original_url: preserved,
            suggested_filename: filename,
            extension: ext,
            default_directory: default_dir_path.to_string_lossy().to_string(),
            mime_type: probe.mime_type,
            content_length: probe.content_length,
            supports_range: probe.supports_range,
        })
    }

    /// Opens the native Windows `IFileSaveDialog` pre-populated with the exact server filename.
    /// Returns `Ok(None)` if the user cancels the dialog.
    pub async fn prompt_native_save_as(&self, raw_url: &str) -> Result<Option<(SaveAsSuggestion, PathBuf)>> {
        let suggestion = self.prepare_save_as(raw_url).await?;
        #[cfg(windows)]
        {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Save Media Permanently — StreamVault")
                .set_directory(&suggestion.default_directory)
                .set_file_name(&suggestion.suggested_filename);

            if let Some(ref ext) = suggestion.extension {
                let clean_ext = ext.trim_start_matches('.');
                if !clean_ext.is_empty() {
                    dialog = dialog.add_filter(
                        &format!("{} Media File (*.{})", clean_ext.to_ascii_uppercase(), clean_ext),
                        &[clean_ext],
                    );
                }
            }
            dialog = dialog.add_filter("All Files (*.*)", &["*"]);

            let picked = dialog.save_file().await;
            return Ok(picked.map(|handle| (suggestion, handle.path().to_path_buf())));
        }
        #[cfg(not(windows))]
        {
            let default_dest = PathBuf::from(&suggestion.default_directory)
                .join(&suggestion.suggested_filename);
            Ok(Some((suggestion, default_dest)))
        }
    }

    /// Starts a permanent download to `requested_destination`.
    /// Never silently overwrites an existing file: if `ensure_unique_name` is true (or if an existing
    /// file/download conflicts), generates a safe `Name (1).ext` path automatically.
    pub async fn start_download(
        &self,
        raw_url: &str,
        requested_destination: Option<PathBuf>,
        ensure_unique_name: bool,
    ) -> Result<DownloadRecord> {
        let suggestion = self.prepare_save_as(raw_url).await?;

        let raw_dest = match requested_destination {
            Some(p) => p,
            None => PathBuf::from(&suggestion.default_directory)
                .join(sanitize_windows_filename(&suggestion.suggested_filename)),
        };

        let parent_dir = raw_dest
            .parent()
            .context("Invalid download destination directory")?;
        fs::create_dir_all(parent_dir).with_context(|| {
            format!(
                "Cannot create or access download destination directory: {}",
                parent_dir.display()
            )
        })?;

        // Verify write access to destination directory
        let write_probe = parent_dir.join(format!(".sv_dl_probe_{}", Uuid::new_v4().simple()));
        fs::write(&write_probe, b"ok").with_context(|| {
            format!(
                "Download destination directory is not writable: {}",
                parent_dir.display()
            )
        })?;
        let _ = fs::remove_file(&write_probe);

        // Disk space check if server provided Content-Length
        if let Some(expected_len) = suggestion.content_length {
            if let Some(avail) = query_dir_free_space(parent_dir) {
                if avail < expected_len.saturating_add(4 * 1024 * 1024) {
                    anyhow::bail!(
                        "Insufficient disk space at '{}': required {} MB, available {} MB",
                        parent_dir.display(),
                        expected_len / (1024 * 1024),
                        avail / (1024 * 1024)
                    );
                }
            }
        }

        // Duplicate filename protection: never silently overwrite an existing file!
        let final_dest = if ensure_unique_name || raw_dest.exists() {
            resolve_non_colliding_destination(&raw_dest)
        } else {
            raw_dest
        };

        let final_filename = final_dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| suggestion.suggested_filename.clone());

        let temp_part = PathBuf::from(format!("{}.svpart", final_dest.display()));
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();

        let rec = DownloadRecord {
            id: id.clone(),
            filename: final_filename,
            original_url: suggestion.original_url,
            destination_path: self.paths.to_portable_string(&final_dest),
            temp_part_path: self.paths.to_portable_string(&temp_part),
            mime_type: suggestion.mime_type,
            downloaded_bytes: 0,
            total_bytes: suggestion.content_length,
            supports_range: suggestion.supports_range,
            etag: None,
            last_modified: None,
            status: "queued".to_string(),
            error_message: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
            speed_bps: 0,
        };

        self.db.upsert_download_record(&rec)?;
        self.spawn_transfer_worker(id.clone(), false)?;

        Ok(self.get_download(&id)?.unwrap_or(rec))
    }

    /// Enqueues multiple URLs into a single destination directory instantaneously for Batch Media Grabber.
    /// Individual workers honor `max_concurrent_downloads` and discover exact `Content-Length` / `ETag`
    /// headers when each slot starts downloading.
    pub fn enqueue_batch_downloads(
        &self,
        urls: Vec<String>,
        requested_dir: Option<PathBuf>,
    ) -> Result<Vec<DownloadRecord>> {
        let settings = self.db.get_settings()?;
        let base_dir = match requested_dir {
            Some(d) => d,
            None => self.paths.resolve_portable_path(&settings.default_download_dir),
        };
        fs::create_dir_all(&base_dir).with_context(|| {
            format!(
                "Cannot create or access batch download directory: {}",
                base_dir.display()
            )
        })?;

        let write_probe = base_dir.join(format!(".sv_batch_probe_{}", Uuid::new_v4().simple()));
        fs::write(&write_probe, b"ok").with_context(|| {
            format!(
                "Batch download directory is not writable: {}",
                base_dir.display()
            )
        })?;
        let _ = fs::remove_file(&write_probe);

        let mut created = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();

        for raw_url in urls {
            let Ok(preserved) = preserve_or_sanitize_http_url(&raw_url) else {
                continue;
            };
            if !seen_urls.insert(preserved.clone()) {
                continue;
            }

            let quick = crate::media_detect::detect_media_from_url_only(&preserved);
            let raw_name = if quick.suggested_filename.trim().is_empty() {
                extract_server_filename(&preserved, None, quick.mime_type.as_deref())
            } else {
                quick.suggested_filename
            };
            let clean_name = sanitize_windows_filename(&raw_name);
            let candidate_dest = base_dir.join(&clean_name);
            let final_dest = resolve_non_colliding_destination(&candidate_dest);
            let final_filename = final_dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(clean_name);
            let temp_part = PathBuf::from(format!("{}.svpart", final_dest.display()));

            // Reserve `.svpart` placeholder immediately so sibling episodes in the same batch never collide
            let _ = OpenOptions::new().create(true).write(true).open(&temp_part);

            let id = Uuid::new_v4().to_string();
            let now = Utc::now().timestamp_millis();
            let rec = DownloadRecord {
                id: id.clone(),
                filename: final_filename,
                original_url: preserved,
                destination_path: self.paths.to_portable_string(&final_dest),
                temp_part_path: self.paths.to_portable_string(&temp_part),
                mime_type: quick.mime_type,
                downloaded_bytes: 0,
                total_bytes: None,
                supports_range: true,
                etag: None,
                last_modified: None,
                status: "queued".to_string(),
                error_message: None,
                created_at: now,
                updated_at: now,
                completed_at: None,
                speed_bps: 0,
            };

            self.db.upsert_download_record(&rec)?;
            self.spawn_transfer_worker(id.clone(), false)?;
            created.push(rec);
        }

        Ok(created)
    }

    pub fn pause_download(&self, id: &str) -> Result<DownloadRecord> {
        if let Some(ctrl) = self.active_controls.read().get(id).cloned() {
            ctrl.pause_requested.store(true, Ordering::SeqCst);
        } else if let Some(mut rec) = self.db.get_download_record(id)? {
            if rec.status == "downloading" || rec.status == "queued" {
                rec.status = "paused".to_string();
                rec.updated_at = Utc::now().timestamp_millis();
                self.db.upsert_download_record(&rec)?;
            }
        }
        self.get_download(id)?
            .context("Download record not found")
    }

    pub fn resume_download(&self, id: &str) -> Result<DownloadRecord> {
        let rec = self
            .db
            .get_download_record(id)?
            .context("Download record not found")?;
        if rec.status == "completed" {
            return Ok(rec);
        }
        if self.active_controls.read().contains_key(id) {
            return Ok(rec);
        }
        self.spawn_transfer_worker(id.to_string(), true)?;
        self.get_download(id)?
            .context("Download record not found")
    }

    pub fn retry_download(&self, id: &str) -> Result<DownloadRecord> {
        let rec = self
            .db
            .get_download_record(id)?
            .context("Download record not found")?;
        if self.active_controls.read().contains_key(id) {
            return Ok(rec);
        }
        let part_path = self.paths.resolve_portable_path(&rec.temp_part_path);
        let resume_if_possible = rec.supports_range && part_path.is_file();
        self.spawn_transfer_worker(id.to_string(), resume_if_possible)?;
        self.get_download(id)?
            .context("Download record not found")
    }

    pub fn cancel_download(&self, id: &str) -> Result<DownloadRecord> {
        if let Some(ctrl) = self.active_controls.read().get(id).cloned() {
            ctrl.cancel_requested.store(true, Ordering::SeqCst);
        }
        if let Some(mut rec) = self.db.get_download_record(id)? {
            // Never delete an already completed permanent file!
            if rec.status != "completed" {
                let part_path = self.paths.resolve_portable_path(&rec.temp_part_path);
                let _ = fs::remove_file(&part_path);
                rec.status = "cancelled".to_string();
                rec.downloaded_bytes = 0;
                rec.updated_at = Utc::now().timestamp_millis();
                self.db.upsert_download_record(&rec)?;
            }
            return Ok(rec);
        }
        anyhow::bail!("Download record not found")
    }

    pub fn remove_download_entry(&self, id: &str) -> Result<()> {
        if let Some(ctrl) = self.active_controls.read().get(id).cloned() {
            ctrl.removed.store(true, Ordering::SeqCst);
            ctrl.cancel_requested.store(true, Ordering::SeqCst);
        }
        if let Some(rec) = self.db.get_download_record(id)? {
            if rec.status != "completed" {
                let part_path = self.paths.resolve_portable_path(&rec.temp_part_path);
                let _ = fs::remove_file(&part_path);
            }
        }
        self.db.delete_download_record(id)
    }

    pub fn shutdown_active_transfers(&self) {
        let ctrls: Vec<(String, Arc<ActiveDownloadControl>)> = self
            .active_controls
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, ctrl) in &ctrls {
            ctrl.shutdown_requested.store(true, Ordering::SeqCst);
            let live = ctrl.live_bytes.load(Ordering::Relaxed);
            if let Ok(Some(mut rec)) = self.db.get_download_record(id) {
                if rec.status == "downloading" || rec.status == "queued" {
                    if live > rec.downloaded_bytes {
                        rec.downloaded_bytes = live;
                    }
                    rec.status = "interrupted".to_string();
                    rec.error_message = Some(
                        "Application exited before download completed; click Resume to continue."
                            .to_string(),
                    );
                    rec.updated_at = Utc::now().timestamp_millis();
                    let _ = self.db.upsert_download_record(&rec);
                }
            }
        }
    }

    pub fn list_downloads_with_live_progress(&self) -> Result<Vec<DownloadRecord>> {
        let mut list = self.db.list_downloads()?;
        let controls = self.active_controls.read();
        for item in &mut list {
            // Resolve portable display path so relocated folders display accurate paths
            let resolved_dest = self.paths.resolve_portable_path(&item.destination_path);
            item.destination_path = resolved_dest.to_string_lossy().to_string();
            if let Some(ctrl) = controls.get(&item.id) {
                item.speed_bps = ctrl.speed_bps.load(Ordering::Relaxed);
                let live = ctrl.live_bytes.load(Ordering::Relaxed);
                if live > item.downloaded_bytes {
                    item.downloaded_bytes = live;
                }
            }
        }
        Ok(list)
    }

    pub fn get_download(&self, id: &str) -> Result<Option<DownloadRecord>> {
        let Some(mut rec) = self.db.get_download_record(id)? else {
            return Ok(None);
        };
        rec.destination_path = self
            .paths
            .resolve_portable_path(&rec.destination_path)
            .to_string_lossy()
            .to_string();
        if let Some(ctrl) = self.active_controls.read().get(id) {
            rec.speed_bps = ctrl.speed_bps.load(Ordering::Relaxed);
            let live = ctrl.live_bytes.load(Ordering::Relaxed);
            if live > rec.downloaded_bytes {
                rec.downloaded_bytes = live;
            }
        }
        Ok(Some(rec))
    }

    pub fn open_downloaded_file(&self, id: &str) -> Result<()> {
        let rec = self
            .db
            .get_download_record(id)?
            .context("Download record not found")?;
        let dest = self.paths.resolve_portable_path(&rec.destination_path);
        if !dest.is_file() {
            anyhow::bail!("Downloaded file not found at '{}'", dest.display());
        }
        open_file_in_os(&dest)
    }

    pub fn open_download_folder(&self, id: &str) -> Result<()> {
        let rec = self
            .db
            .get_download_record(id)?
            .context("Download record not found")?;
        let dest = self.paths.resolve_portable_path(&rec.destination_path);
        let temp_part = self.paths.resolve_portable_path(&rec.temp_part_path);
        reveal_in_file_manager(&dest, Some(&temp_part), &self.paths.downloads_dir)
    }

    fn spawn_transfer_worker(&self, id: String, try_resume: bool) -> Result<()> {
        let ctrl = Arc::new(ActiveDownloadControl {
            pause_requested: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            shutdown_requested: AtomicBool::new(false),
            speed_bps: AtomicU64::new(0),
            live_bytes: AtomicU64::new(0),
        });
        self.active_controls.write().insert(id.clone(), ctrl.clone());

        let mgr = self.clone();
        tokio::spawn(async move {
            let outcome = mgr.execute_transfer(&id, try_resume, &ctrl).await;
            mgr.active_controls.write().remove(&id);
            if let Err(err) = outcome {
                if ctrl.removed.load(Ordering::SeqCst) {
                    return;
                }
                mgr.logger.error_ctx(
                    "downloader",
                    "Permanent media download failed",
                    serde_json::json!({ "id": id, "error": err.to_string() }),
                );
                if let Ok(Some(mut rec)) = mgr.db.get_download_record(&id) {
                    if rec.status != "paused"
                        && rec.status != "cancelled"
                        && rec.status != "interrupted"
                    {
                        rec.status = "failed".to_string();
                        rec.error_message = Some(err.to_string());
                        rec.updated_at = Utc::now().timestamp_millis();
                        let _ = mgr.db.upsert_download_record(&rec);
                    }
                }
            }
        });

        Ok(())
    }

    async fn execute_transfer(
        &self,
        id: &str,
        try_resume: bool,
        ctrl: &Arc<ActiveDownloadControl>,
    ) -> Result<()> {
        let mut rec = self
            .db
            .get_download_record(id)?
            .context("Download record vanished")?;

        let final_dest = self.paths.resolve_portable_path(&rec.destination_path);
        let part_path = self.paths.resolve_portable_path(&rec.temp_part_path);
        if let Some(parent) = part_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Wait in "queued" state until an active concurrency slot (`max_concurrent_downloads`) is free!
        loop {
            if ctrl.removed.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&part_path);
                return Ok(());
            }
            if ctrl.cancel_requested.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&part_path);
                if !ctrl.removed.load(Ordering::SeqCst) {
                    rec.status = "cancelled".to_string();
                    rec.downloaded_bytes = 0;
                    rec.updated_at = Utc::now().timestamp_millis();
                    self.db.upsert_download_record(&rec)?;
                }
                return Ok(());
            }
            if ctrl.shutdown_requested.load(Ordering::Relaxed) {
                rec.status = "interrupted".to_string();
                rec.updated_at = Utc::now().timestamp_millis();
                let _ = self.db.upsert_download_record(&rec);
                return Ok(());
            }
            if ctrl.pause_requested.load(Ordering::Relaxed) {
                rec.status = "paused".to_string();
                rec.updated_at = Utc::now().timestamp_millis();
                self.db.upsert_download_record(&rec)?;
                return Ok(());
            }

            let max_conc = self
                .db
                .get_settings()
                .map(|s| s.max_concurrent_downloads.clamp(1, 16))
                .unwrap_or(3);
            let curr = self.running_transfers.load(Ordering::SeqCst);
            if curr < max_conc {
                if self
                    .running_transfers
                    .compare_exchange(curr, curr + 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        }

        struct RunningSlotGuard(Arc<AtomicUsize>);
        impl Drop for RunningSlotGuard {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _slot_guard = RunningSlotGuard(self.running_transfers.clone());

        let mut existing_offset: u64 = 0;
        if try_resume && part_path.is_file() {
            existing_offset = fs::metadata(&part_path)?.len();
        }
        ctrl.live_bytes.store(existing_offset, Ordering::Relaxed);

        rec.status = "downloading".to_string();
        rec.error_message = None;
        rec.downloaded_bytes = existing_offset;
        rec.updated_at = Utc::now().timestamp_millis();
        self.db.upsert_download_record(&rec)?;

        let mut req = self.client.get(&rec.original_url);
        if existing_offset > 0 {
            req = req.header(RANGE, format!("bytes={existing_offset}-"));
            if let Some(ref etag) = rec.etag {
                req = req.header(IF_RANGE, etag);
            }
        }

        let resp = tokio::time::timeout(Duration::from_secs(30), req.send())
            .await
            .context("Timed out connecting to media server for permanent download")?
            .context("Failed connecting to media server for permanent download")?;

        let status = resp.status();
        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            anyhow::bail!("Server responded with HTTP {status}");
        }

        if let Some(etag) = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()) {
            rec.etag = Some(etag.to_string());
        }
        if let Some(lm) = resp
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
        {
            rec.last_modified = Some(lm.to_string());
        }
        if resp
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase().contains("bytes"))
            .unwrap_or(false)
        {
            rec.supports_range = true;
        }

        let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT && existing_offset > 0 {
            rec.supports_range = true;
            if let Some(cr) = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
            {
                if let Some(tot_s) = cr.rsplit('/').next() {
                    if let Ok(tot) = tot_s.trim().parse::<u64>() {
                        rec.total_bytes = Some(tot);
                    }
                }
            }
            OpenOptions::new().create(true).append(true).open(&part_path)?
        } else {
            // Server returned 200 OK from byte 0
            if existing_offset > 0 {
                rec.supports_range = false;
                existing_offset = 0;
                ctrl.live_bytes.store(0, Ordering::Relaxed);
            }
            if let Some(cl) = resp.content_length() {
                rec.total_bytes = Some(cl);
            }
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part_path)?
        };

        self.db.upsert_download_record(&rec)?;

        let mut downloaded = existing_offset;
        let mut stream = resp.bytes_stream();
        let mut last_db_sync = Instant::now();
        let mut speed_bytes = 0u64;
        let mut speed_timer = Instant::now();
        let mut throttle_limit_kbps = self
            .db
            .get_settings()
            .map(|s| s.download_speed_limit_kbps)
            .unwrap_or(0);
        let mut throttle_window_bytes = 0u64;
        let mut throttle_window_start = Instant::now();

        loop {
            if ctrl.removed.load(Ordering::Relaxed) {
                drop(file);
                let _ = fs::remove_file(&part_path);
                return Ok(());
            }

            if ctrl.cancel_requested.load(Ordering::Relaxed) {
                drop(file);
                let _ = fs::remove_file(&part_path);
                if !ctrl.removed.load(Ordering::SeqCst) {
                    rec.status = "cancelled".to_string();
                    rec.downloaded_bytes = 0;
                    rec.updated_at = Utc::now().timestamp_millis();
                    self.db.upsert_download_record(&rec)?;
                }
                return Ok(());
            }

            if ctrl.shutdown_requested.load(Ordering::Relaxed) {
                let _ = file.flush();
                drop(file);
                rec.status = "interrupted".to_string();
                rec.downloaded_bytes = downloaded;
                rec.error_message = Some(
                    "Application exited before download completed; click Resume to continue."
                        .to_string(),
                );
                rec.updated_at = Utc::now().timestamp_millis();
                let _ = self.db.upsert_download_record(&rec);
                return Ok(());
            }

            if ctrl.pause_requested.load(Ordering::Relaxed) {
                file.flush()?;
                drop(file);
                rec.status = "paused".to_string();
                rec.downloaded_bytes = downloaded;
                rec.updated_at = Utc::now().timestamp_millis();
                self.db.upsert_download_record(&rec)?;
                self.logger.info_ctx(
                    "downloader",
                    "Paused permanent download",
                    serde_json::json!({ "id": id, "downloaded_bytes": downloaded }),
                );
                return Ok(());
            }

            let next_chunk = tokio::time::timeout(Duration::from_secs(45), stream.next())
                .await
                .context("Media server stalled during permanent download")?;
            let Some(chunk_res) = next_chunk else {
                break;
            };
            let chunk = chunk_res.context("Network interrupted during permanent download")?;
            file.write_all(&chunk)
                .context("Failed writing to permanent download file (check disk space)")?;

            downloaded += chunk.len() as u64;
            ctrl.live_bytes.store(downloaded, Ordering::Relaxed);
            speed_bytes += chunk.len() as u64;
            throttle_window_bytes += chunk.len() as u64;

            if throttle_limit_kbps > 0 {
                let target_bps = (throttle_limit_kbps as f64) * 1024.0;
                let expected_secs = (throttle_window_bytes as f64) / target_bps;
                let actual_secs = throttle_window_start.elapsed().as_secs_f64();
                if expected_secs > actual_secs {
                    let sleep_dur =
                        Duration::from_secs_f64((expected_secs - actual_secs).min(0.45));
                    if sleep_dur >= Duration::from_millis(8) {
                        tokio::time::sleep(sleep_dur).await;
                    }
                }
                if throttle_window_start.elapsed() >= Duration::from_millis(800) {
                    throttle_window_bytes = 0;
                    throttle_window_start = Instant::now();
                }
            }

            let speed_elapsed = speed_timer.elapsed();
            if speed_elapsed >= Duration::from_millis(400) {
                let bps = (speed_bytes as f64 / speed_elapsed.as_secs_f64()) as u64;
                ctrl.speed_bps.store(bps, Ordering::Relaxed);
                speed_bytes = 0;
                speed_timer = Instant::now();
            }

            if last_db_sync.elapsed() >= Duration::from_millis(750) {
                if !ctrl.removed.load(Ordering::Relaxed) {
                    rec.downloaded_bytes = downloaded;
                    rec.updated_at = Utc::now().timestamp_millis();
                    let _ = self.db.upsert_download_record(&rec);
                }
                if let Ok(s) = self.db.get_settings() {
                    throttle_limit_kbps = s.download_speed_limit_kbps;
                }
                last_db_sync = Instant::now();
            }
        }

        if ctrl.removed.load(Ordering::SeqCst) || ctrl.cancel_requested.load(Ordering::SeqCst) {
            drop(file);
            let _ = fs::remove_file(&part_path);
            if !ctrl.removed.load(Ordering::SeqCst) {
                rec.status = "cancelled".to_string();
                rec.downloaded_bytes = 0;
                rec.updated_at = Utc::now().timestamp_millis();
                self.db.upsert_download_record(&rec)?;
            }
            return Ok(());
        }

        file.flush()?;
        drop(file);

        // Final collision guard right before atomic rename so we NEVER overwrite a file created meanwhile!
        let safe_final_dest = if final_dest.exists() {
            resolve_non_colliding_destination(&final_dest)
        } else {
            final_dest
        };
        fs::rename(&part_path, &safe_final_dest).with_context(|| {
            format!(
                "Failed finalizing downloaded file at {}",
                safe_final_dest.display()
            )
        })?;

        let now = Utc::now().timestamp_millis();
        rec.destination_path = self.paths.to_portable_string(&safe_final_dest);
        rec.filename = safe_final_dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(rec.filename);
        rec.downloaded_bytes = downloaded;
        if rec.total_bytes.is_none() {
            rec.total_bytes = Some(downloaded);
        }
        rec.status = "completed".to_string();
        rec.error_message = None;
        rec.updated_at = now;
        rec.completed_at = Some(now);
        self.db.upsert_download_record(&rec)?;

        self.logger.info_ctx(
            "downloader",
            "Permanent media download completed",
            serde_json::json!({
                "id": id,
                "filename": rec.filename,
                "bytes": downloaded,
                "destination": safe_final_dest.display().to_string()
            }),
        );

        Ok(())
    }
}

/// Ensures duplicate filenames are safely disambiguated (`Movie (1).mkv`, `Movie (2).mkv`)
/// while preserving the exact original stem and extension.
pub fn resolve_non_colliding_destination(desired: &Path) -> PathBuf {
    let part_candidate = PathBuf::from(format!("{}.svpart", desired.display()));
    if !desired.exists() && !part_candidate.exists() {
        return desired.to_path_buf();
    }

    let parent = desired.parent().unwrap_or_else(|| Path::new("."));
    let file_name = desired
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "media_file".to_string());

    let (stem, ext_suffix) = match file_name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (file_name, String::new()),
    };

    for i in 1..10_000 {
        let candidate_name = format!("{stem} ({i}){ext_suffix}");
        let candidate = parent.join(&candidate_name);
        let candidate_part = PathBuf::from(format!("{}.svpart", candidate.display()));
        if !candidate.exists() && !candidate_part.exists() {
            return candidate;
        }
    }

    parent.join(format!("{stem}_{}{ext_suffix}", Uuid::new_v4().simple()))
}

fn query_dir_free_space(dir: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut avail = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
        if ok != 0 {
            return Some(avail);
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

fn open_file_in_os(file_path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::anyhow;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let clean_file = crate::portable::normalize_native_path(file_path.to_path_buf());
        let parent_dir = clean_file
            .parent()
            .map(|p| crate::portable::normalize_native_path(p.to_path_buf()));

        std::thread::spawn(move || -> Result<()> {
            let wide_open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
            let wide_file: Vec<u16> = clean_file
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let wide_dir: Option<Vec<u16>> = parent_dir.map(|d| {
                d.as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect()
            });

            unsafe {
                let hr_init = CoInitializeEx(
                    ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                );
                let hinst = ShellExecuteW(
                    ptr::null_mut(),
                    wide_open.as_ptr(),
                    wide_file.as_ptr(),
                    ptr::null(),
                    wide_dir.as_ref().map(|v| v.as_ptr()).unwrap_or(ptr::null()),
                    SW_SHOWNORMAL,
                );
                if hr_init == 0 || hr_init == 1 {
                    CoUninitialize();
                }
                if (hinst as isize) <= 32 {
                    return Err(anyhow!(
                        "Windows ShellExecuteW failed to open '{}' (code {})",
                        clean_file.display(),
                        hinst as isize
                    ));
                }
            }
            Ok(())
        })
        .join()
        .map_err(|_| anyhow!("Shell open worker panicked"))??;

        Ok(())
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open")
            .arg(file_path.as_os_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed opening downloaded file")?;
        Ok(())
    }
}

pub fn reveal_in_file_manager(
    dest_file: &Path,
    temp_part_file: Option<&Path>,
    fallback_downloads_dir: &Path,
) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::anyhow;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        use windows_sys::Win32::UI::Shell::{
            ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let clean_dest = crate::portable::normalize_native_path(dest_file.to_path_buf());
        let clean_part = temp_part_file.map(|p| crate::portable::normalize_native_path(p.to_path_buf()));

        // Pick whichever file exists on disk right now (completed file first, or active .svpart)
        let item_to_select: Option<PathBuf> = if clean_dest.exists() {
            Some(clean_dest.clone())
        } else if let Some(ref part) = clean_part {
            if part.exists() {
                Some(part.clone())
            } else {
                None
            }
        } else {
            None
        };

        // Determine the exact destination folder where this download lives
        let mut target_folder = clean_dest
            .parent()
            .map(|p| crate::portable::normalize_native_path(p.to_path_buf()))
            .unwrap_or_else(|| crate::portable::normalize_native_path(fallback_downloads_dir.to_path_buf()));
        if !target_folder.exists() {
            let _ = fs::create_dir_all(&target_folder);
        }
        if !target_folder.exists() {
            target_folder = crate::portable::normalize_native_path(fallback_downloads_dir.to_path_buf());
        }

        std::thread::spawn(move || -> Result<()> {
            unsafe {
                let hr_init = CoInitializeEx(
                    ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                );

                let mut opened = false;

                // 1. Primary path: Official Windows Shell PIDL API (`SHOpenFolderAndSelectItems`)
                // Opens Explorer directly to the exact parent folder AND highlights the file,
                // completely avoiding explorer.exe's broken `/select,"..."` command-line parser!
                if let Some(ref item_path) = item_to_select {
                    let wide_item: Vec<u16> = item_path
                        .as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();
                    let pidl = ILCreateFromPathW(wide_item.as_ptr());
                    if !pidl.is_null() {
                        let hr = SHOpenFolderAndSelectItems(pidl, 0, ptr::null(), 0);
                        ILFree(pidl);
                        if hr >= 0 {
                            opened = true;
                        }
                    }
                }

                // 2. Fallback: Open the exact destination directory directly via `ShellExecuteW`
                if !opened {
                    let wide_explore: Vec<u16> =
                        "open".encode_utf16().chain(std::iter::once(0)).collect();
                    let wide_folder: Vec<u16> = target_folder
                        .as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();
                    let hinst = ShellExecuteW(
                        ptr::null_mut(),
                        wide_explore.as_ptr(),
                        wide_folder.as_ptr(),
                        ptr::null(),
                        ptr::null(),
                        SW_SHOWNORMAL,
                    );
                    if (hinst as isize) <= 32 {
                        if hr_init == 0 || hr_init == 1 {
                            CoUninitialize();
                        }
                        return Err(anyhow!(
                            "Failed opening download folder '{}' (code {})",
                            target_folder.display(),
                            hinst as isize
                        ));
                    }
                }

                if hr_init == 0 || hr_init == 1 {
                    CoUninitialize();
                }
            }
            Ok(())
        })
        .join()
        .map_err(|_| anyhow!("Explorer folder open worker panicked"))??;

        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = temp_part_file;
        let folder = dest_file
            .parent()
            .filter(|p| p.exists())
            .unwrap_or(fallback_downloads_dir);
        Command::new("xdg-open")
            .arg(folder.as_os_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed opening destination folder")?;
        Ok(())
    }
}
