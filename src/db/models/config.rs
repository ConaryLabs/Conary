// src/db/models/config.rs

//! Configuration file tracking model
//!
//! Tracks configuration files with special handling for upgrades:
//! - Preserves user modifications during package updates
//! - Backs up configs before modification
//! - Enables config diff between installed and package versions

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::fmt;

/// Status of a configuration file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatus {
    /// File is unchanged from package version
    Pristine,
    /// User has modified the file
    Modified,
    /// File has been deleted from filesystem
    Missing,
}

impl ConfigStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigStatus::Pristine => "pristine",
            ConfigStatus::Modified => "modified",
            ConfigStatus::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pristine" => Some(ConfigStatus::Pristine),
            "modified" => Some(ConfigStatus::Modified),
            "missing" => Some(ConfigStatus::Missing),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Source that declared a file as configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// Automatically detected (e.g., file in /etc/)
    Auto,
    /// RPM %config directive
    Rpm,
    /// Debian conffiles
    Deb,
    /// Arch package
    Arch,
}

impl ConfigSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigSource::Auto => "auto",
            ConfigSource::Rpm => "rpm",
            ConfigSource::Deb => "deb",
            ConfigSource::Arch => "arch",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(ConfigSource::Auto),
            "rpm" => Some(ConfigSource::Rpm),
            "deb" => Some(ConfigSource::Deb),
            "arch" => Some(ConfigSource::Arch),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A tracked configuration file
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub id: Option<i64>,
    /// Reference to files table
    pub file_id: Option<i64>,
    /// Filesystem path
    pub path: String,
    /// Owning package
    pub trove_id: i64,
    /// Hash of file as shipped by package
    pub original_hash: String,
    /// Current hash on filesystem (None if not checked)
    pub current_hash: Option<String>,
    /// If true, preserve user's version on upgrade
    pub noreplace: bool,
    /// Current status
    pub status: ConfigStatus,
    /// When modification was detected
    pub modified_at: Option<String>,
    /// Source that declared this as config
    pub source: ConfigSource,
}

impl ConfigFile {
    /// Create a new config file entry
    pub fn new(path: String, trove_id: i64, original_hash: String) -> Self {
        Self {
            id: None,
            file_id: None,
            path,
            trove_id,
            original_hash: original_hash.clone(),
            current_hash: Some(original_hash),
            noreplace: false,
            status: ConfigStatus::Pristine,
            modified_at: None,
            source: ConfigSource::Auto,
        }
    }

    /// Create a config file with noreplace flag (like RPM %config(noreplace))
    pub fn new_noreplace(path: String, trove_id: i64, original_hash: String) -> Self {
        let mut config = Self::new(path, trove_id, original_hash);
        config.noreplace = true;
        config
    }

    /// Insert this config file into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO config_files (file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.file_id,
                &self.path,
                self.trove_id,
                &self.original_hash,
                &self.current_hash,
                self.noreplace as i32,
                self.status.as_str(),
                &self.modified_at,
                self.source.as_str(),
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or update config file
    pub fn upsert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO config_files (file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(path) DO UPDATE SET
                file_id = excluded.file_id,
                trove_id = excluded.trove_id,
                original_hash = excluded.original_hash,
                current_hash = excluded.current_hash,
                noreplace = excluded.noreplace,
                status = excluded.status,
                modified_at = excluded.modified_at,
                source = excluded.source",
            params![
                self.file_id,
                &self.path,
                self.trove_id,
                &self.original_hash,
                &self.current_hash,
                self.noreplace as i32,
                self.status.as_str(),
                &self.modified_at,
                self.source.as_str(),
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a config file by path
    pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source
             FROM config_files WHERE path = ?1",
        )?;

        let config = stmt.query_row([path], Self::from_row).optional()?;
        Ok(config)
    }

