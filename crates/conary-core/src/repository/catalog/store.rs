// crates/conary-core/src/repository/catalog/store.rs

//! Deterministic standalone SQLite storage for immutable repository catalogs.

#[cfg(test)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::contract::{validate_identity, validate_sha256};
use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogContentV1, CatalogCountsV1,
    CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogProvideRecordV1,
    CatalogRequirementAtomV1, CatalogRequirementGroupV1, CatalogScopeV1, CatalogSourceEvidenceV1,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{DebianMultiArch, ProvideVersionRelation};
use crate::repository::versioning::VersionScheme;

mod stream;
mod util;
pub(super) use stream::digest_catalog_connection;
pub(super) use util::{
    canonical_json_string, checked_i64, checked_ordinal, create_private_file, hash_file,
    sidecar_path, sync_parent, validate_candidate_path,
};
use util::{
    checked_sqlite_usize, conversion_error, parse_json_column, read_u64, reject_nonempty_sidecars,
};

pub(super) const CATALOG_APPLICATION_ID: i64 = 0x434e_5259;

pub(super) const CATALOG_SCHEMA: &str = r#"
CREATE TABLE catalog_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    scope_json TEXT NOT NULL,
    logical_digest_sha256 TEXT NOT NULL CHECK (length(logical_digest_sha256) = 64),
    package_count INTEGER NOT NULL CHECK (package_count >= 0),
    provide_count INTEGER NOT NULL CHECK (provide_count >= 0),
    requirement_group_count INTEGER NOT NULL CHECK (requirement_group_count >= 0),
    requirement_atom_count INTEGER NOT NULL CHECK (requirement_atom_count >= 0),
    source_evidence_count INTEGER NOT NULL CHECK (source_evidence_count >= 0)
) STRICT;

CREATE TABLE catalog_source_evidence (
    ordinal INTEGER PRIMARY KEY CHECK (ordinal >= 0),
    evidence_json TEXT NOT NULL
) STRICT;

CREATE TABLE catalog_packages (
    package_key_sha256 TEXT PRIMARY KEY CHECK (length(package_key_sha256) = 64),
    origin_kind TEXT NOT NULL CHECK (origin_kind IN ('source', 'profile')),
    member_ordinal INTEGER CHECK (member_ordinal >= 0),
    source_identity TEXT NOT NULL,
    repository_identity TEXT NOT NULL,
    source_snapshot_sha256 TEXT CHECK (
        source_snapshot_sha256 IS NULL OR length(source_snapshot_sha256) = 64
    ),
    source_profile TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    package_release TEXT NOT NULL,
    architecture TEXT,
    debian_multi_arch TEXT,
    description TEXT,
    checksum TEXT NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    download_url TEXT NOT NULL,
    metadata TEXT,
    is_security_update INTEGER NOT NULL CHECK (is_security_update IN (0, 1)),
    severity TEXT,
    cve_ids TEXT,
    advisory_id TEXT,
    advisory_url TEXT,
    version_scheme TEXT NOT NULL,
    CHECK (
        (origin_kind = 'source' AND member_ordinal IS NULL AND source_snapshot_sha256 IS NULL)
        OR
        (origin_kind = 'profile' AND member_ordinal IS NOT NULL AND source_snapshot_sha256 IS NOT NULL)
    )
) STRICT, WITHOUT ROWID;

CREATE INDEX catalog_packages_name ON catalog_packages(name, version, package_key_sha256);
CREATE INDEX catalog_packages_origin ON catalog_packages(repository_identity, package_key_sha256);

