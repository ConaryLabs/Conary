// crates/conary-core/src/repository/catalog/candidate.rs

//! Bounded construction of one deterministic standalone catalog candidate.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::record::canonicalize_provides;
use super::store::{
    CATALOG_APPLICATION_ID, CATALOG_PROVIDE_INDEX_SCHEMA, CATALOG_SCHEMA, create_private_file,
    insert_package, load_provides, replace_package_provides, validate_candidate_path,
};
use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProfileCandidateScratchV1, CatalogProvideRecordV1, CatalogReader, CatalogScopeV1,
    CatalogScratchAdmission, CatalogSourceCandidateScratchV1,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::RepositoryRequirementExpression;
use crate::repository::parsers::{ArchPackageFragmentKind, ArchPackageRecord};

mod finalization;
mod lifecycle;
use lifecycle::{remove_candidate_files, table_exists};

/// Private catalog writer that retains one normalized package at a time.
pub struct CatalogCandidateWriter {
    path: PathBuf,
    scope: CatalogScopeV1,
    connection: Option<Connection>,
    scratch_admission: Option<Arc<dyn CatalogScratchAdmission>>,
    growth_lease: Option<Box<dyn Send>>,
    database_bytes_bound: Option<u64>,
    provide_indexes_deferred: bool,
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
        connection.execute_batch(CATALOG_PROVIDE_INDEX_SCHEMA)?;
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
            provide_indexes_deferred: false,
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
        if !self.provide_indexes_deferred {
            self.connection()?.execute_batch(
                "DROP INDEX catalog_provides_capability;
                 DROP INDEX catalog_provides_raw;
                 CREATE TABLE catalog_ingest_join_marks (
                     join_kind TEXT NOT NULL,
                     package_key_sha256 TEXT NOT NULL
                         REFERENCES catalog_packages(package_key_sha256) ON DELETE CASCADE,
                     PRIMARY KEY (join_kind, package_key_sha256)
                 ) STRICT, WITHOUT ROWID;",
            )?;
            self.provide_indexes_deferred = true;
        }
        let primary = self.connection()?.prepare_cached(
                "SELECT package_key_sha256, name, version, architecture
                 FROM catalog_packages WHERE checksum = ?1
                 ORDER BY package_key_sha256 LIMIT 1",
            )?.query_row([checksum], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
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
        if let Err(error) = self
            .connection()?
            .prepare_cached(
                "INSERT INTO catalog_ingest_join_marks (join_kind, package_key_sha256)
                 VALUES (?1, ?2)",
            )?
            .execute(params![join, &package_key])
        {
            return Err(Error::ConflictError(format!(
                "authenticated child metadata repeats package {name} {version} ({checksum}): {error}"
            )));
        }

        let mut package_provides = load_provides(self.connection()?, &package_key)?;
        let mut known = package_provides.iter().cloned().collect::<HashSet<_>>();
        let mut result = CatalogProvideMerge {
            matched_packages: 1,
            ..CatalogProvideMerge::default()
        };
        for provide in provides {
            provide.validate()?;
            if !known.insert(provide.clone()) {
                result.already_known += 1;
            } else {
                package_provides.push(provide);
                result.added += 1;
            }
        }
        canonicalize_provides(&mut package_provides)?;
        replace_package_provides(self.connection()?, &package_key, &package_provides)?;
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
mod tests;