    /// Find a config file by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source
             FROM config_files WHERE id = ?1",
        )?;

        let config = stmt.query_row([id], Self::from_row).optional()?;
        Ok(config)
    }

    /// Find all config files for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source
             FROM config_files WHERE trove_id = ?1 ORDER BY path",
        )?;

        let configs = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(configs)
    }

    /// Find all config files with a given status
    pub fn find_by_status(conn: &Connection, status: ConfigStatus) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source
             FROM config_files WHERE status = ?1 ORDER BY path",
        )?;

        let configs = stmt
            .query_map([status.as_str()], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(configs)
    }

    /// Find all modified config files
    pub fn find_modified(conn: &Connection) -> Result<Vec<Self>> {
        Self::find_by_status(conn, ConfigStatus::Modified)
    }

    /// List all config files
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_id, path, trove_id, original_hash, current_hash, noreplace, status, modified_at, source
             FROM config_files ORDER BY path",
        )?;

        let configs = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(configs)
    }

    /// Update the status and current hash
    pub fn update_status(&self, conn: &Connection, status: ConfigStatus, current_hash: Option<&str>) -> Result<()> {
        let modified_at = if status == ConfigStatus::Modified {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        conn.execute(
            "UPDATE config_files SET status = ?1, current_hash = ?2, modified_at = ?3 WHERE id = ?4",
            params![status.as_str(), current_hash, modified_at, self.id],
        )?;

        Ok(())
    }

    /// Mark as pristine (unchanged from package)
    pub fn mark_pristine(&self, conn: &Connection, hash: &str) -> Result<()> {
        self.update_status(conn, ConfigStatus::Pristine, Some(hash))
    }

    /// Mark as modified (user changed)
    pub fn mark_modified(&self, conn: &Connection, hash: &str) -> Result<()> {
        self.update_status(conn, ConfigStatus::Modified, Some(hash))
    }

    /// Mark as missing (file deleted)
    pub fn mark_missing(&self, conn: &Connection) -> Result<()> {
        self.update_status(conn, ConfigStatus::Missing, None)
    }

    /// Delete a config file entry
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM config_files WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Delete all config files for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM config_files WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Check if user has modified this config file
    pub fn is_modified(&self) -> bool {
        self.status == ConfigStatus::Modified
    }

    /// Check if this file should preserve user changes on upgrade
    pub fn should_preserve(&self) -> bool {
        self.noreplace && self.is_modified()
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let status_str: String = row.get(7)?;
        let source_str: String = row.get(9)?;

        Ok(Self {
            id: Some(row.get(0)?),
            file_id: row.get(1)?,
            path: row.get(2)?,
            trove_id: row.get(3)?,
            original_hash: row.get(4)?,
            current_hash: row.get(5)?,
            noreplace: row.get::<_, i32>(6)? != 0,
            status: ConfigStatus::parse(&status_str).unwrap_or(ConfigStatus::Pristine),
            modified_at: row.get(8)?,
            source: ConfigSource::parse(&source_str).unwrap_or(ConfigSource::Auto),
        })
    }
}

/// A backup of a configuration file
#[derive(Debug, Clone)]
pub struct ConfigBackup {
    pub id: Option<i64>,
    /// Reference to config_files table
    pub config_file_id: i64,
    /// Hash of backed-up content (stored in CAS)
    pub backup_hash: String,
    /// Reason: upgrade, restore, manual
    pub reason: String,
    /// Changeset that triggered this backup
    pub changeset_id: Option<i64>,
    /// When the backup was created
    pub created_at: Option<String>,
}

impl ConfigBackup {
    /// Create a new backup entry
    pub fn new(config_file_id: i64, backup_hash: String, reason: String) -> Self {
        Self {
            id: None,
            config_file_id,
            backup_hash,
            reason,
            changeset_id: None,
            created_at: None,
        }
    }

    /// Create a backup for an upgrade
    pub fn for_upgrade(config_file_id: i64, backup_hash: String, changeset_id: i64) -> Self {
        let mut backup = Self::new(config_file_id, backup_hash, "upgrade".to_string());
        backup.changeset_id = Some(changeset_id);
        backup
    }

    /// Insert this backup into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO config_backups (config_file_id, backup_hash, reason, changeset_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.config_file_id,
                &self.backup_hash,
                &self.reason,
                self.changeset_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find all backups for a config file
    pub fn find_by_config_file(conn: &Connection, config_file_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, config_file_id, backup_hash, reason, changeset_id, created_at
             FROM config_backups WHERE config_file_id = ?1 ORDER BY created_at DESC",
        )?;