CREATE TABLE catalog_provides (
    package_key_sha256 TEXT NOT NULL REFERENCES catalog_packages(package_key_sha256) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    capability TEXT NOT NULL,
    version TEXT,
    version_relation TEXT,
    kind TEXT NOT NULL,
    raw TEXT,
    version_scheme TEXT NOT NULL,
    architecture_qualifier_json TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    PRIMARY KEY (package_key_sha256, ordinal),
    CHECK ((version IS NULL) = (version_relation IS NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX catalog_provides_capability
    ON catalog_provides(capability, kind, package_key_sha256, ordinal);
CREATE INDEX catalog_provides_raw
    ON catalog_provides(raw, package_key_sha256, ordinal)
    WHERE raw IS NOT NULL AND raw != '';

CREATE TABLE catalog_requirement_groups (
    package_key_sha256 TEXT NOT NULL REFERENCES catalog_packages(package_key_sha256) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL,
    behavior TEXT NOT NULL,
    description TEXT,
    native_text TEXT,
    expression_json TEXT NOT NULL,
    PRIMARY KEY (package_key_sha256, ordinal)
) STRICT, WITHOUT ROWID;

CREATE TABLE catalog_requirement_atoms (
    package_key_sha256 TEXT NOT NULL,
    group_ordinal INTEGER NOT NULL CHECK (group_ordinal >= 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    capability TEXT NOT NULL,
    version_constraint TEXT,
    kind TEXT NOT NULL,
    dependency_type TEXT NOT NULL,
    raw TEXT,
    PRIMARY KEY (package_key_sha256, group_ordinal, ordinal),
    FOREIGN KEY (package_key_sha256, group_ordinal)
        REFERENCES catalog_requirement_groups(package_key_sha256, ordinal)
        ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX catalog_requirement_atoms_capability
    ON catalog_requirement_atoms(capability, kind, package_key_sha256, group_ordinal, ordinal);
"#;

const INSERT_PACKAGE: &str = r#"
INSERT INTO catalog_packages (
    package_key_sha256, origin_kind, member_ordinal, source_identity,
    repository_identity, source_snapshot_sha256, source_profile, name, version,
    package_release, architecture, debian_multi_arch, description, checksum, size,
    download_url, metadata, is_security_update, severity, cve_ids, advisory_id,
    advisory_url, version_scheme
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
    ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
)
"#;

const SELECT_PACKAGES: &str = r#"
SELECT package_key_sha256, origin_kind, member_ordinal, source_identity,
       repository_identity, source_snapshot_sha256, source_profile, name, version,
       package_release, architecture, debian_multi_arch, description, checksum, size,
       download_url, metadata, is_security_update, severity, cve_ids, advisory_id,
       advisory_url, version_scheme
FROM catalog_packages
"#;

/// Exact manifest values that bind one standalone catalog artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogBindingV1 {
    pub scope: CatalogScopeV1,
    pub artifact: CatalogArtifactV1,
    pub logical_digest_sha256: String,
    pub counts: CatalogCountsV1,
}

impl CatalogBindingV1 {
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        validate_sha256(&self.artifact.sha256, "catalog artifact SHA-256")?;
        validate_sha256(&self.logical_digest_sha256, "catalog logical digest")
    }
}

/// One bounded page from the lexically ordered names of downloadable catalog
/// packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPackageNamePageV1 {
    pub total: usize,
    pub names: Vec<String>,
}

/// A byte- and schema-verified immutable catalog connection.
pub struct CatalogReader {
    path: PathBuf,
    binding: CatalogBindingV1,
    connection: Connection,
}

impl CatalogReader {
    pub fn open_verified(path: impl AsRef<Path>, expected: &CatalogBindingV1) -> Result<Self> {
        Self::open_verified_inner(path.as_ref(), expected, true)
    }

    /// Open an exact signed catalog artifact without replaying its complete
    /// logical projection through Rust values.
    ///
    /// The Remi publisher performs the logical/schema replay before its
    /// dedicated universe role signs the artifact. A universe client has
    /// already verified that signature and the exact file SHA-256; it repeats
    /// the physical schema, integrity, binding, and relational-count checks
    /// here, then copies normalized rows directly between SQLite databases.
    /// This avoids turning one arbitrarily large presentation or expression
    /// field into synchronization memory.
    pub(in crate::repository) fn open_verified_signed_artifact(
        path: impl AsRef<Path>,
        expected: &CatalogBindingV1,
    ) -> Result<Self> {
        Self::open_verified_inner(path.as_ref(), expected, false)
    }

    fn open_verified_inner(
        path: &Path,
        expected: &CatalogBindingV1,
        verify_logical_content: bool,
    ) -> Result<Self> {
        expected.validate()?;
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            Error::IoError(format!(
                "inspect immutable catalog {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(Error::InvalidPath(format!(
                "immutable catalog {} must be a regular file, never a symlink",
                path.display()
            )));
        }
        reject_nonempty_sidecars(path)?;
        if metadata.len() != expected.artifact.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", expected.artifact.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        let actual_sha256 = hash_file(path)?;
        if actual_sha256 != expected.artifact.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: expected.artifact.sha256.clone(),
                actual: actual_sha256,
            });
        }

        let canonical_path = path.canonicalize()?;
        let mut uri = url::Url::from_file_path(&canonical_path).map_err(|_| {
            Error::InvalidPath(format!(
                "catalog path {} cannot be represented as an immutable SQLite URI",
                canonical_path.display()
            ))
        })?;
        uri.query_pairs_mut().append_pair("immutable", "1");
        let connection = Connection::open_with_flags(
            uri.as_str(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.execute_batch(
            "PRAGMA query_only = ON; PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;",
        )?;
        let application_id: i64 =
            connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if application_id != CATALOG_APPLICATION_ID {
            return Err(Error::ConfigError(format!(
                "catalog {} has application id {application_id:#x}; expected {CATALOG_APPLICATION_ID:#x}",
                path.display()
            )));
        }
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::InitError(format!(
                "catalog {} failed SQLite integrity_check: {integrity}",
                path.display()
            )));
        }
        let stored = read_binding(&connection, expected.artifact.clone())?;
        if &stored != expected {
            return Err(Error::ConflictError(format!(
                "catalog {} metadata does not match its exact manifest binding",
                path.display()
            )));
        }
        verify_relational_counts(&connection, expected.counts)?;
        let reader = Self {
            path: canonical_path,
            binding: expected.clone(),
            connection,
        };
        if verify_logical_content {
            reader.verify_logical_content()?;
        }
        Ok(reader)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn binding(&self) -> &CatalogBindingV1 {
        &self.binding
    }

    pub fn packages(&self) -> Result<Vec<CatalogPackageRecordV1>> {
        let mut packages = Vec::new();
        self.for_each_package(|package| {
            packages.push(package);
            Ok(())
        })?;
        Ok(packages)
    }

    /// Visit catalog packages in canonical key order with one complete package
    /// projection retained at a time.
    pub fn for_each_package(
        &self,
        visitor: impl FnMut(CatalogPackageRecordV1) -> Result<()>,
    ) -> Result<()> {
        for_each_package_connection(&self.connection, &self.binding.scope, visitor)
    }

    pub fn find_packages_by_name(&self, name: &str) -> Result<Vec<CatalogPackageRecordV1>> {
        validate_identity(name, "catalog package query name")?;
        self.load_packages(
            &format!("{SELECT_PACKAGES} WHERE name = ?1 ORDER BY package_key_sha256"),
            [name],
        )
    }

    /// Return one exact page of distinct downloadable package names.
    ///
    /// `offset` is zero-based. Both the total and the page use the same
    /// minimum-size predicate, and names are ordered with SQLite's binary
    /// collation so repeated reads have one lexical order independent of
    /// package insertion order.
    pub fn find_downloadable_package_name_page(
        &self,
        offset: usize,
        limit: usize,
        minimum_size: u64,
    ) -> Result<CatalogPackageNamePageV1> {
        if limit == 0 {
            return Err(Error::ConfigError(
                "catalog package-name page limit must be positive".to_string(),
            ));
        }
        let offset = checked_sqlite_usize(offset, "offset")?;
        let limit = checked_sqlite_usize(limit, "limit")?;
        let minimum_size = checked_i64(minimum_size, "minimum package size")?;

        let total = self.connection.query_row(
            "SELECT COUNT(DISTINCT name)
             FROM catalog_packages
             WHERE size >= ?1",
            [minimum_size],
            |row| row.get::<_, i64>(0),
        )?;
        let total = usize::try_from(total).map_err(|_| {
            Error::ConfigError(format!(
                "catalog downloadable package-name count {total} exceeds platform usize range"
            ))
        })?;

        let mut statement = self.connection.prepare(
            "SELECT DISTINCT name
             FROM catalog_packages
             WHERE size >= ?1
             ORDER BY name COLLATE BINARY
             LIMIT ?2 OFFSET ?3",
        )?;
        let names = statement
            .query_map(params![minimum_size, limit, offset], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(CatalogPackageNamePageV1 { total, names })
    }

    pub fn source_evidence(&self) -> Result<Vec<CatalogSourceEvidenceV1>> {
        load_source_evidence(&self.connection)
    }

    fn verify_logical_content(&self) -> Result<()> {
        let evidence = self.source_evidence()?;
        let (actual, counts) =
            digest_catalog_connection(&self.connection, &self.binding.scope, &evidence)?;
        if actual != self.binding.logical_digest_sha256 {
            return Err(Error::ChecksumMismatch {
                expected: self.binding.logical_digest_sha256.clone(),
                actual,
            });
        }
        if counts != self.binding.counts {
            return Err(Error::ConflictError(format!(
                "catalog {} logical counts disagree with its manifest binding",
                self.path.display()
            )));
        }
        Ok(())
    }

    fn load_packages<P>(&self, sql: &str, parameters: P) -> Result<Vec<CatalogPackageRecordV1>>
    where
        P: rusqlite::Params,
    {
        let mut statement = self.connection.prepare(sql)?;
        let base = statement
            .query_map(parameters, package_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        base.into_iter()
            .map(|mut package| {
                package.provides = load_provides(&self.connection, &package.package_key_sha256)?;
                package.requirement_groups =
                    load_requirement_groups(&self.connection, &package.package_key_sha256)?;
                package.validate(&self.binding.scope)?;
                Ok(package)
            })
            .collect()
    }
}

/// Build a new deterministic catalog candidate without touching operational SQLite.
pub fn write_catalog_candidate(
    path: impl AsRef<Path>,
    content: &CatalogContentV1,
) -> Result<CatalogBindingV1> {
    content.validate()?;
    let path = path.as_ref();
    validate_candidate_path(path)?;
    create_private_file(path)?;
    let result = write_catalog_candidate_inner(path, content);
    if result.is_err() {
        let _ = fs::remove_file(path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let _ = fs::remove_file(sidecar_path(path, suffix));
        }
    }
    result
}

fn write_catalog_candidate_inner(
    path: &Path,
    content: &CatalogContentV1,
) -> Result<CatalogBindingV1> {
    let logical_digest_sha256 = content.logical_digest_sha256()?;
    let counts = content.counts()?;
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch(&format!(
        "PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;
         PRAGMA page_size = 4096;
         PRAGMA auto_vacuum = NONE;
         PRAGMA application_id = {CATALOG_APPLICATION_ID};
         PRAGMA user_version = {CATALOG_CONTENT_SCHEMA_V1};"
    ))?;
    connection.execute_batch(CATALOG_SCHEMA)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO catalog_metadata (
             singleton, schema_version, scope_json, logical_digest_sha256,
             package_count, provide_count, requirement_group_count,
             requirement_atom_count, source_evidence_count
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            i64::from(CATALOG_CONTENT_SCHEMA_V1),
            canonical_json_string(&content.scope)?,
            &logical_digest_sha256,
            checked_i64(counts.packages, "package count")?,
            checked_i64(counts.provides, "provide count")?,
            checked_i64(counts.requirement_groups, "requirement group count")?,
            checked_i64(counts.requirement_atoms, "requirement atom count")?,
            checked_i64(counts.source_evidence, "source evidence count")?,
        ],
    )?;
    for (ordinal, evidence) in content.source_evidence.iter().enumerate() {
        transaction.execute(
            "INSERT INTO catalog_source_evidence (ordinal, evidence_json) VALUES (?1, ?2)",
            params![
                checked_ordinal(ordinal, "source evidence")?,
                canonical_json_string(evidence)?,
            ],
        )?;
    }
    for package in &content.packages {
        insert_package(&transaction, package)?;
    }
    transaction.commit()?;
    connection.execute_batch("VACUUM;")?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(Error::InitError(format!(
            "new catalog {} failed SQLite integrity_check: {integrity}",
            path.display()
        )));
    }
    connection
        .close()
        .map_err(|(_, error)| Error::Database(error))?;
    File::open(path)?.sync_all()?;
    sync_parent(path)?;
    let artifact = CatalogArtifactV1 {
        sha256: hash_file(path)?,
        size: fs::metadata(path)?.len(),
    };
    let binding = CatalogBindingV1 {
        scope: content.scope.clone(),
        artifact,
        logical_digest_sha256,
        counts,
    };
    CatalogReader::open_verified(path, &binding)?;
    Ok(binding)
}

