// conary-core/src/db/models/provide_entry.rs

//! ProvideEntry model - capabilities that packages offer
//!
//! This tracks what each package "provides" - the capabilities it offers
//! that can satisfy dependencies. This enables self-contained dependency
//! resolution without querying the host package manager.

use crate::error::Result;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryCapabilityKind,
};
use crate::repository::versioning::{VersionScheme, validate_repo_version};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::repository::version_scheme_from_row;

/// A capability that a package provides
#[derive(Debug, Clone)]
pub struct ProvideEntry {
    pub id: Option<i64>,
    pub trove_id: i64,
    /// The capability name (package name, virtual provide, library, or file path)
    pub capability: String,
    /// Optional version of this capability
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub kind: RepositoryCapabilityKind,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
    pub provenance: CapabilityProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledProvideContract {
    pub package_name: String,
    pub package_version: String,
    pub capability_version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier: ProvideArchitectureQualifier,
    pub provenance: CapabilityProvenance,
}

impl ProvideEntry {
    /// Column list for SELECT queries.
    const COLUMNS: &'static str = "id, trove_id, capability, version, version_relation, kind, version_scheme, architecture_qualifier_kind, architecture_qualifier, provenance";

    /// Create a new ProvideEntry
    pub fn new(
        trove_id: i64,
        capability: String,
        version: Option<String>,
        version_scheme: VersionScheme,
    ) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            id: None,
            trove_id,
            capability,
            version,
            version_relation,
            kind: RepositoryCapabilityKind::PackageName,
            version_scheme,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }
    }

    /// Create a new typed ProvideEntry
    pub fn new_typed(
        trove_id: i64,
        kind: RepositoryCapabilityKind,
        capability: String,
        version: Option<String>,
        version_scheme: VersionScheme,
        architecture_qualifier: ProvideArchitectureQualifier,
    ) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            id: None,
            trove_id,
            capability,
            version,
            version_relation,
            kind,
            version_scheme,
            architecture_qualifier,
            provenance: CapabilityProvenance::AuthorDeclared,
        }
    }

    pub fn from_declared(
        trove_id: i64,
        provide: &crate::repository::dependency_model::ProvidedCapability,
    ) -> Self {
        let mut entry = Self::new_typed(
            trove_id,
            provide.kind,
            provide.name.clone(),
            provide.version.clone(),
            provide.version_scheme,
            provide.architecture_qualifier.clone(),
        )
        .with_version_relation(provide.version_relation);
        entry.provenance = provide.provenance.clone();
        entry
    }

    /// Persist one package's complete typed resolution capability set.
    ///
    /// Every installed package must contribute exactly one exact-identity
    /// self provider. Source-declared capabilities may additionally name the
    /// package, but cannot replace that identity authority.
    pub fn insert_package_capabilities(
        conn: &Connection,
        trove_id: i64,
        package_name: &str,
        package_version: &str,
        package_scheme: VersionScheme,
        provides: &[crate::repository::dependency_model::ProvidedCapability],
    ) -> Result<()> {
        let exact_self = provides
            .iter()
            .filter(|provide| {
                provide.kind == RepositoryCapabilityKind::PackageName
                    && provide.name == package_name
                    && provide.version.as_deref() == Some(package_version)
                    && provide.version_relation == Some(ProvideVersionRelation::Equal)
                    && provide.version_scheme == package_scheme
                    && provide.architecture_qualifier == ProvideArchitectureQualifier::Implicit
                    && provide.provenance == CapabilityProvenance::ExactIdentity
            })
            .count();
        if exact_self != 1 {
            return Err(crate::error::Error::ConfigError(format!(
                "package '{package_name}-{package_version}' must declare exactly one exact package self-provider; found {exact_self}"
            )));
        }
        for provide in provides {
            provide.validate()?;
        }
        for provide in provides {
            Self::from_declared(trove_id, provide).insert_or_ignore(conn)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn with_version_relation(
        mut self,
        version_relation: Option<ProvideVersionRelation>,
    ) -> Self {
        self.version_relation = version_relation;
        self
    }

    /// Insert this provide into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_version()?;
        conn.execute(
            "INSERT INTO provides (
                trove_id, capability, version, version_relation, kind, version_scheme,
                architecture_qualifier_kind, architecture_qualifier, provenance
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &self.trove_id,
                &self.capability,
                &self.version,
                self.version_relation.map(ProvideVersionRelation::as_str),
                capability_kind_to_db(self.kind),
                self.version_scheme.as_str(),
                architecture_qualifier_to_db(&self.architecture_qualifier).0,
                architecture_qualifier_to_db(&self.architecture_qualifier).1,
                provenance_to_db(&self.provenance)?,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Insert or ignore if already exists (for idempotent imports)
    pub fn insert_or_ignore(&mut self, conn: &Connection) -> Result<i64> {
        self.validate_version()?;
        conn.execute(
            "INSERT OR IGNORE INTO provides (
                trove_id, capability, version, version_relation, kind, version_scheme,
                architecture_qualifier_kind, architecture_qualifier, provenance
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &self.trove_id,
                &self.capability,
                &self.version,
                self.version_relation.map(ProvideVersionRelation::as_str),
                capability_kind_to_db(self.kind),
                self.version_scheme.as_str(),
                architecture_qualifier_to_db(&self.architecture_qualifier).0,
                architecture_qualifier_to_db(&self.architecture_qualifier).1,
                provenance_to_db(&self.provenance)?,
            ],
        )?;

        // Get the ID (either new or existing)
        let id = conn.query_row(
            "SELECT id FROM provides
             WHERE trove_id = ?1 AND capability = ?2 AND kind = ?3
               AND version IS ?4 AND version_relation IS ?5 AND version_scheme = ?6
               AND architecture_qualifier_kind = ?7
               AND architecture_qualifier IS ?8 AND provenance = ?9",
            params![
                &self.trove_id,
                &self.capability,
                capability_kind_to_db(self.kind),
                &self.version,
                self.version_relation.map(ProvideVersionRelation::as_str),
                self.version_scheme.as_str(),
                architecture_qualifier_to_db(&self.architecture_qualifier).0,
                architecture_qualifier_to_db(&self.architecture_qualifier).1,
                provenance_to_db(&self.provenance)?,
            ],
            |row| row.get(0),
        )?;

        self.id = Some(id);
        Ok(id)
    }

    fn validate_version(&self) -> Result<()> {
        let Some(version) = self.version.as_deref() else {
            return Ok(());
        };
        validate_repo_version(self.version_scheme, version).map_err(|error| {
            crate::error::Error::ConfigError(format!(
                "provide {} has a version outside its declared grammar: {error}",
                self.capability
            ))
        })
    }

    /// Find a provide by capability name
    ///
    /// Returns the first matching provider, ordered deterministically by trove_id.
    /// Prefer `find_all_by_capability()` when multiple providers need consideration.
    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE capability = ?1 ORDER BY trove_id ASC LIMIT 1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provide = stmt.query_row([capability], Self::from_row).optional()?;
        Ok(provide)
    }

    /// Find a provide by kind and capability name
    ///
    /// Returns the first match, ordered deterministically by trove_id.
    /// Prefer `find_all_typed()` when multiple providers need consideration.
    pub fn find_typed(
        conn: &Connection,
        kind: RepositoryCapabilityKind,
        capability: &str,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE kind = ?1 AND capability = ?2 ORDER BY trove_id ASC LIMIT 1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provide = stmt
            .query_row([capability_kind_to_db(kind), capability], Self::from_row)
            .optional()?;
        Ok(provide)
    }

    /// Find all troves that provide a capability
    pub fn find_all_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE capability = ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Find rows that exactly match a CLI query without interpreting normalized
    /// non-package capability rows as untyped user input.
    pub fn find_all_by_cli_exact_query(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides
             WHERE capability = ?1
               AND (kind IS NULL OR kind = '' OR kind = 'package')",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Find all typed provides (by kind and capability)
    pub fn find_all_typed(
        conn: &Connection,
        kind: RepositoryCapabilityKind,
        capability: &str,
    ) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE kind = ?1 AND capability = ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([capability_kind_to_db(kind), capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Find all provides for a trove
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Vec<Self>> {
        let sql = format!("SELECT {} FROM provides WHERE trove_id = ?1", Self::COLUMNS);
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([trove_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Find all provides of a specific kind for a trove
    pub fn find_by_trove_and_kind(
        conn: &Connection,
        trove_id: i64,
        kind: RepositoryCapabilityKind,
    ) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE trove_id = ?1 AND kind = ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map(
                params![trove_id, capability_kind_to_db(kind)],
                Self::from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Check if a capability is provided by any installed package
    pub fn is_capability_satisfied(conn: &Connection, capability: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provides WHERE capability = ?1",
            [capability],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find provider with version constraint check
    ///
    /// Returns the trove name and version if the capability is satisfied
    pub fn find_satisfying_provider(
        conn: &Connection,
        capability: &str,
    ) -> Result<Option<(String, String)>> {
        Self::find_declared_satisfying_provider(conn, capability)
    }

    /// Find an installed provider only through declared capability metadata.
    ///
    /// This intentionally avoids cross-distro/package-name guesses. A query
    /// matches either the stored capability text exactly or an explicit typed
    /// query such as `soname(libssl.so.3)` against `(kind, capability)`.
    pub fn find_declared_satisfying_provider(
        conn: &Connection,
        capability: &str,
    ) -> Result<Option<(String, String)>> {
        Ok(Self::find_declared_provider_contracts(conn, capability)?
            .into_iter()
            .next()
            .map(|provider| (provider.package_name, provider.package_version)))
    }

    /// Find every installed provider with its declared capability version and
    /// package comparison scheme.
    pub fn find_declared_provider_contracts(
        conn: &Connection,
        capability: &str,
    ) -> Result<Vec<InstalledProvideContract>> {
        let mut exact_stmt = conn.prepare(
            "SELECT t.name, t.version, p.version, p.version_relation, p.version_scheme,
                    p.architecture_qualifier_kind, p.architecture_qualifier, p.provenance
             FROM provides p
             JOIN troves t ON p.trove_id = t.id
             WHERE p.capability = ?1
             ORDER BY t.id ASC, p.id ASC",
        )?;
        let exact = exact_stmt
            .query_map([capability], |row| {
                Ok(InstalledProvideContract {
                    package_name: row.get(0)?,
                    package_version: row.get(1)?,
                    capability_version: row.get(2)?,
                    version_relation: version_relation_from_row(row, 3, 2)?,
                    version_scheme: version_scheme_from_row(row, 4)?,
                    architecture_qualifier: architecture_qualifier_from_row(row, 5, 6)?,
                    provenance: provenance_from_row(row, 7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !exact.is_empty() {
            return Ok(exact);
        }

        let Some((kind, name)) = parse_typed_capability_query(capability) else {
            return Ok(Vec::new());
        };
        let mut typed_stmt = conn.prepare(
            "SELECT t.name, t.version, p.version, p.version_relation, p.version_scheme,
                    p.architecture_qualifier_kind, p.architecture_qualifier, p.provenance
             FROM provides p
             JOIN troves t ON p.trove_id = t.id
             WHERE p.kind = ?1 AND p.capability = ?2
             ORDER BY t.id ASC, p.id ASC",
        )?;
        let typed = typed_stmt
            .query_map(params![kind, name], |row| {
                Ok(InstalledProvideContract {
                    package_name: row.get(0)?,
                    package_version: row.get(1)?,
                    capability_version: row.get(2)?,
                    version_relation: version_relation_from_row(row, 3, 2)?,
                    version_scheme: version_scheme_from_row(row, 4)?,
                    architecture_qualifier: architecture_qualifier_from_row(row, 5, 6)?,
                    provenance: provenance_from_row(row, 7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(typed)
    }

    pub fn is_declared_capability_satisfied(conn: &Connection, capability: &str) -> Result<bool> {
        Ok(Self::find_declared_satisfying_provider(conn, capability)?.is_some())
    }

    /// Search for capabilities matching a pattern (using SQL LIKE)
    pub fn search_capability(conn: &Connection, pattern: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE capability LIKE ?1",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Search for typed capabilities matching a kind and pattern
    pub fn search_typed(
        conn: &Connection,
        kind: RepositoryCapabilityKind,
        pattern: &str,
    ) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM provides WHERE kind = ?1 AND capability LIKE ?2",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let provides = stmt
            .query_map([capability_kind_to_db(kind), pattern], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(provides)
    }

    /// Delete all provides for a trove
    pub fn delete_by_trove(conn: &Connection, trove_id: i64) -> Result<()> {
        conn.execute("DELETE FROM provides WHERE trove_id = ?1", [trove_id])?;
        Ok(())
    }

    /// Convert a database row to a ProvideEntry
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            trove_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
            version_relation: version_relation_from_row(row, 4, 3)?,
            kind: capability_kind_from_db(&row.get::<_, String>(5)?)?,
            version_scheme: version_scheme_from_row(row, 6)?,
            architecture_qualifier: architecture_qualifier_from_row(row, 7, 8)?,
            provenance: provenance_from_row(row, 9)?,
        })
    }

    /// Format this provide as a typed string (e.g., "python(requests)")
    pub fn to_typed_string(&self) -> String {
        if self.kind == RepositoryCapabilityKind::PackageName {
            self.capability.clone()
        } else {
            format!("{}({})", capability_kind_to_db(self.kind), self.capability)
        }
    }
}

fn provenance_to_db(provenance: &CapabilityProvenance) -> Result<String> {
    serde_json::to_string(provenance).map_err(|error| {
        crate::Error::InternalError(format!("serialize capability provenance: {error}"))
    })
}

fn provenance_from_row(row: &Row<'_>, column: usize) -> rusqlite::Result<CapabilityProvenance> {
    let raw = row.get::<_, String>(column)?;
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_typed_capability_query(capability: &str) -> Option<(&'static str, &str)> {
    let (kind, value) = capability.split_once('(')?;
    let value = value.strip_suffix(')')?;
    if value.is_empty() {
        return None;
    }
    let kind = capability_kind_from_db(kind).ok()?;
    Some((capability_kind_to_db(kind), value))
}

fn capability_kind_to_db(kind: RepositoryCapabilityKind) -> &'static str {
    match kind {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Path => "path",
        RepositoryCapabilityKind::Binary => "binary",
        RepositoryCapabilityKind::PkgConfig => "pkgconfig",
        RepositoryCapabilityKind::Generic => "generic",
    }
}

fn capability_kind_from_db(value: &str) -> rusqlite::Result<RepositoryCapabilityKind> {
    match value {
        "package" => Ok(RepositoryCapabilityKind::PackageName),
        "virtual" => Ok(RepositoryCapabilityKind::Virtual),
        "soname" => Ok(RepositoryCapabilityKind::Soname),
        "file" => Ok(RepositoryCapabilityKind::File),
        "path" => Ok(RepositoryCapabilityKind::Path),
        "binary" => Ok(RepositoryCapabilityKind::Binary),
        "pkgconfig" => Ok(RepositoryCapabilityKind::PkgConfig),
        "generic" => Ok(RepositoryCapabilityKind::Generic),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported persisted capability kind '{other}'"),
            )),
        )),
    }
}

fn architecture_qualifier_to_db(
    qualifier: &ProvideArchitectureQualifier,
) -> (&'static str, Option<&str>) {
    match qualifier {
        ProvideArchitectureQualifier::Implicit => ("implicit", None),
        ProvideArchitectureQualifier::Any => ("any", None),
        ProvideArchitectureQualifier::Exact(architecture) => ("exact", Some(architecture)),
    }
}

fn version_relation_from_row(
    row: &Row<'_>,
    relation_index: usize,
    version_index: usize,
) -> rusqlite::Result<Option<ProvideVersionRelation>> {
    let relation = row.get::<_, Option<String>>(relation_index)?;
    let version = row.get::<_, Option<String>>(version_index)?;
    match (relation, version) {
        (None, None) => Ok(None),
        (Some(relation), Some(_)) => ProvideVersionRelation::parse_exact(&relation)
            .map(Some)
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    relation_index,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                )
            }),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            relation_index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "installed provide must carry both version relation and boundary, or neither",
            )),
        )),
    }
}

fn architecture_qualifier_from_row(
    row: &Row<'_>,
    kind_index: usize,
    architecture_index: usize,
) -> rusqlite::Result<ProvideArchitectureQualifier> {
    let kind = row.get::<_, String>(kind_index)?;
    let architecture = row.get::<_, Option<String>>(architecture_index)?;
    match (kind.as_str(), architecture) {
        ("implicit", None) => Ok(ProvideArchitectureQualifier::Implicit),
        ("any", None) => Ok(ProvideArchitectureQualifier::Any),
        ("exact", Some(architecture)) if !architecture.is_empty() => {
            Ok(ProvideArchitectureQualifier::Exact(architecture))
        }
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            kind_index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid persisted provider architecture qualifier",
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                version_scheme TEXT NOT NULL
            );
            CREATE TABLE provides (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trove_id INTEGER NOT NULL REFERENCES troves(id),
                capability TEXT NOT NULL,
                version TEXT,
                version_relation TEXT,
                kind TEXT NOT NULL,
                version_scheme TEXT NOT NULL,
                architecture_qualifier_kind TEXT NOT NULL,
                architecture_qualifier TEXT,
                provenance TEXT NOT NULL
            );
            CREATE INDEX idx_provides_capability ON provides(capability);
            CREATE INDEX idx_provides_kind ON provides(kind);

            INSERT INTO troves (id, name, version, version_scheme)
            VALUES (1, 'perl-Text-CharWidth', '0.04', 'rpm');
            ",
        )
        .unwrap();
        conn
    }

    fn typed(
        trove_id: i64,
        kind: RepositoryCapabilityKind,
        capability: &str,
        version: Option<&str>,
    ) -> ProvideEntry {
        ProvideEntry::new_typed(
            trove_id,
            kind,
            capability.to_string(),
            version.map(str::to_string),
            VersionScheme::Rpm,
            ProvideArchitectureQualifier::Implicit,
        )
    }

    #[test]
    fn test_insert_and_find() {
        let conn = setup_test_db();

        let mut provide = typed(
            1,
            RepositoryCapabilityKind::Generic,
            "Text::CharWidth",
            Some("0.04"),
        );
        provide.insert(&conn).unwrap();

        let found =
            ProvideEntry::find_typed(&conn, RepositoryCapabilityKind::Generic, "Text::CharWidth")
                .unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.capability, "Text::CharWidth");
        assert_eq!(found.kind, RepositoryCapabilityKind::Generic);
        assert_eq!(found.version, Some("0.04".to_string()));
        assert_eq!(found.provenance, CapabilityProvenance::AuthorDeclared);
    }

    #[test]
    fn writes_reject_versions_outside_declared_scheme() {
        for insert_or_ignore in [false, true] {
            let conn = setup_test_db();
            let mut provide = ProvideEntry::new(
                1,
                "captured-root".to_string(),
                Some("snapshot".to_string()),
                VersionScheme::Conary,
            );

            let error = if insert_or_ignore {
                provide.insert_or_ignore(&conn).unwrap_err()
            } else {
                provide.insert(&conn).unwrap_err()
            };

            assert!(
                error
                    .to_string()
                    .contains("invalid conary version 'snapshot'"),
                "{error}"
            );
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM provides", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn test_is_capability_satisfied() {
        let conn = setup_test_db();

        let mut provide = typed(1, RepositoryCapabilityKind::Soname, "libc.so.6", None);
        provide.insert(&conn).unwrap();

        // Direct capability check still works
        assert!(ProvideEntry::is_capability_satisfied(&conn, "libc.so.6").unwrap());
        assert!(!ProvideEntry::is_capability_satisfied(&conn, "libfoo.so.1").unwrap());
    }

    #[test]
    fn test_search_capability() {
        let conn = setup_test_db();

        let mut p1 = typed(
            1,
            RepositoryCapabilityKind::Generic,
            "Text::CharWidth",
            None,
        );
        let mut p2 = typed(1, RepositoryCapabilityKind::Generic, "Text::Wrap", None);
        p1.insert(&conn).unwrap();
        p2.insert(&conn).unwrap();

        let results =
            ProvideEntry::search_typed(&conn, RepositoryCapabilityKind::Generic, "Text::%")
                .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn cli_exact_query_does_not_match_normalized_non_package_rows() {
        let conn = setup_test_db();

        let mut typed = typed(1, RepositoryCapabilityKind::Soname, "libssl.so.3", None);
        typed.insert(&conn).unwrap();
        let mut raw = ProvideEntry::new(
            1,
            "soname(libssl.so.3)".to_string(),
            None,
            VersionScheme::Rpm,
        );
        raw.insert(&conn).unwrap();

        let untyped = ProvideEntry::find_all_by_cli_exact_query(&conn, "libssl.so.3").unwrap();
        assert!(untyped.is_empty());

        let raw = ProvideEntry::find_all_by_cli_exact_query(&conn, "soname(libssl.so.3)").unwrap();
        assert_eq!(raw.len(), 1);
    }

    #[test]
    fn test_to_typed_string() {
        let provide = typed(
            1,
            RepositoryCapabilityKind::Generic,
            "requests",
            Some("2.28"),
        );
        assert_eq!(provide.to_typed_string(), "generic(requests)");

        let provide = ProvideEntry::new(
            1,
            "nginx".to_string(),
            Some("1.24".to_string()),
            VersionScheme::Rpm,
        );
        assert_eq!(provide.to_typed_string(), "nginx");
    }

    #[test]
    fn find_satisfying_provider_does_not_guess_soname_suffix_metadata() {
        let conn = setup_test_db();

        conn.execute(
            "INSERT INTO troves (id, name, version, version_scheme)
             VALUES (2, 'glibc', '2.42', 'rpm')",
            [],
        )
        .unwrap();

        let mut provide = ProvideEntry::new(
            2,
            "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        let result = ProvideEntry::find_satisfying_provider(&conn, "libc.so.6").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn declared_satisfying_provider_accepts_explicit_typed_query() {
        let conn = setup_test_db();

        conn.execute(
            "INSERT INTO troves (id, name, version, version_scheme)
             VALUES (2, 'glibc', '2.42', 'rpm')",
            [],
        )
        .unwrap();

        let mut provide = typed(2, RepositoryCapabilityKind::Soname, "libc.so.6", None);
        provide.insert(&conn).unwrap();

        let result =
            ProvideEntry::find_declared_satisfying_provider(&conn, "soname(libc.so.6)").unwrap();
        assert_eq!(result, Some(("glibc".to_string(), "2.42".to_string())));
    }

    #[test]
    fn declared_provider_contract_requires_exact_installed_version_scheme() {
        let conn = setup_test_db();
        let mut provide = typed(
            1,
            RepositoryCapabilityKind::Virtual,
            "virtual-abi",
            Some("2"),
        );
        provide.insert(&conn).unwrap();

        let contracts =
            ProvideEntry::find_declared_provider_contracts(&conn, "virtual-abi").unwrap();
        assert_eq!(contracts[0].version_scheme, VersionScheme::Rpm);

        conn.execute(
            "UPDATE provides SET version_scheme = 'unknown' WHERE trove_id = 1",
            [],
        )
        .unwrap();
        let error = ProvideEntry::find_declared_provider_contracts(&conn, "virtual-abi")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported persisted native version scheme 'unknown'"),
            "{error}"
        );
    }

    #[test]
    fn find_satisfying_provider_does_not_guess_versioned_soname_to_base_provider() {
        let conn = setup_test_db();

        conn.execute(
            "INSERT INTO troves (id, name, version, version_scheme)
             VALUES (2, 'glibc', '2.42', 'rpm')",
            [],
        )
        .unwrap();

        let mut provide = ProvideEntry::new(
            2,
            "libm.so.6()(64bit)".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        let result = ProvideEntry::find_satisfying_provider(&conn, "libm.so.6(GLIBC_2.0)").unwrap();
        assert_eq!(result, None);
    }
}
