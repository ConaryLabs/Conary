// conary-core/src/repository/catalog/candidate.rs

//! Bounded construction of one deterministic standalone catalog candidate.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::store::{
    CATALOG_APPLICATION_ID, CATALOG_SCHEMA, canonical_json_string, checked_i64, checked_ordinal,
    create_private_file, digest_catalog_connection, hash_file, insert_package, package_by_key,
    replace_package_provides, sync_parent, validate_candidate_path,
};
use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogBindingV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogProfileCandidateScratchV1, CatalogProvideRecordV1,
    CatalogReader, CatalogScopeV1, CatalogScratchAdmission, CatalogSourceCandidateScratchV1,
    CatalogSourceEvidenceV1,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryRequirementExpression;
use crate::repository::parsers::{ArchPackageFragmentKind, ArchPackageRecord};

mod lifecycle;
use lifecycle::{read_positive_pragma, remove_candidate_files, table_exists};

/// Private catalog writer that retains one normalized package at a time.
pub struct CatalogCandidateWriter {
    path: PathBuf,
    scope: CatalogScopeV1,
    connection: Option<Connection>,
    scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
    growth_lease: Option<Box<dyn Send>>,
    database_bytes_bound: Option<u64>,
    complete: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::repository) struct CatalogProvideMerge {
    pub matched_packages: usize,
    pub added: usize,
    pub already_known: usize,
}

impl CatalogCandidateWriter {
    pub fn create(path: impl AsRef<Path>, scope: CatalogScopeV1) -> Result<Self> {
        Self::create_inner(path.as_ref(), scope, None, None, None)
    }

    /// Create a writer whose final compaction requires a retained scratch lease.
    pub fn create_with_scratch_admission(
        path: impl AsRef<Path>,
        scope: CatalogScopeV1,
        scratch_admission: Arc<dyn CatalogScratchAdmission>,
    ) -> Result<Self> {
        Self::create_inner(path.as_ref(), scope, Some(scratch_admission), None, None)
    }

    /// Create a profile writer only after its complete construction bound is reserved.
    pub fn create_with_profile_scratch_admission(
        path: impl AsRef<Path>,
        scope: CatalogScopeV1,
        scratch_admission: Arc<dyn CatalogScratchAdmission>,
        requirement: CatalogProfileCandidateScratchV1,
    ) -> Result<Self> {
        if !matches!(scope, CatalogScopeV1::Profile { .. }) {
            return Err(Error::ConfigError(
                "profile candidate scratch admission requires profile scope".to_string(),
            ));
        }
        requirement.validate()?;
        let database_bytes_bound = requirement.candidate_database_bytes;
        let lease = scratch_admission.reserve_profile_candidate(path.as_ref(), requirement)?;
        Self::create_inner(
            path.as_ref(),
            scope,
            Some(scratch_admission),
            Some(lease),
            Some(database_bytes_bound),
        )
    }

    /// Create a native source writer only after its complete construction bound is reserved.
    pub fn create_with_source_scratch_admission(
        path: impl AsRef<Path>,
        scope: CatalogScopeV1,
        scratch_admission: Arc<dyn CatalogScratchAdmission>,
        requirement: CatalogSourceCandidateScratchV1,
    ) -> Result<Self> {
        if !matches!(scope, CatalogScopeV1::Source { .. }) {
            return Err(Error::ConfigError(
                "source candidate scratch admission requires source scope".to_string(),
            ));
        }
        requirement.validate()?;
        let database_bytes_bound = requirement.candidate_database_bytes;
        let lease = scratch_admission.reserve_source_candidate(path.as_ref(), requirement)?;
        Self::create_inner(
            path.as_ref(),
            scope,
            Some(scratch_admission),
            Some(lease),
            Some(database_bytes_bound),
        )
    }

    fn create_inner(
        path: &Path,
        scope: CatalogScopeV1,
        scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
        growth_lease: Option<Box<dyn Send>>,
        database_bytes_bound: Option<u64>,
    ) -> Result<Self> {
        scope.validate()?;
        let path = path.to_path_buf();
        validate_candidate_path(&path)?;
        create_private_file(&path)?;
        let result = Self::open_created(
            path.clone(),
            scope,
            scratch_admission,
            growth_lease,
            database_bytes_bound,
        );
        if result.is_err() {
            remove_candidate_files(&path);
        }
        result
    }