pub(super) fn insert_package(
    connection: &Connection,
    package: &CatalogPackageRecordV1,
) -> Result<()> {
    insert_package_base(connection, package)?;
    for (ordinal, provide) in package.provides.iter().enumerate() {
        insert_provide(
            connection,
            &package.package_key_sha256,
            checked_ordinal(ordinal, "provide")?,
            provide,
        )?;
    }
    for (group_ordinal, group) in package.requirement_groups.iter().enumerate() {
        insert_requirement_group(
            connection,
            &package.package_key_sha256,
            checked_ordinal(group_ordinal, "requirement group")?,
            group,
        )?;
    }
    Ok(())
}

fn insert_package_base(connection: &Connection, package: &CatalogPackageRecordV1) -> Result<()> {
    let (origin_kind, member_ordinal, source_identity, repository_identity, snapshot) =
        match &package.origin {
            CatalogPackageOriginV1::Source {
                source_identity,
                repository_identity,
            } => (
                "source",
                None,
                source_identity.as_str(),
                repository_identity.as_str(),
                None,
            ),
            CatalogPackageOriginV1::Profile {
                member_ordinal,
                source_identity,
                repository_identity,
                source_snapshot_sha256,
            } => (
                "profile",
                Some(i64::from(*member_ordinal)),
                source_identity.as_str(),
                repository_identity.as_str(),
                Some(source_snapshot_sha256.as_str()),
            ),
        };
    connection.execute(
        INSERT_PACKAGE,
        params![
            &package.package_key_sha256,
            origin_kind,
            member_ordinal,
            source_identity,
            repository_identity,
            snapshot,
            &package.source_profile,
            &package.name,
            &package.version,
            &package.package_release,
            &package.architecture,
            package.debian_multi_arch.map(DebianMultiArch::as_str),
            &package.description,
            &package.checksum,
            checked_i64(package.size, "package size")?,
            &package.download_url,
            &package.metadata,
            if package.is_security_update {
                1_i64
            } else {
                0_i64
            },
            &package.severity,
            &package.cve_ids,
            &package.advisory_id,
            &package.advisory_url,
            package.version_scheme.as_str(),
        ],
    )?;
    Ok(())
}

