use crate::db::Database;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub id: String,
    pub filename: String,
    pub original_url: String,
    pub destination_path: String,
    pub temp_part_path: String,
    pub mime_type: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub supports_range: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub status: String, // "queued" | "downloading" | "paused" | "interrupted" | "completed" | "failed" | "cancelled"
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub speed_bps: u64,
}

impl Database {
    pub fn upsert_download_record(&self, rec: &DownloadRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO download_history (
                    id, filename, original_url, destination_path, temp_part_path, mime_type,
                    downloaded_bytes, total_bytes, supports_range, etag, last_modified,
                    status, error_message, created_at, updated_at, completed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(id) DO UPDATE SET
                    filename = excluded.filename,
                    destination_path = excluded.destination_path,
                    temp_part_path = excluded.temp_part_path,
                    mime_type = COALESCE(excluded.mime_type, download_history.mime_type),
                    downloaded_bytes = excluded.downloaded_bytes,
                    total_bytes = COALESCE(excluded.total_bytes, download_history.total_bytes),
                    supports_range = excluded.supports_range,
                    etag = COALESCE(excluded.etag, download_history.etag),
                    last_modified = COALESCE(excluded.last_modified, download_history.last_modified),
                    status = excluded.status,
                    error_message = excluded.error_message,
                    updated_at = excluded.updated_at,
                    completed_at = excluded.completed_at",
                params![
                    &rec.id,
                    &rec.filename,
                    &rec.original_url,
                    &rec.destination_path,
                    &rec.temp_part_path,
                    &rec.mime_type,
                    rec.downloaded_bytes as i64,
                    rec.total_bytes.map(|v| v as i64),
                    if rec.supports_range { 1 } else { 0 },
                    &rec.etag,
                    &rec.last_modified,
                    &rec.status,
                    &rec.error_message,
                    rec.created_at,
                    rec.updated_at,
                    rec.completed_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_download_record(&self, id: &str) -> Result<Option<DownloadRecord>> {
        self.with_conn(|conn| {
            let rec = conn
                .query_row(
                    "SELECT id, filename, original_url, destination_path, temp_part_path, mime_type,
                            downloaded_bytes, total_bytes, supports_range, etag, last_modified,
                            status, error_message, created_at, updated_at, completed_at
                     FROM download_history WHERE id = ?1",
                    params![id],
                    |r| {
                        Ok(DownloadRecord {
                            id: r.get(0)?,
                            filename: r.get(1)?,
                            original_url: r.get(2)?,
                            destination_path: r.get(3)?,
                            temp_part_path: r.get(4)?,
                            mime_type: r.get(5)?,
                            downloaded_bytes: r.get::<_, i64>(6)?.max(0) as u64,
                            total_bytes: r.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
                            supports_range: r.get::<_, i64>(8)? != 0,
                            etag: r.get(9)?,
                            last_modified: r.get(10)?,
                            status: r.get(11)?,
                            error_message: r.get(12)?,
                            created_at: r.get(13)?,
                            updated_at: r.get(14)?,
                            completed_at: r.get(15)?,
                            speed_bps: 0,
                        })
                    },
                )
                .optional()?;
            Ok(rec)
        })
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, filename, original_url, destination_path, temp_part_path, mime_type,
                        downloaded_bytes, total_bytes, supports_range, etag, last_modified,
                        status, error_message, created_at, updated_at, completed_at
                 FROM download_history
                 ORDER BY created_at DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(DownloadRecord {
                        id: r.get(0)?,
                        filename: r.get(1)?,
                        original_url: r.get(2)?,
                        destination_path: r.get(3)?,
                        temp_part_path: r.get(4)?,
                        mime_type: r.get(5)?,
                        downloaded_bytes: r.get::<_, i64>(6)?.max(0) as u64,
                        total_bytes: r.get::<_, Option<i64>>(7)?.map(|v| v.max(0) as u64),
                        supports_range: r.get::<_, i64>(8)? != 0,
                        etag: r.get(9)?,
                        last_modified: r.get(10)?,
                        status: r.get(11)?,
                        error_message: r.get(12)?,
                        created_at: r.get(13)?,
                        updated_at: r.get(14)?,
                        completed_at: r.get(15)?,
                        speed_bps: 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub fn delete_download_record(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM download_history WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn clear_completed_download_records(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let n = conn.execute(
                "DELETE FROM download_history WHERE status IN ('completed', 'cancelled', 'failed')",
                [],
            )?;
            Ok(n)
        })
    }

    /// Marks any download left in `downloading` or `queued` state from a prior run as `interrupted`
    /// so the user can resume or retry it cleanly.
    pub fn recover_interrupted_downloads(&self) -> Result<usize> {
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE download_history
                 SET status = 'interrupted',
                     error_message = 'Application exited before download completed; click Resume to continue.',
                     updated_at = ?1
                 WHERE status IN ('downloading', 'queued')",
                params![now],
            )?;
            Ok(n)
        })
    }
}
