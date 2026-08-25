// conary-core/src/repository/catalog/capacity.rs

//! Typed scratch-space contract for immutable SQLite catalog finalization.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::Result;

/// Current schema for exact SQLite catalog-finalization scratch evidence.
pub const CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1: u32 = 1;

/// Exact additional filesystem allocation required while SQLite compacts a
/// private catalog alongside its current database file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFinalizationScratchV1 {
    /// Contract schema version.
    pub schema_version: u32,
    /// Positive SQLite page size read after committing catalog metadata.
    pub database_page_size: u64,
    /// Positive SQLite page count read from the same committed candidate.
    pub database_page_count: u64,
    /// Exact committed database bytes before compaction.
    pub database_bytes: u64,
    /// Conservative temporary-database allocation used by `VACUUM`.
    pub temporary_copy_bytes: u64,
    /// Conservative rollback-journal allocation while copying back.
    pub rollback_journal_bytes: u64,
    /// Sum of the two transient allocations required by `VACUUM`.
    pub required_additional_bytes: u64,
}

impl CatalogFinalizationScratchV1 {
    /// Derive SQLite's documented worst-case free-space requirement from
    /// positive page facts read after committing the private candidate.
    ///
    /// <https://www.sqlite.org/lang_vacuum.html#how_vacuum_works>
    pub fn from_page_facts(page_size: u64, page_count: u64) -> Result<Self> {
        if page_size == 0 || page_count == 0 {
            return Err(crate::Error::ConfigError(
                "catalog finalization requires positive SQLite page facts".to_string(),
            ));
        }
        let database_bytes = page_size.checked_mul(page_count).ok_or_else(|| {
            crate::Error::IoError(
                "catalog finalization scratch-space arithmetic overflow".to_string(),
            )
        })?;
        let required_additional_bytes = database_bytes.checked_mul(2).ok_or_else(|| {
            crate::Error::IoError(
                "catalog finalization scratch-space arithmetic overflow".to_string(),
            )
        })?;
        let requirement = Self {
            schema_version: CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1,
            database_page_size: page_size,
            database_page_count: page_count,
            database_bytes,
            temporary_copy_bytes: database_bytes,
            rollback_journal_bytes: database_bytes,
            required_additional_bytes,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    /// Reject contradictory or superseded scratch evidence.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1
            || self.database_page_size == 0
            || self.database_page_count == 0
        {
            return Err(crate::Error::ConfigError(
                "catalog finalization scratch evidence has invalid schema or page facts"
                    .to_string(),
            ));
        }
        let expected_database = self
            .database_page_size
            .checked_mul(self.database_page_count);
        let expected_required = self
            .temporary_copy_bytes
            .checked_add(self.rollback_journal_bytes);
        if expected_database != Some(self.database_bytes)
            || self.temporary_copy_bytes != self.database_bytes
            || self.rollback_journal_bytes != self.database_bytes
            || expected_required != Some(self.required_additional_bytes)
        {
            return Err(crate::Error::ConfigError(
                "catalog finalization scratch evidence contradicts its SQLite page facts"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Stable capacity evidence returned before SQLite begins compacting a
/// private catalog candidate.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "catalog finalization requires {required_bytes} additional bytes, but the filesystem has \
     {available_bytes} available with {reserved_bytes} bytes reserved by concurrent finalizers"
)]
pub struct CatalogScratchCapacityError {
    /// Additional bytes required by this finalizer.
    pub required_bytes: u64,
    /// Bytes reported available on the owning filesystem at admission.
    pub available_bytes: u64,
    /// Bytes already promised to other finalizers in this process.
    pub reserved_bytes: u64,
}

/// Process owner that admits an exact finalization requirement. The returned
/// lease must retain the reservation until finalization completes or aborts.
pub trait CatalogScratchAdmission: Send + Sync {
    /// Reserve the exact additional bytes and retain them in the returned lease.
    fn reserve_finalization(
        &self,
        candidate_path: &Path,
        requirement: CatalogFinalizationScratchV1,
    ) -> Result<Box<dyn Send>>;
}
