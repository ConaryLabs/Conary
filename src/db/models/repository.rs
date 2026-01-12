// src/db/models/repository.rs

//! Repository and RepositoryPackage models - remote package sources

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

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
        let search_pattern = format!("%{pattern}%");
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

    /// Parse dependencies from JSON field
    ///
    /// Returns a list of dependency package names. Filters out rpmlib() and file path dependencies.
    pub fn parse_dependencies(&self) -> Result<Vec<String>> {
        if let Some(deps_json) = &self.dependencies {
            let deps: Vec<String> = serde_json::from_str(deps_json)
                .map_err(|e| Error::ParseError(format!("Failed to parse dependencies: {e}")))?;

            // Filter out rpmlib() and file path dependencies (same as resolve_dependencies)
            let filtered: Vec<String> = deps
                .into_iter()
                .filter(|dep| !dep.starts_with("rpmlib(") && !dep.starts_with('/'))
                .collect();

            Ok(filtered)
        } else {
            Ok(Vec::new())
        }
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
