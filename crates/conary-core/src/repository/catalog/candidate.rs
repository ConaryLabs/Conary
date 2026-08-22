// conary-core/src/repository/catalog/candidate.rs

//! Bounded construction of one deterministic standalone catalog candidate.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};

use super::record::CatalogLogicalDigestV1;
use super::store::{
    CATALOG_APPLICATION_ID, CATALOG_SCHEMA, canonical_json_string, checked_i64, checked_ordinal,
    create_private_file, for_each_package_connection, hash_file, insert_package, sidecar_path,
    sync_parent, validate_candidate_path,
};
use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogBindingV1, CatalogPackageRecordV1,
    CatalogReader, CatalogScopeV1, CatalogSourceEvidenceV1,
};
use crate::error::{Error, Result};

/// Private catalog writer that retains one normalized package at a time.
pub struct CatalogCandidateWriter {
    path: PathBuf,
    scope: CatalogScopeV1,
    connection: Option<Connection>,
    complete: bool,
}

impl CatalogCandidateWriter {
    pub fn create(path: impl AsRef<Path>, scope: CatalogScopeV1) -> Result<Self> {
        scope.validate()?;
        let path = path.as_ref().to_path_buf();
        validate_candidate_path(&path)?;
        create_private_file(&path)?;
        let result = Self::open_created(path.clone(), scope);
        if result.is_err() {
            remove_candidate_files(&path);
        }
        result
    }

    fn open_created(path: PathBuf, scope: CatalogScopeV1) -> Result<Self> {
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
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            path,
            scope,
            connection: Some(connection),
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

    /// Bind exact source evidence, calculate the canonical logical identity by
    /// ordered iteration, and reopen the resulting artifact before returning.
    pub fn finish(
        mut self,
        source_evidence: Vec<CatalogSourceEvidenceV1>,
    ) -> Result<CatalogBindingV1> {
        let scope = self.scope.clone();
        self.connection()?.execute_batch("COMMIT")?;

        let mut digest = CatalogLogicalDigestV1::new(&scope, &source_evidence)?;
        for_each_package_connection(self.connection()?, &scope, |package| {
            digest.package(&package)
        })?;
        let (logical_digest_sha256, counts) = digest.finish()?;

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
        connection.execute_batch("COMMIT; VACUUM;")?;
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

fn remove_candidate_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(sidecar_path(path, suffix));
    }
}
