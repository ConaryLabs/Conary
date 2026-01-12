// src/db/models/file_entry.rs

//! FileEntry model - tracked files in the filesystem

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// A File represents a tracked file in the filesystem
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: Option<i64>,
    pub path: String,
    pub sha256_hash: String,
    pub size: i64,
    pub permissions: i32,
    pub owner: Option<String>,
    pub group_name: Option<String>,
    pub trove_id: i64,
    pub installed_at: Option<String>,
}

impl FileEntry {
    /// Create a new FileEntry
    pub fn new(
        path: String,
        sha256_hash: String,
        size: i64,
        permissions: i32,
        trove_id: i64,
    ) -> Self {
        Self {
            id: None,
            path,
            sha256_hash,
            size,
            permissions,
            owner: None,
            group_name: None,
            trove_id,
            installed_at: None,
        }
    }

    /// Insert this file into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, owner, group_name, trove_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.path,
                &self.sha256_hash,
                &self.size,
                &self.permissions,
                &self.owner,
                &self.group_name,
                &self.trove_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a file by path
    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, trove_id, installed_at
             FROM files WHERE path = ?1",
        )?;

        let file = stmt.query_row([path], Self::from_row).optional()?;

        Ok(file)
    }

    /// Find all files belonging to a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, trove_id, installed_at
             FROM files WHERE trove_id = ?1",
        )?;

        let files = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(files)
    }

    /// Delete a file by path
    pub fn delete(conn: &Connection, path: &str) -> Result<()> {
        conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
        Ok(())
    }

    /// Convert a database row to a FileEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            sha256_hash: row.get(2)?,
            size: row.get(3)?,
            permissions: row.get(4)?,
            owner: row.get(5)?,
            group_name: row.get(6)?,
            trove_id: row.get(7)?,
            installed_at: row.get(8)?,
        })
    }
}
