// conary-core/src/db/models/trove.rs

//! Trove model - the core package/component/collection type

use crate::error::Result;
use crate::flavor::FlavorSpec;
use rusqlite::{Connection, OptionalExtension, Row, params};
use strum_macros::{AsRefStr, Display, EnumString};
use tracing::warn;

/// Type of trove (package, component, collection, or redirect)
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum TroveType {
    Package,
    Component,
    Collection,
    /// A redirect points to another package (for renames, obsoletes, etc.)
    Redirect,
}

impl TroveType {
    /// Get string representation (for backwards compatibility)
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

/// Source of package installation
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum InstallSource {
    /// Installed from local package file
    File,
    /// Installed from Conary repository
    Repository,
    /// Adopted from system, metadata only (files not in CAS)
    AdoptedTrack,
    /// Adopted from system with full CAS storage
    AdoptedFull,
    /// Taken over from system PM. Conary fully owns files.
    Taken,
}

impl InstallSource {
    /// Get string representation (for backwards compatibility)
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    pub fn is_adopted(&self) -> bool {
        matches!(
            self,
            InstallSource::AdoptedTrack | InstallSource::AdoptedFull
        )
    }

    /// Returns true if Conary fully owns the package files (not just tracking)
    pub fn is_conary_owned(&self) -> bool {
        matches!(
            self,
            InstallSource::File | InstallSource::Repository | InstallSource::Taken
        )
    }
}

/// Reason why a package was installed
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum InstallReason {
    /// User explicitly requested this package
    Explicit,
    /// Installed automatically as a dependency of another package
    Dependency,
}

impl InstallReason {
    /// Get string representation (for backwards compatibility)
    pub fn as_str(&self) -> &str {
        self.as_ref()
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
    /// Label ID for package provenance tracking (repository@namespace:tag)
    pub label_id: Option<i64>,
    /// When this package became orphaned (no longer required by any explicit package).
    /// NULL means not orphaned. Used for grace period policies.
    pub orphan_since: Option<String>,
    /// Distro identity the installed package originally came from.
    pub source_distro: Option<String>,
    /// Native version scheme for the installed package (rpm, debian, arch).
    pub version_scheme: Option<String>,
}

impl Trove {
    /// Column list for SELECT queries.
    pub(crate) const COLUMNS: &'static str = "id, name, version, type, architecture, description, \
         installed_at, installed_by_changeset_id, install_source, install_reason, \
         flavor_spec, pinned, selection_reason, label_id, orphan_since, source_distro, \
         version_scheme";

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
            label_id: None,
            orphan_since: None,
            source_distro: None,
            version_scheme: None,
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
            label_id: None,
            orphan_since: None,
            source_distro: None,
            version_scheme: None,
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
            label_id: None,
            orphan_since: None,
            source_distro: None,
            version_scheme: None,
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
            label_id: None,
            orphan_since: None,
            source_distro: None,
            version_scheme: None,
        }
    }

    /// Insert this trove into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, description, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason, label_id, source_distro, version_scheme)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                &self.label_id,
                &self.source_distro,
                &self.version_scheme,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find a trove by ID
    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!("SELECT {} FROM troves WHERE id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let trove = stmt.query_row([id], Self::from_row).optional()?;
        Ok(trove)
    }

    /// Find troves by name
    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM troves WHERE name = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let troves = stmt
            .query_map([name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(troves)
    }

    /// List all troves
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM troves ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let troves = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(troves)
    }

    /// Find orphaned packages (installed as dependency, no longer needed)
    pub fn find_orphans(conn: &Connection) -> Result<Vec<Self>> {
        // Find packages that:
        // 1. Were installed as dependencies (not explicitly)
        // 2. Are not transitively reachable from any explicitly-installed package
        let sql = format!(
            "WITH RECURSIVE reachable(name) AS ( \
                 SELECT DISTINCT depends_on_name FROM dependencies \
                 WHERE trove_id IN (SELECT id FROM troves WHERE install_reason = 'explicit') \
                 UNION \
                 SELECT DISTINCT d.depends_on_name FROM dependencies d \
                 JOIN troves t ON d.trove_id = t.id \
                 JOIN reachable r ON t.name = r.name \
             ) \
             SELECT {} FROM troves \
             WHERE install_reason = 'dependency' \
             AND name NOT IN (SELECT name FROM reachable) \
             ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
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
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
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
        let install_source = match &source_str {
            Some(s) => match s.parse::<InstallSource>() {
                Ok(src) => src,
                Err(_) => {
                    warn!(
                        value = %s,
                        "Trove::from_row: failed to parse install_source, falling back to File"
                    );
                    InstallSource::File
                }
            },
            None => {
                warn!("Trove::from_row: missing install_source, falling back to File");
                InstallSource::File
            }
        };

        // Handle install_reason with default for older databases
        let reason_str: Option<String> = row.get(9)?;
        let install_reason = match &reason_str {
            Some(s) => match s.parse::<InstallReason>() {
                Ok(reason) => reason,
                Err(_) => {
                    warn!(
                        value = %s,
                        "Trove::from_row: failed to parse install_reason, falling back to Explicit"
                    );
                    InstallReason::Explicit
                }
            },
            None => {
                warn!("Trove::from_row: missing install_reason, falling back to Explicit");
                InstallReason::Explicit
            }
        };

        // flavor_spec is nullable
        let flavor_spec: Option<String> = row.get(10)?;

        // Handle pinned with default for older databases
        let pinned: i32 = row.get(11).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read pinned column, falling back to 0");
            0
        });