fn insert_provide(
    connection: &Connection,
    package_key_sha256: &str,
    ordinal: i64,
    provide: &CatalogProvideRecordV1,
) -> Result<()> {
    connection.execute(
        "INSERT INTO catalog_provides (
             package_key_sha256, ordinal, capability, version, version_relation,
             kind, raw, version_scheme, architecture_qualifier_json, provenance_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            package_key_sha256,
            ordinal,
            &provide.capability,
            &provide.version,
            provide.version_relation.map(ProvideVersionRelation::as_str),
            &provide.kind,
            &provide.raw,
            provide.version_scheme.as_str(),
            canonical_json_string(&provide.architecture_qualifier)?,
            canonical_json_string(&provide.provenance)?,
        ],
    )?;
    Ok(())
}

fn insert_requirement_group(
    connection: &Connection,
    package_key_sha256: &str,
    group_ordinal: i64,
    group: &CatalogRequirementGroupV1,
) -> Result<()> {
    connection.execute(
        "INSERT INTO catalog_requirement_groups (
                 package_key_sha256, ordinal, kind, behavior, description,
                 native_text, expression_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            package_key_sha256,
            group_ordinal,
            &group.kind,
            &group.behavior,
            &group.description,
            &group.native_text,
            &group.expression_json,
        ],
    )?;
    for (ordinal, atom) in group.atoms.iter().enumerate() {
        connection.execute(
            "INSERT INTO catalog_requirement_atoms (
                     package_key_sha256, group_ordinal, ordinal, capability,
                     version_constraint, kind, dependency_type, raw
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                package_key_sha256,
                group_ordinal,
                checked_ordinal(ordinal, "requirement atom")?,
                &atom.capability,
                &atom.version_constraint,
                &atom.kind,
                &atom.dependency_type,
                &atom.raw,
            ],
        )?;
    }
    Ok(())
}