    fn open_created(
        path: PathBuf,
        scope: CatalogScopeV1,
        scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
        growth_lease: Option<Box<dyn Send>>,
        database_bytes_bound: Option<u64>,
    ) -> Result<Self> {
        let connection = Connection::open_with_flags(
            &path,
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
        // RPM filelists records join back to their primary package by the
        // authenticated package checksum. Keep that construction-only lookup
        // indexed: without it, one complete filelists document performs a
        // package-table scan for every package record. The index is dropped
        // before publication so it does not change the immutable catalog
        // schema or invalidate otherwise reusable projections.
        connection.execute_batch(
            "CREATE INDEX catalog_ingest_packages_checksum
                 ON catalog_packages(checksum, package_key_sha256);",
        )?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            path,
            scope,
            connection: Some(connection),
            scratch_admission,
            growth_lease,
            database_bytes_bound,
            complete: false,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &CatalogScopeV1 {
        &self.scope
    }

    /// Insert one complete package projection into private SQLite state.
    pub fn package(&mut self, mut package: CatalogPackageRecordV1) -> Result<()> {
        package.canonicalize_for_scope(&self.scope)?;
        insert_package(self.connection()?, &package)
    }

    /// Project one verified source member into a profile catalog while
    /// streaming its normalized relation rows.
    pub(in crate::repository) fn copy_profile_member(
        &mut self,
        reader: &CatalogReader,
        member_ordinal: u32,
        source_identity: String,
        repository_identity: String,
        source_snapshot_sha256: String,
    ) -> Result<()> {
        let scope = self.scope.clone();
        let origin = CatalogPackageOriginV1::Profile {
            member_ordinal,
            source_identity,
            repository_identity,
            source_snapshot_sha256,
        };
        reader.copy_packages_to(self.connection()?, &scope, Some(&origin))
    }

    pub(in crate::repository) fn extend_package_provides(
        &mut self,
        join: &str,
        checksum: &str,
        name: &str,
        version: &str,
        architecture: Option<&str>,
        provides: Vec<CatalogProvideRecordV1>,
    ) -> Result<CatalogProvideMerge> {
        self.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_ingest_join_marks (
                 join_kind TEXT NOT NULL,
                 package_key_sha256 TEXT NOT NULL
                     REFERENCES catalog_packages(package_key_sha256) ON DELETE CASCADE,
                 PRIMARY KEY (join_kind, package_key_sha256)
             ) STRICT, WITHOUT ROWID;",
        )?;
        let primary = self
            .connection()?
            .query_row(
                "SELECT package_key_sha256, name, version, architecture
                 FROM catalog_packages WHERE checksum = ?1
                 ORDER BY package_key_sha256 LIMIT 1",
                [checksum],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                Error::ParseError(format!(
                    "signed filelists.xml publishes file records for pkgid {checksum}, which the signed primary.xml does not publish"
                ))
            })?;
        let (package_key, primary_name, primary_version, primary_architecture) = primary;
        let disagreement = if primary_name != name {
            Some(("name", name, primary_name.as_str()))
        } else if primary_architecture.as_deref() != architecture {
            Some((
                "architecture",
                architecture.unwrap_or_default(),
                primary_architecture.as_deref().unwrap_or_default(),
            ))
        } else if primary_version != version {
            Some(("version", version, primary_version.as_str()))
        } else {
            None
        };
        if let Some((field, child, primary)) = disagreement {
            return Err(Error::ParseError(format!(
                "signed filelists.xml and primary.xml disagree on pkgid {checksum}: filelists {field} is '{child}' but primary {field} is '{primary}'"
            )));
        }
        if let Err(error) = self.connection()?.execute(
            "INSERT INTO catalog_ingest_join_marks (join_kind, package_key_sha256)
             VALUES (?1, ?2)",
            params![join, &package_key],
        ) {
            return Err(Error::ConflictError(format!(
                "authenticated child metadata repeats package {name} {version} ({checksum}): {error}"
            )));
        }

        let mut package = package_by_key(self.connection()?, &self.scope, &package_key)?;
        let mut result = CatalogProvideMerge {
            matched_packages: 1,
            ..CatalogProvideMerge::default()
        };
        for provide in provides {
            if package.provides.contains(&provide) {
                result.already_known += 1;
            } else {
                package.provides.push(provide);
                result.added += 1;
            }
        }
        package.canonicalize_for_scope(&self.scope)?;
        replace_package_provides(self.connection()?, &package)?;
        Ok(result)
    }

