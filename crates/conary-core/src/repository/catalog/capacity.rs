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
    /// One complete additional database allocation required by `VACUUM`.
    pub required_additional_bytes: u64,
}

impl CatalogFinalizationScratchV1 {
    pub(super) fn from_page_facts(page_size: u64, page_count: u64) -> Result<Self> {
        let required_additional_bytes = page_size.checked_mul(page_count).ok_or_else(|| {
            crate::Error::IoError(
                "catalog finalization scratch-space arithmetic overflow".to_string(),
            )
        })?;
        Ok(Self {
            schema_version: CATALOG_FINALIZATION_SCRATCH_SCHEMA_V1,
            database_page_size: page_size,
            database_page_count: page_count,
            required_additional_bytes,
        })
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
