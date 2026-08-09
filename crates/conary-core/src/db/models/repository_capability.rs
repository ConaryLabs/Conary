// conary-core/src/db/models/repository_capability.rs

//! Normalized repository-native capability tables.

use crate::error::Result;
use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryCapabilityKind,
};
use crate::repository::distro::version_scheme_from_db;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, Row, params};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// Every statement this module issues against `repository_provides`.
///
/// The SQL lives in named constants so the query-plan proof in this module's
/// tests reads the same text the production path executes. `repository_provides`
/// is the largest table Conary persists (10.07M rows on Fedora 44
/// `Everything/x86_64` with `filelists.xml`), so which index each statement can
/// seek through is a contract, not an implementation detail.
const INSERT_PROVIDE_SQL: &str = "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw, version_scheme,
              version_relation, architecture_qualifier_kind, architecture, provenance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

const SELECT_BY_PACKAGE_SQL: &str =
    "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                    version_scheme, architecture_qualifier_kind, architecture, provenance
             FROM repository_provides
             WHERE repository_package_id = ?1
             ORDER BY capability, version";

const SELECT_CONVERSION_INPUTS_SQL: &str =
    "SELECT p.id, p.repository_package_id, p.capability, p.version,
                    p.version_relation, p.kind, p.raw, p.version_scheme,
                    p.architecture_qualifier_kind, p.architecture, p.provenance,
                    rp.id, rp.checksum
             FROM repository_packages rp
             JOIN repositories r ON r.id = rp.repository_id
             LEFT JOIN repository_provides p ON p.repository_package_id = rp.id
             WHERE r.enabled = 1
               AND r.source_profile = ?1
               AND rp.name = ?2
               AND rp.version = ?3
               AND rp.architecture = ?4
               AND rp.size > 0
             ORDER BY rp.id, p.id";

/// `{placeholders}` is replaced with the bound package-id list for one batch.
const SELECT_BY_PACKAGES_TEMPLATE: &str =
    "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                        version_scheme, architecture_qualifier_kind, architecture, provenance
                 FROM repository_provides
                 WHERE repository_package_id IN ({placeholders})";

const SELECT_BY_CAPABILITY_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version";

const SELECT_BY_CLI_EXACT_QUERY_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
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
             ORDER BY rp.capability, rp.version";

const SELECT_BY_CAPABILITY_WITH_NAME_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance, pkg.name
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version";

const SELECT_BY_CAPABILITY_AND_KIND_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM repository_provides rp
             JOIN repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1 AND rp.kind = ?2
             ORDER BY rp.capability, rp.version";

const DELETE_BY_PACKAGE_SQL: &str =
    "DELETE FROM repository_provides WHERE repository_package_id = ?1";