    pub(in crate::repository) fn finish_package_join(&mut self, join: &str) -> Result<()> {
        let marked: i64 = self.connection()?.query_row(
            "SELECT count(*) FROM catalog_ingest_join_marks WHERE join_kind = ?1",
            [join],
            |row| row.get(0),
        )?;
        let packages: i64 =
            self.connection()?
                .query_row("SELECT count(*) FROM catalog_packages", [], |row| {
                    row.get(0)
                })?;
        if marked != packages {
            let missing = self.connection()?.query_row(
                "SELECT name, version, checksum FROM catalog_packages AS package
                 WHERE NOT EXISTS (
                     SELECT 1 FROM catalog_ingest_join_marks AS mark
                     WHERE mark.join_kind = ?1
                       AND mark.package_key_sha256 = package.package_key_sha256
                 )
                 ORDER BY package.package_key_sha256 LIMIT 1",
                [join],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?;
            return Err(Error::ParseError(format!(
                "signed filelists.xml publishes no file record for package {} {} (pkgid {})",
                missing.0, missing.1, missing.2
            )));
        }
        self.connection()?.execute(
            "DELETE FROM catalog_ingest_join_marks WHERE join_kind = ?1",
            [join],
        )?;
        Ok(())
    }

    /// Audit RPM path requirements against the exact primary.xml projection
    /// already retained by this candidate transaction.
    pub(in crate::repository) fn validate_rpm_primary_file_requirements(
        &self,
        repo_url: &str,
    ) -> Result<()> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT package.name, package.version, requirement.expression_json
             FROM catalog_requirement_groups AS requirement
             JOIN catalog_packages AS package
               ON package.package_key_sha256 = requirement.package_key_sha256
             WHERE requirement.kind IN ('depends', 'pre_depends')
               AND EXISTS (
                   SELECT 1 FROM catalog_requirement_atoms AS atom
                   WHERE atom.package_key_sha256 = requirement.package_key_sha256
                     AND atom.group_ordinal = requirement.ordinal
                     AND substr(atom.capability, 1, 1) = '/'
               )
             ORDER BY requirement.package_key_sha256, requirement.ordinal",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let version: String = row.get(1)?;
            let expression_json: String = row.get(2)?;
            let expression: RepositoryRequirementExpression =
                serde_json::from_str(&expression_json)?;
            let mut provided = |path: &str| -> Result<bool> {
                Ok(connection
                    .query_row(
                        "SELECT 1 FROM catalog_provides
                         WHERE capability = ?1 AND kind = 'file'
                         ORDER BY package_key_sha256, ordinal LIMIT 1",
                        [path],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            };
            crate::repository::parsers::fedora::audit::require_primary_file_providers(
                repo_url,
                &name,
                &version,
                &expression,
                &mut provided,
            )?;
        }
        Ok(())
    }

    pub(in crate::repository) fn stage_arch_package_fragment(
        &self,
        directory: String,
        kind: ArchPackageFragmentKind,
        content: String,
    ) -> Result<()> {
        if directory.is_empty() {
            return Err(Error::ParseError(
                "Arch repository package directory is empty".to_string(),
            ));
        }
        let connection = self.connection()?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_ingest_arch_fragments (
                 directory TEXT PRIMARY KEY,
                 desc TEXT,
                 depends TEXT,
                 CHECK (desc IS NOT NULL OR depends IS NOT NULL)
             ) STRICT, WITHOUT ROWID;",
        )?;
        let (field, label) = match kind {
            ArchPackageFragmentKind::Desc => ("desc", "desc"),
            ArchPackageFragmentKind::Depends => ("depends", "depends"),
        };
        let changed = connection.execute(
            &format!(
                "INSERT INTO catalog_ingest_arch_fragments (directory, {field}) VALUES (?1, ?2)
                 ON CONFLICT(directory) DO UPDATE SET {field} = excluded.{field}
                 WHERE catalog_ingest_arch_fragments.{field} IS NULL"
            ),
            params![directory, content],
        )?;
        if changed != 1 {
            return Err(Error::ParseError(format!(
                "Arch repository repeats {label} metadata for {directory}"
            )));
        }
        Ok(())
    }

