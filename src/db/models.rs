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
    pub reversed_by_changeset_id: Option<i64>,
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
            reversed_by_changeset_id: None,
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
            "SELECT id, description, status, created_at, applied_at, rolled_back_at, reversed_by_changeset_id
             FROM changesets WHERE id = ?1",
        )?;

        let changeset = stmt.query_row([id], Self::from_row).optional()?;

        Ok(changeset)
    }

    /// List all changesets
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, description, status, created_at, applied_at, rolled_back_at, reversed_by_changeset_id
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
            reversed_by_changeset_id: row.get(6)?,
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

/// A Flavor represents a build-time variation (e.g., architecture, features, toolchain)
#[derive(Debug, Clone)]
pub struct Flavor {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub key: String,
    pub value: String,
}

impl Flavor {
    /// Create a new Flavor
    pub fn new(trove_id: i64, key: String, value: String) -> Self {
        Self {
            id: None,
            trove_id,
            key,
            value,
        }
    }

    /// Insert this flavor into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO flavors (trove_id, key, value) VALUES (?1, ?2, ?3)",
            params![&self.trove_id, &self.key, &self.value],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all flavors for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, key, value FROM flavors WHERE trove_id = ?1 ORDER BY key",
        )?;

        let flavors = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(flavors)
    }

    /// Find flavors by key name across all troves
    pub fn find_by_key(conn: &Connection, key: &str) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, trove_id, key, value FROM flavors WHERE key = ?1")?;

        let flavors = stmt
            .query_map([key], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(flavors)
    }

    /// Delete a flavor by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM flavors WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a Flavor
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            key: row.get(2)?,
            value: row.get(3)?,
        })
    }
}

/// Provenance tracks the supply chain for a trove
#[derive(Debug, Clone)]
pub struct Provenance {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub source_url: Option<String>,
    pub source_branch: Option<String>,
    pub source_commit: Option<String>,
    pub build_host: Option<String>,
    pub build_time: Option<String>,
    pub builder: Option<String>,
}

impl Provenance {
    /// Create a new Provenance
    pub fn new(trove_id: i64) -> Self {
        Self {
            id: None,
            trove_id,
            source_url: None,
            source_branch: None,
            source_commit: None,
            build_host: None,
            build_time: None,
            builder: None,
        }
    }

    /// Insert this provenance into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO provenance (trove_id, source_url, source_branch, source_commit, build_host, build_time, builder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.trove_id,
                &self.source_url,
                &self.source_branch,
                &self.source_commit,
                &self.build_host,
                &self.build_time,
                &self.builder,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find provenance for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, source_url, source_branch, source_commit, build_host, build_time, builder
             FROM provenance WHERE trove_id = ?1",
        )?;

        let provenance = stmt.query_row([trove_id], Self::from_row).optional()?;

        Ok(provenance)
    }

    /// Update provenance information
    pub fn update(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update provenance without ID".to_string())
        })?;

        conn.execute(
            "UPDATE provenance SET source_url = ?1, source_branch = ?2, source_commit = ?3,
             build_host = ?4, build_time = ?5, builder = ?6 WHERE id = ?7",
            params![
                &self.source_url,
                &self.source_branch,
                &self.source_commit,
                &self.build_host,
                &self.build_time,
                &self.builder,
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete provenance by trove ID
    pub fn delete(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM provenance WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a Provenance
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            source_url: row.get(2)?,
            source_branch: row.get(3)?,
            source_commit: row.get(4)?,
            build_host: row.get(5)?,
            build_time: row.get(6)?,
            builder: row.get(7)?,
        })
    }
}

/// Dependency entry linking troves to their dependencies
#[derive(Debug, Clone)]
pub struct DependencyEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    pub depends_on_name: String,
    pub depends_on_version: Option<String>,
    pub dependency_type: String,
    pub version_constraint: Option<String>,
}

impl DependencyEntry {
    /// Create a new DependencyEntry
    pub fn new(
        trove_id: i64,
        depends_on_name: String,
        depends_on_version: Option<String>,
        dependency_type: String,
        version_constraint: Option<String>,
    ) -> Self {
        Self {
            id: None,
            trove_id,
            depends_on_name,
            depends_on_version,
            dependency_type,
            version_constraint,
        }
    }

