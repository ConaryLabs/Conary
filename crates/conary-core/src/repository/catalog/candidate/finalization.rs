// crates/conary-core/src/repository/catalog/candidate/finalization.rs

//! Logical binding, compaction, durability, and verified candidate reopen.

use std::fs::{self, File};

use rusqlite::params;

use super::CatalogCandidateWriter;
use super::lifecycle::{read_positive_pragma, table_exists};
use crate::error::{Error, Result};
use crate::repository::catalog::store::{
    canonical_json_string, checked_i64, checked_ordinal, digest_catalog_connection, hash_file,
    sync_parent,
};
use crate::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogBindingV1, CatalogFinalizationScratchV1,
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
        let scratch = CatalogFinalizationScratchV1::from_page_facts(page_size, page_count)?;
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
        File::open(&self.path)?.sync_all()?;
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
        let reader = CatalogReader::open_verified(&self.path, &binding)?;
        self.complete = true;
        Ok((binding, reader))
    }
}
