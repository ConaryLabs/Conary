// apps/remi/src/server/conversion.rs
//! Package conversion service for the Remi server
//!
//! Downloads legacy packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

mod benchmark;
mod lookup;
mod metadata;
mod persistence;
mod recipe;
mod safety;
mod storage;
#[cfg(test)]
mod test_support;
mod types;

use crate::server::R2Store;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::publication::ServerConversionOutcome;
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use conary_core::db::models::RepositoryPackage;
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;

use lookup::PackageDownloadRefresh;
use persistence::PersistConversionInput;

pub use types::{ConversionBenchmarkEvidence, ScriptletPackageMetadata, ServerConversionResult};

/// Conversion service for Remi
#[derive(Clone)]
pub struct ConversionService {
    /// Path to chunk storage
    chunk_dir: PathBuf,
    /// Path to cache/scratch directory
    cache_dir: PathBuf,
    /// Database path
    db_path: PathBuf,
    /// Optional R2 store for write-through
    r2_store: Option<Arc<R2Store>>,
}

struct ParsedConversion {
    metadata: PackageMetadata,
    format: &'static str,
    original_checksum: String,
    conversion_result: ConversionResult,
    repo_pkg: RepositoryPackage,
    phase_timings: Vec<ConversionPhaseTiming>,
    skipped_phases: Vec<ConversionSkippedPhase>,
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
        }
    }

    /// Convert a package from a repository.
    pub async fn convert_package_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ServerConversionOutcome> {
        let mut timing = ConversionTimingReport::new(distro, package_name, version);
        let result = self
            .convert_package_async_inner(distro, package_name, version, architecture, &mut timing)
            .await;

        match result {
            Ok(mut result) => {
                timing.finish(true);
                Self::log_conversion_timing(&timing);
                result.result_mut().timing = Some(timing);
                Ok(result)
            }
            Err(err) => {
                timing.finish(false);
                Self::log_conversion_timing(&timing);
                Err(err)
            }
        }
    }

    async fn convert_package_async_inner(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        timing: &mut ConversionTimingReport,
    ) -> Result<ServerConversionOutcome> {
        // Refuse to convert critical system packages
        Self::ensure_package_name_not_critical(package_name)?;

        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        let started = Instant::now();
        let repo_pkg = self
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        timing.record(ConversionPhase::PackageLookup, started.elapsed());
        if timing.version.is_none() {
            timing.version = Some(repo_pkg.version.clone());
        }
        info!(
            "Found package: {} {} from repo {}",
            repo_pkg.name, repo_pkg.version, repo_pkg.repository_id
        );

        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;

        let started = Instant::now();
        let (repo_pkg, pkg_path) = self
            .download_package_with_refresh_async(PackageDownloadRefresh {
                distro,
                package_name,
                version,
                architecture,
                repo_pkg,
                dest_dir: temp_dir.path(),
            })
            .await
            .map_err(|e| anyhow!("Failed to download package: {}", e))?;
        timing.record(ConversionPhase::Download, started.elapsed());
        info!("Downloaded to: {:?}", pkg_path);

        let checksum_path = pkg_path.clone();
        let started = Instant::now();
        let original_checksum =
            tokio::task::spawn_blocking(move || Self::calculate_checksum(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());

        let started = Instant::now();
        if let Some(existing) = self
            .cached_conversion_result_async(distro, &repo_pkg, &original_checksum)
            .await?
        {
            timing.record(ConversionPhase::CacheLookup, started.elapsed());
            Self::record_cache_hit_skips(timing);
            return Ok(existing);
        }
        timing.record(ConversionPhase::CacheLookup, started.elapsed());

        let parse_service = self.clone();
        let distro_owned = distro.to_string();
        let output_dir = temp_dir.path().join("output");
        let parsed = tokio::task::spawn_blocking(move || {
            parse_service.parse_and_convert_package(
                &distro_owned,
                repo_pkg,
                pkg_path,
                output_dir,
                original_checksum,
            )
        })
        .await
        .map_err(|e| anyhow!("conversion task panicked: {e}"))??;
        timing.phases.extend(parsed.phase_timings.clone());
        timing.skipped_phases.extend(parsed.skipped_phases.clone());

        let stored_chunks = self
            .store_chunks_with_timing(&parsed.conversion_result)
            .await?;
        timing.record(ConversionPhase::CasWrite, stored_chunks.cas_duration);
        if let Some(duration) = stored_chunks.r2_duration {
            timing.record(ConversionPhase::R2WriteThrough, duration);
        } else {
            timing.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        }
        info!("Stored {} chunks/blobs", stored_chunks.chunk_hashes.len());

        let persist_service = self.clone();
        let distro_owned = distro.to_string();
        let started = Instant::now();
        tokio::task::spawn_blocking(move || {
            persist_service.persist_conversion_result(PersistConversionInput {
                distro: distro_owned,
                metadata: parsed.metadata,
                format: parsed.format,
                original_checksum: parsed.original_checksum,
                conversion_result: parsed.conversion_result,
                repo_pkg: parsed.repo_pkg,
                chunk_hashes: stored_chunks.chunk_hashes,
            })
        })
        .await
        .map_err(|e| anyhow!("conversion persistence task panicked: {e}"))?
        .inspect(|_| timing.record(ConversionPhase::Persistence, started.elapsed()))
    }

    fn record_cache_hit_skips(timing: &mut ConversionTimingReport) {
        for phase in [
            ConversionPhase::ArchiveExtraction,
            ConversionPhase::NativeMetadataExtraction,
            ConversionPhase::Capture,
            ConversionPhase::AdapterDispatch,
            ConversionPhase::Chunking,
            ConversionPhase::CasWrite,
            ConversionPhase::R2WriteThrough,
            ConversionPhase::Persistence,
        ] {
            timing.record_skipped(phase, "cache hit; phase did not run");
        }
    }

    fn log_conversion_timing(timing: &ConversionTimingReport) {
        tracing::info!(
            target: "remi::conversion_timing",
            distro = %timing.distro,
            package = %timing.package,
            total_ms = timing.total_ms,
            success = timing.success,
            phases = %serde_json::to_string(&timing.phases)
                .unwrap_or_else(|_| "[]".to_string()),
            skipped_phases = %serde_json::to_string(&timing.skipped_phases)
                .unwrap_or_else(|_| "[]".to_string()),
            "conversion timing report"
        );
    }

    fn parse_and_convert_package(
        &self,
        distro: &str,
        repo_pkg: RepositoryPackage,
        pkg_path: PathBuf,
        output_dir: PathBuf,
        original_checksum: String,
    ) -> Result<ParsedConversion> {
        let mut phase_timings = Vec::new();
        let mut skipped_phases = Vec::new();

        let conn = conary_core::db::open(&self.db_path)?;
        let started = Instant::now();
        let (mut metadata, files, format) = self.parse_package(&pkg_path, distro)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::ArchiveExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        let started = Instant::now();
        Self::apply_repository_identity(&mut metadata, &repo_pkg);
        Self::merge_repository_provides(&conn, &repo_pkg, &mut metadata)?;
        Self::ensure_metadata_not_critical(&metadata)?;
        info!(
            "Parsed: {} v{} ({} files, {} native provides)",
            metadata.name,
            metadata.version,
            files.len(),
            metadata.provides.len()
        );
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::NativeMetadataExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        std::fs::create_dir_all(&output_dir)?;
        let output_dir = output_dir.canonicalize().unwrap_or(output_dir);

        let options = ConversionOptions {
            enable_chunking: true,
            output_dir,
            auto_classify: true,
            ..Default::default()
        };

        let converter = LegacyConverter::new(options)
            .with_source_distro(distro)
            .with_conversion_tool("remi");
        if metadata.scriptlets.is_empty() {
            skipped_phases.push(ConversionSkippedPhase {
                phase: ConversionPhase::Capture,
                reason: "package has no native scriptlets to capture".to_string(),
            });
        } else {
            skipped_phases.push(ConversionSkippedPhase {
                phase: ConversionPhase::Capture,
                reason: "capture timing is included in legacy converter timing".to_string(),
            });
        }
        skipped_phases.push(ConversionSkippedPhase {
            phase: ConversionPhase::AdapterDispatch,
            reason: "adapter dispatch timing is included in legacy converter timing".to_string(),
        });

        let started = Instant::now();
        let conversion_result = converter
            .convert(&metadata, &files, format, &original_checksum)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::Chunking,
            duration_ms: started.elapsed().as_millis(),
        });

        info!(
            "Conversion complete: fidelity={}",
            conversion_result.fidelity.level
        );

        Ok(ParsedConversion {
            metadata,
            format,
            original_checksum,
            conversion_result,
            repo_pkg,
            phase_timings,
            skipped_phases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn remi_server_conversion_paths_do_not_block_on_async_work() {
        for relative_path in [
            "src/server/admin_service.rs",
            "src/server/conversion.rs",
            "src/server/handlers/packages.rs",
            "src/server/prewarm.rs",
        ] {
            let source = production_source_without_comments(relative_path);
            assert!(
                !source.contains(".block_on("),
                "{relative_path} must not call Handle::block_on in production Remi server paths"
            );
        }
    }

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
    }
}