fn insert_provides(
    connection: &Connection,
    package_key_sha256: &str,
    provides: &[CatalogProvideRecordV1],
) -> Result<()> {
    for (ordinal, provide) in provides.iter().enumerate() {
        insert_provide(
            connection,
            package_key_sha256,
            checked_ordinal(ordinal, "provide")?,
            provide,
        )?;
    }
    Ok(())
}

pub(super) fn package_by_key(
    connection: &Connection,
    scope: &CatalogScopeV1,
    package_key_sha256: &str,
) -> Result<CatalogPackageRecordV1> {
    let mut package = connection
        .query_row(
            &format!("{SELECT_PACKAGES} WHERE package_key_sha256 = ?1"),
            [package_key_sha256],
            package_from_row,
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("catalog package key {package_key_sha256}")))?;
    package.provides = load_provides(connection, package_key_sha256)?;
    package.requirement_groups = load_requirement_groups(connection, package_key_sha256)?;
    package.validate(scope)?;
    Ok(package)
}

pub(super) fn replace_package_provides(
    connection: &Connection,
    package: &CatalogPackageRecordV1,
) -> Result<()> {
    connection.execute(
        "DELETE FROM catalog_provides WHERE package_key_sha256 = ?1",
        [&package.package_key_sha256],
    )?;
    insert_provides(connection, &package.package_key_sha256, &package.provides)
}