const DELETE_BY_REPOSITORY_SQL: &str = "DELETE FROM repository_provides
             WHERE repository_package_id IN (
                 SELECT id FROM repository_packages WHERE repository_id = ?1
             )";

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
    pub provenance: CapabilityProvenance,
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
            provenance: CapabilityProvenance::AuthorDeclared,
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

    #[must_use]
    pub fn with_provenance(mut self, provenance: CapabilityProvenance) -> Self {
        self.provenance = provenance;
        self
    }

    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        let (qualifier_kind, architecture) =
            architecture_qualifier_to_db(&self.architecture_qualifier);
        conn.execute(
            INSERT_PROVIDE_SQL,
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
                provenance_to_db(&self.provenance)?,
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

        let mut stmt = conn.prepare_cached(INSERT_PROVIDE_SQL)?;

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
                provenance_to_db(&provide.provenance)?,
            ])?;
        }

        Ok(provides.len())
    }

    pub fn find_by_repository_package(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(SELECT_BY_PACKAGE_SQL)?;
        let rows = stmt
            .query_map([repository_package_id], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return one atomic repository-metadata projection and digest for
    /// conversion-cache invalidation. These rows never mutate artifact authority.
    pub fn conversion_capabilities_with_digest(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<(Vec<ProvidedCapability>, String)> {
        Self::project_conversion_capabilities(Self::find_by_repository_package(
            conn,
            repository_package_id,
        )?)
    }

    /// Load every exact source-package checksum and provider digest for one
    /// repository conversion identity from a single SQLite read snapshot.
    pub(crate) fn conversion_inputs_for_source_identity(
        conn: &Connection,
        source_profile: &str,
        package_name: &str,
        package_version: &str,
        package_architecture: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(SELECT_CONVERSION_INPUTS_SQL)?;
        let rows = stmt
            .query_map(
                params![
                    source_profile,
                    package_name,
                    package_version,
                    package_architecture,
                ],
                |row| {
                    let provide = row
                        .get::<_, Option<i64>>(0)?
                        .map(|_| Self::from_row(row))
                        .transpose()?;
                    Ok((row.get::<_, i64>(11)?, row.get::<_, String>(12)?, provide))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut package_inputs = BTreeMap::<i64, (String, Vec<Self>)>::new();
        for (repository_package_id, checksum, provide) in rows {
            let (persisted_checksum, provides) = package_inputs
                .entry(repository_package_id)
                .or_insert_with(|| (checksum.clone(), Vec::new()));
            if persisted_checksum != &checksum {
                return Err(crate::Error::InternalError(format!(
                    "repository package {repository_package_id} yielded conflicting checksums"
                )));
            }
            if let Some(provide) = provide {
                provides.push(provide);
            }
        }

        package_inputs
            .into_values()
            .map(|(checksum, provides)| {
                let (_, digest) = Self::project_conversion_capabilities(provides)?;
                Ok((checksum, digest))
            })
            .collect()
    }

    fn project_conversion_capabilities(
        provides: impl IntoIterator<Item = Self>,
    ) -> Result<(Vec<ProvidedCapability>, String)> {
        let mut capabilities = BTreeMap::<Vec<u8>, ProvidedCapability>::new();
        for provide in provides {
            let capability = provide.as_provided_capability()?;
            capability.validate()?;
            let canonical = crate::json::canonical_json(&capability).map_err(|error| {
                crate::Error::InternalError(format!(
                    "failed to canonicalize repository provide projection: {error}"
                ))
            })?;
            capabilities.entry(canonical).or_insert(capability);
        }
        let capabilities = capabilities.into_values().collect::<Vec<_>>();
        let canonical = crate::json::canonical_json(&capabilities).map_err(|error| {
            crate::Error::InternalError(format!(
                "failed to canonicalize repository conversion metadata: {error}"
            ))
        })?;
        let digest = crate::hash::sha256_prefixed(&canonical);
        Ok((capabilities, digest))
    }

    /// Digest repository metadata that invalidates a cached native conversion
    /// without becoming signed artifact identity or capability authority.
    pub fn conversion_capabilities_digest(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<String> {
        Self::conversion_capabilities_with_digest(conn, repository_package_id)
            .map(|(_, digest)| digest)
    }

    fn as_provided_capability(&self) -> Result<ProvidedCapability> {
        let kind = match self.kind.as_str() {
            "package" => RepositoryCapabilityKind::PackageName,
            "virtual" => RepositoryCapabilityKind::Virtual,
            "soname" => RepositoryCapabilityKind::Soname,
            "file" => RepositoryCapabilityKind::File,
            "path" => RepositoryCapabilityKind::Path,
            "binary" => RepositoryCapabilityKind::Binary,
            "pkgconfig" => RepositoryCapabilityKind::PkgConfig,
            "generic" => RepositoryCapabilityKind::Generic,
            other => {
                return Err(crate::Error::InternalError(format!(
                    "unsupported normalized repository provide kind '{other}'"
                )));
            }
        };
        Ok(ProvidedCapability {
            kind,
            name: self.capability.clone(),
            version: self.version.clone(),
            version_relation: self.version_relation,
            version_scheme: self.version_scheme,
            architecture_qualifier: self.architecture_qualifier.clone(),
            provenance: self.provenance.clone(),
        })
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
        let package_ids = repository_package_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let batch_size = super::sqlite_variable_batch_size(conn)?;
        let mut rows = Vec::new();
        for package_ids in package_ids.chunks(batch_size) {
            let placeholders = (1..=package_ids.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = SELECT_BY_PACKAGES_TEMPLATE.replace("{placeholders}", &placeholders);
            let mut stmt = conn.prepare(&sql)?;
            rows.extend(
                stmt.query_map(
                    rusqlite::params_from_iter(package_ids.iter()),
                    Self::from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
        rows.sort_by(|left, right| {
            (left.repository_package_id, &left.capability, &left.version).cmp(&(
                right.repository_package_id,
                &right.capability,
                &right.version,
            ))
        });
        Ok(rows)
    }

    pub fn find_by_capability(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(SELECT_BY_CAPABILITY_SQL)?;
        let rows = stmt
            .query_map([capability], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find rows that exactly match a CLI query without interpreting normalized
    /// typed capability rows as untyped user input.
    pub fn find_by_cli_exact_query(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(SELECT_BY_CLI_EXACT_QUERY_SQL)?;
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
        let mut stmt = conn.prepare(SELECT_BY_CAPABILITY_WITH_NAME_SQL)?;
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
                    provenance: provenance_from_row(row, 10)?,
                };
                let pkg_name: String = row.get(11)?;
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
        let mut stmt = conn.prepare(SELECT_BY_CAPABILITY_AND_KIND_SQL)?;
        let rows = stmt
            .query_map(params![capability, kind], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all provides for a specific repository package.
    pub fn delete_by_package(conn: &Connection, repository_package_id: i64) -> Result<()> {
        conn.execute(DELETE_BY_PACKAGE_SQL, [repository_package_id])?;
        Ok(())
    }

    /// Delete all provides for packages belonging to a repository.
    pub fn delete_by_repository(conn: &Connection, repository_id: i64) -> Result<()> {
        conn.execute(DELETE_BY_REPOSITORY_SQL, [repository_id])?;
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
            provenance: provenance_from_row(row, 10)?,
        })
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

    /// Every statement this module issues against `repository_provides`, with
    /// the batch template resolved to a bound list. The query-plan proof below
    /// reads this list, so a statement added without a plan is a test failure,
    /// not an unnoticed table scan.
    fn owned_statements() -> Vec<(&'static str, String)> {
        vec![
            ("insert", INSERT_PROVIDE_SQL.to_string()),
            (
                "find_by_repository_package",
                SELECT_BY_PACKAGE_SQL.to_string(),
            ),
            (
                "conversion_inputs_for_source_identity",
                SELECT_CONVERSION_INPUTS_SQL.to_string(),
            ),
            (
                "find_by_repository_packages",
                SELECT_BY_PACKAGES_TEMPLATE.replace("{placeholders}", "?1, ?2"),
            ),
            ("find_by_capability", SELECT_BY_CAPABILITY_SQL.to_string()),
            (
                "find_by_cli_exact_query",
                SELECT_BY_CLI_EXACT_QUERY_SQL.to_string(),
            ),
            (
                "find_by_capability_with_name",
                SELECT_BY_CAPABILITY_WITH_NAME_SQL.to_string(),
            ),
            (
                "find_by_capability_and_kind",
                SELECT_BY_CAPABILITY_AND_KIND_SQL.to_string(),
            ),
            ("delete_by_package", DELETE_BY_PACKAGE_SQL.to_string()),
            ("delete_by_repository", DELETE_BY_REPOSITORY_SQL.to_string()),
        ]
    }

    fn query_plan(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap_or_else(|error| panic!("prepare plan for {sql}: {error}"));
        for index in 1..=stmt.parameter_count() {
            stmt.raw_bind_parameter(index, rusqlite::types::Null)
                .unwrap();
        }
        let mut rows = stmt.raw_query();
        let mut plan = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            plan.push(row.get::<_, String>(3).unwrap());
        }
        plan
    }

    /// `repository_provides` is the largest table Conary persists, so its index
    /// inventory is a contract. A `(kind, capability)` composite cannot serve a
    /// capability-only seek, which is what every provider lookup issues, so
    /// exactly two indexes are carried: the package key and the capability key.
    #[test]
    fn repository_provides_carries_exactly_the_package_and_capability_indexes() {
        let conn = test_db();

        let mut indexes = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'repository_provides'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        indexes.sort();

        assert_eq!(
            indexes,
            vec![
                "idx_repository_provides_capability".to_string(),
                "idx_repository_provides_pkg".to_string(),
            ],
            "an index on repository_provides costs ~0.8 GB at filelists scale; \
             adding one needs its own query-plan proof"
        );
    }

    /// The property, not the instance: no statement this module owns may reach
    /// `repository_provides` by scanning it, and every statement that filters
    /// `capability` must seek the capability index -- including the one that
    /// also filters `kind`, which no longer has a composite to seek.
    #[test]
    fn every_owned_statement_reaches_repository_provides_through_an_index() {
        let conn = test_db();

        for (label, sql) in owned_statements() {
            let plan = query_plan(&conn, &sql);
            for step in &plan {
                assert!(
                    !(step.starts_with("SCAN") && step.contains("repository_provides")),
                    "{label} scans repository_provides: {plan:?}"
                );
            }
        }

        for label in [
            "find_by_capability",
            "find_by_capability_with_name",
            "find_by_capability_and_kind",
        ] {
            let (_, sql) = owned_statements()
                .into_iter()
                .find(|(name, _)| *name == label)
                .expect("statement is inventoried");
            let plan = query_plan(&conn, &sql);
            assert!(
                plan.iter().any(|step| step
                    == "SEARCH rp USING INDEX idx_repository_provides_capability (capability=?)"),
                "{label} must seek the capability index: {plan:?}"
            );
        }
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
        .with_architecture_qualifier(ProvideArchitectureQualifier::Exact("arm64".to_string()))
        .with_provenance(CapabilityProvenance::SourceDeclared {
            format: crate::repository::dependency_model::SourcePackageFormat::Rpm,
            record_index: 7,
        });
        provide.insert(&conn).unwrap();

        let found = RepositoryProvide::find_by_repository_package(&conn, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].capability, "mail-transport-agent");
        assert_eq!(found[0].version_scheme, VersionScheme::Rpm);
        assert_eq!(
            found[0].architecture_qualifier,
            ProvideArchitectureQualifier::Exact("arm64".to_string())
        );
        assert_eq!(found[0].provenance, provide.provenance);
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
    fn conversion_cache_digest_tracks_only_exact_repository_projection() {
        let conn = test_db();
        seed_repo_and_package(&conn);

        let empty = RepositoryProvide::conversion_capabilities_digest(&conn, 1).unwrap();
        let mut shell = RepositoryProvide::new(
            1,
            "/usr/bin/sh".to_string(),
            None,
            "file".to_string(),
            Some("first diagnostic spelling".to_string()),
            VersionScheme::Rpm,
        );
        shell.insert(&conn).unwrap();
        let with_shell = RepositoryProvide::conversion_capabilities_digest(&conn, 1).unwrap();
        assert_ne!(empty, with_shell);

        let mut duplicate = RepositoryProvide::new(
            1,
            "/usr/bin/sh".to_string(),
            None,
            "file".to_string(),
            Some("different diagnostic spelling".to_string()),
            VersionScheme::Rpm,
        );
        duplicate.insert(&conn).unwrap();
        assert_eq!(
            RepositoryProvide::conversion_capabilities_digest(&conn, 1).unwrap(),
            with_shell,
            "raw diagnostic text and duplicate rows do not change the cache projection"
        );

        let mut kernel_install = RepositoryProvide::new(
            1,
            "/usr/bin/kernel-install".to_string(),
            None,
            "file".to_string(),
            None,
            VersionScheme::Rpm,
        );
        kernel_install.insert(&conn).unwrap();
        assert_ne!(
            RepositoryProvide::conversion_capabilities_digest(&conn, 1).unwrap(),
            with_shell
        );
    }

    #[test]
    fn multi_package_lookup_chunks_at_the_runtime_sqlite_variable_limit() {
        let conn = test_db();
        seed_repo_and_package(&conn);
        for package_id in 2..=5 {
            conn.execute(
                "INSERT INTO repository_packages
                 (id, repository_id, name, version, checksum, size, download_url, version_scheme)
                 VALUES (?1, 1, ?2, '1.0', 'sha256:test', 1, ?3, 'rpm')",
                params![
                    package_id,
                    format!("pkg-{package_id}"),
                    format!("https://example.test/pkg-{package_id}")
                ],
            )
            .unwrap();
        }
        for package_id in 1..=5 {
            let mut provide = RepositoryProvide::new(
                package_id,
                format!("cap-{package_id}"),
                None,
                "package".to_string(),
                None,
                VersionScheme::Rpm,
            );
            provide.insert(&conn).unwrap();
        }
        conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 2)
            .unwrap();

        let found =
            RepositoryProvide::find_by_repository_packages(&conn, &[5, 4, 3, 2, 1, 1]).unwrap();

        assert_eq!(found.len(), 5);
        assert_eq!(
            found
                .iter()
                .map(|provide| provide.repository_package_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
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