        let backups = stmt
            .query_map([config_file_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(backups)
    }

    /// Find the most recent backup for a config file
    pub fn find_latest(conn: &Connection, config_file_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, config_file_id, backup_hash, reason, changeset_id, created_at
             FROM config_backups WHERE config_file_id = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;

        let backup = stmt.query_row([config_file_id], Self::from_row).optional()?;
        Ok(backup)
    }

    /// Find backups by changeset
    pub fn find_by_changeset(conn: &Connection, changeset_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, config_file_id, backup_hash, reason, changeset_id, created_at
             FROM config_backups WHERE changeset_id = ?1 ORDER BY created_at DESC",
        )?;

        let backups = stmt
            .query_map([changeset_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(backups)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            config_file_id: row.get(1)?,
            backup_hash: row.get(2)?,
            reason: row.get(3)?,
            changeset_id: row.get(4)?,
            created_at: row.get(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();

        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );

            CREATE TABLE files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL
            );

            CREATE TABLE changesets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                description TEXT
            );

            CREATE TABLE config_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
                original_hash TEXT NOT NULL,
                current_hash TEXT,
                noreplace INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'pristine',
                modified_at TEXT,
                source TEXT DEFAULT 'auto',
                UNIQUE(path)
            );

            CREATE TABLE config_backups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                config_file_id INTEGER NOT NULL REFERENCES config_files(id) ON DELETE CASCADE,
                backup_hash TEXT NOT NULL,
                reason TEXT NOT NULL,
                changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            INSERT INTO troves (name) VALUES ('nginx');
            ",
        )
        .unwrap();

        (temp_file, conn)
    }

    #[test]
    fn test_config_file_crud() {
        let (_temp, conn) = create_test_db();

        let mut config = ConfigFile::new(
            "/etc/nginx/nginx.conf".to_string(),
            1,
            "abc123".to_string(),
        );
        config.insert(&conn).unwrap();

        assert!(config.id.is_some());

        let found = ConfigFile::find_by_path(&conn, "/etc/nginx/nginx.conf")
            .unwrap()
            .unwrap();
        assert_eq!(found.path, "/etc/nginx/nginx.conf");
        assert_eq!(found.original_hash, "abc123");
        assert_eq!(found.status, ConfigStatus::Pristine);
    }

    #[test]
    fn test_config_file_status_update() {
        let (_temp, conn) = create_test_db();

        let mut config = ConfigFile::new(
            "/etc/nginx/nginx.conf".to_string(),
            1,
            "abc123".to_string(),
        );
        config.insert(&conn).unwrap();

        // Mark as modified
        config.mark_modified(&conn, "def456").unwrap();

        let found = ConfigFile::find_by_id(&conn, config.id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.status, ConfigStatus::Modified);
        assert_eq!(found.current_hash.as_deref(), Some("def456"));
        assert!(found.modified_at.is_some());
    }

    #[test]
    fn test_config_file_noreplace() {
        let (_temp, conn) = create_test_db();

        let mut config = ConfigFile::new_noreplace(
            "/etc/myapp/config.conf".to_string(),
            1,
            "abc123".to_string(),
        );
        config.insert(&conn).unwrap();

        let found = ConfigFile::find_by_path(&conn, "/etc/myapp/config.conf")
            .unwrap()
            .unwrap();
        assert!(found.noreplace);
        assert!(!found.should_preserve()); // Not modified yet

        // Now modify it
        found.mark_modified(&conn, "def456").unwrap();
        let found = ConfigFile::find_by_path(&conn, "/etc/myapp/config.conf")
            .unwrap()
            .unwrap();
        assert!(found.should_preserve()); // Now should preserve
    }

    #[test]
    fn test_config_backup_crud() {
        let (_temp, conn) = create_test_db();

        let mut config = ConfigFile::new(
            "/etc/nginx/nginx.conf".to_string(),
            1,
            "abc123".to_string(),
        );
        config.insert(&conn).unwrap();

        let mut backup = ConfigBackup::new(
            config.id.unwrap(),
            "abc123".to_string(),
            "manual".to_string(),
        );
        backup.insert(&conn).unwrap();

        let backups = ConfigBackup::find_by_config_file(&conn, config.id.unwrap()).unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].reason, "manual");
    }

    #[test]
    fn test_find_modified_configs() {
        let (_temp, conn) = create_test_db();

        // Create pristine config
        let mut config1 = ConfigFile::new(
            "/etc/nginx/nginx.conf".to_string(),
            1,
            "abc123".to_string(),
        );
        config1.insert(&conn).unwrap();

        // Create modified config
        let mut config2 = ConfigFile::new(
            "/etc/nginx/proxy.conf".to_string(),
            1,
            "def456".to_string(),
        );
        config2.status = ConfigStatus::Modified;
        config2.insert(&conn).unwrap();

        let modified = ConfigFile::find_modified(&conn).unwrap();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].path, "/etc/nginx/proxy.conf");
    }
}