fn package_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogPackageRecordV1> {
    let origin_kind: String = row.get(1)?;
    let member_ordinal: Option<i64> = row.get(2)?;
    let source_identity: String = row.get(3)?;
    let repository_identity: String = row.get(4)?;
    let source_snapshot_sha256: Option<String> = row.get(5)?;
    let origin = match (origin_kind.as_str(), member_ordinal, source_snapshot_sha256) {
        ("source", None, None) => CatalogPackageOriginV1::Source {
            source_identity,
            repository_identity,
        },
        ("profile", Some(member_ordinal), Some(source_snapshot_sha256)) => {
            CatalogPackageOriginV1::Profile {
                member_ordinal: u32::try_from(member_ordinal).map_err(|_| {
                    conversion_error(
                        2,
                        format!("invalid profile member ordinal {member_ordinal}"),
                    )
                })?,
                source_identity,
                repository_identity,
                source_snapshot_sha256,
            }
        }
        _ => {
            return Err(conversion_error(
                1,
                "invalid catalog package origin tuple".to_string(),
            ));
        }
    };
    let multi_arch = row
        .get::<_, Option<String>>(11)?
        .map(|value| {
            DebianMultiArch::parse_exact(&value).map_err(|error| conversion_error(11, error))
        })
        .transpose()?;
    let version_scheme_raw: String = row.get(22)?;
    let version_scheme = VersionScheme::from_str(&version_scheme_raw)
        .map_err(|error| conversion_error(22, error))?;
    let size: i64 = row.get(14)?;
    Ok(CatalogPackageRecordV1 {
        package_key_sha256: row.get(0)?,
        origin,
        source_profile: row.get(6)?,
        name: row.get(7)?,
        version: row.get(8)?,
        package_release: row.get(9)?,
        architecture: row.get(10)?,
        debian_multi_arch: multi_arch,
        description: row.get(12)?,
        checksum: row.get(13)?,
        size: u64::try_from(size)
            .map_err(|_| conversion_error(14, format!("invalid package size {size}")))?,
        download_url: row.get(15)?,
        metadata: row.get(16)?,
        is_security_update: row.get::<_, i64>(17)? != 0,
        severity: row.get(18)?,
        cve_ids: row.get(19)?,
        advisory_id: row.get(20)?,
        advisory_url: row.get(21)?,
        version_scheme,
        provides: Vec::new(),
        requirement_groups: Vec::new(),
    })
}

