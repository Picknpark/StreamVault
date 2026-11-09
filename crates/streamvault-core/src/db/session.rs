use crate::db::Database;
use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSessionTab {
    pub tab_order: i64,
    pub title: String,
    pub url: String,
    pub is_active: bool,
}

impl Database {
    pub fn save_browser_session(&self, tabs: &[SavedSessionTab]) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM browser_session_tabs", [])?;
            for (idx, t) in tabs.iter().enumerate() {
                if t.url.trim().is_empty() {
                    continue;
                }
                tx.execute(
                    "INSERT INTO browser_session_tabs (tab_order, title, url, is_active, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        idx as i64,
                        t.title.trim(),
                        t.url.trim(),
                        if t.is_active { 1 } else { 0 },
                        now
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn load_browser_session(&self) -> Result<Vec<SavedSessionTab>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tab_order, title, url, is_active
                 FROM browser_session_tabs
                 ORDER BY tab_order ASC, id ASC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(SavedSessionTab {
                        tab_order: r.get(0)?,
                        title: r.get(1)?,
                        url: r.get(2)?,
                        is_active: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}
