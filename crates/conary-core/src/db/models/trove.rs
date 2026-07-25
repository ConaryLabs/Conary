// conary-core/src/db/models/trove.rs

//! Trove model - the core package/component/collection type

use crate::error::{Error, Result};
use crate::flavor::FlavorSpec;
use crate::packages::InstalledPackageIdentity;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, OptionalExtension, Row, params};
use strum_macros::{AsRefStr, Display, EnumString};

use super::repository::version_scheme_from_row;

/// Type of trove (package, component, or collection)
#[derive(Debug, Clone, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum TroveType {
    Package,
    Component,
    Collection,
}

impl TroveType {
    /// Return the persisted database representation.
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
    /// Synthetic full-root CAS capture with no native package-manager record.
    CapturedRoot,
}

impl InstallSource {
    /// Return the persisted database representation.
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
            InstallSource::File
                | InstallSource::Repository
                | InstallSource::Taken
                | InstallSource::CapturedRoot
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
    /// Return the persisted database representation.
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
    /// Exact version grammar and comparison authority for this installed package.
    pub version_scheme: VersionScheme,
    /// Exact native package-manager record identity for adopted or taken packages.
    pub native_package_identity: Option<InstalledPackageIdentity>,
    /// Repository this package was installed from (for provenance/affinity).
    pub installed_from_repository_id: Option<i64>,
}

impl Trove {
    /// Column list for SELECT queries.
    pub(crate) const COLUMNS: &'static str = "id, name, version, type, architecture, description, \
         installed_at, installed_by_changeset_id, install_source, install_reason, \
         flavor_spec, pinned, selection_reason, label_id, orphan_since, source_distro, \
         version_scheme, native_package_identity_json, installed_from_repository_id";

