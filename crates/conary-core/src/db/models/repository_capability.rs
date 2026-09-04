// conary-core/src/db/models/repository_capability.rs

//! Normalized repository-native capability tables.

mod kind;
mod projection;

use crate::error::Result;
use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
};
use crate::repository::distro::version_scheme_from_db;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, Row, params};
use std::collections::BTreeSet;
use std::io;

/// Every statement this module issues against `repository_provides`.
///
/// The SQL lives in named constants so the query-plan proof in this module's
/// tests reads the same text the production path executes. `repository_provides`
/// is the largest table Conary persists (10.07M rows on Fedora 44
/// `Everything/x86_64` with `filelists.xml`), so which index each statement can
/// seek through is a contract, not an implementation detail. The CLI
/// raw-spelling arm (`SELECT_BY_CLI_RAW_QUERY_SQL`) seeks the partial raw index.
const INSERT_PROVIDE_SQL: &str = "INSERT INTO repository_provides
             (repository_package_id, capability, version, kind, raw, version_scheme,
              version_relation, architecture_qualifier_kind, architecture, provenance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

const SELECT_BY_PACKAGE_SQL: &str =
    "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                    version_scheme, architecture_qualifier_kind, architecture, provenance
             FROM resolved_repository_provides
             WHERE repository_package_id = ?1
             ORDER BY capability, version";

/// `{placeholders}` is replaced with the bound package-id list for one batch.
const SELECT_BY_PACKAGES_TEMPLATE: &str =
    "SELECT id, repository_package_id, capability, version, version_relation, kind, raw,
                        version_scheme, architecture_qualifier_kind, architecture, provenance
                 FROM resolved_repository_provides
                 WHERE repository_package_id IN ({placeholders})";

const SELECT_BY_CAPABILITY_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM resolved_repository_provides rp
             JOIN resolved_repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1
             ORDER BY rp.capability, rp.version";

/// Capability arm of the CLI exact query: untyped input matching a normalized
/// package-name capability. This statement seeks
/// `idx_repository_provides_capability`; the raw-spelling arm below is a
/// separate statement so the seek never rides an OR the planner cannot index.
const SELECT_BY_CLI_EXACT_QUERY_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM resolved_repository_provides rp
             JOIN resolved_repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1
               AND rp.capability = ?1
               AND (rp.kind IS NULL OR rp.kind = '' OR rp.kind = 'package')
               AND (rp.raw IS NULL OR rp.raw = '')
             ORDER BY rp.capability, rp.version";

/// Raw-spelling arm of the CLI exact query: rows whose source-native text equals
/// the untyped query (e.g. `libssl.so.3()(64bit)`), including typed rows no
/// normalized capability equals.
///
/// `raw` now seeks the partial `idx_repository_provides_raw` index. Its
/// predicate is the same non-null/non-empty guard as the raw arm below. The
/// measured 69,476,352-byte cost (0.9% of a 27.8M-row three-distro client DB,
/// dbstat page-exact, 2026-08-09 fresh production sync; methodology on issue
/// #341) answers the #343 resurrection concern.
/// It stays a separate statement so the capability arm above keeps its seek.
/// The `raw != ''` guard keeps the two arms disjoint: empty-raw rows belong to
/// the capability arm, exactly as the original OR partitioned them for every
/// non-empty query.
const SELECT_BY_CLI_RAW_QUERY_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM resolved_repository_provides rp
             JOIN resolved_repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1
               AND rp.raw = ?1
               AND rp.raw IS NOT NULL
               AND rp.raw != ''
             ORDER BY rp.capability, rp.version";

const SELECT_BY_CAPABILITY_AND_KIND_SQL: &str =
    "SELECT rp.id, rp.repository_package_id, rp.capability, rp.version,
                    rp.version_relation, rp.kind, rp.raw, rp.version_scheme,
                    rp.architecture_qualifier_kind, rp.architecture, rp.provenance
             FROM resolved_repository_provides rp
             JOIN resolved_repository_packages pkg ON pkg.id = rp.repository_package_id
             JOIN repositories repo ON repo.id = pkg.repository_id
             WHERE repo.enabled = 1 AND rp.capability = ?1 AND rp.kind = ?2
             ORDER BY rp.capability, rp.version";

const DELETE_BY_PACKAGE_SQL: &str =
    "DELETE FROM repository_provides WHERE repository_package_id = ?1";

const DELETE_BY_REPOSITORY_SQL: &str = "DELETE FROM repository_provides
             WHERE repository_package_id IN (
                 SELECT id FROM repository_packages WHERE repository_id = ?1
             )";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    /// Return one atomic repository-metadata projection and diagnostic digest.
    /// These rows never establish conversion identity or mutate artifact
    /// authority.
    pub fn conversion_capabilities_with_digest(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<(Vec<ProvidedCapability>, String)> {
        projection::project(Self::find_by_repository_package(
            conn,
            repository_package_id,
        )?)
    }

    /// Digest repository metadata for diagnostics and indexed projections
    /// without becoming conversion identity or capability authority.
    pub fn conversion_capabilities_digest(
        conn: &Connection,
        repository_package_id: i64,
    ) -> Result<String> {
        Self::conversion_capabilities_with_digest(conn, repository_package_id)
            .map(|(_, digest)| digest)
    }

    fn as_provided_capability(&self) -> Result<ProvidedCapability> {
        let kind = kind::decode(&self.kind)?;
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
    ///
    /// Runs the package-name arm (a capability-index seek) and the raw-spelling
    /// arm (a separate statement) and merges their rows in the original
    /// `capability, version` order. See the query-plan proof below.
    pub fn find_by_cli_exact_query(conn: &Connection, capability: &str) -> Result<Vec<Self>> {
        // An empty query names nothing. The pre-split OR accidentally matched
        // rows whose raw column was the empty string; the arms below would
        // partition that accident differently, so the degenerate input is
        // answered here instead of by either statement.
        if capability.is_empty() {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        let mut stmt = conn.prepare(SELECT_BY_CLI_EXACT_QUERY_SQL)?;
        rows.extend(
            stmt.query_map([capability], Self::from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        let mut stmt = conn.prepare(SELECT_BY_CLI_RAW_QUERY_SQL)?;
        rows.extend(
            stmt.query_map([capability], Self::from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        rows.sort_by(|left, right| {
            (&left.capability, &left.version, left.id).cmp(&(
                &right.capability,
                &right.version,
                right.id,
            ))
        });
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
mod tests;
