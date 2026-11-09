use crate::db::Database;
use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub visited_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressSuggestion {
    pub source: String, // "bookmark" | "history"
    pub title: String,
    pub url: String,
    pub timestamp: i64,
}

impl Database {
    /// Records a webpage visit when `browsing_history_enabled` is true.
    /// Avoids duplicate back-to-back entries within 3 seconds on the exact same URL.
    pub fn record_history_visit(&self, title: &str, url: &str) -> Result<Option<HistoryEntry>> {
        let settings = self.get_settings()?;
        if !settings.browsing_history_enabled {
            return Ok(None);
        }

        let clean_url = url.trim();
        if clean_url.is_empty()
            || clean_url.starts_with("about:")
            || clean_url.starts_with("tauri:")
            || clean_url.starts_with("data:")
        {
            return Ok(None);
        }

        let clean_title = if title.trim().is_empty() {
            clean_url
        } else {
            title.trim()
        };
        let now = Utc::now().timestamp_millis();

        self.with_conn(|conn| {
            let recent_same: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT id, visited_at FROM browsing_history ORDER BY visited_at DESC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok()
                .and_then(|(id, ts): (i64, i64)| {
                    let prev_url: String = conn
                        .query_row(
                            "SELECT url FROM browsing_history WHERE id = ?1",
                            params![id],
                            |r| r.get(0),
                        )
                        .unwrap_or_default();
                    if prev_url == clean_url && (now - ts).abs() < 3_000 {
                        Some((id, ts))
                    } else {
                        None
                    }
                });

            if let Some((existing_id, _)) = recent_same {
                conn.execute(
                    "UPDATE browsing_history SET title = ?1, visited_at = ?2 WHERE id = ?3",
                    params![clean_title, now, existing_id],
                )?;
                return Ok(Some(HistoryEntry {
                    id: existing_id,
                    title: clean_title.to_string(),
                    url: clean_url.to_string(),
                    visited_at: now,
                }));
            }

            conn.execute(
                "INSERT INTO browsing_history (title, url, visited_at) VALUES (?1, ?2, ?3)",
                params![clean_title, clean_url, now],
            )?;
            let id = conn.last_insert_rowid();
            Ok(Some(HistoryEntry {
                id,
                title: clean_title.to_string(),
                url: clean_url.to_string(),
                visited_at: now,
            }))
        })
    }

    pub fn list_history(&self, query: Option<&str>, limit: usize) -> Result<Vec<HistoryEntry>> {
        let max_rows = limit.clamp(1, 2000) as i64;
        self.with_conn(|conn| {
            if let Some(q) = query.map(|s| s.trim()).filter(|s| !s.is_empty()) {
                let pattern = format!("%{}%", q.to_lowercase());
                let mut stmt = conn.prepare(
                    "SELECT id, title, url, visited_at
                     FROM browsing_history
                     WHERE LOWER(title) LIKE ?1 OR LOWER(url) LIKE ?1
                     ORDER BY visited_at DESC, id DESC
                     LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(params![pattern, max_rows], |r| {
                        Ok(HistoryEntry {
                            id: r.get(0)?,
                            title: r.get(1)?,
                            url: r.get(2)?,
                            visited_at: r.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            } else {
                let mut stmt = conn.prepare(
                    "SELECT id, title, url, visited_at
                     FROM browsing_history
                     ORDER BY visited_at DESC, id DESC
                     LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map(params![max_rows], |r| {
                        Ok(HistoryEntry {
                            id: r.get(0)?,
                            title: r.get(1)?,
                            url: r.get(2)?,
                            visited_at: r.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            }
        })
    }

    pub fn delete_history_entry(&self, id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM browsing_history WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Clears browsing history for a selected period:
    /// `"1h"` | `"24h"` | `"7d"` | `"30d"` | `"all"`
    pub fn clear_history_period(&self, period: &str) -> Result<usize> {
        let now = Utc::now().timestamp_millis();
        let cutoff_ms: Option<i64> = match period {
            "1h" => Some(now - 3_600_000),
            "24h" => Some(now - 86_400_000),
            "7d" => Some(now - 7 * 86_400_000),
            "30d" => Some(now - 30 * 86_400_000),
            "all" => None,
            other => anyhow::bail!("Unsupported history clear period: {other}"),
        };

        self.with_conn(|conn| {
            let deleted = match cutoff_ms {
                Some(cutoff) => conn.execute(
                    "DELETE FROM browsing_history WHERE visited_at >= ?1",
                    params![cutoff],
                )?,
                None => conn.execute("DELETE FROM browsing_history", [])?,
            };
            Ok(deleted)
        })
    }

    /// Provides fast autocomplete suggestions from Bookmarks and Browsing History for the Address Bar.
    pub fn suggest_addresses(&self, query: &str, limit: usize) -> Result<Vec<AddressSuggestion>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", trimmed.to_lowercase());
        let cap = limit.clamp(1, 20);

        self.with_conn(|conn| {
            let mut out = Vec::new();
            let mut seen = std::collections::HashSet::new();

            let mut b_stmt = conn.prepare(
                "SELECT title, url, updated_at FROM bookmarks
                 WHERE LOWER(title) LIKE ?1 OR LOWER(url) LIKE ?1
                 ORDER BY updated_at DESC LIMIT ?2",
            )?;
            for row in b_stmt.query_map(params![&pattern, cap as i64], |r| {
                Ok(AddressSuggestion {
                    source: "bookmark".to_string(),
                    title: r.get(0)?,
                    url: r.get(1)?,
                    timestamp: r.get(2)?,
                })
            })? {
                let item = row?;
                if seen.insert(item.url.clone()) {
                    out.push(item);
                }
            }

            if out.len() < cap {
                let remaining = (cap - out.len()) as i64;
                let mut h_stmt = conn.prepare(
                    "SELECT title, url, MAX(visited_at) as last_visit
                     FROM browsing_history
                     WHERE LOWER(title) LIKE ?1 OR LOWER(url) LIKE ?1
                     GROUP BY url
                     ORDER BY last_visit DESC
                     LIMIT ?2",
                )?;
                for row in h_stmt.query_map(params![&pattern, remaining], |r| {
                    Ok(AddressSuggestion {
                        source: "history".to_string(),
                        title: r.get(0)?,
                        url: r.get(1)?,
                        timestamp: r.get(2)?,
                    })
                })? {
                    let item = row?;
                    if seen.insert(item.url.clone()) {
                        out.push(item);
                    }
                }
            }

            Ok(out)
        })
    }
}
