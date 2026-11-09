use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub up_sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "001_initial_schema",
        up_sql: r#"
            CREATE TABLE IF NOT EXISTS bookmark_folders (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL,
                parent_id   INTEGER NULL REFERENCES bookmark_folders(id) ON DELETE CASCADE,
                position    INTEGER NOT NULL DEFAULT 0,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_bookmark_folders_parent ON bookmark_folders(parent_id, position);

            CREATE TABLE IF NOT EXISTS bookmarks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_id       INTEGER NULL REFERENCES bookmark_folders(id) ON DELETE SET NULL,
                title           TEXT NOT NULL,
                url             TEXT NOT NULL,
                favicon_hint    TEXT NULL,
                show_on_bar     INTEGER NOT NULL DEFAULT 1,
                position        INTEGER NOT NULL DEFAULT 0,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_bookmarks_folder ON bookmarks(folder_id, position);
            CREATE INDEX IF NOT EXISTS idx_bookmarks_url ON bookmarks(url);
            CREATE INDEX IF NOT EXISTS idx_bookmarks_title ON bookmarks(title);

            CREATE TABLE IF NOT EXISTS browsing_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                title       TEXT NOT NULL,
                url         TEXT NOT NULL,
                visited_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_browsing_history_visited ON browsing_history(visited_at DESC);
            CREATE INDEX IF NOT EXISTS idx_browsing_history_url ON browsing_history(url);
            CREATE INDEX IF NOT EXISTS idx_browsing_history_title ON browsing_history(title);

            CREATE TABLE IF NOT EXISTS playback_positions (
                media_key           TEXT PRIMARY KEY,
                original_url        TEXT NOT NULL,
                position_ms         INTEGER NOT NULL DEFAULT 0,
                duration_ms         INTEGER NOT NULL DEFAULT 0,
                completed           INTEGER NOT NULL DEFAULT 0,
                audio_track_id      INTEGER NULL,
                subtitle_track_id   INTEGER NULL,
                playback_speed      REAL NOT NULL DEFAULT 1.0,
                aspect_ratio        TEXT NOT NULL DEFAULT 'auto',
                updated_at          INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_playback_positions_updated ON playback_positions(updated_at DESC);

            CREATE TABLE IF NOT EXISTS recent_media (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                media_key           TEXT NOT NULL UNIQUE,
                title               TEXT NOT NULL,
                original_url        TEXT NOT NULL,
                mime_type           TEXT NULL,
                thumbnail_svg       TEXT NULL,
                last_played_at      INTEGER NOT NULL,
                last_position_ms    INTEGER NOT NULL DEFAULT 0,
                duration_ms         INTEGER NOT NULL DEFAULT 0,
                completed           INTEGER NOT NULL DEFAULT 0,
                playback_method     TEXT NOT NULL DEFAULT 'in_app',
                play_count          INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_recent_media_last_played ON recent_media(last_played_at DESC);

            CREATE TABLE IF NOT EXISTS application_settings (
                key         TEXT PRIMARY KEY,
                value_json  TEXT NOT NULL,
                category    TEXT NOT NULL,
                updated_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_application_settings_category ON application_settings(category);

            CREATE TABLE IF NOT EXISTS custom_external_players (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                name                TEXT NOT NULL,
                executable_path     TEXT NOT NULL UNIQUE,
                custom_args         TEXT NULL,
                created_at          INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS download_history (
                id                  TEXT PRIMARY KEY,
                filename            TEXT NOT NULL,
                original_url        TEXT NOT NULL,
                destination_path    TEXT NOT NULL,
                temp_part_path      TEXT NOT NULL,
                mime_type           TEXT NULL,
                downloaded_bytes    INTEGER NOT NULL DEFAULT 0,
                total_bytes         INTEGER NULL,
                supports_range      INTEGER NOT NULL DEFAULT 0,
                etag                TEXT NULL,
                last_modified       TEXT NULL,
                status              TEXT NOT NULL,
                error_message       TEXT NULL,
                created_at          INTEGER NOT NULL,
                updated_at          INTEGER NOT NULL,
                completed_at        INTEGER NULL
            );
            CREATE INDEX IF NOT EXISTS idx_download_history_created ON download_history(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_download_history_status ON download_history(status);

            CREATE TABLE IF NOT EXISTS browser_session_tabs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                tab_order   INTEGER NOT NULL,
                title       TEXT NOT NULL,
                url         TEXT NOT NULL,
                is_active   INTEGER NOT NULL DEFAULT 0,
                updated_at  INTEGER NOT NULL
            );
        "#,
    },
    Migration {
        version: 2,
        name: "002_seed_default_folders_and_indexes",
        up_sql: r#"
            CREATE INDEX IF NOT EXISTS idx_bookmarks_bar ON bookmarks(show_on_bar, position);
            CREATE INDEX IF NOT EXISTS idx_recent_media_url ON recent_media(original_url);
        "#,
    },
];

pub fn run_migrations(conn: &mut Connection) -> Result<Vec<String>> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            applied_at  INTEGER NOT NULL
        );",
    )
    .context("Failed creating schema_migrations table")?;

    let mut applied = Vec::new();
    for migration in MIGRATIONS {
        let already_applied: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                params![migration.version],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !already_applied {
            let tx = conn
                .transaction()
                .with_context(|| format!("Failed starting transaction for migration {}", migration.name))?;
            tx.execute_batch(migration.up_sql)
                .with_context(|| format!("Failed executing migration {}", migration.name))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                params![migration.version, migration.name, Utc::now().timestamp_millis()],
            )?;
            tx.commit()?;
            applied.push(migration.name.to_string());
        }
    }

    Ok(applied)
}
