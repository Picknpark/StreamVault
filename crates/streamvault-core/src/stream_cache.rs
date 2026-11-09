use crate::logging::StructuredLogger;
use crate::media_detect::{extract_server_filename, preserve_or_sanitize_http_url, probe_media_url};
use crate::portable::PortablePaths;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use parking_lot::{Mutex, RwLock};
use reqwest::header::RANGE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

/// Tracks coalesced half-open byte intervals `[start, end)` stored on disk in `.svcache`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ByteIntervalSet {
    intervals: Vec<(u64, u64)>,
}

impl ByteIntervalSet {
    pub fn insert(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        self.intervals.push((start, end));
        self.intervals.sort_by_key(|&(s, _)| s);
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.intervals.len());
        for (s, e) in self.intervals.drain(..) {
            if let Some(last) = merged.last_mut() {
                if s <= last.1 {
                    last.1 = last.1.max(e);
                    continue;
                }
            }
            merged.push((s, e));
        }
        self.intervals = merged;
    }

    /// Returns the furthest contiguous byte offset available starting at `offset`,
    /// or `offset` if `offset` is not yet cached.
    pub fn contiguous_end_from(&self, offset: u64) -> u64 {
        for &(s, e) in &self.intervals {
            if offset >= s && offset < e {
                return e;
            }
            if s > offset {
                break;
            }
        }
        offset
    }

    pub fn total_cached_bytes(&self) -> u64 {
        self.intervals.iter().map(|(s, e)| e - s).sum()
    }

    pub fn normalized_ranges(&self, total_bytes: Option<u64>) -> Vec<(f64, f64)> {
        let Some(total) = total_bytes.filter(|&t| t > 0) else {
            return Vec::new();
        };
        let t = total as f64;
        self.intervals
            .iter()
            .map(|&(s, e)| ((s as f64 / t).clamp(0.0, 1.0), (e as f64 / t).clamp(0.0, 1.0)))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamCacheStatus {
    pub session_id: String,
    pub original_url: String,
    pub local_stream_url: String,
    pub filename: String,
    pub mime_type: String,
    pub total_bytes: Option<u64>,
    pub cached_bytes: u64,
    pub cached_percent: f64,
    pub download_speed_bps: u64,
    pub supports_range: bool,
    pub network_state: String, // "initializing" | "streaming" | "retrying" | "completed" | "non_range_mode" | "error"
    pub range_notice: Option<String>,
    pub buffered_ranges: Vec<(f64, f64)>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSummary {
    pub cache_dir: String,
    pub total_files: usize,
    pub total_bytes: u64,
    pub active_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCleanupReport {
    pub removed_files: usize,
    pub freed_bytes: u64,
    pub skipped_active_files: usize,
}

struct StreamSession {
    session_id: String,
    original_url: String,
    filename: String,
    mime_type: String,
    cache_file_path: PathBuf,
    meta_file_path: PathBuf,
    total_bytes: RwLock<Option<u64>>,
    supports_range: AtomicBool,
    intervals: Mutex<ByteIntervalSet>,
    file_io_lock: Mutex<File>,
    notify: Notify,
    reposition_tx: mpsc::UnboundedSender<u64>,
    cancelled: AtomicBool,
    speed_bps: AtomicU64,
    network_state: RwLock<String>,
    last_error: RwLock<Option<String>>,
}

impl StreamSession {
    fn write_chunk_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        {
            let mut file = self.file_io_lock.lock();
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(data)?;
        }
        {
            let mut iv = self.intervals.lock();
            iv.insert(offset, offset + data.len() as u64);
        }
        self.notify.notify_waiters();
        Ok(())
    }

    fn read_cached_slice(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let mut file = self.file_io_lock.lock();
        file.seek(SeekFrom::Start(offset))?;
        let n = file.read(buf)?;
        Ok(n)
    }

    fn build_status(&self, local_port: u16) -> StreamCacheStatus {
        let total = *self.total_bytes.read();
        let (cached, ranges) = {
            let iv = self.intervals.lock();
            (iv.total_cached_bytes(), iv.normalized_ranges(total))
        };
        let pct = match total {
            Some(t) if t > 0 => ((cached as f64 / t as f64) * 100.0).clamp(0.0, 100.0),
            _ => 0.0,
        };
        let supports_range = self.supports_range.load(Ordering::Relaxed);
        let encoded_name = url::form_urlencoded::byte_serialize(self.filename.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let local_stream_url = format!(
            "http://127.0.0.1:{local_port}/stream/{}/{}",
            self.session_id, encoded_name
        );
        let range_notice = if !supports_range {
            Some(
                "This media server does not support HTTP Range requests. Progressive playback and backward seeking use the local disk cache, while forward seeking ahead of buffered data is limited."
                    .to_string(),
            )
        } else {
            None
        };

        StreamCacheStatus {
            session_id: self.session_id.clone(),
            original_url: self.original_url.clone(),
            local_stream_url,
            filename: self.filename.clone(),
            mime_type: self.mime_type.clone(),
            total_bytes: total,
            cached_bytes: cached,
            cached_percent: pct,
            download_speed_bps: self.speed_bps.load(Ordering::Relaxed),
            supports_range,
            network_state: self.network_state.read().clone(),
            range_notice,
            buffered_ranges: ranges,
            last_error: self.last_error.read().clone(),
        }
    }
}

#[derive(Clone)]
pub struct StreamCacheEngine {
    paths: PortablePaths,
    client: reqwest::Client,
    logger: StructuredLogger,
    sessions: Arc<RwLock<HashMap<String, Arc<StreamSession>>>>,
    active_session_id: Arc<RwLock<Option<String>>>,
    server_port: Arc<RwLock<Option<u16>>>,
}

impl StreamCacheEngine {
    pub fn new(paths: PortablePaths, client: reqwest::Client, logger: StructuredLogger) -> Self {
        Self {
            paths,
            client,
            logger,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            active_session_id: Arc::new(RwLock::new(None)),
            server_port: Arc::new(RwLock::new(None)),
        }
    }

    /// Startup crash recovery: removes orphaned `.svcache` and `.meta.json` temporary files
    /// left behind in `data/cache/media/` if StreamVault was force-terminated previously.
    /// Never touches `data/downloads/` or any permanent downloads.
    pub fn startup_recovery(&self) -> Result<CacheCleanupReport> {
        self.paths.ensure_directories()?;
        let report = self.cleanup_cache_files(false)?;
        if report.removed_files > 0 {
            self.logger.info_ctx(
                "stream_cache",
                "Startup cache recovery cleaned orphaned temporary streaming files",
                serde_json::json!({
                    "removed_files": report.removed_files,
                    "freed_bytes": report.freed_bytes
                }),
            );
        }
        Ok(report)
    }

    /// Ensures the local loopback HTTP/1.1 Range server is running on `127.0.0.1:<port>`.
    pub async fn ensure_server_started(&self) -> Result<u16> {
        if let Some(port) = *self.server_port.read() {
            return Ok(port);
        }

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind local streaming cache loopback server")?;
        let addr: SocketAddr = listener.local_addr()?;
        let port = addr.port();
        *self.server_port.write() = Some(port);

        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((socket, _peer)) => {
                        let eng = engine.clone();
                        tokio::spawn(async move {
                            let _ = eng.handle_loopback_connection(socket).await;
                        });
                    }
                    Err(err) => {
                        engine.logger.warn_ctx(
                            "stream_cache",
                            "Loopback listener accept error",
                            serde_json::json!({ "error": err.to_string() }),
                        );
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });

        self.logger.info_ctx(
            "stream_cache",
            "Started local HTTP Range disk-cache proxy",
            serde_json::json!({ "bind": format!("127.0.0.1:{port}") }),
        );
        Ok(port)
    }

    /// Starts a new progressive disk-cached streaming session for `original_url`.
    /// Stops and cleans up any previous inactive session automatically while protecting the new active session.
    pub async fn start_stream_session(&self, raw_url: &str) -> Result<StreamCacheStatus> {
        self.paths.ensure_directories()?;
        let preserved_url = preserve_or_sanitize_http_url(raw_url)?;
        let port = self.ensure_server_started().await?;

        // Stop previous active session before starting the new one
        self.stop_active_session();
        let _ = self.cleanup_cache_files(true);

        // Probe upstream URL for Range support, Content-Length, MIME, and original filename
        let probe = probe_media_url(&self.client, &preserved_url)
            .await
            .unwrap_or_else(|_| crate::media_detect::detect_media_from_url_only(&preserved_url));

        let filename = extract_server_filename(
            &preserved_url,
            None,
            probe.mime_type.as_deref(),
        );
        let mime_type = probe
            .mime_type
            .clone()
            .unwrap_or_else(|| guess_mime_from_filename(&filename));

        let session_id = Uuid::new_v4().simple().to_string();
        let cache_file_path = self
            .paths
            .media_cache_dir
            .join(format!("session_{session_id}.svcache"));
        let meta_file_path = self
            .paths
            .media_cache_dir
            .join(format!("session_{session_id}.meta.json"));

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&cache_file_path)
            .with_context(|| {
                format!(
                    "Failed to create temporary cache file at {}",
                    cache_file_path.display()
                )
            })?;

        let meta_json = serde_json::json!({
            "session_id": session_id,
            "pid": std::process::id(),
            "original_url": preserved_url,
            "filename": filename,
            "total_bytes": probe.content_length,
            "supports_range": probe.supports_range,
            "created_at": Utc::now().to_rfc3339(),
        });
        let _ = fs::write(&meta_file_path, serde_json::to_string_pretty(&meta_json)?);

        let (reposition_tx, reposition_rx) = mpsc::unbounded_channel::<u64>();
        let initial_state = if probe.supports_range {
            "streaming".to_string()
        } else {
            "non_range_mode".to_string()
        };

        let session = Arc::new(StreamSession {
            session_id: session_id.clone(),
            original_url: preserved_url.clone(),
            filename,
            mime_type,
            cache_file_path,
            meta_file_path,
            total_bytes: RwLock::new(probe.content_length),
            supports_range: AtomicBool::new(probe.supports_range),
            intervals: Mutex::new(ByteIntervalSet::default()),
            file_io_lock: Mutex::new(file),
            notify: Notify::new(),
            reposition_tx,
            cancelled: AtomicBool::new(false),
            speed_bps: AtomicU64::new(0),
            network_state: RwLock::new(initial_state),
            last_error: RwLock::new(None),
        });

        self.sessions
            .write()
            .insert(session_id.clone(), session.clone());
        *self.active_session_id.write() = Some(session_id.clone());

        // Spawn progressive background downloader starting at byte 0
        self.spawn_progressive_worker(session.clone(), reposition_rx);

        // Wait briefly (up to 350ms) for initial header bytes so mpv gets instant startup
        let wait_start = Instant::now();
        while wait_start.elapsed() < Duration::from_millis(350) {
            if session.intervals.lock().contiguous_end_from(0) >= 65_536 {
                break;
            }
            tokio::select! {
                _ = session.notify.notified() => {},
                _ = tokio::time::sleep(Duration::from_millis(40)) => {},
            }
        }

        self.logger.info_ctx(
            "stream_cache",
            "Initialized progressive disk-cached media stream",
            serde_json::json!({
                "session_id": session_id,
                "url": preserved_url,
                "supports_range": probe.supports_range,
                "total_bytes": probe.content_length
            }),
        );

        Ok(session.build_status(port))
    }

    pub fn get_active_status(&self) -> Option<StreamCacheStatus> {
        let port = (*self.server_port.read())?;
        let active_id = self.active_session_id.read().clone()?;
        let session = self.sessions.read().get(&active_id).cloned()?;
        Some(session.build_status(port))
    }

    pub fn get_session_status(&self, session_id: &str) -> Option<StreamCacheStatus> {
        let port = (*self.server_port.read())?;
        let session = self.sessions.read().get(session_id).cloned()?;
        Some(session.build_status(port))
    }

    pub fn stop_active_session(&self) {
        let prev = self.active_session_id.write().take();
        if let Some(id) = prev {
            if let Some(sess) = self.sessions.write().remove(&id) {
                sess.cancelled.store(true, Ordering::SeqCst);
                sess.notify.notify_waiters();
                let _ = fs::remove_file(&sess.cache_file_path);
                let _ = fs::remove_file(&sess.meta_file_path);
            }
        }
    }

    /// Shutdown cleanup invoked when StreamVault closes: stops active playback stream
    /// and removes all temporary files in `data/cache/media/` if `clear_on_exit` is enabled.
    pub fn shutdown_cleanup(&self, clear_on_exit: bool) -> Result<CacheCleanupReport> {
        self.stop_active_session();
        if clear_on_exit {
            let report = self.cleanup_cache_files(false)?;
            self.logger.info_ctx(
                "stream_cache",
                "Cleaned temporary streaming cache on application exit",
                serde_json::json!({
                    "removed_files": report.removed_files,
                    "freed_bytes": report.freed_bytes
                }),
            );
            Ok(report)
        } else {
            Ok(CacheCleanupReport {
                removed_files: 0,
                freed_bytes: 0,
                skipped_active_files: 0,
            })
        }
    }

    /// Manual cache clear from Settings: deletes inactive temporary media files while
    /// protecting any `.svcache` currently being played by the built-in player!
    pub fn clear_inactive_cache(&self) -> Result<CacheCleanupReport> {
        self.cleanup_cache_files(true)
    }

    pub fn inspect_cache(&self) -> Result<CacheSummary> {
        self.paths.ensure_directories()?;
        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        if let Ok(entries) = fs::read_dir(&self.paths.media_cache_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        total_files += 1;
                        total_bytes += meta.len();
                    }
                }
            }
        }
        Ok(CacheSummary {
            cache_dir: self.paths.media_cache_dir.display().to_string(),
            total_files,
            total_bytes,
            active_session_id: self.active_session_id.read().clone(),
        })
    }

    fn cleanup_cache_files(&self, protect_active: bool) -> Result<CacheCleanupReport> {
        self.paths.ensure_directories()?;
        let active_id = if protect_active {
            self.active_session_id.read().clone()
        } else {
            None
        };

        let mut removed_files = 0usize;
        let mut freed_bytes = 0u64;
        let mut skipped_active_files = 0usize;

        if let Ok(entries) = fs::read_dir(&self.paths.media_cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Some(ref act) = active_id {
                    if fname.contains(act) {
                        skipped_active_files += 1;
                        continue;
                    }
                }
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(&path).is_ok() {
                    removed_files += 1;
                    freed_bytes += len;
                }
            }
        }

        Ok(CacheCleanupReport {
            removed_files,
            freed_bytes,
            skipped_active_files,
        })
    }

    fn spawn_progressive_worker(
        &self,
        session: Arc<StreamSession>,
        mut reposition_rx: mpsc::UnboundedReceiver<u64>,
    ) {
        let client = self.client.clone();
        let logger = self.logger.clone();

        tokio::spawn(async move {
            let mut target_offset: u64 = 0;

            'outer: loop {
                if session.cancelled.load(Ordering::Relaxed) {
                    break 'outer;
                }

                // Advance target_offset past any already-cached bytes
                let skip_to = session.intervals.lock().contiguous_end_from(target_offset);
                target_offset = skip_to;

                let total_opt = { *session.total_bytes.read() };
                if let Some(total) = total_opt {
                    if total > 0 && target_offset >= total {
                        // Check if byte 0..total is completely cached; if a seek jumped mid-file earlier,
                        // wrap back to fill any remaining gap from 0!
                        let from_zero = session.intervals.lock().contiguous_end_from(0);
                        if from_zero >= total {
                            session.speed_bps.store(0, Ordering::Relaxed);
                            *session.network_state.write() = "completed".to_string();
                            session.notify.notify_waiters();
                            // Wait in case a reader requests something else, else exit cleanly
                            match reposition_rx.recv().await {
                                Some(new_pos) => {
                                    target_offset = new_pos;
                                    continue 'outer;
                                }
                                None => break 'outer,
                            }
                        } else {
                            target_offset = from_zero;
                        }
                    }
                }

                let supports_range = session.supports_range.load(Ordering::Relaxed);
                if !supports_range && target_offset > 0 {
                    // Non-range upstream cannot jump; wait or finish
                    if let Some(new_pos) = reposition_rx.recv().await {
                        if new_pos == 0 {
                            target_offset = 0;
                            continue 'outer;
                        }
                    } else {
                        break 'outer;
                    }
                }

                let mut retries = 0u32;
                let resp = loop {
                    if session.cancelled.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    let mut req = client.get(&session.original_url);
                    if supports_range && target_offset > 0 {
                        req = req.header(RANGE, format!("bytes={target_offset}-"));
                    } else if target_offset == 0 && supports_range {
                        req = req.header(RANGE, "bytes=0-");
                    }

                    let send_res = tokio::time::timeout(Duration::from_secs(20), req.send()).await;
                    match send_res {
                        Ok(Ok(r)) if r.status().is_success() || r.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                            break r;
                        }
                        Ok(Ok(r)) => {
                            retries += 1;
                            let msg = format!("Upstream HTTP {}", r.status());
                            *session.last_error.write() = Some(msg.clone());
                            if retries > 5 {
                                *session.network_state.write() = "error".to_string();
                                logger.error_ctx(
                                    "stream_cache",
                                    "Upstream media server returned persistent error",
                                    serde_json::json!({ "url": session.original_url, "status": r.status().as_u16() }),
                                );
                                session.notify.notify_waiters();
                                break 'outer;
                            }
                            *session.network_state.write() = "retrying".to_string();
                            tokio::time::sleep(Duration::from_millis(300 * (1 << retries.min(4)))).await;
                        }
                        Ok(Err(err)) => {
                            retries += 1;
                            *session.last_error.write() = Some(err.to_string());
                            if retries > 6 {
                                *session.network_state.write() = "error".to_string();
                                logger.error_ctx(
                                    "stream_cache",
                                    "Upstream network connection failed after retries",
                                    serde_json::json!({ "url": session.original_url, "error": err.to_string() }),
                                );
                                session.notify.notify_waiters();
                                break 'outer;
                            }
                            *session.network_state.write() = "retrying".to_string();
                            tokio::time::sleep(Duration::from_millis(350 * (1 << retries.min(4)))).await;
                        }
                        Err(_) => {
                            retries += 1;
                            *session.last_error.write() = Some("Upstream connection timed out".to_string());
                            if retries > 6 {
                                *session.network_state.write() = "error".to_string();
                                logger.error_ctx(
                                    "stream_cache",
                                    "Upstream network connection timed out after retries",
                                    serde_json::json!({ "url": session.original_url }),
                                );
                                session.notify.notify_waiters();
                                break 'outer;
                            }
                            *session.network_state.write() = "retrying".to_string();
                            tokio::time::sleep(Duration::from_millis(350 * (1 << retries.min(4)))).await;
                        }
                    }
                };

                // Inspect actual response status: if we asked for `bytes=target_offset-` with `target_offset > 0`
                // and server ignored Range returning 200 OK, reset offset to 0 and mark non-range!
                let actual_status = resp.status();
                let mut write_cursor = if actual_status == reqwest::StatusCode::PARTIAL_CONTENT {
                    session.supports_range.store(true, Ordering::Relaxed);
                    *session.network_state.write() = "streaming".to_string();
                    if let Some(cr) = resp
                        .headers()
                        .get(reqwest::header::CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                    {
                        if let Some(tot_str) = cr.rsplit('/').next() {
                            if let Ok(tot) = tot_str.trim().parse::<u64>() {
                                *session.total_bytes.write() = Some(tot);
                            }
                        }
                    }
                    target_offset
                } else {
                    if target_offset > 0 {
                        session.supports_range.store(false, Ordering::Relaxed);
                    }
                    if session. total_bytes.read().is_none() {
                        if let Some(cl) = resp.content_length() {
                            *session.total_bytes.write() = Some(cl);
                        }
                    }
                    *session.network_state.write() = if session.supports_range.load(Ordering::Relaxed) {
                        "streaming".to_string()
                    } else {
                        "non_range_mode".to_string()
                    };
                    0
                };

                let mut stream = resp.bytes_stream();
                let mut speed_window_bytes = 0u64;
                let mut speed_window_start = Instant::now();

                loop {
                    if session.cancelled.load(Ordering::Relaxed) {
                        break 'outer;
                    }

                    tokio::select! {
                        biased;
                        maybe_jump = reposition_rx.recv() => {
                            if let Some(new_offset) = maybe_jump {
                                // Only interrupt if `new_offset` is not already cached and not right ahead of `write_cursor`
                                let cached_end = session.intervals.lock().contiguous_end_from(new_offset);
                                if cached_end == new_offset && (new_offset < write_cursor || new_offset > write_cursor + 512 * 1024) {
                                    target_offset = new_offset;
                                    continue 'outer;
                                }
                            }
                        }
                        chunk_timed = tokio::time::timeout(Duration::from_secs(30), stream.next()) => {
                            match chunk_timed {
                                Ok(Some(Ok(chunk))) => {
                                    if let Err(io_err) = session.write_chunk_at(write_cursor, &chunk) {
                                        *session.last_error.write() = Some(format!("Disk cache write error: {io_err}"));
                                        *session.network_state.write() = "error".to_string();
                                        session.notify.notify_waiters();
                                        break 'outer;
                                    }
                                    write_cursor += chunk.len() as u64;
                                    speed_window_bytes += chunk.len() as u64;
                                    let elapsed = speed_window_start.elapsed();
                                    if elapsed >= Duration::from_millis(400) {
                                        let bps = (speed_window_bytes as f64 / elapsed.as_secs_f64()) as u64;
                                        session.speed_bps.store(bps, Ordering::Relaxed);
                                        speed_window_bytes = 0;
                                        speed_window_start = Instant::now();
                                    }
                                }
                                Ok(Some(Err(net_err))) => {
                                    // Transient network interruption: resume seamlessly from `write_cursor`!
                                    *session.last_error.write() = Some(net_err.to_string());
                                    *session.network_state.write() = "retrying".to_string();
                                    target_offset = write_cursor;
                                    tokio::time::sleep(Duration::from_millis(400)).await;
                                    continue 'outer;
                                }
                                Err(_) => {
                                    // Read stall: reconnect via Range from `write_cursor`
                                    *session.last_error.write() = Some("Stream read stalled; reconnecting".to_string());
                                    *session.network_state.write() = "retrying".to_string();
                                    target_offset = write_cursor;
                                    tokio::time::sleep(Duration::from_millis(300)).await;
                                    continue 'outer;
                                }
                                Ok(None) => {
                                    if session.total_bytes.read().is_none() && write_cursor > 0 {
                                        *session.total_bytes.write() = Some(write_cursor);
                                    }
                                    target_offset = write_cursor;
                                    continue 'outer;
                                }
                            }
                        }
                    }
                }
            }
            session.speed_bps.store(0, Ordering::Relaxed);
        });
    }

    /// Performs an immediate bounded HTTP Range fetch (e.g., when `libmpv` reads MKV `Cues` at the end of a 2 GB file)
    /// without disrupting the primary sequential progressive stream!
    async fn fetch_bounded_range_into_cache(
        &self,
        session: &Arc<StreamSession>,
        start: u64,
        end_inclusive: u64,
    ) -> Result<()> {
        let resp = self
            .client
            .get(&session.original_url)
            .header(RANGE, format!("bytes={start}-{end_inclusive}"))
            .timeout(Duration::from_secs(20))
            .send()
            .await?;

        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            anyhow::bail!("Upstream did not honor bounded Range request");
        }

        let mut cursor = start;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            session.write_chunk_at(cursor, &bytes)?;
            cursor += bytes.len() as u64;
        }
        Ok(())
    }

    async fn handle_loopback_connection(&self, mut socket: TcpStream) -> Result<()> {
        let mut req_buf = vec![0u8; 8192];
        let mut read_total = 0usize;
        loop {
            let n = socket.read(&mut req_buf[read_total..]).await?;
            if n == 0 {
                return Ok(());
            }
            read_total += n;
            if req_buf[..read_total].windows(4).any(|w| w == b"\r\n\r\n") || read_total >= req_buf.len() {
                break;
            }
        }

        let req_str = String::from_utf8_lossy(&req_buf[..read_total]);
        let mut lines = req_str.lines();
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("GET").to_uppercase();
        let path = parts.next().unwrap_or("/");

        // Expect `/stream/<session_id>/...`
        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if segs.len() < 2 || segs[0] != "stream" {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            return Ok(());
        }
        let session_id = segs[1];
        let Some(session) = self.sessions.read().get(session_id).cloned() else {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            return Ok(());
        };

        let mut range_header: Option<String> = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("range") {
                    range_header = Some(v.trim().to_string());
                }
            }
        }

        let total_opt = *session.total_bytes.read();
        let supports_range = session.supports_range.load(Ordering::Relaxed);

        if method == "HEAD" {
            let mut hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: {}\r\nConnection: close\r\n",
                session.mime_type,
                if supports_range { "bytes" } else { "none" }
            );
            if let Some(total) = total_opt {
                hdr.push_str(&format!("Content-Length: {total}\r\n"));
            }
            hdr.push_str("\r\n");
            socket.write_all(hdr.as_bytes()).await?;
            return Ok(());
        }

        // Parse Range header if supplied
        let parsed_range = range_header
            .as_deref()
            .and_then(|rh| parse_http_byte_range(rh, total_opt));

        if let Some((start, end_opt)) = parsed_range {
            // Check if non-range upstream and requested start is far beyond cached data
            let cached_from_zero = session.intervals.lock().contiguous_end_from(0);
            if !supports_range && start > cached_from_zero + 262_144 {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                return Ok(());
            }

            if let Some(total) = total_opt {
                if start >= total {
                    let resp = format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return Ok(());
                }
                let end = end_opt.unwrap_or(total - 1).min(total - 1);
                let content_len = end - start + 1;
                let hdr = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n",
                    session.mime_type
                );
                socket.write_all(hdr.as_bytes()).await?;
                self.stream_cached_range_to_socket(&session, &mut socket, start, Some(end))
                    .await?;
                return Ok(());
            }
        }

        // Full 200 OK progressive stream from byte 0
        let mut hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: {}\r\nConnection: close\r\n",
            session.mime_type,
            if supports_range { "bytes" } else { "none" }
        );
        if let Some(total) = total_opt {
            hdr.push_str(&format!("Content-Length: {total}\r\n"));
        }
        hdr.push_str("\r\n");
        socket.write_all(hdr.as_bytes()).await?;
        let end_opt = total_opt.and_then(|t| if t > 0 { Some(t - 1) } else { None });
        self.stream_cached_range_to_socket(&session, &mut socket, 0, end_opt)
            .await?;
        Ok(())
    }

    async fn stream_cached_range_to_socket(
        &self,
        session: &Arc<StreamSession>,
        socket: &mut TcpStream,
        start: u64,
        end_inclusive: Option<u64>,
    ) -> Result<()> {
        let mut cursor = start;
        let mut read_buf = vec![0u8; 131_072]; // 128 KiB streaming buffer (constant tiny RAM!)
        let mut requested_jump = false;

        loop {
            if session.cancelled.load(Ordering::Relaxed) {
                break;
            }
            if let Some(end) = end_inclusive {
                if cursor > end {
                    break;
                }
            } else if let Some(total) = *session.total_bytes.read() {
                if total > 0 && cursor >= total && *session.network_state.read() == "completed" {
                    break;
                }
            }

            let avail_end = session.intervals.lock().contiguous_end_from(cursor);
            if avail_end > cursor {
                let max_want = match end_inclusive {
                    Some(end) => (end - cursor + 1).min(read_buf.len() as u64) as usize,
                    None => read_buf.len(),
                };
                let to_read = ((avail_end - cursor) as usize).min(max_want);
                let n = session.read_cached_slice(cursor, &mut read_buf[..to_read])?;
                if n > 0 {
                    socket.write_all(&read_buf[..n]).await?;
                    cursor += n as u64;
                    requested_jump = false;
                    continue;
                }
            }

            if *session.network_state.read() == "error" {
                break;
            }
            if *session.network_state.read() == "completed" {
                if let Some(total) = *session.total_bytes.read() {
                    if cursor >= total {
                        break;
                    }
                }
            }

            // Cache miss at `cursor`: decide between bounded side-fetch (e.g. MKV Cues tail) vs repositioning main stream
            if !requested_jump && session.supports_range.load(Ordering::Relaxed) {
                requested_jump = true;
                if let Some(end) = end_inclusive {
                    let span = end.saturating_sub(cursor) + 1;
                    let total = (*session.total_bytes.read()).unwrap_or(0);
                    // Bounded small probe (<= 1 MiB) far from start (e.g., MKV Cues index at tail)
                    if span <= 1_048_576 && cursor > 2 * 1_048_576 && (end + 1 == total || span <= 262_144) {
                        let eng = self.clone();
                        let sess = session.clone();
                        let _ = eng.fetch_bounded_range_into_cache(&sess, cursor, end).await;
                        continue;
                    }
                }
                let _ = session.reposition_tx.send(cursor);
            }

            tokio::select! {
                _ = session.notify.notified() => {},
                _ = tokio::time::sleep(Duration::from_millis(120)) => {},
            }
        }

        let _ = socket.flush().await;
        Ok(())
    }
}

fn parse_http_byte_range(header: &str, total_bytes: Option<u64>) -> Option<(u64, Option<u64>)> {
    let rest = header.trim().strip_prefix("bytes=")?;
    let first_range = rest.split(',').next()?.trim();
    let (start_s, end_s) = first_range.split_once('-')?;

    if start_s.trim().is_empty() {
        // Suffix range `bytes=-65536`
        let suffix_len: u64 = end_s.trim().parse().ok()?;
        let total = total_bytes?;
        if suffix_len == 0 || total == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix_len);
        return Some((start, Some(total - 1)));
    }

    let start: u64 = start_s.trim().parse().ok()?;
    let end_opt = if end_s.trim().is_empty() {
        None
    } else {
        let end = end_s.trim().parse::<u64>().ok()?;
        if end < start {
            return None;
        }
        Some(end)
    };
    Some((start, end_opt))
}

fn guess_mime_from_filename(filename: &str) -> String {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mkv" => "video/x-matroska",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "mpeg" | "mpg" => "video/mpeg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        _ => "application/octet-stream",
    }
    .to_string()
}
