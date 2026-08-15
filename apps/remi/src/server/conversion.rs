// apps/remi/src/server/conversion.rs
//! Package conversion service for the Remi server
//!
//! Downloads native packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

mod benchmark;
mod lookup;
mod metadata;
mod persistence;
mod recipe;
mod storage;
#[cfg(test)]
pub(crate) mod test_support;
mod types;
mod workflow;

use crate::server::database_writer::DatabaseWriter;
use crate::server::{BoundedCache, R2Store};
use std::path::PathBuf;
use std::sync::Arc;
pub use types::{
    CONVERSION_BENCHMARK_SCHEMA_V1, ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence,
    ConversionBenchmarkSample, ConversionBenchmarkSampleClass, ScriptletPackageMetadata,
    ServerConversionResult,
};

/// Conversion service for Remi
#[derive(Clone)]
pub struct ConversionService {
    /// Path to chunk storage
    chunk_dir: PathBuf,
    /// Path to cache/scratch directory
    cache_dir: PathBuf,
    /// Database path
    db_path: PathBuf,
    /// Optional R2 durable authority (absent for local-only deployments).
    r2_store: Option<Arc<R2Store>>,
    /// Serialized owner for the R2-verified local cache bound.
    bounded_cache: Option<BoundedCache>,
    /// Remi-owned per-distro TUF keys used to authenticate converted CCS.
    repository_keys_dir: Option<PathBuf>,
    /// Shared owner for the short SQLite mutation phases of conversion work.
    database_writer: DatabaseWriter,
}

impl ConversionService {
    pub fn new(
        chunk_dir: PathBuf,
        cache_dir: PathBuf,
        db_path: PathBuf,
        r2_store: Option<Arc<R2Store>>,
    ) -> Self {
        Self {
            chunk_dir,
            cache_dir,
            db_path,
            r2_store,
            bounded_cache: None,
            repository_keys_dir: None,
            database_writer: DatabaseWriter::default(),
        }
    }

    pub(crate) fn with_database_writer(mut self, database_writer: DatabaseWriter) -> Self {
        self.database_writer = database_writer;
        self
    }

    pub(crate) fn with_r2_store(mut self, r2_store: Option<Arc<R2Store>>) -> Self {
        self.r2_store = r2_store;
        self
    }

    pub(crate) fn with_bounded_cache(mut self, bounded_cache: BoundedCache) -> Self {
        self.bounded_cache = Some(bounded_cache);
        self
    }

    pub fn with_repository_keys_dir(mut self, keys_dir: Option<PathBuf>) -> Self {
        self.repository_keys_dir = keys_dir;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ConversionService::new tests ---

    #[test]
    fn test_conversion_service_new() {
        let service = ConversionService::new(
            PathBuf::from("/chunks"),
            PathBuf::from("/cache"),
            PathBuf::from("/db.sqlite"),
            None,
        );
        assert_eq!(service.chunk_dir, PathBuf::from("/chunks"));
        assert_eq!(service.cache_dir, PathBuf::from("/cache"));
        assert_eq!(service.db_path, PathBuf::from("/db.sqlite"));
        assert!(service.r2_store.is_none());
        assert!(service.repository_keys_dir.is_none());
    }

    #[test]
    fn cloned_services_share_the_conversion_database_writer() {
        let service = ConversionService::new(
            PathBuf::from("/chunks"),
            PathBuf::from("/cache"),
            PathBuf::from("/db.sqlite"),
            None,
        );
        let clone = service.clone();

        assert!(
            service
                .database_writer
                .shares_owner_with(&clone.database_writer)
        );
    }
}