        // Handle selection_reason (added in v16)
        let selection_reason: Option<String> = row.get(12).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read selection_reason column, falling back to None");
            None
        });

        // Handle label_id (added in v20)
        let label_id: Option<i64> = row.get(13).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read label_id column, falling back to None");
            None
        });

        // Handle orphan_since (added in v39)
        let orphan_since: Option<String> = row.get(14).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read orphan_since column, falling back to None");
            None
        });

        let source_distro: Option<String> = row.get(15).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read source_distro column, falling back to None");
            None
        });

        let version_scheme: Option<String> = row.get(16).unwrap_or_else(|_| {
            warn!("Trove::from_row: failed to read version_scheme column, falling back to None");
            None
        });

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
            label_id,
            orphan_since,
            source_distro,
            version_scheme,
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

    pub fn update_source_identity(
        conn: &Connection,
        id: i64,
        source_distro: Option<&str>,
        version_scheme: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "UPDATE troves
             SET source_distro = ?1,
                 version_scheme = ?2
             WHERE id = ?3",
            params![source_distro, version_scheme, id],
        )?;
        Ok(())
    }

    /// Find all pinned packages
    pub fn find_pinned(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM troves WHERE pinned = 1 ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
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
        let sql = format!(
            "SELECT {} FROM troves WHERE selection_reason LIKE ?1 ORDER BY name, version",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
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

    /// Promote a dependency to explicit installation
    ///
    /// If the package is currently installed as a dependency, this updates it
    /// to be marked as explicitly installed. This prevents autoremove from
    /// removing it when the original requiring package is removed.
    ///
    /// Returns `Ok(true)` if the package was promoted, `Ok(false)` if it was
    /// already explicit or not found.
    pub fn promote_to_explicit(
        conn: &Connection,
        name: &str,
        reason: Option<&str>,
    ) -> Result<bool> {
        let rows = conn.execute(
            "UPDATE troves
             SET install_reason = 'explicit',
                 selection_reason = ?1
             WHERE name = ?2
               AND install_reason = 'dependency'
               AND type = 'package'",
            rusqlite::params![reason.unwrap_or("Explicitly installed"), name,],
        )?;
        Ok(rows > 0)
    }

    /// Find a single trove by name (returns the first match if multiple exist)
    pub fn find_one_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let troves = Self::find_by_name(conn, name)?;
        Ok(troves.into_iter().next())
    }

    /// Find adopted troves that have not been converted to CCS format
    ///
    /// Returns troves with install_source of 'adopted-track' or 'adopted-full'
    /// that do not have a corresponding entry in the converted_packages table.
    pub fn find_adopted_unconverted(conn: &Connection) -> Result<Vec<Self>> {
        // Prefix each column with `t.` for the JOIN query
        let prefixed: String = Self::COLUMNS
            .split(", ")
            .map(|c| format!("t.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {prefixed} FROM troves t \
             LEFT JOIN converted_packages cp ON cp.trove_id = t.id \
             WHERE t.install_source IN ('adopted-track', 'adopted-full') \
             AND cp.id IS NULL \
             ORDER BY t.name"
        );
        let mut stmt = conn.prepare(&sql)?;
        let troves = stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(troves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> (tempfile::TempDir, Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open(&db_path).unwrap();
        (temp_dir, conn)
    }

    #[test]
    fn trove_round_trips_source_identity() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new_with_source(
            "bash".to_string(),
            "5.2.37-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedTrack,
        );
        trove.source_distro = Some("arch".to_string());
        trove.version_scheme = Some("arch".to_string());

        let trove_id = trove.insert(&conn).unwrap();
        let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();

        assert_eq!(loaded.source_distro.as_deref(), Some("arch"));
        assert_eq!(loaded.version_scheme.as_deref(), Some("arch"));
    }

    #[test]
    fn trove_update_source_identity_backfills_existing_rows() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new(
            "bash".to_string(),
            "5.2.37-1".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        Trove::update_source_identity(&conn, trove_id, Some("fedora-43"), Some("rpm")).unwrap();

        let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
        assert_eq!(loaded.source_distro.as_deref(), Some("fedora-43"));
        assert_eq!(loaded.version_scheme.as_deref(), Some("rpm"));
    }
}