    pub(in crate::repository) fn take_arch_package_record(
        &self,
    ) -> Result<Option<ArchPackageRecord>> {
        let connection = self.connection()?;
        if !table_exists(connection, "catalog_ingest_arch_fragments")? {
            return Ok(None);
        }
        let record = connection
            .query_row(
                "SELECT directory, desc, depends FROM catalog_ingest_arch_fragments
                 ORDER BY directory LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((directory, desc, depends)) = record else {
            connection.execute_batch("DROP TABLE catalog_ingest_arch_fragments")?;
            return Ok(None);
        };
        let desc = desc.ok_or_else(|| {
            Error::ParseError(format!(
                "Arch repository has depends metadata without desc metadata for {directory}"
            ))
        })?;
        connection.execute(
            "DELETE FROM catalog_ingest_arch_fragments WHERE directory = ?1",
            [&directory],
        )?;
        Ok(Some(ArchPackageRecord {
            directory,
            desc,
            depends,
        }))
    }

    /// Bind exact source evidence, calculate the canonical logical identity by
    /// ordered iteration, and reopen the resulting artifact before returning.
    pub fn finish(
        mut self,
        source_evidence: Vec<CatalogSourceEvidenceV1>,
    ) -> Result<CatalogBindingV1> {
        let scope = self.scope.clone();
        if table_exists(self.connection()?, "catalog_ingest_join_marks")? {
            let unfinished: i64 = self.connection()?.query_row(
                "SELECT count(*) FROM catalog_ingest_join_marks",
                [],
                |row| row.get(0),
            )?;
            if unfinished != 0 {
                return Err(Error::ConflictError(format!(
                    "catalog candidate has {unfinished} unfinished authenticated child join marks"
                )));
            }
            self.connection()?
                .execute_batch("DROP TABLE catalog_ingest_join_marks")?;
        }
        if table_exists(self.connection()?, "catalog_ingest_arch_fragments")? {
            return Err(Error::ConflictError(
                "catalog candidate has unfinished Arch package fragments".to_string(),
            ));
        }
        self.connection()?
            .execute_batch("DROP INDEX catalog_ingest_packages_checksum")?;
        self.connection()?.execute_batch("COMMIT")?;

        let (logical_digest_sha256, counts) =
            digest_catalog_connection(self.connection()?, &scope, &source_evidence)?;

        {
            let connection = self.connection()?;
            connection.execute_batch("BEGIN IMMEDIATE")?;
            connection.execute(
                "INSERT INTO catalog_metadata (
                     singleton, schema_version, scope_json, logical_digest_sha256,
                     package_count, provide_count, requirement_group_count,
                     requirement_atom_count, source_evidence_count
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    i64::from(CATALOG_CONTENT_SCHEMA_V1),
                    canonical_json_string(&scope)?,
                    &logical_digest_sha256,
                    checked_i64(counts.packages, "package count")?,
                    checked_i64(counts.provides, "provide count")?,
                    checked_i64(counts.requirement_groups, "requirement group count")?,
                    checked_i64(counts.requirement_atoms, "requirement atom count")?,
                    checked_i64(counts.source_evidence, "source evidence count")?,
                ],
            )?;
            for (ordinal, evidence) in source_evidence.iter().enumerate() {
                connection.execute(
                    "INSERT INTO catalog_source_evidence (ordinal, evidence_json) VALUES (?1, ?2)",
                    params![
                        checked_ordinal(ordinal, "source evidence")?,
                        canonical_json_string(evidence)?,
                    ],
                )?;
            }
            connection.execute_batch("COMMIT")?;
        }
        if self.growth_lease.is_some() {
            std::fs::File::open(&self.path)?.sync_all()?;
            sync_parent(&self.path)?;
            let candidate_bytes = std::fs::metadata(&self.path)?.len();
            let database_bytes_bound = self.database_bytes_bound.ok_or_else(|| {
                Error::InternalError(
                    "catalog candidate growth lease has no database byte bound".to_string(),
                )
            })?;
            if candidate_bytes > database_bytes_bound {
                return Err(Error::InternalError(format!(
                    "catalog candidate used {candidate_bytes} database bytes above its admitted {database_bytes_bound}-byte bound"
                )));
            }
            drop(self.growth_lease.take());
            self.database_bytes_bound = None;
        }
        let connection = self.connection()?;
        let page_size = read_positive_pragma(connection, "page_size")?;
        let page_count = read_positive_pragma(connection, "page_count")?;
        let scratch = super::CatalogFinalizationScratchV1::from_page_facts(page_size, page_count)?;
        let _scratch_lease = self
            .scratch_admission
            .as_ref()
            .map(|admission| admission.reserve_finalization(&self.path, scratch))
            .transpose()?;
        connection.execute_batch("VACUUM")?;
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::InitError(format!(
                "new catalog {} failed SQLite integrity_check: {integrity}",
                self.path.display()
            )));
        }

        self.connection
            .take()
            .expect("catalog candidate connection exists")
            .close()
            .map_err(|(_, error)| Error::Database(error))?;
        std::fs::File::open(&self.path)?.sync_all()?;
        sync_parent(&self.path)?;
        let artifact = CatalogArtifactV1 {
            sha256: hash_file(&self.path)?,
            size: fs::metadata(&self.path)?.len(),
        };
        let binding = CatalogBindingV1 {
            scope,
            artifact,
            logical_digest_sha256,
            counts,
        };
        drop(CatalogReader::open_verified(&self.path, &binding)?);
        self.complete = true;
        Ok(binding)
    }

    fn connection(&self) -> Result<&Connection> {
        self.connection.as_ref().ok_or_else(|| {
            Error::InternalError("catalog candidate writer has no open connection".to_string())
        })
    }
}