    /// Create a new Trove
    pub fn new(
        name: String,
        version: String,
        trove_type: TroveType,
        version_scheme: VersionScheme,
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
            install_source: InstallSource::File,
            install_reason: InstallReason::Explicit,
            flavor_spec: None,
            pinned: false,
            selection_reason: Some("Explicitly installed".to_string()),
            label_id: None,
            orphan_since: None,
            source_distro: None,
            version_scheme,
            native_package_identity: None,
            installed_from_repository_id: None,
        }
    }

    /// Create a new Trove with a specific install source
    pub fn new_with_source(
        name: String,
        version: String,
        trove_type: TroveType,
        install_source: InstallSource,
        version_scheme: VersionScheme,
    ) -> Self {
        let mut trove = Self::new(name, version, trove_type, version_scheme);
        trove.install_source = install_source;
        trove
    }

    /// Insert this trove into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        let native_package_identity_json = self.validated_native_identity_json()?;
        conn.execute(
            "INSERT INTO troves (name, version, type, architecture, description, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason, label_id, source_distro, version_scheme, native_package_identity_json, installed_from_repository_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                self.version_scheme.as_str(),
                &native_package_identity_json,
                &self.installed_from_repository_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    fn validated_native_identity_json(&self) -> Result<Option<String>> {
        let native_source = matches!(
            self.install_source,
            InstallSource::AdoptedTrack | InstallSource::AdoptedFull | InstallSource::Taken
        );
        match (&self.native_package_identity, native_source) {
            (Some(identity), true) => {
                identity.validate()?;
                if identity.version_scheme() != self.version_scheme {
                    return Err(Error::ConfigError(format!(
                        "{} {} native package identity uses {} versioning, not the trove's {} scheme",
                        self.name,
                        self.version,
                        identity.version_scheme().as_str(),
                        self.version_scheme.as_str()
                    )));
                }
                if identity.name() != self.name {
                    return Err(Error::ConfigError(format!(
                        "{} {} native package identity names {}, not the owning trove",
                        self.name,
                        self.version,
                        identity.name()
                    )));
                }
                if identity.version() != self.version {
                    return Err(Error::ConfigError(format!(
                        "{} {} native package identity has canonical version {}",
                        self.name,
                        self.version,
                        identity.version()
                    )));
                }
                if Some(identity.architecture()) != self.architecture.as_deref() {
                    return Err(Error::ConfigError(format!(
                        "{} {} native package identity architecture {} does not match trove architecture {:?}",
                        self.name,
                        self.version,
                        identity.architecture(),
                        self.architecture
                    )));
                }
                serde_json::to_string(identity)
                    .map(Some)
                    .map_err(Into::into)
            }
            (None, false) => Ok(None),
            (None, true) => Err(Error::ConfigError(format!(
                "{} {} uses install source {} without an exact native package identity",
                self.name, self.version, self.install_source
            ))),
            (Some(_), false) => Err(Error::ConfigError(format!(
                "{} {} uses install source {} but carries a native package identity",
                self.name, self.version, self.install_source
            ))),
        }
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

    /// List only troves of type `package` (excludes components and collections).
    pub fn list_packages(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM troves WHERE type = 'package' ORDER BY name, version",
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
        let installed = crate::resolver::requirements::load_installed_package_identities(conn)?;
        let requirements =
            super::installed_requirement_group::InstalledRequirementGroup::list_all(conn)?;
        let mut orphans = Vec::new();

        for trove in Self::list_packages(conn)?
            .into_iter()
            .filter(|trove| trove.install_reason == InstallReason::Dependency)
        {
            let Some(trove_id) = trove.id else {
                continue;
            };
            let remaining = installed
                .iter()
                .filter(|package| package.installed_trove_id != Some(trove_id))
                .cloned()
                .collect::<Vec<_>>();
            let mut required = false;

            for group in requirements.iter().filter(|group| {
                group.trove_id != trove_id
                    && matches!(
                        group.kind,
                        crate::repository::dependency_model::RepositoryRequirementKind::Depends
                            | crate::repository::dependency_model::RepositoryRequirementKind::PreDepends
                    )
            }) {
                let satisfied_before =
                    crate::resolver::requirements::requirement_expression_satisfied(
                        &group.requirement.expression,
                        group.version_scheme,
                        &installed,
                    )?;
                let satisfied_after =
                    crate::resolver::requirements::requirement_expression_satisfied(
                        &group.requirement.expression,
                        group.version_scheme,
                        &remaining,
                    )?;
                if satisfied_before && !satisfied_after {
                    required = true;
                    break;
                }
            }

            if !required {
                orphans.push(trove);
            }
        }

        orphans.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(orphans)
    }

    /// Delete a trove by ID
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM troves WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a Trove
    ///
    /// The current schema epoch guarantees all columns exist.
    pub fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let type_str: String = row.get(3)?;
        let trove_type = type_str.parse::<TroveType>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        let source_str: String = row.get(8)?;
        let install_source = source_str.parse::<InstallSource>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        let reason_str: String = row.get(9)?;
        let install_reason = reason_str.parse::<InstallReason>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            )
        })?;

        let native_identity_json: Option<String> = row.get(17)?;
        let native_package_identity = native_identity_json
            .as_deref()
            .map(serde_json::from_str::<InstalledPackageIdentity>)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    17,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
        if let Some(identity) = &native_package_identity {
            identity.validate().map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    17,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error.to_string(),
                    )),
                )
            })?;
        }

        let trove = Self {
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
            flavor_spec: row.get(10)?,
            pinned: row.get::<_, i32>(11)? != 0,
            selection_reason: row.get(12)?,
            label_id: row.get(13)?,
            orphan_since: row.get(14)?,
            source_distro: row.get(15)?,
            version_scheme: version_scheme_from_row(row, 16)?,
            native_package_identity,
            installed_from_repository_id: row.get(18)?,
        };
        trove.validated_native_identity_json().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                17,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.to_string(),
                )),
            )
        })?;
        Ok(trove)
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

    pub fn update_replatform_metadata(
        conn: &Connection,
        id: i64,
        source_distro: &str,
        version_scheme: VersionScheme,
        installed_from_repository_id: i64,
        selection_reason: &str,
    ) -> Result<()> {
        conn.execute(
            "UPDATE troves
             SET source_distro = ?1,
                 version_scheme = ?2,
                 installed_from_repository_id = ?3,
                 selection_reason = ?4
             WHERE id = ?5",
            params![
                source_distro,
                version_scheme.as_str(),
                installed_from_repository_id,
                selection_reason,
                id
            ],
        )?;
        Ok(())
    }

    pub fn update_selection_reason(
        conn: &Connection,
        id: i64,
        selection_reason: &str,
    ) -> Result<()> {
        conn.execute(
            "UPDATE troves
             SET selection_reason = ?1
             WHERE id = ?2",
            params![selection_reason, id],
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
    ///
    /// Note: The patterns passed to this function are developer-controlled
    /// (e.g., "Required by *"), not raw user input. The `*` glob is
    /// converted to SQL `%` for LIKE matching. Do not pass unsanitized
    /// user input directly.
    pub fn find_by_reason(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        // Convert glob-style pattern to SQL LIKE pattern.
        // Callers pass fixed patterns ("Required by *"), not user input,
        // so we do not need to escape `%` or `_` in the pattern itself.
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
        trove_id: i64,
        reason: Option<&str>,
    ) -> Result<bool> {
        let rows = conn.execute(
            "UPDATE troves
             SET install_reason = 'explicit',
                 selection_reason = ?1
             WHERE id = ?2
               AND install_reason = 'dependency'
               AND type = 'package'",
            rusqlite::params![reason.unwrap_or("Explicitly installed"), trove_id],
        )?;
        Ok(rows > 0)
    }

    /// Find the unique installed trove with this name.
    ///
    /// Name-only lookups never pick an arbitrary version or architecture.
    /// Callers that can encounter parallel variants must use an exact selector.
    pub fn find_one_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        let mut troves = Self::find_by_name(conn, name)?;
        match troves.len() {
            0 => Ok(None),
            1 => Ok(troves.pop()),
            count => Err(Error::ConflictError(format!(
                "package '{name}' has {count} installed variants; select version and architecture"
            ))),
        }
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
            VersionScheme::Arch,
        );
        trove.source_distro = Some("arch".to_string());
        trove.architecture = Some("x86_64".to_string());
        trove.native_package_identity =
            Some(InstalledPackageIdentity::pacman("bash", "bash", "5.2.37-1", "x86_64").unwrap());

        let trove_id = trove.insert(&conn).unwrap();
        let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();

        assert_eq!(loaded.source_distro.as_deref(), Some("arch"));
        assert_eq!(loaded.version_scheme, VersionScheme::Arch);
        assert_eq!(
            loaded
                .native_package_identity
                .as_ref()
                .map(InstalledPackageIdentity::selector),
            Some("bash")
        );
    }

    #[test]
    fn native_owned_trove_requires_exact_package_identity() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new_with_source(
            "bash".to_string(),
            "5.2.37-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            VersionScheme::Arch,
        );

        let error = trove.insert(&conn).unwrap_err().to_string();
        assert!(error.contains("without an exact native package identity"));
    }

    #[test]
    fn native_owned_trove_identity_must_match_version_scheme() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.8.0-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            VersionScheme::Debian,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.native_package_identity = Some(
            InstalledPackageIdentity::rpm(
                "curl-8.8.0-1.x86_64",
                "curl",
                None,
                "8.8.0",
                "1",
                "x86_64",
            )
            .unwrap(),
        );

        let error = trove.insert(&conn).unwrap_err().to_string();
        assert!(error.contains("identity uses rpm versioning"), "{error}");
        assert!(error.contains("trove's debian scheme"), "{error}");
    }

    #[test]
    fn native_owned_trove_identity_must_match_name_version_and_architecture() {
        let (_dir, conn) = setup_test_db();
        let identity =
            InstalledPackageIdentity::dpkg("libc6:i386", "libc6", "2.41-12", "i386").unwrap();

        for (name, version, architecture, expected) in [
            (
                "wrong-name",
                "2.41-12",
                Some("i386"),
                "identity names libc6",
            ),
            (
                "libc6",
                "2.41-11",
                Some("i386"),
                "canonical version 2.41-12",
            ),
            ("libc6", "2.41-12", Some("amd64"), "architecture i386"),
            ("libc6", "2.41-12", None, "architecture i386"),
        ] {
            let mut trove = Trove::new_with_source(
                name.to_string(),
                version.to_string(),
                TroveType::Package,
                InstallSource::AdoptedTrack,
                VersionScheme::Debian,
            );
            trove.architecture = architecture.map(str::to_string);
            trove.native_package_identity = Some(identity.clone());

            let error = trove.insert(&conn).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn new_trove_has_required_conary_version_scheme() {
        let trove = Trove::new(
            "bash".to_string(),
            "5.2.37-1".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        assert_eq!(trove.version_scheme, VersionScheme::Conary);
    }

    #[test]
    fn database_decode_rejects_unknown_installed_version_scheme() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new(
            "bash".to_string(),
            "5.2.37-1".to_string(),
            TroveType::Package,
            VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        conn.pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        conn.execute(
            "UPDATE troves SET version_scheme = 'unknown' WHERE id = ?1",
            [trove_id],
        )
        .unwrap();

        let error = Trove::find_by_id(&conn, trove_id).unwrap_err().to_string();
        assert!(
            error.contains("unsupported persisted native version scheme 'unknown'"),
            "{error}"
        );
    }

    #[test]
    fn trove_update_replatform_metadata_sets_all_provenance_fields() {
        let (_dir, conn) = setup_test_db();
        let mut repo = crate::db::models::Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();
        let mut trove = Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        Trove::update_replatform_metadata(
            &conn,
            trove_id,
            "arch",
            VersionScheme::Arch,
            repo_id,
            "Replatformed from fedora-44 to arch by model apply",
        )
        .unwrap();

        let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
        assert_eq!(loaded.source_distro.as_deref(), Some("arch"));
        assert_eq!(loaded.version_scheme, VersionScheme::Arch);
        assert_eq!(loaded.installed_from_repository_id, Some(repo_id));
        assert_eq!(
            loaded.selection_reason.as_deref(),
            Some("Replatformed from fedora-44 to arch by model apply")
        );
    }

    #[test]
    fn trove_update_selection_reason_overwrites_existing_reason() {
        let (_dir, conn) = setup_test_db();
        let mut trove = Trove::new(
            "vim".to_string(),
            "9.1.0".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();

        Trove::update_selection_reason(&conn, trove_id, "Replatform partial failure").unwrap();

        let loaded = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
        assert_eq!(
            loaded.selection_reason.as_deref(),
            Some("Replatform partial failure")
        );
    }

    #[test]
    fn test_taken_variant_roundtrip() {
        let taken = InstallSource::Taken;
        let s = taken.as_str();
        assert_eq!(s, "taken");
        let parsed: InstallSource = s.parse().unwrap();
        assert_eq!(parsed, InstallSource::Taken);
    }

    #[test]
    fn test_taken_is_conary_owned() {
        assert!(InstallSource::Taken.is_conary_owned());
        assert!(InstallSource::CapturedRoot.is_conary_owned());
        assert!(InstallSource::File.is_conary_owned());
        assert!(InstallSource::Repository.is_conary_owned());
        assert!(!InstallSource::AdoptedTrack.is_conary_owned());
        assert!(!InstallSource::AdoptedFull.is_conary_owned());
    }

    #[test]
    fn test_taken_is_not_adopted() {
        assert!(!InstallSource::Taken.is_adopted());
        assert!(!InstallSource::CapturedRoot.is_adopted());
    }
}