fn load_provides(connection: &Connection, key: &str) -> Result<Vec<CatalogProvideRecordV1>> {
    let mut statement = connection.prepare(
        "SELECT capability, version, version_relation, kind, raw, version_scheme,
                architecture_qualifier_json, provenance_json
         FROM catalog_provides
         WHERE package_key_sha256 = ?1
         ORDER BY ordinal",
    )?;
    statement
        .query_map([key], provide_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::from)
}

fn provide_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogProvideRecordV1> {
    let version: Option<String> = row.get(1)?;
    let relation = row
        .get::<_, Option<String>>(2)?
        .map(|value| {
            ProvideVersionRelation::parse_exact(&value).map_err(|error| conversion_error(2, error))
        })
        .transpose()?;
    if version.is_some() != relation.is_some() {
        return Err(conversion_error(
            2,
            "catalog provide version and relation disagree".to_string(),
        ));
    }
    let scheme: String = row.get(5)?;
    Ok(CatalogProvideRecordV1 {
        capability: row.get(0)?,
        version,
        version_relation: relation,
        kind: row.get(3)?,
        raw: row.get(4)?,
        version_scheme: VersionScheme::from_str(&scheme)
            .map_err(|error| conversion_error(5, error))?,
        architecture_qualifier: parse_json_column(row, 6)?,
        provenance: parse_json_column(row, 7)?,
    })
}