    /// Insert this dependency into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO dependencies (trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.trove_id,
                &self.depends_on_name,
                &self.depends_on_version,
                &self.dependency_type,
                &self.version_constraint,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all dependencies for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint
             FROM dependencies WHERE trove_id = ?1",
        )?;

        let deps = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all troves that depend on a given package name (reverse dependencies)
    pub fn find_dependents(conn: &Connection, package_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, depends_on_name, depends_on_version, dependency_type, version_constraint
             FROM dependencies WHERE depends_on_name = ?1",
        )?;

        let deps = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(deps)
    }

    /// Find all packages that can satisfy a dependency (by name)
    pub fn find_providers(conn: &Connection, dependency_name: &str) -> Result<Vec<Trove>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
             FROM troves WHERE name = ?1",
        )?;

        let troves = stmt
            .query_map([dependency_name], Trove::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(troves)
    }

    /// Delete a specific dependency
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM dependencies WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all dependencies for a trove (called when removing a package)
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM dependencies WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a DependencyEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            depends_on_name: row.get(2)?,
            depends_on_version: row.get(3)?,
            dependency_type: row.get(4)?,
            version_constraint: row.get(5)?,
        })
    }
}

/// Repository represents a remote package source
#[derive(Debug, Clone)]
pub struct Repository {
    pub id: Option<i64>,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub priority: i32,
    pub gpg_check: bool,
    pub gpg_key_url: Option<String>,
    pub metadata_expire: i32,
    pub last_sync: Option<String>,
    pub created_at: Option<String>,
}

impl Repository {
    /// Create a new Repository
    pub fn new(name: String, url: String) -> Self {
        Self {
            id: None,
            name,
            url,
            enabled: true,
            priority: 0,
            gpg_check: true,
            gpg_key_url: None,
            metadata_expire: 3600, // Default: 1 hour
            last_sync: None,
            created_at: None,
        }
    }

    /// Insert this repository into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, gpg_check, gpg_key_url, metadata_expire)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &self.name,
                &self.url,
                self.enabled as i32,
                &self.priority,
                self.gpg_check as i32,
                &self.gpg_key_url,
                &self.metadata_expire,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a repository by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, url, enabled, priority, gpg_check, gpg_key_url, metadata_expire, last_sync, created_at
             FROM repositories WHERE id = ?1",
        )?;

        let repo = stmt.query_row([id], Self::from_row).optional()?;

        Ok(repo)
    }

    /// Find a repository by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, url, enabled, priority, gpg_check, gpg_key_url, metadata_expire, last_sync, created_at
             FROM repositories WHERE name = ?1",
        )?;

        let repo = stmt.query_row([name], Self::from_row).optional()?;

        Ok(repo)
    }

    /// List all repositories
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, url, enabled, priority, gpg_check, gpg_key_url, metadata_expire, last_sync, created_at
             FROM repositories ORDER BY priority DESC, name",
        )?;

        let repos = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(repos)
    }

    /// List enabled repositories
    pub fn list_enabled(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, url, enabled, priority, gpg_check, gpg_key_url, metadata_expire, last_sync, created_at
             FROM repositories WHERE enabled = 1 ORDER BY priority DESC, name",
        )?;

        let repos = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(repos)
    }

    /// Update repository metadata
    pub fn update(&self, conn: &Connection) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::error::Error::InitError("Cannot update repository without ID".to_string())
        })?;

        conn.execute(
            "UPDATE repositories SET name = ?1, url = ?2, enabled = ?3, priority = ?4,
             gpg_check = ?5, gpg_key_url = ?6, metadata_expire = ?7, last_sync = ?8 WHERE id = ?9",
            params![
                &self.name,
                &self.url,
                self.enabled as i32,
                &self.priority,
                self.gpg_check as i32,
                &self.gpg_key_url,
                &self.metadata_expire,
                &self.last_sync,
                id,
            ],
        )?;

        Ok(())
    }

    /// Delete a repository by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM repositories WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a Repository
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            url: row.get(2)?,
            enabled: row.get::<_, i32>(3)? != 0,
            priority: row.get(4)?,
            gpg_check: row.get::<_, i32>(5)? != 0,
            gpg_key_url: row.get(6)?,
            metadata_expire: row.get(7)?,
            last_sync: row.get(8)?,
            created_at: row.get(9)?,
        })
    }
}

/// RepositoryPackage represents a package available from a repository
#[derive(Debug, Clone)]
pub struct RepositoryPackage {
    pub id: Option<i64>,
    pub repository_id: i64,
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    pub dependencies: Option<String>,
    pub metadata: Option<String>,
    pub synced_at: Option<String>,
}

impl RepositoryPackage {
    /// Create a new RepositoryPackage
    pub fn new(
        repository_id: i64,
        name: String,
        version: String,
        checksum: String,
        size: i64,
        download_url: String,
    ) -> Self {
        Self {
            id: None,
            repository_id,
            name,
            version,
            architecture: None,
            description: None,
            checksum,
            size,
            download_url,
            dependencies: None,
            metadata: None,
            synced_at: None,
        }
    }