impl Drop for CatalogCandidateWriter {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        if let Some(connection) = self.connection.take() {
            let _ = connection.close();
        }
        remove_candidate_files(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::catalog::{
        CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1, CatalogCopyScratchV1, CatalogFinalizationScratchV1,
        CatalogMetadataScratchV1, CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
        CatalogPackageOriginV1, CatalogPackageRecordV1, CatalogProfileCandidateScratchV1,
        CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1,
        CatalogScratchCapacityError,
    };
    use crate::repository::dependency_model::{
        ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryRequirementClause,
    };
    use crate::repository::dependency_source::{CapabilityProvenance, SourcePackageFormat};
    use crate::repository::versioning::VersionScheme;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingAdmission {
        requirement: Mutex<Option<CatalogFinalizationScratchV1>>,
        lease_drops: Arc<AtomicUsize>,
        refuse: bool,
    }

    struct RecordingLease(Arc<AtomicUsize>);

    impl Drop for RecordingLease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl CatalogScratchAdmission for RecordingAdmission {
        fn reserve_source_candidate(
            &self,
            _candidate_path: &Path,
            _requirement: CatalogSourceCandidateScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("finalization-only writer must not request source growth admission")
        }

        fn reserve_profile_candidate(
            &self,
            _candidate_path: &Path,
            _requirement: CatalogProfileCandidateScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("finalization-only writer must not request profile growth admission")
        }

        fn reserve_metadata(
            &self,
            _work_directory: &Path,
            _requirement: CatalogMetadataScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("candidate writer must not request metadata admission")
        }

        fn stream_metadata(
            &self,
            _work_directory: &Path,
            _requirement: CatalogMetadataStreamScratchV1,
        ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
            panic!("candidate writer must not request streamed metadata admission")
        }

        fn reserve_finalization(
            &self,
            _candidate_path: &Path,
            requirement: CatalogFinalizationScratchV1,
        ) -> Result<Box<dyn Send>> {
            *self.requirement.lock().unwrap() = Some(requirement);
            if self.refuse {
                return Err(CatalogScratchCapacityError {
                    required_bytes: requirement.required_additional_bytes,
                    available_bytes: requirement.required_additional_bytes - 1,
                    reserved_bytes: 0,
                }
                .into());
            }
            Ok(Box::new(RecordingLease(Arc::clone(&self.lease_drops))))
        }

        fn reserve_copy(
            &self,
            _destination_root: &Path,
            _requirement: CatalogCopyScratchV1,
        ) -> Result<Box<dyn Send>> {
            panic!("candidate writer must not request a catalog-copy reservation")
        }
    }

    fn scope() -> CatalogScopeV1 {
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
        }
    }

