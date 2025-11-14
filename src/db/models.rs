// src/db/models.rs

//! Data models for Conary database entities
//!
//! This module defines Rust structs that correspond to database tables
//! and provides methods for creating, reading, updating, and deleting records.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::str::FromStr;

/// Type of trove (package, component, or collection)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TroveType {
    Package,
    Component,
    Collection,
}

impl TroveType {
    pub fn as_str(&self) -> &str {
        match self {
            TroveType::Package => "package",
            TroveType::Component => "component",
            TroveType::Collection => "collection",
        }
    }
}

impl FromStr for TroveType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "package" => Ok(TroveType::Package),
            "component" => Ok(TroveType::Component),
            "collection" => Ok(TroveType::Collection),
            _ => Err(format!("Invalid trove type: {}", s)),
        }
    }
}

/// A Trove represents a package, component, or collection
#[derive(Debug, Clone)]
pub struct Trove {
    pub id: Option<i64>,
    pub name: String,
    pub version: String,
    pub trove_type: TroveType,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub installed_by_changeset_id: Option<i64>,
}

impl Trove {
    /// Create a new Trove
    pub fn new(name: String, version: String, trove_type: TroveType) -> Self {
        Self {
            id: None,
            name,
            version,
            trove_type,
            architecture: None,
            description: None,
            installed_at: None,
            installed_by_changeset_id: None,
        }
    }

    /// Insert this trove into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, description, installed_by_changeset_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &self.name,
                &self.version,
                self.trove_type.as_str(),
                &self.architecture,
                &self.description,
                &self.installed_by_changeset_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a trove by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves WHERE id = ?1")?;

        let trove = stmt.query_row([id], Self::from_row).optional()?;

        Ok(trove)
    }

    /// Find troves by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves WHERE name = ?1")?;

        let troves = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// List all troves
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves ORDER BY name, version")?;

        let troves = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// Delete a trove by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM troves WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a Trove
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let type_str: String = row.get(3)?;
        let trove_type = type_str.parse::<TroveType>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            version: row.get(2)?,
            trove_type,
            architecture: row.get(4)?,
            description: row.get(5)?,
            installed_at: row.get(6)?,
            installed_by_changeset_id: row.get(7)?,
        })
    }
}

/// Changeset status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesetStatus {
    Pending,
    Applied,
    RolledBack,
}

impl ChangesetStatus {
    pub fn as_str(&self) -> &str {
        match self {
            ChangesetStatus::Pending => "pending",
            ChangesetStatus::Applied => "applied",
            ChangesetStatus::RolledBack => "rolled_back",
        }
    }
}

impl FromStr for ChangesetStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(ChangesetStatus::Pending),
            "applied" => Ok(ChangesetStatus::Applied),
            "rolled_back" => Ok(ChangesetStatus::RolledBack),
            _ => Err(format!("Invalid changeset status: {}", s)),
        }
    }
}

/// A Changeset represents an atomic transactional operation
#[derive(Debug, Clone)]
pub struct Changeset {
    pub id: Option<i64>,
    pub description: String,
    pub status: ChangesetStatus,
    pub created_at: Option<String>,
    pub applied_at: Option<String>,
    pub rolled_back_at: Option<String>,
}

impl Changeset {
    /// Create a new Changeset
    pub fn new(description: String) -> Self {
        Self {
            id: None,
            description,
            status: ChangesetStatus::Pending,
            created_at: None,
            applied_at: None,
            rolled_back_at: None,
        }
    }

    /// Insert this changeset into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES (?1, ?2)",
            params![&self.description, self.status.as_str()],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a changeset by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, description, status, created_at, applied_at, rolled_back_at
             FROM changesets WHERE id = ?1",
        )?;

        let changeset = stmt.query_row([id], Self::from_row).optional()?;

        Ok(changeset)
    }

    /// List all changesets
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, description, status, created_at, applied_at, rolled_back_at
             FROM changesets ORDER BY created_at DESC",
        )?;

        let changesets = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(changesets)
    }

    /// Update changeset status
    pub fn update_status(&mut self, conn: &Connection, new_status: ChangesetStatus) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update changeset without ID".to_string())
        })?;

        let timestamp_field = match new_status {
            ChangesetStatus::Applied => "applied_at",
            ChangesetStatus::RolledBack => "rolled_back_at",
            _ => "",
        };

        if !timestamp_field.is_empty() {
            conn.execute(
                &format!(
                    "UPDATE changesets SET status = ?1, {} = CURRENT_TIMESTAMP WHERE id = ?2",
                    timestamp_field
                ),
                params![new_status.as_str(), id],
            )?;
        } else {
            conn.execute(
                "UPDATE changesets SET status = ?1 WHERE id = ?2",
                params![new_status.as_str(), id],
            )?;
        }

        self.status = new_status;
        Ok(())
    }

    /// Convert a database row to a Changeset
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let status_str: String = row.get(2)?;
        let status = status_str.parse::<ChangesetStatus>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        Ok(Self {
            id: Some(row.get(0)?),
            description: row.get(1)?,
            status,
            created_at: row.get(3)?,
            applied_at: row.get(4)?,
            rolled_back_at: row.get(5)?,
        })
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_trove_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.description = Some("A test package".to_string());

        let id = trove.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(trove.id, Some(id));

        // Find by ID
        let found = Trove::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "test-package");
        assert_eq!(found.version, "1.0.0");
        assert_eq!(found.trove_type, TroveType::Package);

        // Find by name
        let by_name = Trove::find_by_name(&conn, "test-package").unwrap();
        assert_eq!(by_name.len(), 1);

        // List all
        let all = Trove::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        Trove::delete(&conn, id).unwrap();
        let deleted = Trove::find_by_id(&conn, id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_changeset_crud() {
        let (_temp, conn) = create_test_db();

        // Create a changeset
        let mut changeset = Changeset::new("Install test-package".to_string());
        let id = changeset.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(changeset.status, ChangesetStatus::Pending);

        // Find by ID
        let found = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.description, "Install test-package");
        assert_eq!(found.status, ChangesetStatus::Pending);

        // Update status
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        let updated = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(updated.status, ChangesetStatus::Applied);
        assert!(updated.applied_at.is_some());

        // List all
        let all = Changeset::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_file_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first (foreign key requirement)
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create a file
        let mut file = FileEntry::new(
            "/usr/bin/test".to_string(),
            "abc123def456".to_string(),
            1024,
            0o755,
            trove_id,
        );
        file.owner = Some("root".to_string());

        let id = file.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by path
        let found = FileEntry::find_by_path(&conn, "/usr/bin/test")
            .unwrap()
            .unwrap();
        assert_eq!(found.sha256_hash, "abc123def456");
        assert_eq!(found.size, 1024);

        // Find by trove
        let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(files.len(), 1);

        // Delete
        FileEntry::delete(&conn, "/usr/bin/test").unwrap();
        let deleted = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with a file
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut file = FileEntry::new(
            "/usr/bin/test".to_string(),
            "abc123".to_string(),
            1024,
            0o755,
            trove_id,
        );
        file.insert(&conn).unwrap();

        // Delete the trove - file should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify file is gone
        let file_exists = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(file_exists.is_none());
    }
}
