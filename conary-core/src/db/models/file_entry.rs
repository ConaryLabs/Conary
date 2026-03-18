// conary-core/src/db/models/file_entry.rs

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
    /// Component ID this file belongs to (None for legacy pre-component installs)
    pub component_id: Option<i64>,
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
            component_id: None,
        }
    }

    /// Create a new FileEntry with a component ID
    pub fn new_with_component(
        path: String,
        sha256_hash: String,
        size: i64,
        permissions: i32,
        trove_id: i64,
        component_id: i64,
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
            component_id: Some(component_id),
        }
    }

    /// Insert this file into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO files (path, sha256_hash, size, permissions, owner, group_name, trove_id, component_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &self.path,
                &self.sha256_hash,
                &self.size,
                &self.permissions,
                &self.owner,
                &self.group_name,
                &self.trove_id,
                &self.component_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or replace this file in the database (handles shared paths)
    ///
    /// Multiple packages may claim the same path (directories, shared files).
    /// This method updates the existing record if the path already exists.
    ///
    /// # Ownership semantics
    ///
    /// `INSERT OR REPLACE` in SQLite is equivalent to a `DELETE` followed by an
    /// `INSERT` when a uniqueness constraint (here, the `path` column) is violated.
    /// This means:
    ///
    /// - The original row's `rowid` is **not** preserved; the replaced row gets a
    ///   new `rowid`. Any foreign keys referencing the old `rowid` will be affected
    ///   (cascade-deleted or set to NULL depending on the schema).
    ///
    /// - During package upgrades, the file's `trove_id` is reassigned to the new
    ///   package version. The old package's ownership of this path is silently lost.
    ///   This is intentional for single-owner files but means that if two packages
    ///   legitimately share a path, only the last writer's `trove_id` is recorded.
    ///
    /// - Callers performing upgrades should install the new package's files (which
    ///   will replace ownership) and then delete the old trove. Reversing this order
    ///   would cascade-delete the files before the new trove can claim them.
    pub fn insert_or_replace(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT OR REPLACE INTO files (path, sha256_hash, size, permissions, owner, group_name, trove_id, component_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &self.path,
                &self.sha256_hash,
                &self.size,
                &self.permissions,
                &self.owner,
                &self.group_name,
                &self.trove_id,
                &self.component_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a file by path
    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files WHERE path = ?1",
        )?;
        let file = stmt.query_row([path], Self::from_row).optional()?;
        Ok(file)
    }

    /// Find all files belonging to a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files WHERE trove_id = ?1",
        )?;
        let files = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    /// Find all files belonging to a specific component
    pub fn find_by_component(conn: &Connection, component_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files WHERE component_id = ?1",
        )?;
        let files = stmt
            .query_map([component_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    /// Return all file entries ordered by path.
    ///
    /// Used by the generation builder to collect every tracked file into the
    /// EROFS image without per-trove round-trips.
    pub fn find_all_ordered(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files ORDER BY path",
        )?;
        let files = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    /// Find files matching a path pattern (LIKE query)
    pub fn find_by_path_pattern(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files WHERE path LIKE ?1 ORDER BY path",
        )?;
        let files = stmt
            .query_map([pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    /// List all files for a trove with ls -l style information
    pub fn list_files_lsl(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, path, sha256_hash, size, permissions, owner, group_name, \
             trove_id, installed_at, component_id \
             FROM files WHERE trove_id = ?1 ORDER BY path",
        )?;
        let files = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(files)
    }

    /// Format permissions as rwx string (e.g., -rw-r--r--)
    pub fn format_permissions(&self) -> String {
        let mode = self.permissions as u32;
        let file_type = match mode & 0o170000 {
            0o120000 => 'l',
            0o040000 => 'd',
            _ => '-',
        };

        let owner_r = if mode & 0o400 != 0 { 'r' } else { '-' };
        let owner_w = if mode & 0o200 != 0 { 'w' } else { '-' };
        let owner_x = if mode & 0o100 != 0 { 'x' } else { '-' };
        let group_r = if mode & 0o040 != 0 { 'r' } else { '-' };
        let group_w = if mode & 0o020 != 0 { 'w' } else { '-' };
        let group_x = if mode & 0o010 != 0 { 'x' } else { '-' };
        let other_r = if mode & 0o004 != 0 { 'r' } else { '-' };
        let other_w = if mode & 0o002 != 0 { 'w' } else { '-' };
        let other_x = if mode & 0o001 != 0 { 'x' } else { '-' };

        format!(
            "{}{}{}{}{}{}{}{}{}{}",
            file_type,
            owner_r,
            owner_w,
            owner_x,
            group_r,
            group_w,
            group_x,
            other_r,
            other_w,
            other_x
        )
    }

    /// Format size as human-readable string
    pub fn size_human(&self) -> String {
        super::format_size(self.size)
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
            component_id: row.get(9)?,
        })
    }
}