    fn evidence() -> Vec<CatalogSourceEvidenceV1> {
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: crate::repository::catalog::SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.gz".to_string(),
            sha256: "a".repeat(64),
            size: 1,
        }]
    }

    fn rpm_package(
        name: &str,
        checksum: &str,
        paths: &[&str],
        required_path: Option<&str>,
    ) -> CatalogPackageRecordV1 {
        let mut provides = vec![CatalogProvideRecordV1 {
            capability: name.to_string(),
            version: Some("1-1".to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            kind: "package".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }];
        provides.extend(paths.iter().map(|path| CatalogProvideRecordV1 {
            capability: (*path).to_string(),
            version: None,
            version_relation: None,
            kind: "file".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::SourceDerivedFile {
                format: SourcePackageFormat::Rpm,
            },
        }));
        let requirement_groups = required_path
            .map(|path| {
                let clause = RepositoryRequirementClause::name_only(path.to_string());
                vec![CatalogRequirementGroupV1 {
                    kind: "depends".to_string(),
                    behavior: "hard".to_string(),
                    description: None,
                    native_text: Some(path.to_string()),
                    expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(
                        clause.clone(),
                    ))
                    .unwrap(),
                    atoms: vec![CatalogRequirementAtomV1 {
                        capability: path.to_string(),
                        version_constraint: None,
                        kind: "file".to_string(),
                        dependency_type: "runtime".to_string(),
                        raw: Some(path.to_string()),
                    }],
                }]
            })
            .unwrap_or_default();
        CatalogPackageRecordV1 {
            package_key_sha256: String::new(),
            origin: CatalogPackageOriginV1::Source {
                source_identity: "fedora-project".to_string(),
                repository_identity: "fedora-everything-x86_64".to_string(),
            },
            source_profile: "fedora-44".to_string(),
            name: name.to_string(),
            version: "1-1".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            description: None,
            checksum: checksum.to_string(),
            size: 1,
            download_url: format!("https://repo.test/{name}.rpm"),
            metadata: None,
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: VersionScheme::Rpm,
            provides,
            requirement_groups,
        }
    }

    #[test]
    fn rpm_primary_file_audit_reads_the_candidate_projection() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.sqlite3");
        let mut writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
        writer
            .package(rpm_package("provider", "a", &["/usr/bin/provided"], None))
            .unwrap();
        writer
            .package(rpm_package("consumer", "b", &[], Some("/usr/bin/provided")))
            .unwrap();

        writer
            .validate_rpm_primary_file_requirements("https://repo.test/fedora")
            .unwrap();

        let missing_path = root.path().join("missing.sqlite3");
        let mut missing = CatalogCandidateWriter::create(&missing_path, scope()).unwrap();
        missing
            .package(rpm_package("consumer", "c", &[], Some("/usr/lib/missing")))
            .unwrap();
        let error = missing
            .validate_rpm_primary_file_requirements("https://repo.test/fedora")
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("consumer 1-1"), "{message}");
        assert!(message.contains("/usr/lib/missing"), "{message}");
        assert!(message.contains("no filelists record"), "{message}");
    }

    #[test]
    fn rpm_filelists_checksum_join_is_indexed_only_during_construction() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.sqlite3");
        let mut writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
        writer
            .package(rpm_package("alpha", "a", &[], None))
            .unwrap();
        writer
            .package(rpm_package("bravo", "b", &[], None))
            .unwrap();

        let details = {
            let mut statement = writer
                .connection()
                .unwrap()
                .prepare(
                    "EXPLAIN QUERY PLAN
                     SELECT package_key_sha256, name, version, architecture
                     FROM catalog_packages WHERE checksum = ?1
                     ORDER BY package_key_sha256 LIMIT 1",
                )
                .unwrap();
            statement
                .query_map(["b"], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("catalog_ingest_packages_checksum")),
            "{details:?}"
        );

        writer.finish(evidence()).unwrap();
        let reopened = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        let published_indexes: i64 = reopened
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'index' AND name = 'catalog_ingest_packages_checksum'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(published_indexes, 0);
    }

    #[test]
    fn arch_fragments_pair_canonically_inside_the_candidate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.sqlite3");
        let writer = CatalogCandidateWriter::create(&path, scope()).unwrap();
        writer
            .stage_arch_package_fragment(
                "zeta-2-1".to_string(),
                ArchPackageFragmentKind::Depends,
                "%DEPENDS%\nlibc\n".to_string(),
            )
            .unwrap();
        writer
            .stage_arch_package_fragment(
                "alpha-1-1".to_string(),
                ArchPackageFragmentKind::Desc,
                "%NAME%\nalpha\n".to_string(),
            )
            .unwrap();
        writer
            .stage_arch_package_fragment(
                "zeta-2-1".to_string(),
                ArchPackageFragmentKind::Desc,
                "%NAME%\nzeta\n".to_string(),
            )
            .unwrap();

        let alpha = writer.take_arch_package_record().unwrap().unwrap();
        assert_eq!(alpha.directory, "alpha-1-1");
        assert_eq!(alpha.desc, "%NAME%\nalpha\n");
        assert_eq!(alpha.depends, None);
        let zeta = writer.take_arch_package_record().unwrap().unwrap();
        assert_eq!(zeta.directory, "zeta-2-1");
        assert_eq!(zeta.depends.as_deref(), Some("%DEPENDS%\nlibc\n"));
        assert_eq!(writer.take_arch_package_record().unwrap(), None);
        assert!(
            !table_exists(
                writer.connection().unwrap(),
                "catalog_ingest_arch_fragments"
            )
            .unwrap()
        );

        writer.finish(evidence()).unwrap();
        let reopened = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert!(!table_exists(&reopened, "catalog_ingest_arch_fragments").unwrap());
    }

    #[test]
    fn arch_fragment_duplicates_orphans_and_unfinished_state_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let duplicate_path = root.path().join("duplicate.sqlite3");
        let duplicate = CatalogCandidateWriter::create(&duplicate_path, scope()).unwrap();
        duplicate
            .stage_arch_package_fragment(
                "pkg".to_string(),
                ArchPackageFragmentKind::Desc,
                "first".to_string(),
            )
            .unwrap();
        let duplicate_error = duplicate
            .stage_arch_package_fragment(
                "pkg".to_string(),
                ArchPackageFragmentKind::Desc,
                "second".to_string(),
            )
            .unwrap_err();
        assert!(
            duplicate_error
                .to_string()
                .contains("repeats desc metadata")
        );

        let orphan_path = root.path().join("orphan.sqlite3");
        let orphan = CatalogCandidateWriter::create(&orphan_path, scope()).unwrap();
        orphan
            .stage_arch_package_fragment(
                "pkg".to_string(),
                ArchPackageFragmentKind::Depends,
                "depends".to_string(),
            )
            .unwrap();
        let orphan_error = orphan.take_arch_package_record().unwrap_err();
        assert!(orphan_error.to_string().contains("without desc metadata"));

        let unfinished_path = root.path().join("unfinished.sqlite3");
        let unfinished = CatalogCandidateWriter::create(&unfinished_path, scope()).unwrap();
        unfinished
            .stage_arch_package_fragment(
                "pkg".to_string(),
                ArchPackageFragmentKind::Desc,
                "desc".to_string(),
            )
            .unwrap();
        let unfinished_error = unfinished.finish(evidence()).unwrap_err();
        assert!(
            unfinished_error
                .to_string()
                .contains("unfinished Arch package fragments")
        );
        assert!(!unfinished_path.exists());
    }

    #[test]
    fn finalization_admits_exact_sqlite_page_facts_and_releases_lease() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.sqlite3");
        let lease_drops = Arc::new(AtomicUsize::new(0));
        let admission = Arc::new(RecordingAdmission {
            requirement: Mutex::new(None),
            lease_drops: Arc::clone(&lease_drops),
            refuse: false,
        });
        let writer = CatalogCandidateWriter::create_with_scratch_admission(
            &path,
            scope(),
            admission.clone(),
        )
        .unwrap();
        writer.finish(evidence()).unwrap();

        let requirement = admission.requirement.lock().unwrap().unwrap();
        assert_eq!(
            requirement.schema_version,
            CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1
        );
        assert_eq!(requirement.database_page_size, 4096);
        assert!(requirement.database_page_count > 0);
        assert_eq!(
            requirement.database_bytes,
            requirement.database_page_size * requirement.database_page_count
        );
        assert_eq!(requirement.temporary_copy_bytes, requirement.database_bytes);
        assert_eq!(
            requirement.rollback_journal_bytes,
            requirement.database_bytes
        );
        assert_eq!(
            requirement.required_additional_bytes,
            requirement.database_bytes * 2
        );
        assert_eq!(lease_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn typed_refusal_precedes_vacuum_and_removes_private_candidate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.sqlite3");
        let admission = Arc::new(RecordingAdmission {
            requirement: Mutex::new(None),
            lease_drops: Arc::new(AtomicUsize::new(0)),
            refuse: true,
        });
        let writer =
            CatalogCandidateWriter::create_with_scratch_admission(&path, scope(), admission)
                .unwrap();
        let error = writer.finish(evidence()).unwrap_err();

        assert!(matches!(error, Error::CatalogScratchCapacity(_)));
        assert!(!path.exists());
    }
}
