use crate::db::Database;
use crate::portable::PortablePaths;
use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutConfig {
    pub play_pause: String,
    pub seek_backward: String,
    pub seek_forward: String,
    pub volume_up: String,
    pub volume_down: String,
    pub toggle_mute: String,
    pub toggle_fullscreen: String,
    pub exit_fullscreen: String,
    pub speed_down: String,
    pub speed_up: String,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            play_pause: "Space".to_string(),
            seek_backward: "ArrowLeft".to_string(),
            seek_forward: "ArrowRight".to_string(),
            volume_up: "ArrowUp".to_string(),
            volume_down: "ArrowDown".to_string(),
            toggle_mute: "KeyM".to_string(),
            toggle_fullscreen: "KeyF".to_string(),
            exit_fullscreen: "Escape".to_string(),
            speed_down: "BracketLeft".to_string(),
            speed_up: "BracketRight".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    // General
    pub startup_behavior: String,         // "home" | "restore_session" | "homepage"
    pub homepage_url: String,             // e.g. "https://www.google.com"
    pub search_engine_template: String,   // e.g. "https://www.google.com/search?q={query}"
    pub restore_previous_session: bool,
    pub theme: String,            // "dark" | "midnight" | "light"
    pub sidebar_collapsed: bool,
    pub show_bookmark_bar: bool,
    pub recent_media_view_mode: String, // "grid" | "list"

    // Media
    pub default_playback_method: String,  // "ask" | "in_app" | "external"
    pub default_external_player: String,  // "ask" | executable path
    pub auto_detect_media: bool,
    pub probe_http_headers_for_media: bool,
    pub resume_playback_behavior: String, // "ask" | "always_resume" | "always_beginning"
    pub default_playback_speed: f64,
    pub seek_interval_seconds: i64,
    pub preferred_subtitle_lang: String,
    pub subtitles_enabled_by_default: bool,
    pub preferred_audio_lang: String,
    pub default_volume: i64,
    pub shortcuts: ShortcutConfig,

    // Downloads
    pub default_download_dir: String,     // Portable relative "data/downloads" by default
    pub ask_save_as_every_time: bool,     // True by default per Section 8 & 14
    pub max_concurrent_downloads: usize,
    pub download_notifications: bool,

    // Cache
    pub clear_temp_cache_on_exit: bool,   // True by default per Section 7 & 14
    pub startup_cache_recovery: bool,     // True by default
    pub max_temp_cache_mb: u64,           // Advisory soft limit (e.g. 8192 MB)

    // Privacy
    pub browsing_history_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            startup_behavior: "home".to_string(),
            homepage_url: "https://www.google.com".to_string(),
            search_engine_template: "https://www.google.com/search?q={query}".to_string(),
            restore_previous_session: true,
            theme: "dark".to_string(),
            sidebar_collapsed: false,
            show_bookmark_bar: true,
            recent_media_view_mode: "grid".to_string(),

            default_playback_method: "ask".to_string(),
            default_external_player: "ask".to_string(),
            auto_detect_media: true,
            probe_http_headers_for_media: true,
            resume_playback_behavior: "ask".to_string(),
            default_playback_speed: 1.0,
            seek_interval_seconds: 10,
            preferred_subtitle_lang: "eng".to_string(),
            subtitles_enabled_by_default: true,
            preferred_audio_lang: "eng".to_string(),
            default_volume: 100,
            shortcuts: ShortcutConfig::default(),

            default_download_dir: "data/downloads".to_string(),
            ask_save_as_every_time: true,
            max_concurrent_downloads: 3,
            download_notifications: true,

            clear_temp_cache_on_exit: true,
            startup_cache_recovery: true,
            max_temp_cache_mb: 8192,

            browsing_history_enabled: true,
        }
    }
}

impl Database {
    pub fn ensure_default_settings(&self, _paths: &PortablePaths) -> Result<()> {
        let count: i64 = self.with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM application_settings", [], |r| r.get(0))?)
        })?;
        if count == 0 {
            let defaults = AppSettings::default();
            self.save_settings(&defaults)?;
        }
        Ok(())
    }

    pub fn get_settings(&self) -> Result<AppSettings> {
        let rows: HashMap<String, serde_json::Value> = self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT key, value_json FROM application_settings")?;
            let iter = stmt.query_map([], |row| {
                let k: String = row.get(0)?;
                let v_str: String = row.get(1)?;
                Ok((k, v_str))
            })?;
            let mut map = HashMap::new();
            for item in iter {
                let (k, v_str) = item?;
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&v_str) {
                    map.insert(k, val);
                }
            }
            Ok(map)
        })?;

        let mut base = serde_json::to_value(AppSettings::default())?;
        if let Some(obj) = base.as_object_mut() {
            for (k, v) in rows {
                obj.insert(k, v);
            }
        }
        let settings: AppSettings = serde_json::from_value(base).unwrap_or_default();
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let value = serde_json::to_value(settings)?;
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            if let Some(obj) = value.as_object() {
                for (key, val) in obj {
                    let category = categorize_setting_key(key);
                    let val_str = serde_json::to_string(val)?;
                    tx.execute(
                        "INSERT INTO application_settings (key, value_json, category, updated_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(key) DO UPDATE SET
                             value_json = excluded.value_json,
                             category = excluded.category,
                             updated_at = excluded.updated_at",
                        params![key, val_str, category, now],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn update_single_setting(&self, key: &str, value: serde_json::Value) -> Result<AppSettings> {
        let mut current_val = serde_json::to_value(self.get_settings()?)?;
        if let Some(obj) = current_val.as_object_mut() {
            if !obj.contains_key(key) {
                anyhow::bail!("Unknown setting key: {key}");
            }
            obj.insert(key.to_string(), value);
        }
        let updated: AppSettings = serde_json::from_value(current_val)?;
        self.save_settings(&updated)?;
        Ok(updated)
    }
}

fn categorize_setting_key(key: &str) -> &'static str {
    match key {
        "startup_behavior"
        | "homepage_url"
        | "search_engine_template"
        | "restore_previous_session"
        | "theme"
        | "sidebar_collapsed"
        | "show_bookmark_bar"
        | "recent_media_view_mode" => "general",
        "default_playback_method"
        | "default_external_player"
        | "auto_detect_media"
        | "probe_http_headers_for_media"
        | "resume_playback_behavior"
        | "default_playback_speed"
        | "seek_interval_seconds"
        | "preferred_subtitle_lang"
        | "subtitles_enabled_by_default"
        | "preferred_audio_lang"
        | "default_volume"
        | "shortcuts" => "media",
        "default_download_dir"
        | "ask_save_as_every_time"
        | "max_concurrent_downloads"
        | "download_notifications" => "downloads",
        "clear_temp_cache_on_exit" | "startup_cache_recovery" | "max_temp_cache_mb" => "cache",
        _ => "privacy",
    }
}
