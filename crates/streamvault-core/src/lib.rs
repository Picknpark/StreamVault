pub mod db;
pub mod downloader;
pub mod external_player;
pub mod logging;
pub mod media_detect;
pub mod mpv_engine;
pub mod portable;
pub mod stream_cache;
pub mod sys_utils;

use crate::db::Database;
use crate::downloader::DownloadManager;
use crate::logging::StructuredLogger;
use crate::mpv_engine::MpvPlayerController;
use crate::portable::PortablePaths;
use crate::stream_cache::StreamCacheEngine;
use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveMediaHighlight {
    pub original_url: Option<String>,
    pub normalized_key: Option<String>,
    pub mode: Option<String>, // "in_app" | "external"
    pub title: Option<String>,
}

#[derive(Clone)]
pub struct StreamVaultCore {
    pub paths: PortablePaths,
    pub logger: StructuredLogger,
    pub db: Database,
    pub http_client: reqwest::Client,
    pub stream_cache: StreamCacheEngine,
    pub downloader: DownloadManager,
    pub player: Arc<MpvPlayerController>,
    pub active_highlight: Arc<RwLock<ActiveMediaHighlight>>,
}

impl StreamVaultCore {
    pub fn initialize(explicit_portable_root: Option<PathBuf>) -> Result<Self> {
        let paths = PortablePaths::resolve(explicit_portable_root)?;
        let logger = StructuredLogger::new(paths.log_file.clone());
        logger.info_ctx(
            "core",
            "Initializing StreamVault portable runtime",
            serde_json::json!({
                "portable_root": paths.root_dir.display().to_string(),
                "database": paths.database_file.display().to_string()
            }),
        );

        let db = Database::open(&paths, logger.clone())?;
        let settings = db.get_settings()?;

        let http_client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) StreamVault/1.0")
            .connect_timeout(Duration::from_secs(12))
            .pool_idle_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;

        let stream_cache = StreamCacheEngine::new(paths.clone(), http_client.clone(), logger.clone());
        if settings.startup_cache_recovery {
            let _ = stream_cache.startup_recovery();
        }

        let downloader = DownloadManager::new(
            paths.clone(),
            db.clone(),
            http_client.clone(),
            logger.clone(),
        );

        let player = MpvPlayerController::new(paths.clone(), logger.clone());

        Ok(Self {
            paths,
            logger,
            db,
            http_client,
            stream_cache,
            downloader,
            player,
            active_highlight: Arc::new(RwLock::new(ActiveMediaHighlight {
                original_url: None,
                normalized_key: None,
                mode: None,
                title: None,
            })),
        })
    }

    pub fn set_active_media_highlight(
        &self,
        original_url: Option<&str>,
        mode: Option<&str>,
        title: Option<&str>,
    ) -> ActiveMediaHighlight {
        let next = ActiveMediaHighlight {
            original_url: original_url.map(|s| s.to_string()),
            normalized_key: original_url.map(media_detect::normalize_media_key),
            mode: mode.map(|s| s.to_string()),
            title: title.map(|s| s.to_string()),
        };
        *self.active_highlight.write() = next.clone();
        next
    }

    /// Clean shutdown handler: saves current playback position, stops `libmpv`,
    /// checkpoints SQLite WAL, and cleans temporary streaming cache without touching permanent downloads!
    pub fn graceful_shutdown(&self) {
        let snap = self.player.snapshot();
        if snap.active {
            if let Some(ref url) = snap.original_url {
                let _ = self.db.save_playback_position(
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
        self.player.shutdown();

        let clear_on_exit = self
            .db
            .get_settings()
            .map(|s| s.clear_temp_cache_on_exit)
            .unwrap_or(true);
        let _ = self.stream_cache.shutdown_cleanup(clear_on_exit);
        self.db.checkpoint_on_exit();
        self.logger
            .info("core", "StreamVault graceful portable shutdown completed");
    }
}
