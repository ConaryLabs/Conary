// src/db/models/trove.rs

//! Trove model - the core package/component/collection type

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
            _ => Err(format!("Invalid trove type: {s}")),
        }
    }
}

/// Source of package installation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSource {
    /// Installed from local package file
    File,
    /// Installed from Conary repository
    Repository,
    /// Adopted from system, metadata only (files not in CAS)
    AdoptedTrack,
    /// Adopted from system with full CAS storage
    AdoptedFull,
}

impl InstallSource {
    pub fn as_str(&self) -> &str {
        match self {
            InstallSource::File => "file",
            InstallSource::Repository => "repository",
            InstallSource::AdoptedTrack => "adopted-track",
            InstallSource::AdoptedFull => "adopted-full",
        }
    }

    pub fn is_adopted(&self) -> bool {
        matches!(self, InstallSource::AdoptedTrack | InstallSource::AdoptedFull)
    }
}

impl FromStr for InstallSource {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "file" => Ok(InstallSource::File),
            "repository" => Ok(InstallSource::Repository),
            "adopted-track" => Ok(InstallSource::AdoptedTrack),
            "adopted-full" => Ok(InstallSource::AdoptedFull),
            _ => Err(format!("Invalid install source: {s}")),
        }
    }
}

/// Reason why a package was installed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallReason {
    /// User explicitly requested this package
    Explicit,
    /// Installed automatically as a dependency of another package
    Dependency,
}

impl InstallReason {
    pub fn as_str(&self) -> &str {
        match self {
            InstallReason::Explicit => "explicit",
            InstallReason::Dependency => "dependency",
        }
    }
}

impl FromStr for InstallReason {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "explicit" => Ok(InstallReason::Explicit),
            "dependency" => Ok(InstallReason::Dependency),
            _ => Err(format!("Invalid install reason: {s}")),
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
    pub install_source: InstallSource,
    pub install_reason: InstallReason,
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
            install_source: InstallSource::File,
            install_reason: InstallReason::Explicit,
        }
    }

    /// Create a new Trove with a specific install source
    pub fn new_with_source(
        name: String,
        version: String,
        trove_type: TroveType,
        install_source: InstallSource,
    ) -> Self {
        Self {
            id: None,
            name,
            version,
            trove_type,
            architecture: None,
            description: None,
            installed_at: None,
            installed_by_changeset_id: None,
            install_source,
            install_reason: InstallReason::Explicit,
        }
    }

    /// Insert this trove into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, description, installed_by_changeset_id, install_source, install_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &self.name,
                &self.version,
                self.trove_type.as_str(),
                &self.architecture,
                &self.description,
                &self.installed_by_changeset_id,
                self.install_source.as_str(),
                self.install_reason.as_str(),
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a trove by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason FROM troves WHERE id = ?1")?;

        let trove = stmt.query_row([id], Self::from_row).optional()?;

        Ok(trove)
    }

    /// Find troves by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason FROM troves WHERE name = ?1")?;

        let troves = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// List all troves
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason FROM troves ORDER BY name, version")?;

        let troves = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// Find orphaned packages (installed as dependency, no longer needed)
    pub fn find_orphans(conn: &Connection) -> Result<Vec<Self>> {
        // Find packages that:
        // 1. Were installed as dependencies (not explicitly)
        // 2. Have no other packages depending on them
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason
             FROM troves
             WHERE install_reason = 'dependency'
             AND name NOT IN (
                 SELECT DISTINCT depends_on_name FROM dependencies
                 WHERE trove_id IN (SELECT id FROM troves)
             )
             ORDER BY name, version"
        )?;

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
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let type_str: String = row.get(3)?;
        let trove_type = type_str.parse::<TroveType>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        // Handle install_source with default for older databases
        let source_str: Option<String> = row.get(8)?;
        let install_source = source_str
            .and_then(|s| s.parse::<InstallSource>().ok())
            .unwrap_or(InstallSource::File);

        // Handle install_reason with default for older databases
        let reason_str: Option<String> = row.get(9)?;
        let install_reason = reason_str
            .and_then(|s| s.parse::<InstallReason>().ok())
            .unwrap_or(InstallReason::Explicit);

        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            version: row.get(2)?,
            trove_type,
            architecture: row.get(4)?,
            description: row.get(5)?,
            installed_at: row.get(6)?,
            installed_by_changeset_id: row.get(7)?,
            install_source,
            install_reason,
        })
    }
}