fn load_requirement_groups(
    connection: &Connection,
    key: &str,
) -> Result<Vec<CatalogRequirementGroupV1>> {
    let mut statement = connection.prepare(
        "SELECT ordinal, kind, behavior, description, native_text, expression_json
         FROM catalog_requirement_groups
         WHERE package_key_sha256 = ?1
         ORDER BY ordinal",
    )?;
    let groups = statement
        .query_map([key], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                CatalogRequirementGroupV1 {
                    kind: row.get(1)?,
                    behavior: row.get(2)?,
                    description: row.get(3)?,
                    native_text: row.get(4)?,
                    expression_json: row.get(5)?,
                    atoms: Vec::new(),
                },
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    groups
        .into_iter()
        .map(|(ordinal, mut group)| {
            group.atoms = load_requirement_atoms(connection, key, ordinal)?;
            Ok(group)
        })
        .collect()
}

fn load_requirement_atoms(
    connection: &Connection,
    key: &str,
    group_ordinal: i64,
) -> Result<Vec<CatalogRequirementAtomV1>> {
    let mut statement = connection.prepare(
        "SELECT capability, version_constraint, kind, dependency_type, raw
         FROM catalog_requirement_atoms
         WHERE package_key_sha256 = ?1 AND group_ordinal = ?2
         ORDER BY ordinal",
    )?;
    statement
        .query_map(params![key, group_ordinal], |row| {
            Ok(CatalogRequirementAtomV1 {
                capability: row.get(0)?,
                version_constraint: row.get(1)?,
                kind: row.get(2)?,
                dependency_type: row.get(3)?,
                raw: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::from)
}

fn load_source_evidence(connection: &Connection) -> Result<Vec<CatalogSourceEvidenceV1>> {
    let mut statement =
        connection.prepare("SELECT evidence_json FROM catalog_source_evidence ORDER BY ordinal")?;
    statement
        .query_map([], |row| parse_json_column(row, 0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::from)
}

pub(super) fn for_each_package_connection(
    connection: &Connection,
    scope: &CatalogScopeV1,
    mut visitor: impl FnMut(CatalogPackageRecordV1) -> Result<()>,
) -> Result<()> {
    let mut statement =
        connection.prepare(&format!("{SELECT_PACKAGES} ORDER BY package_key_sha256"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let mut package = package_from_row(row)?;
        package.provides = load_provides(connection, &package.package_key_sha256)?;
        package.requirement_groups =
            load_requirement_groups(connection, &package.package_key_sha256)?;
        package.validate(scope)?;
        visitor(package)?;
    }
    Ok(())
}

fn read_binding(connection: &Connection, artifact: CatalogArtifactV1) -> Result<CatalogBindingV1> {
    connection
        .query_row(
            "SELECT schema_version, scope_json, logical_digest_sha256,
                    package_count, provide_count, requirement_group_count,
                    requirement_atom_count, source_evidence_count
             FROM catalog_metadata WHERE singleton = 1",
            [],
            |row| {
                let schema_version: i64 = row.get(0)?;
                if schema_version != i64::from(CATALOG_CONTENT_SCHEMA_V1) {
                    return Err(conversion_error(
                        0,
                        format!("unsupported catalog schema {schema_version}"),
                    ));
                }
                Ok(CatalogBindingV1 {
                    scope: parse_json_column(row, 1)?,
                    artifact,
                    logical_digest_sha256: row.get(2)?,
                    counts: CatalogCountsV1 {
                        packages: read_u64(row, 3, "package count")?,
                        provides: read_u64(row, 4, "provide count")?,
                        requirement_groups: read_u64(row, 5, "requirement group count")?,
                        requirement_atoms: read_u64(row, 6, "requirement atom count")?,
                        source_evidence: read_u64(row, 7, "source evidence count")?,
                    },
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::InitError("catalog metadata singleton is missing".to_string()))
}

fn verify_relational_counts(connection: &Connection, expected: CatalogCountsV1) -> Result<()> {
    for (table, value, label) in [
        ("catalog_packages", expected.packages, "packages"),
        ("catalog_provides", expected.provides, "provides"),
        (
            "catalog_requirement_groups",
            expected.requirement_groups,
            "requirement groups",
        ),
        (
            "catalog_requirement_atoms",
            expected.requirement_atoms,
            "requirement atoms",
        ),
        (
            "catalog_source_evidence",
            expected.source_evidence,
            "source evidence",
        ),
    ] {
        let actual: i64 =
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
        if u64::try_from(actual).ok() != Some(value) {
            return Err(Error::ConflictError(format!(
                "catalog relational {label} count {actual} does not match manifest count {value}"
            )));
        }
    }
    let mut violations = connection.prepare("PRAGMA foreign_key_check")?;
    if violations.exists([])? {
        return Err(Error::InitError(
            "catalog failed SQLite foreign_key_check".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