    /// Insert this repository package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, architecture, description, checksum, size, download_url, dependencies, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &self.repository_id,
                &self.name,
                &self.version,
                &self.architecture,
                &self.description,
                &self.checksum,
                &self.size,
                &self.download_url,
                &self.dependencies,
                &self.metadata,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find repository packages by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_id, name, version, architecture, description, checksum, size,
                    download_url, dependencies, metadata, synced_at
             FROM repository_packages WHERE name = ?1",
        )?;

        let packages = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Find repository packages by repository ID
    pub fn find_by_repository(conn: &Connection, repository_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_id, name, version, architecture, description, checksum, size,
                    download_url, dependencies, metadata, synced_at
             FROM repository_packages WHERE repository_id = ?1",
        )?;

        let packages = stmt
            .query_map([repository_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Search repository packages by pattern (name or description)
    pub fn search(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let search_pattern = format!("%{}%", pattern);
        let mut stmt = conn.prepare(
            "SELECT id, repository_id, name, version, architecture, description, checksum, size,
                    download_url, dependencies, metadata, synced_at
             FROM repository_packages
             WHERE name LIKE ?1 OR description LIKE ?1
             ORDER BY name, version",
        )?;

        let packages = stmt
            .query_map([&search_pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(packages)
    }

    /// Delete all packages for a repository (used when syncing)
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_packages WHERE repository_id = ?1",
            [repository_id],
        )?;
        Ok(())
    }

    /// Delete a specific package by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM repository_packages WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a RepositoryPackage
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            architecture: row.get(4)?,
            description: row.get(5)?,
            checksum: row.get(6)?,
            size: row.get(7)?,
            download_url: row.get(8)?,
            dependencies: row.get(9)?,
            metadata: row.get(10)?,
            synced_at: row.get(11)?,
        })
    }
}

/// Package delta information for efficient updates
#[derive(Debug, Clone)]
pub struct PackageDelta {
    pub id: Option<i64>,
    pub package_name: String,
    pub from_version: String,
    pub to_version: String,
    pub from_hash: String,
    pub to_hash: String,
    pub delta_url: String,
    pub delta_size: i64,
    pub delta_checksum: String,
    pub full_size: i64,
    pub compression_ratio: f64,
    pub created_at: Option<String>,
}

impl PackageDelta {
    /// Create a new PackageDelta
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_name: String,
        from_version: String,
        to_version: String,
        from_hash: String,
        to_hash: String,
        delta_url: String,
        delta_size: i64,
        delta_checksum: String,
        full_size: i64,
    ) -> Self {
        let compression_ratio = if full_size > 0 {
            delta_size as f64 / full_size as f64
        } else {
            1.0
        };

        Self {
            id: None,
            package_name,
            from_version,
            to_version,
            from_hash,
            to_hash,
            delta_url,
            delta_size,
            delta_checksum,
            full_size,
            compression_ratio,
            created_at: None,
        }
    }

    /// Insert this package delta into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO package_deltas
             (package_name, from_version, to_version, from_hash, to_hash, delta_url, delta_size, delta_checksum, full_size, compression_ratio)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &self.package_name,
                &self.from_version,
                &self.to_version,
                &self.from_hash,
                &self.to_hash,
                &self.delta_url,
                &self.delta_size,
                &self.delta_checksum,
                &self.full_size,
                &self.compression_ratio,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find delta for a specific version transition
    pub fn find_delta(
        conn: &Connection,
        package_name: &str,
        from_version: &str,
        to_version: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, package_name, from_version, to_version, from_hash, to_hash,
                    delta_url, delta_size, delta_checksum, full_size, compression_ratio, created_at
             FROM package_deltas
             WHERE package_name = ?1 AND from_version = ?2 AND to_version = ?3",
        )?;

        let delta = stmt
            .query_row([package_name, from_version, to_version], Self::from_row)
            .optional()?;

        Ok(delta)
    }

    /// Find all available deltas for a package
    pub fn find_by_package(conn: &Connection, package_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, package_name, from_version, to_version, from_hash, to_hash,
                    delta_url, delta_size, delta_checksum, full_size, compression_ratio, created_at
             FROM package_deltas
             WHERE package_name = ?1
             ORDER BY created_at DESC",
        )?;

        let deltas = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deltas)
    }

    /// Delete a package delta
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM package_deltas WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a PackageDelta
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            package_name: row.get(1)?,
            from_version: row.get(2)?,
            to_version: row.get(3)?,
            from_hash: row.get(4)?,
            to_hash: row.get(5)?,
            delta_url: row.get(6)?,
            delta_size: row.get(7)?,
            delta_checksum: row.get(8)?,
            full_size: row.get(9)?,
            compression_ratio: row.get(10)?,
            created_at: row.get(11)?,
        })
    }
}

