pub mod bookmarks;
pub mod downloads;
pub mod history;
pub mod migrations;
pub mod recent_media;
pub mod session;
pub mod settings;

use crate::logging::StructuredLogger;
use crate::portable::PortablePaths;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    backups_dir: PathBuf,
    #[allow(dead_code)]
    logger: StructuredLogger,
    recovered_from_corruption: bool,
}

impl Database {
    /// Opens the portable SQLite database, verifies integrity, recovers automatically
    /// from corruption if detected, runs migrations, seeds defaults, and snapshots a clean backup.
    pub fn open(paths: &PortablePaths, logger: StructuredLogger) -> Result<Self> {
        paths.ensure_directories()?;
        let db_path = paths.database_file.clone();
        let backups_dir = paths.backups_dir.clone();

        let mut recovered = false;
        let mut conn = match Self::try_open_and_verify(&db_path) {
            Ok(c) => c,
            Err(err) => {
                recovered = true;
                logger.error_ctx(
                    "database",
                    "Detected corrupted SQLite database; initiating automatic recovery",
                    serde_json::json!({
                        "db_path": db_path.display().to_string(),
                        "error": err.to_string()
                    }),
                );
                Self::recover_corrupted_db(&db_path, &backups_dir, &logger)?;
                Self::open_raw_connection(&db_path)?
            }
        };

        Self::configure_pragmas(&conn)?;
        let applied = migrations::run_migrations(&mut conn)?;
        if !applied.is_empty() {
            logger.info_ctx(
                "database",
                "Applied SQLite schema migrations",
                serde_json::json!({ "migrations": applied }),
            );
        }

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
            backups_dir,
            logger,
            recovered_from_corruption: recovered,
        };

        db.ensure_default_settings(paths)?;
        // Mark any downloads that were mid-flight during an unexpected exit as interrupted/paused
        let _ = db.recover_interrupted_downloads();
        // Create rolling online backup if healthy
        if !recovered {
            let _ = db.create_rolling_backup();
        }

        Ok(db)
    }

    pub fn was_recovered_from_corruption(&self) -> bool {
        self.recovered_from_corruption
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T>,
    {
        let mut guard = self.conn.lock();
        f(&mut guard)
    }

    fn open_raw_connection(db_path: &Path) -> Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        Connection::open_with_flags(db_path, flags)
            .with_context(|| format!("Failed to open SQLite database at {}", db_path.display()))
    }

    fn configure_pragmas(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Duration::from_millis(5_000))?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;",
        )?;
        Ok(())
    }

    fn try_open_and_verify(db_path: &Path) -> Result<Connection> {
        if db_path.exists() {
            let meta = fs::metadata(db_path)?;
            if meta.len() > 0 && meta.len() < 100 {
                anyhow::bail!("SQLite file truncated ({} bytes)", meta.len());
            }
            if meta.len() >= 16 {
                let header = fs::read(db_path)?;
                if &header[..16] != b"SQLite format 3\0" {
                    anyhow::bail!("Invalid SQLite file header signature");
                }
            }
        }

        let conn = Self::open_raw_connection(db_path)?;
        Self::configure_pragmas(&conn)?;
        let check: String = conn.query_row("PRAGMA quick_check(1);", [], |r| r.get(0))?;
        if check.to_lowercase() != "ok" {
            anyhow::bail!("SQLite quick_check failed: {check}");
        }
        Ok(conn)
    }

    fn recover_corrupted_db(
        db_path: &Path,
        backups_dir: &Path,
        logger: &StructuredLogger,
    ) -> Result<()> {
        fs::create_dir_all(backups_dir)?;
        if db_path.exists() {
            let quarantine = backups_dir.join("streamvault.corrupt.db");
            let _ = fs::remove_file(&quarantine);
            let _ = fs::rename(db_path, &quarantine);
            // Clean up stale WAL/SHM sidecars
            let _ = fs::remove_file(format!("{}-wal", db_path.display()));
            let _ = fs::remove_file(format!("{}-shm", db_path.display()));
        }

        // Primary single-file backup (`streamvault.backup.db`), plus any legacy timestamped backup fallback
        let mut candidates: Vec<PathBuf> = Vec::new();
        let single_backup = backups_dir.join("streamvault.backup.db");
        if single_backup.is_file() {
            candidates.push(single_backup);
        }
        if let Ok(entries) = fs::read_dir(backups_dir) {
            let mut legacy: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    name.starts_with("streamvault.backup.") && name.ends_with(".db")
                })
                .collect();
            legacy.sort();
            legacy.reverse();
            candidates.extend(legacy);
        }

        for candidate in candidates {
            if Self::try_open_and_verify(&candidate).is_ok() {
                fs::copy(&candidate, db_path)?;
                logger.info_ctx(
                    "database",
                    "Restored SQLite database from clean portable backup",
                    serde_json::json!({ "backup_used": candidate.display().to_string() }),
                );
                return Ok(());
            }
        }

        logger.warn(
            "database",
            "No prior backup found; initializing fresh portable SQLite database",
        );
        Ok(())
    }

    /// Creates a live online backup in `data/backups/streamvault.backup.db`, atomically
    /// overriding the single backup file every time and removing any older backup files.
    pub fn create_rolling_backup(&self) -> Result<PathBuf> {
        fs::create_dir_all(&self.backups_dir)?;
        let dest_path = self.backups_dir.join("streamvault.backup.db");
        let tmp_path = self.backups_dir.join("streamvault.backup.db.tmp");
        let _ = fs::remove_file(&tmp_path);

        {
            let src_conn = self.conn.lock();
            let mut dst_conn = Connection::open(&tmp_path)?;
            let backup = Backup::new(&src_conn, &mut dst_conn)?;
            backup.run_to_completion(100, Duration::from_millis(10), None)?;
        }

        if dest_path.exists() {
            let _ = fs::remove_file(&dest_path);
        }
        fs::rename(&tmp_path, &dest_path)?;

        // Clean up any legacy multi-backup files (`streamvault.backup.*.db`) or SQLite sidecars
        // so `data/backups/` strictly retains only the single `streamvault.backup.db`.
        if let Ok(entries) = fs::read_dir(&self.backups_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p == dest_path {
                    continue;
                }
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if (name.starts_with("streamvault.backup.") && name != "streamvault.backup.db")
                    || name.ends_with(".tmp")
                {
                    let _ = fs::remove_file(&p);
                }
            }
        }

        Ok(dest_path)
    }

    /// Ensures WAL frames are checkpointed cleanly before portable shutdown.
    pub fn checkpoint_on_exit(&self) {
        let guard = self.conn.lock();
        let _ = guard.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}
