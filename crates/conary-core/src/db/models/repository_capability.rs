// conary-core/src/db/models/repository_capability.rs

//! Normalized repository-native capability tables.

use crate::error::Result;
use crate::repository::dependency_model::{ProvideArchitectureQualifier, ProvideVersionRelation};
use crate::repository::distro::version_scheme_from_db;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, Row, params};
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryProvide {
    pub id: Option<i64>,
    pub repository_package_id: i64,
    pub capability: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub kind: String,
    pub raw: Option<String>,
    /// Exact version comparison contract for this normalized provide.
    pub version_scheme: VersionScheme,
    /// Exact source-native architecture qualifier for this capability.
    pub architecture_qualifier: ProvideArchitectureQualifier,
}

impl RepositoryProvide {
    pub fn new(
        repository_package_id: i64,
        capability: String,
        version: Option<String>,
        kind: String,
        raw: Option<String>,
        version_scheme: VersionScheme,
    ) -> Self {
        let version_relation = version.as_ref().map(|_| ProvideVersionRelation::Equal);
        Self {
            id: None,
            repository_package_id,
            capability,
            version,
            version_relation,
            kind,
            raw,
            version_scheme,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        }
    }

    #[must_use]
    pub fn with_architecture_qualifier(
        mut self,
        architecture_qualifier: ProvideArchitectureQualifier,
    ) -> Self {
        self.architecture_qualifier = architecture_qualifier;
        self
    }

    #[must_use]
    pub fn with_version_relation(
        mut self,
        version_relation: Option<ProvideVersionRelation>,
    ) -> Self {
        self.version_relation = version_relation;
        self
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        let (qualifier_kind, architecture) =
            architecture_qualifier_to_db(&self.architecture_qualifier);
        conn.execute(
            "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw, version_scheme,
              version_relation, architecture_qualifier_kind, architecture)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.repository_package_id,
                &self.capability,
                &self.version,
                &self.kind,
                &self.raw,
                self.version_scheme.as_str(),
                self.version_relation.map(ProvideVersionRelation::as_str),
                qualifier_kind,
                architecture,
            ],
        )?;
        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    pub fn batch_insert(conn: &Connection, provides: &[Self]) -> Result<usize> {
        if provides.is_empty() {
            return Ok(0);
        }

        let mut stmt = conn.prepare_cached(
            "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw, version_scheme,
              version_relation, architecture_qualifier_kind, architecture)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;

        for provide in provides {
            let (qualifier_kind, architecture) =
                architecture_qualifier_to_db(&provide.architecture_qualifier);
            stmt.execute(params![
                provide.repository_package_id,
                &provide.capability,
                &provide.version,
                &provide.kind,
                &provide.raw,
                provide.version_scheme.as_str(),
                provide.version_relation.map(ProvideVersionRelation::as_str),
                qualifier_kind,
                architecture,
            ])?;
        }

        Ok(provides.len())
    }

    pub fn find_by_repository_package(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                    version_scheme, architecture_qualifier_kind, architecture
             FROM repository_provides
             WHERE repository_package_id = ?1
             ORDER BY capability, version",
        )?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Load exact provides for a bounded set of repository packages in one
    /// query.
    pub fn find_by_repository_packages(
        conn: &Connection,
        repository_package_ids: &[i64],
    ) -> Result<Vec<Self>> {
        if repository_package_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = repository_package_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                    version_scheme, architecture_qualifier_kind, architecture
             FROM repository_provides
             WHERE repository_package_id IN ({placeholders})
             ORDER BY repository_package_id, capability, version"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(repository_package_ids.iter()),
                Self::from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find rows that exactly match a CLI query without interpreting normalized
    /// typed capability rows as untyped user input.
    pub fn find_by_cli_exact_query(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1
               AND (
                 rp.raw = ?1
                 OR (
                   (rp.raw IS NULL OR rp.raw = '')
                   AND (rp.kind IS NULL OR rp.kind = '' OR rp.kind = 'package')
                   AND rp.capability = ?1
                 )
               )
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find provides matching a capability and return each paired with its package name.
    ///
    /// This single JOIN avoids the N+1 pattern of calling `find_by_capability` and then
    /// issuing a separate `SELECT name FROM repository_packages WHERE id = ?` for each
    /// result row. The `pkg.name` column is appended after the capability columns so the
    /// existing `from_row` mapper is not disturbed.
    pub fn find_by_capability_with_name(
        conn: &Connection,
        capability: &str,
    ) -> Result<Vec<(Self, String)>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, pkg.name
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map([capability], |row| {
                let provide = Self {
                    id: row.get(0)?,
                    repository_package_id: row.get(1)?,
                    capability: row.get(2)?,
                    version: row.get(3)?,
                    version_relation: version_relation_from_row(row, 4, 3)?,
                    kind: row.get(5)?,
                    raw: row.get(6)?,
                    version_scheme: version_scheme_from_row(row, 7)?,
                    architecture_qualifier: architecture_qualifier_from_row(row, 8, 9)?,
                };
                let pkg_name: String = row.get(10)?;
                Ok((provide, pkg_name))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find provides matching both capability name and kind in enabled repositories.
    pub fn find_by_capability_and_kind(
        conn: &Connection,
        capability: &str,
        kind: &str,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1 AND rp.kind = ?2
             ORDER BY rp.capability, rp.version",
        )?;
        let rows = stmt
            .query_map(params![capability, kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all provides for a specific repository package.
    pub fn delete_by_package(conn: &Connection, repository_package_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_provides WHERE repository_package_id = ?1",
            [repository_package_id],
        )?;
        Ok(())
    }

    /// Delete all provides for packages belonging to a repository.
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM repository_provides
             WHERE repository_package_id IN (
                 SELECT id FROM repository_packages WHERE repository_id = ?1
             )",
            [repository_id],
        )?;
        Ok(())
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            repository_package_id: row.get(1)?,
            capability: row.get(2)?,
            version: row.get(3)?,
            version_relation: version_relation_from_row(row, 4, 3)?,
            kind: row.get(5)?,
            raw: row.get(6)?,
            version_scheme: version_scheme_from_row(row, 7)?,
            architecture_qualifier: architecture_qualifier_from_row(row, 8, 9)?,
        })
    }
}