/// Delta statistics for tracking bandwidth savings
#[derive(Debug, Clone)]
pub struct DeltaStats {
    pub id: Option<i64>,
    pub changeset_id: i64,
    pub total_bytes_saved: i64,
    pub deltas_applied: i32,
    pub full_downloads: i32,
    pub delta_failures: i32,
    pub created_at: Option<String>,
}

impl DeltaStats {
    /// Create new DeltaStats
    pub fn new(changeset_id: i64) -> Self {
        Self {
            id: None,
            changeset_id,
            total_bytes_saved: 0,
            deltas_applied: 0,
            full_downloads: 0,
            delta_failures: 0,
            created_at: None,
        }
    }

    /// Insert delta stats into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO delta_stats
             (changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.changeset_id,
                &self.total_bytes_saved,
                &self.deltas_applied,
                &self.full_downloads,
                &self.delta_failures,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find delta stats by changeset ID
    pub fn find_by_changeset(conn: &Connection, changeset_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats
             WHERE changeset_id = ?1",
        )?;

        let stats = stmt.query_row([changeset_id], Self::from_row).optional()?;

        Ok(stats)
    }

    /// Get aggregate statistics across all changesets
    pub fn get_total_stats(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT 0, 0,
                    SUM(total_bytes_saved),
                    SUM(deltas_applied),
                    SUM(full_downloads),
                    SUM(delta_failures),
                    NULL
             FROM delta_stats",
        )?;

        let stats = stmt.query_row([], Self::from_row)?;

        Ok(stats)
    }

    /// Convert a database row to DeltaStats
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: {
                let id: i64 = row.get(0)?;
                if id == 0 {
                    None
                } else {
                    Some(id)
                }
            },
            changeset_id: row.get(1)?,
            total_bytes_saved: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            deltas_applied: row.get::<_, Option<i32>>(3)?.unwrap_or(0),
            full_downloads: row.get::<_, Option<i32>>(4)?.unwrap_or(0),
            delta_failures: row.get::<_, Option<i32>>(5)?.unwrap_or(0),
            created_at: row.get(6)?,
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

    #[test]
    fn test_flavor_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create flavors
        let mut flavor1 = Flavor::new(trove_id, "ssl".to_string(), "enabled".to_string());
        let id1 = flavor1.insert(&conn).unwrap();
        assert!(id1 > 0);

        let mut flavor2 = Flavor::new(trove_id, "http3".to_string(), "enabled".to_string());
        flavor2.insert(&conn).unwrap();

        // Find by trove
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 2);
        assert_eq!(flavors[0].key, "http3"); // Ordered by key
        assert_eq!(flavors[1].key, "ssl");

        // Find by key
        let ssl_flavors = Flavor::find_by_key(&conn, "ssl").unwrap();
        assert_eq!(ssl_flavors.len(), 1);
        assert_eq!(ssl_flavors[0].value, "enabled");

        // Delete
        Flavor::delete(&conn, id1).unwrap();
        let remaining = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key, "http3");
    }

    #[test]
    fn test_provenance_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create provenance
        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://github.com/nginx/nginx".to_string());
        prov.source_branch = Some("main".to_string());
        prov.source_commit = Some("abc123def456".to_string());
        prov.build_host = Some("builder01.example.com".to_string());
        prov.builder = Some("builder-bot".to_string());

        let id = prov.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by trove
        let found = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(
            found.source_url,
            Some("https://github.com/nginx/nginx".to_string())
        );
        assert_eq!(found.source_commit, Some("abc123def456".to_string()));
        assert_eq!(found.builder, Some("builder-bot".to_string()));

        // Update
        let mut updated_prov = found.clone();
        updated_prov.source_commit = Some("new_commit_hash".to_string());
        updated_prov.update(&conn).unwrap();

        let reloaded = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(reloaded.source_commit, Some("new_commit_hash".to_string()));

        // Delete
        Provenance::delete(&conn, trove_id).unwrap();
        let deleted = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_flavor_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with flavors
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut flavor = Flavor::new(trove_id, "feature".to_string(), "enabled".to_string());
        flavor.insert(&conn).unwrap();

        // Delete the trove - flavors should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify flavors are gone
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 0);
    }

    #[test]
    fn test_provenance_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with provenance
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://example.com".to_string());
        prov.insert(&conn).unwrap();

        // Delete the trove - provenance should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify provenance is gone
        let prov_exists = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(prov_exists.is_none());
    }
}
