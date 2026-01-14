// src/db/models/trove.rs

//! Trove model - the core package/component/collection type

use crate::error::Result;
use crate::flavor::FlavorSpec;
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
    /// Conary-style flavor specification (e.g., `[ssl, !debug, is: x86_64]`)
    pub flavor_spec: Option<String>,
    /// Whether this package is pinned (protected from updates/removal)
    pub pinned: bool,
    /// Human-readable reason for installation (e.g., "Required by nginx", "Installed via @server")
    pub selection_reason: Option<String>,
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
            flavor_spec: None,
            pinned: false,
            selection_reason: Some("Explicitly installed".to_string()),
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
            flavor_spec: None,
            pinned: false,
            selection_reason: Some("Explicitly installed".to_string()),
        }
    }

    /// Create a Trove installed as a dependency of another package
    pub fn new_as_dependency(
        name: String,
        version: String,
        trove_type: TroveType,
        required_by: &str,
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
            install_source: InstallSource::Repository,
            install_reason: InstallReason::Dependency,
            flavor_spec: None,
            pinned: false,
            selection_reason: Some(format!("Required by {}", required_by)),
        }
    }

    /// Create a Trove installed via a collection
    pub fn new_from_collection(
        name: String,
        version: String,
        trove_type: TroveType,
        collection_name: &str,
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
            install_source: InstallSource::Repository,
            install_reason: InstallReason::Explicit,
            flavor_spec: None,
            pinned: false,
            selection_reason: Some(format!("Installed via @{}", collection_name)),
        }
    }

    /// Insert this trove into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, description, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &self.name,
                &self.version,
                self.trove_type.as_str(),
                &self.architecture,
                &self.description,
                &self.installed_by_changeset_id,
                self.install_source.as_str(),
                self.install_reason.as_str(),
                &self.flavor_spec,
                self.pinned,
                &self.selection_reason,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a trove by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason FROM troves WHERE id = ?1")?;

        let trove = stmt.query_row([id], Self::from_row).optional()?;

        Ok(trove)
    }

    /// Find troves by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason FROM troves WHERE name = ?1")?;

        let troves = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// List all troves
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt =
            conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason FROM troves ORDER BY name, version")?;

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
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec
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

        // flavor_spec is nullable
        let flavor_spec: Option<String> = row.get(10)?;

        // Handle pinned with default for older databases
        let pinned: i32 = row.get(11).unwrap_or(0);

        // Handle selection_reason (added in v16)
        let selection_reason: Option<String> = row.get(12).unwrap_or(None);

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
            flavor_spec,
            pinned: pinned != 0,
            selection_reason,
        })
    }

    /// Parse the flavor specification into a `FlavorSpec`
    ///
    /// Returns `None` if no flavor is set or if parsing fails.
    pub fn flavor(&self) -> Option<FlavorSpec> {
        self.flavor_spec.as_ref().and_then(|s| s.parse().ok())
    }

    /// Set the flavor specification from a `FlavorSpec`
    ///
    /// The flavor is canonicalized before storing to ensure consistent
    /// storage and comparison.
    pub fn set_flavor(&mut self, flavor: &FlavorSpec) {
        let mut canonical = flavor.clone();
        canonical.canonicalize();
        self.flavor_spec = Some(canonical.to_string());
    }

    /// Pin a package to prevent updates/removal
    pub fn pin(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("UPDATE troves SET pinned = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Unpin a package to allow updates/removal
    pub fn unpin(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("UPDATE troves SET pinned = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Find all pinned packages
    pub fn find_pinned(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason FROM troves WHERE pinned = 1 ORDER BY name, version"
        )?;

        let troves = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// Check if a package is pinned by name
    pub fn is_pinned_by_name(conn: &Connection, name: &str) -> Result<bool> {
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM troves WHERE name = ?1 AND pinned = 1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find troves by selection reason pattern
    ///
    /// Supports patterns like:
    /// - "Required by *" - packages installed as dependencies
    /// - "Installed via @*" - packages installed via collections
    /// - "Explicitly installed" - packages installed directly
    pub fn find_by_reason(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        // Convert glob-style pattern to SQL LIKE pattern
        let sql_pattern = pattern.replace('*', "%");

        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason
             FROM troves
             WHERE selection_reason LIKE ?1
             ORDER BY name, version"
        )?;

        let troves = stmt
            .query_map([sql_pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(troves)
    }

    /// Find all packages installed as dependencies
    pub fn find_dependencies_installed(conn: &Connection) -> Result<Vec<Self>> {
        Self::find_by_reason(conn, "Required by *")
    }

    /// Find all packages installed via collections
    pub fn find_collection_installed(conn: &Connection) -> Result<Vec<Self>> {
        Self::find_by_reason(conn, "Installed via @*")
    }

    /// Find all explicitly installed packages
    pub fn find_explicitly_installed(conn: &Connection) -> Result<Vec<Self>> {
        Self::find_by_reason(conn, "Explicitly installed")
    }
}
