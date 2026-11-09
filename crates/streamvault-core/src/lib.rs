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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseUpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
    pub release_name: Option<String>,
    pub published_at: Option<String>,
    pub notes_excerpt: Option<String>,
    pub configured_repo: String,
    pub portable_upgrade_note: String,
}

#[derive(Debug, Deserialize)]
struct GithubLatestReleaseDto {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
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
        self.downloader.shutdown_active_transfers();
        self.db.checkpoint_on_exit();
        self.logger
            .info("core", "StreamVault graceful portable shutdown completed");
    }

    /// Checks GitHub Releases API (`https://api.github.com/repos/{owner}/{repo}/releases/latest`)
    /// without auto-overwriting any files, ensuring 100% safety for `StreamVault\data\` portable data.
    pub async fn check_github_release_update(
        &self,
        repo_slug_override: Option<String>,
    ) -> Result<ReleaseUpdateCheckResult> {
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        let configured_repo = match repo_slug_override {
            Some(s) if !s.trim().is_empty() => s.trim().trim_matches('/').to_string(),
            _ => self
                .db
                .get_settings()
                .map(|s| s.github_repo_slug.trim().trim_matches('/').to_string())
                .unwrap_or_default(),
        };

        let portable_upgrade_note = "Portable-Safe Upgrade: Download the new portable .zip and replace StreamVault.exe (and runtime\\ if updated) while keeping your existing data\\ folder intact — bookmarks, history, settings, downloads, and data\\backups\\streamvault.backup.db stay untouched.".to_string();

        if configured_repo.is_empty() || !configured_repo.contains('/') {
            anyhow::bail!(
                "Configure a GitHub repository slug (e.g. owner/StreamVault) in Settings → About first."
            );
        }

        let parts: Vec<&str> = configured_repo.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            anyhow::bail!("Invalid GitHub repository slug '{configured_repo}' (expected owner/repo)");
        }

        let api_url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            parts[0], parts[1]
        );
        let resp = self
            .http_client
            .get(&api_url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        if resp.status().as_u16() == 404 {
            return Ok(ReleaseUpdateCheckResult {
                current_version,
                latest_version: None,
                update_available: false,
                release_url: Some(format!("https://github.com/{configured_repo}/releases")),
                release_name: Some("No published GitHub release yet".to_string()),
                published_at: None,
                notes_excerpt: None,
                configured_repo,
                portable_upgrade_note,
            });
        }

        if !resp.status().is_success() {
            anyhow::bail!("GitHub API returned HTTP {}", resp.status());
        }

        let raw_body = resp.text().await?;
        let dto: GithubLatestReleaseDto = serde_json::from_str(&raw_body)?;
        let latest_clean = dto.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
        let update_available = is_newer_semver(&current_version, &latest_clean);
        let notes_excerpt = dto.body.map(|b| {
            let trimmed = b.trim();
            if trimmed.chars().count() > 280 {
                let mut s: String = trimmed.chars().take(280).collect();
                s.push('…');
                s
            } else {
                trimmed.to_string()
            }
        });

        Ok(ReleaseUpdateCheckResult {
            current_version,
            latest_version: Some(latest_clean),
            update_available,
            release_url: dto
                .html_url
                .or_else(|| Some(format!("https://github.com/{configured_repo}/releases"))),
            release_name: dto.name.or(Some(dto.tag_name)),
            published_at: dto.published_at,
            notes_excerpt,
            configured_repo,
            portable_upgrade_note,
        })
    }
}

pub fn is_newer_semver(current: &str, candidate: &str) -> bool {
    fn parse_parts(v: &str) -> (u64, u64, u64, bool) {
        let clean = v.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = match clean.split_once('-') {
            Some((c, p)) => (c, !p.is_empty()),
            None => (clean, false),
        };
        let mut nums = core.split('.').map(|s| s.parse::<u64>().unwrap_or(0));
        (
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
            nums.next().unwrap_or(0),
            pre,
        )
    }
    let (cmaj, cmin, cpat, cpre) = parse_parts(current);
    let (nmaj, nmin, npat, npre) = parse_parts(candidate);
    match (nmaj, nmin, npat).cmp(&(cmaj, cmin, cpat)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => cpre && !npre,
    }
}
