use crate::db::Database;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkFolder {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub title: String,
    pub url: String,
    pub favicon_hint: Option<String>,
    pub show_on_bar: bool,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkTree {
    pub folders: Vec<BookmarkFolder>,
    pub bookmarks: Vec<Bookmark>,
}

impl Database {
    pub fn list_bookmark_tree(&self) -> Result<BookmarkTree> {
        self.with_conn(|conn| {
            let mut f_stmt = conn.prepare(
                "SELECT id, name, parent_id, position, created_at, updated_at
                 FROM bookmark_folders
                 ORDER BY position ASC, id ASC",
            )?;
            let folders = f_stmt
                .query_map([], |r| {
                    Ok(BookmarkFolder {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        parent_id: r.get(2)?,
                        position: r.get(3)?,
                        created_at: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut b_stmt = conn.prepare(
                "SELECT id, folder_id, title, url, favicon_hint, show_on_bar, position, created_at, updated_at
                 FROM bookmarks
                 ORDER BY position ASC, id ASC",
            )?;
            let bookmarks = b_stmt
                .query_map([], |r| {
                    Ok(Bookmark {
                        id: r.get(0)?,
                        folder_id: r.get(1)?,
                        title: r.get(2)?,
                        url: r.get(3)?,
                        favicon_hint: r.get(4)?,
                        show_on_bar: r.get::<_, i64>(5)? != 0,
                        position: r.get(6)?,
                        created_at: r.get(7)?,
                        updated_at: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(BookmarkTree { folders, bookmarks })
        })
    }

    pub fn find_bookmark_by_url(&self, url: &str) -> Result<Option<Bookmark>> {
        let trimmed = url.trim();
        self.with_conn(|conn| {
            let b = conn
                .query_row(
                    "SELECT id, folder_id, title, url, favicon_hint, show_on_bar, position, created_at, updated_at
                     FROM bookmarks WHERE url = ?1 LIMIT 1",
                    params![trimmed],
                    |r| {
                        Ok(Bookmark {
                            id: r.get(0)?,
                            folder_id: r.get(1)?,
                            title: r.get(2)?,
                            url: r.get(3)?,
                            favicon_hint: r.get(4)?,
                            show_on_bar: r.get::<_, i64>(5)? != 0,
                            position: r.get(6)?,
                            created_at: r.get(7)?,
                            updated_at: r.get(8)?,
                        })
                    },
                )
                .optional()?;
            Ok(b)
        })
    }

    pub fn add_bookmark(
        &self,
        title: &str,
        url: &str,
        folder_id: Option<i64>,
        show_on_bar: bool,
    ) -> Result<Bookmark> {
        let clean_url = url.trim();
        if clean_url.is_empty() {
            anyhow::bail!("Bookmark URL cannot be empty");
        }
        let clean_title = if title.trim().is_empty() {
            clean_url
        } else {
            title.trim()
        };
        let now = Utc::now().timestamp_millis();

        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let next_pos: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) + 1 FROM bookmarks WHERE folder_id IS ?1",
                    params![folder_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);

            tx.execute(
                "INSERT INTO bookmarks (folder_id, title, url, show_on_bar, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    folder_id,
                    clean_title,
                    clean_url,
                    if show_on_bar { 1 } else { 0 },
                    next_pos,
                    now,
                    now
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;

            Ok(Bookmark {
                id,
                folder_id,
                title: clean_title.to_string(),
                url: clean_url.to_string(),
                favicon_hint: None,
                show_on_bar,
                position: next_pos,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn update_bookmark(
        &self,
        id: i64,
        title: &str,
        url: &str,
        folder_id: Option<i64>,
        show_on_bar: bool,
    ) -> Result<Bookmark> {
        let clean_url = url.trim();
        if clean_url.is_empty() {
            anyhow::bail!("Bookmark URL cannot be empty");
        }
        let clean_title = if title.trim().is_empty() {
            clean_url
        } else {
            title.trim()
        };
        let now = Utc::now().timestamp_millis();

        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE bookmarks
                 SET title = ?1, url = ?2, folder_id = ?3, show_on_bar = ?4, updated_at = ?5
                 WHERE id = ?6",
                params![
                    clean_title,
                    clean_url,
                    folder_id,
                    if show_on_bar { 1 } else { 0 },
                    now,
                    id
                ],
            )?;
            if changed == 0 {
                anyhow::bail!("Bookmark #{id} not found");
            }
            let b = conn.query_row(
                "SELECT id, folder_id, title, url, favicon_hint, show_on_bar, position, created_at, updated_at
                 FROM bookmarks WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Bookmark {
                        id: r.get(0)?,
                        folder_id: r.get(1)?,
                        title: r.get(2)?,
                        url: r.get(3)?,
                        favicon_hint: r.get(4)?,
                        show_on_bar: r.get::<_, i64>(5)? != 0,
                        position: r.get(6)?,
                        created_at: r.get(7)?,
                        updated_at: r.get(8)?,
                    })
                },
            )?;
            Ok(b)
        })
    }

    pub fn move_bookmark(&self, id: i64, folder_id: Option<i64>, position: Option<i64>) -> Result<()> {
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let pos = match position {
                Some(p) => p,
                None => tx
                    .query_row(
                        "SELECT COALESCE(MAX(position), -1) + 1 FROM bookmarks WHERE folder_id IS ?1",
                        params![folder_id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0),
            };
            tx.execute(
                "UPDATE bookmarks SET folder_id = ?1, position = ?2, updated_at = ?3 WHERE id = ?4",
                params![folder_id, pos, now, id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn create_bookmark_folder(&self, name: &str, parent_id: Option<i64>) -> Result<BookmarkFolder> {
        let clean = name.trim();
        if clean.is_empty() {
            anyhow::bail!("Folder name cannot be empty");
        }
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let next_pos: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) + 1 FROM bookmark_folders WHERE parent_id IS ?1",
                    params![parent_id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            tx.execute(
                "INSERT INTO bookmark_folders (name, parent_id, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![clean, parent_id, next_pos, now, now],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(BookmarkFolder {
                id,
                name: clean.to_string(),
                parent_id,
                position: next_pos,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn rename_bookmark_folder(&self, id: i64, new_name: &str) -> Result<()> {
        let clean = new_name.trim();
        if clean.is_empty() {
            anyhow::bail!("Folder name cannot be empty");
        }
        let now = Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE bookmark_folders SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![clean, now, id],
            )?;
            if changed == 0 {
                anyhow::bail!("Bookmark folder #{id} not found");
            }
            Ok(())
        })
    }

    pub fn delete_bookmark_folder(&self, id: i64, delete_contained_bookmarks: bool) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            if delete_contained_bookmarks {
                tx.execute("DELETE FROM bookmarks WHERE folder_id = ?1", params![id])?;
            } else {
                tx.execute(
                    "UPDATE bookmarks SET folder_id = NULL WHERE folder_id = ?1",
                    params![id],
                )?;
            }
            tx.execute("DELETE FROM bookmark_folders WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn search_bookmarks(&self, query: &str) -> Result<Vec<Bookmark>> {
        let pattern = format!("%{}%", query.trim().to_lowercase());
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, folder_id, title, url, favicon_hint, show_on_bar, position, created_at, updated_at
                 FROM bookmarks
                 WHERE LOWER(title) LIKE ?1 OR LOWER(url) LIKE ?1
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 200",
            )?;
            let rows = stmt
                .query_map(params![pattern], |r| {
                    Ok(Bookmark {
                        id: r.get(0)?,
                        folder_id: r.get(1)?,
                        title: r.get(2)?,
                        url: r.get(3)?,
                        favicon_hint: r.get(4)?,
                        show_on_bar: r.get::<_, i64>(5)? != 0,
                        position: r.get(6)?,
                        created_at: r.get(7)?,
                        updated_at: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
}