fn version_scheme_from_row(row: &Row<'_>, column: usize) -> rusqlite::Result<VersionScheme> {
    let raw: String = row.get(column)?;
    version_scheme_from_db(Some(&raw)).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported repository provide version scheme '{raw}'"),
            )),
        )
    })
}

fn version_relation_from_row(
    row: &Row<'_>,
    relation_column: usize,
    version_column: usize,
) -> rusqlite::Result<Option<ProvideVersionRelation>> {
    let relation = row.get::<_, Option<String>>(relation_column)?;
    let version = row.get::<_, Option<String>>(version_column)?;
    match (relation, version) {
        (None, None) => Ok(None),
        (Some(relation), Some(_)) => ProvideVersionRelation::parse_exact(&relation)
            .map(Some)
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    relation_column,
                    rusqlite::types::Type::Text,
                    Box::new(io::Error::new(io::ErrorKind::InvalidData, error)),
                )
            }),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            relation_column,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "repository provide must carry both version relation and boundary, or neither",
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
        ProvideArchitectureQualifier::Exact(architecture) => ("exact", Some(architecture.as_str())),
    }
}

fn architecture_qualifier_from_row(
    row: &Row<'_>,
    kind_column: usize,
    architecture_column: usize,
) -> rusqlite::Result<ProvideArchitectureQualifier> {
    let kind: String = row.get(kind_column)?;
    let architecture: Option<String> = row.get(architecture_column)?;
    match (kind.as_str(), architecture) {
        ("implicit", None) => Ok(ProvideArchitectureQualifier::Implicit),
        ("any", None) => Ok(ProvideArchitectureQualifier::Any),
        ("exact", Some(architecture)) if !architecture.is_empty() => {
            Ok(ProvideArchitectureQualifier::Exact(architecture))
        }
        (_, architecture) => Err(rusqlite::Error::FromSqlConversionFailure(
            kind_column,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid repository provide architecture qualifier kind='{kind}' architecture={architecture:?}"
                ),
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        conn
    }

    fn seed_repo_and_package(conn: &Connection) {
        conn.execute(
            "INSERT INTO repositories (name, url) VALUES ('repo', 'https://example.test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (1, 'pkg', '1.0', 'sha256:test', 1, 'https://example.test/pkg', 'rpm')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn repository_provide_round_trip() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide = RepositoryProvide::new(
            1,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            Some("mail-transport-agent".to_string()),
            VersionScheme::Rpm,
        )
        .with_architecture_qualifier(ProvideArchitectureQualifier::Exact("arm64".to_string()));
        provide.insert(&conn).unwrap();

        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].capability, "mail-transport-agent");
        assert_eq!(found[0].version_scheme, VersionScheme::Rpm);
        assert_eq!(
            found[0].architecture_qualifier,
            ProvideArchitectureQualifier::Exact("arm64".to_string())
        );
    }

    #[test]
    fn repository_provide_with_version_scheme() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide = RepositoryProvide::new(
            1,
            "libc.so.6".to_string(),
            Some("2.34".to_string()),
            "soname".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].version_scheme, VersionScheme::Rpm);
    }

    #[test]
    fn find_by_capability_and_kind_filters_correctly() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut p1 = RepositoryProvide::new(
            1,
            "foo".to_string(),
            None,
            "package".to_string(),
            None,
            VersionScheme::Rpm,
        );
        p1.insert(&conn).unwrap();
        let mut p2 = RepositoryProvide::new(
            1,
            "foo".to_string(),
            None,
            "virtual".to_string(),
            None,
            VersionScheme::Rpm,
        );
        p2.insert(&conn).unwrap();

        let pkg_only =
            RepositoryProvide::find_by_capability_and_kind(&conn, "foo", "package").unwrap();
        assert_eq!(pkg_only.len(), 1);
        assert_eq!(pkg_only[0].kind, "package");
    }

    #[test]
    fn cli_exact_query_matches_raw_or_package_rows_only() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut typed = RepositoryProvide::new(
            1,
            "libssl.so.3".to_string(),
            None,
            "soname".to_string(),
            Some("libssl.so.3()(64bit)".to_string()),
            VersionScheme::Rpm,
        );
        typed.insert(&conn).unwrap();
        let mut package = RepositoryProvide::new(
            1,
            "openssl".to_string(),
            None,
            "package".to_string(),
            None,
            VersionScheme::Rpm,
        );
        package.insert(&conn).unwrap();

        let untyped = RepositoryProvide::find_by_cli_exact_query(&conn, "libssl.so.3").unwrap();
        assert!(untyped.is_empty());

        let raw =
            RepositoryProvide::find_by_cli_exact_query(&conn, "libssl.so.3()(64bit)").unwrap();
        assert_eq!(raw.len(), 1);

        let package = RepositoryProvide::find_by_cli_exact_query(&conn, "openssl").unwrap();
        assert_eq!(package.len(), 1);
    }

    #[test]
    fn find_by_capability_with_name_requires_normalized_capability() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut typed = RepositoryProvide::new(
            1,
            "libssl.so.3".to_string(),
            None,
            "soname".to_string(),
            Some("libssl.so.3()(64bit)".to_string()),
            VersionScheme::Rpm,
        );
        typed.insert(&conn).unwrap();

        let raw =
            RepositoryProvide::find_by_capability_with_name(&conn, "libssl.so.3()(64bit)").unwrap();
        assert!(raw.is_empty());

        let normalized =
            RepositoryProvide::find_by_capability_with_name(&conn, "libssl.so.3").unwrap();
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].0.capability, "libssl.so.3");
        assert_eq!(normalized[0].0.raw.as_deref(), Some("libssl.so.3()(64bit)"));
        assert_eq!(normalized[0].1, "pkg");
    }

    #[test]
    fn delete_by_package_removes_provides() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide = RepositoryProvide::new(
            1,
            "cap".to_string(),
            None,
            "virtual".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        RepositoryProvide::delete_by_package(&conn, 1).unwrap();
        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_by_repository_removes_provides() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let mut provide = RepositoryProvide::new(
            1,
            "cap".to_string(),
            None,
            "virtual".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        RepositoryProvide::delete_by_repository(&conn, 1).unwrap();
        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert!(found.is_empty());
    }
}
