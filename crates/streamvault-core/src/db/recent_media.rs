use crate::db::Database;
use crate::media_detect::{extract_display_title_from_url, normalize_media_key};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const MIN_RESUME_POSITION_MS: i64 = 5_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentMediaItem {
    pub id: i64,
    pub media_key: String,
    pub title: String,
    pub original_url: String,
    pub mime_type: Option<String>,
    pub thumbnail_svg: Option<String>,
    pub last_played_at: i64,
    pub last_position_ms: i64,
    pub duration_ms: i64,
    pub completed: bool,
    pub playback_method: String, // "in_app" | "external"
    pub play_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackPositionRecord {
    pub media_key: String,
    pub original_url: String,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub completed: bool,
    pub audio_track_id: Option<i64>,
    pub subtitle_track_id: Option<i64>,
    pub playback_speed: f64,
    pub aspect_ratio: String,
    pub updated_at: i64,
    pub should_offer_resume: bool,
    pub formatted_position: String,
}

pub fn is_playback_completed(position_ms: i64, duration_ms: i64) -> bool {
    if duration_ms <= 0 || position_ms <= 0 {
        return false;
    }
    if duration_ms > 30_000 && position_ms >= duration_ms - 15_000 {
        return true;
    }
    (position_ms as f64) / (duration_ms as f64) >= 0.96
}

pub fn format_duration_ms(ms: i64) -> String {
    let total_secs = (ms.max(0)) / 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

impl Database {
    /// Records that a media URL was opened in StreamVault (`"in_app"`) or an external player (`"external"`).
    /// Completely independent of Bookmarks and Browsing History.
    pub fn record_recent_media_open(
        &self,
        original_url: &str,
        custom_title: Option<&str>,
        mime_type: Option<&str>,
        playback_method: &str,
    ) -> Result<RecentMediaItem> {
        let clean_url = original_url.trim();
        if clean_url.is_empty() {
            anyhow::bail!("Media URL cannot be empty");
        }
        let media_key = normalize_media_key(clean_url);
        let derived_title = custom_title
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| extract_display_title_from_url(clean_url));
        let now = Utc::now().timestamp_millis();
        let method = if playback_method == "external" {
            "external"
        } else {
            "in_app"
        };

        self.with_conn(|conn| {
            let tx = conn.transaction()?;

            // Fetch existing playback position if any
            let saved_pos: Option<(i64, i64, i64)> = tx
                .query_row(
                    "SELECT position_ms, duration_ms, completed FROM playback_positions WHERE media_key = ?1",
                    params![&media_key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;

            let (pos_ms, dur_ms, comp) = saved_pos.unwrap_or((0, 0, 0));

            tx.execute(
                "INSERT INTO recent_media (
                    media_key, title, original_url, mime_type, last_played_at,
                    last_position_ms, duration_ms, completed, playback_method, play_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)
                 ON CONFLICT(media_key) DO UPDATE SET
                    title = excluded.title,
                    original_url = excluded.original_url,
                    mime_type = COALESCE(excluded.mime_type, recent_media.mime_type),
                    last_played_at = excluded.last_played_at,
                    playback_method = excluded.playback_method,
                    play_count = recent_media.play_count + 1",
                params![
                    &media_key,
                    &derived_title,
                    clean_url,
                    mime_type,
                    now,
                    pos_ms,
                    dur_ms,
                    comp,
                    method
                ],
            )?;

            let item = tx.query_row(
                "SELECT id, media_key, title, original_url, mime_type, thumbnail_svg,
                        last_played_at, last_position_ms, duration_ms, completed, playback_method, play_count
                 FROM recent_media WHERE media_key = ?1",
                params![&media_key],
                |r| {
                    Ok(RecentMediaItem {
                        id: r.get(0)?,
                        media_key: r.get(1)?,
                        title: r.get(2)?,
                        original_url: r.get(3)?,
                        mime_type: r.get(4)?,
                        thumbnail_svg: r.get(5)?,
                        last_played_at: r.get(6)?,
                        last_position_ms: r.get(7)?,
                        duration_ms: r.get(8)?,
                        completed: r.get::<_, i64>(9)? != 0,
                        playback_method: r.get(10)?,
                        play_count: r.get(11)?,
                    })
                },
            )?;
            tx.commit()?;
            Ok(item)
        })
    }

    /// Saves periodic/pause/close playback position in numeric milliseconds.
    /// Automatically detects completion so finished videos aren't flagged as unfinished sessions.
    pub fn save_playback_position(
        &self,
        original_url: &str,
        position_ms: i64,
        duration_ms: i64,
        audio_track_id: Option<i64>,
        subtitle_track_id: Option<i64>,
        playback_speed: Option<f64>,
        aspect_ratio: Option<&str>,
    ) -> Result<PlaybackPositionRecord> {
        let clean_url = original_url.trim();
        let media_key = normalize_media_key(clean_url);
        let clamped_pos = position_ms.max(0);
        let clamped_dur = duration_ms.max(0);
        let completed = is_playback_completed(clamped_pos, clamped_dur);
        // Per Section 10: avoid saving completed playback as an unfinished session
        let stored_pos = if completed { 0 } else { clamped_pos };
        let speed = playback_speed.unwrap_or(1.0).clamp(0.1, 4.0);
        let aspect = aspect_ratio.unwrap_or("auto");
        let now = Utc::now().timestamp_millis();

        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO playback_positions (
                    media_key, original_url, position_ms, duration_ms, completed,
                    audio_track_id, subtitle_track_id, playback_speed, aspect_ratio, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(media_key) DO UPDATE SET
                    original_url = excluded.original_url,
                    position_ms = excluded.position_ms,
                    duration_ms = CASE WHEN excluded.duration_ms > 0 THEN excluded.duration_ms ELSE playback_positions.duration_ms END,
                    completed = excluded.completed,
                    audio_track_id = COALESCE(excluded.audio_track_id, playback_positions.audio_track_id),
                    subtitle_track_id = COALESCE(excluded.subtitle_track_id, playback_positions.subtitle_track_id),
                    playback_speed = excluded.playback_speed,
                    aspect_ratio = excluded.aspect_ratio,
                    updated_at = excluded.updated_at",
                params![
                    &media_key,
                    clean_url,
                    stored_pos,
                    clamped_dur,
                    if completed { 1 } else { 0 },
                    audio_track_id,
                    subtitle_track_id,
                    speed,
                    aspect,
                    now
                ],
            )?;

            tx.execute(
                "UPDATE recent_media
                 SET last_position_ms = ?1,
                     duration_ms = CASE WHEN ?2 > 0 THEN ?2 ELSE duration_ms END,
                     completed = ?3,
                     last_played_at = ?4
                 WHERE media_key = ?5",
                params![
                    if completed { clamped_dur } else { stored_pos },
                    clamped_dur,
                    if completed { 1 } else { 0 },
                    now,
                    &media_key
                ],
            )?;

            tx.commit()?;
            Ok(())
        })?;

        Ok(self
            .get_playback_position(clean_url)?
            .expect("playback position just written"))
    }

    pub fn get_playback_position(&self, original_url: &str) -> Result<Option<PlaybackPositionRecord>> {
        let media_key = normalize_media_key(original_url.trim());
        self.with_conn(|conn| {
            let rec = conn
                .query_row(
                    "SELECT media_key, original_url, position_ms, duration_ms, completed,
                            audio_track_id, subtitle_track_id, playback_speed, aspect_ratio, updated_at
                     FROM playback_positions WHERE media_key = ?1",
                    params![&media_key],
                    |r| {
                        let pos_ms: i64 = r.get(2)?;
                        let dur_ms: i64 = r.get(3)?;
                        let completed = r.get::<_, i64>(4)? != 0;
                        let should_offer_resume = !completed && pos_ms >= MIN_RESUME_POSITION_MS;
                        Ok(PlaybackPositionRecord {
                            media_key: r.get(0)?,
                            original_url: r.get(1)?,
                            position_ms: pos_ms,
                            duration_ms: dur_ms,
                            completed,
                            audio_track_id: r.get(5)?,
                            subtitle_track_id: r.get(6)?,
                            playback_speed: r.get(7)?,
                            aspect_ratio: r.get(8)?,
                            updated_at: r.get(9)?,
                            should_offer_resume,
                            formatted_position: format_duration_ms(pos_ms),
                        })
                    },
                )
                .optional()?;
            Ok(rec)
        })
    }

    pub fn reset_playback_position(&self, original_url: &str) -> Result<()> {
        let media_key = normalize_media_key(original_url.trim());
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE playback_positions SET position_ms = 0, completed = 0, updated_at = ?1 WHERE media_key = ?2",
                params![now, &media_key],
            )?;
            tx.execute(
                "UPDATE recent_media SET last_position_ms = 0, completed = 0 WHERE media_key = ?1",
                params![&media_key],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn clear_all_playback_positions(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let n = tx.execute("DELETE FROM playback_positions", [])?;
            tx.execute("UPDATE recent_media SET last_position_ms = 0, completed = 0", [])?;
            tx.commit()?;
            Ok(n)
        })
    }

    pub fn list_recent_media(&self, limit: usize) -> Result<Vec<RecentMediaItem>> {
        let cap = limit.clamp(1, 500) as i64;
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, media_key, title, original_url, mime_type, thumbnail_svg,
                        last_played_at, last_position_ms, duration_ms, completed, playback_method, play_count
                 FROM recent_media
                 ORDER BY last_played_at DESC
                 LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![cap], |r| {
                    Ok(RecentMediaItem {
                        id: r.get(0)?,
                        media_key: r.get(1)?,
                        title: r.get(2)?,
                        original_url: r.get(3)?,
                        mime_type: r.get(4)?,
                        thumbnail_svg: r.get(5)?,
                        last_played_at: r.get(6)?,
                        last_position_ms: r.get(7)?,
                        duration_ms: r.get(8)?,
                        completed: r.get::<_, i64>(9)? != 0,
                        playback_method: r.get(10)?,
                        play_count: r.get(11)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Removes a Recent Media entry from SQLite without touching any permanent downloads on disk.
    pub fn remove_recent_media(&self, id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM recent_media WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Clears all Recent Media entries without touching Bookmarks, History, or permanent downloads.
    pub fn clear_recent_media(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let deleted = conn.execute("DELETE FROM recent_media", [])?;
            Ok(deleted)
        })
    }
}
