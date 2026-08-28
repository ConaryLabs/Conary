// crates/conary-core/src/repository/catalog/candidate/finalization.rs

//! Logical binding, compaction, durability, and verified candidate reopen.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::params;

use super::CatalogCandidateWriter;
use super::lifecycle::{read_positive_pragma, table_exists};
use crate::error::{Error, Result};
use crate::repository::catalog::store::{
    CATALOG_PROVIDE_INDEX_SCHEMA, canonical_json_string, checked_i64, checked_ordinal,
    create_private_file, digest_catalog_connection, hash_file, sync_parent,
};
use crate::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogBindingV1, CatalogFinalizationScratchV2,
    CatalogReader, CatalogSourceEvidenceV1,
};

impl CatalogCandidateWriter {
    /// Bind exact source evidence, calculate the canonical logical identity by
    /// ordered iteration, and reopen the resulting artifact before returning.
    pub fn finish(self, source_evidence: Vec<CatalogSourceEvidenceV1>) -> Result<CatalogBindingV1> {
        self.finish_verified(source_evidence)
            .map(|(binding, _reader)| binding)
    }

    /// Finish one candidate and retain the full logical-verification reader so
    /// later same-process moves can prove the same exact bytes without replaying
    /// every normalized row again.
    pub(in crate::repository) fn finish_verified(
        mut self,
        source_evidence: Vec<CatalogSourceEvidenceV1>,
    ) -> Result<(CatalogBindingV1, CatalogReader)> {
        let finalization_started = Instant::now();
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
        if self.provide_indexes_deferred {
            self.connection()?
                .execute_batch(CATALOG_PROVIDE_INDEX_SCHEMA)?;
            self.provide_indexes_deferred = false;
        }
        self.connection()?
            .execute_batch("DROP INDEX catalog_ingest_packages_checksum")?;
        self.connection()?.execute_batch("COMMIT")?;

        let digest_started = Instant::now();
        let (logical_digest_sha256, counts) =
            digest_catalog_connection(self.connection()?, &scope, &source_evidence)?;
        tracing::info!(
            catalog_scope = ?scope,
            elapsed_ms = digest_started.elapsed().as_millis(),
            packages = counts.packages,
            provides = counts.provides,
            requirement_groups = counts.requirement_groups,
            requirement_atoms = counts.requirement_atoms,
            "Catalog logical digest completed"
        );

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
            File::open(&self.path)?.sync_all()?;
            sync_parent(&self.path)?;
            let candidate_bytes = fs::metadata(&self.path)?.len();
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
        let scratch = CatalogFinalizationScratchV2::from_page_facts(page_size, page_count)?;
        let _scratch_lease = self
            .scratch_admission
            .as_ref()
            .map(|admission| admission.reserve_finalization(&self.path, scratch))
            .transpose()?;

        let compacted = PrivateCompactionTarget::create(&self.path)?;
        let compacted_path = compacted.path().to_str().ok_or_else(|| {
            Error::InvalidPath(format!(
                "catalog compaction target {} is not valid UTF-8 for SQLite",
                compacted.path().display()
            ))
        })?;
        let compaction_started = Instant::now();
        connection.execute("VACUUM INTO ?1", [compacted_path])?;
        File::open(compacted.path())?.sync_all()?;
        let compacted_bytes = fs::metadata(compacted.path())?.len();
        if compacted_bytes > scratch.compacted_copy_bytes {
            return Err(Error::InternalError(format!(
                "catalog compaction produced {compacted_bytes} bytes above its admitted {}-byte \
                 output bound",
                scratch.compacted_copy_bytes
            )));
        }
        tracing::info!(
            catalog_scope = ?scope,
            elapsed_ms = compaction_started.elapsed().as_millis(),
            input_bytes = scratch.database_bytes,
            output_bytes = compacted_bytes,
            "Catalog direct-output compaction completed"
        );
        self.connection
            .take()
            .expect("catalog candidate connection exists")
            .close()
            .map_err(|(_, error)| Error::Database(error))?;
        compacted.replace(&self.path)?;
        sync_parent(&self.path)?;
        let hash_started = Instant::now();
        let artifact_sha256 = hash_file(&self.path)?;
        tracing::info!(
            catalog_scope = ?scope,
            elapsed_ms = hash_started.elapsed().as_millis(),
            artifact_bytes = compacted_bytes,
            "Catalog artifact hash completed"
        );
        let artifact = CatalogArtifactV1 {
            sha256: artifact_sha256,
            size: fs::metadata(&self.path)?.len(),
        };
        let binding = CatalogBindingV1 {
            scope,
            artifact,
            logical_digest_sha256,
            counts,
        };
        let reopen_started = Instant::now();
        let reader = CatalogReader::open_verified(&self.path, &binding)?;
        tracing::info!(
            catalog_scope = ?binding.scope,
            elapsed_ms = reopen_started.elapsed().as_millis(),
            total_elapsed_ms = finalization_started.elapsed().as_millis(),
            "Catalog independent reopen completed"
        );
        self.complete = true;
        Ok((binding, reader))
    }
}

/// Private same-directory output for one SQLite `VACUUM INTO` hard cut.
///
/// The output is never an authority until it replaces the unpublished
/// candidate and that exact path passes the normal independent reopen. Drop
/// removes both the file and any SQLite sidecars after every failed attempt.
struct PrivateCompactionTarget {
    path: PathBuf,
    replaced: bool,
}

impl PrivateCompactionTarget {
    fn create(candidate_path: &Path) -> Result<Self> {
        let parent = candidate_path.parent().ok_or_else(|| {
            Error::InvalidPath(format!(
                "catalog candidate {} has no parent directory",
                candidate_path.display()
            ))
        })?;
        let path = parent.join(format!(".catalog-compaction-{}", uuid::Uuid::new_v4()));
        create_private_file(&path)?;
        Ok(Self {
            path,
            replaced: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn replace(mut self, candidate_path: &Path) -> Result<()> {
        fs::rename(&self.path, candidate_path)?;
        self.replaced = true;
        Ok(())
    }
}

impl Drop for PrivateCompactionTarget {
    fn drop(&mut self) {
        if !self.replaced {
            super::lifecycle::remove_candidate_files(&self.path);
        }
    }
}
