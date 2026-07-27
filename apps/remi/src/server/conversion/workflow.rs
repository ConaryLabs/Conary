// apps/remi/src/server/conversion/workflow.rs
//! Cold/hot package conversion workflow orchestration.

use super::ConversionService;
use super::lookup::PackageDownloadRefresh;
use super::persistence::PersistConversionInput;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, NativePackageConverter};
use conary_core::db::models::RepositoryPackage;
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;

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
    fn public_feed_for_route(
        route: &str,
    ) -> Result<&'static conary_core::repository::supported_profiles::SupportedProfile> {
        conary_core::repository::supported_profiles::profile_for_remi_route(route).ok_or_else(
            || anyhow!("release route {route} does not map to exactly one repository feed"),
        )
    }

    /// Convert a package from a repository.
    pub async fn convert_package_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<super::ServerConversionResult> {
        let mut timing = ConversionTimingReport::new(distro, package_name, version);
        let result = self
            .convert_package_async_inner(distro, package_name, version, architecture, &mut timing)
            .await;

        match result {
            Ok(mut result) => {
                timing.finish(true);
                Self::log_conversion_timing(&timing);
                result.timing = Some(timing);
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
    ) -> Result<super::ServerConversionResult> {
        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        let started = Instant::now();
        let source_feed = Self::public_feed_for_route(distro)?;
        let repo_pkg = self
            .find_package_for_conversion_async(
                source_feed.id(),
                package_name,
                version,
                architecture,
            )
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
                profile: source_feed.id(),
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
            tokio::task::spawn_blocking(move || Self::calculate_sha256(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());
        let original_checksum_text = original_checksum.to_prefixed_string();

        let started = Instant::now();
        if let Some(existing) = self
            .cached_conversion_result_async(source_feed.id(), &repo_pkg, &original_checksum_text)
            .await?
        {
            timing.record(ConversionPhase::CacheLookup, started.elapsed());
            Self::record_cache_hit_skips(timing);
            return Ok(existing);
        }
        timing.record(ConversionPhase::CacheLookup, started.elapsed());

        let parse_service = self.clone();
        let source_profile = source_feed.id().to_string();
        let output_dir = temp_dir.path().join("output");
        let parsed = tokio::task::spawn_blocking(move || {
            parse_service.parse_and_convert_package(
                &source_profile,
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
        timing.record(ConversionPhase::Chunking, stored_chunks.chunking_duration);
        timing.record(ConversionPhase::CasWrite, stored_chunks.cas_duration);
        if let Some(duration) = stored_chunks.r2_duration {
            timing.record(ConversionPhase::R2WriteThrough, duration);
        } else {
            timing.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        }
        info!(
            "Stored {} emitted-artifact chunks",
            stored_chunks.chunk_hashes.len()
        );

        let persist_service = self.clone();
        let source_profile_owned = source_feed.id().to_string();
        let started = Instant::now();
        tokio::task::spawn_blocking(move || {
            persist_service.persist_conversion_result(PersistConversionInput {
                source_profile: source_profile_owned,
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
            ConversionPhase::NativeShellAstExtraction,
            ConversionPhase::AdapterDispatch,
            ConversionPhase::CcsEmission,
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
        source_profile: &str,
        repo_pkg: RepositoryPackage,
        pkg_path: PathBuf,
        output_dir: PathBuf,
        original_checksum: conary_core::hash::Hash,
    ) -> Result<ParsedConversion> {
        let mut phase_timings = Vec::new();
        let mut skipped_phases = Vec::new();

        let conn = conary_core::db::open(&self.db_path)?;
        let started = Instant::now();
        let (mut metadata, files, format) = self.parse_package(&pkg_path, source_profile)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::ArchiveExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        let started = Instant::now();
        Self::apply_repository_identity(&mut metadata, &repo_pkg);
        Self::merge_repository_provides(&conn, &repo_pkg, &mut metadata)?;
        info!(
            "Parsed: {} v{} ({} files, {} native provides)",
            metadata.name,
            metadata.version,
            files.files().len(),
            metadata.provides.len()
        );
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::NativeShellAstExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        std::fs::create_dir_all(&output_dir)?;
        let output_dir = output_dir.canonicalize().unwrap_or(output_dir);

        let options = ConversionOptions { output_dir };

        let keys_dir = self.repository_keys_dir.as_ref().context(
            "Remi conversion requires release_publish.repository_keys_dir for CCS authority signing",
        )?;
        let signing_key = load_role_key(keys_dir, source_profile, RepositorySigningRole::Targets)?;
        let converter = NativePackageConverter::new(options)
            .with_source_profile(source_profile)
            .with_conversion_tool("remi")
            .with_signing_key(std::sync::Arc::new(signing_key));
        skipped_phases.push(ConversionSkippedPhase {
            phase: ConversionPhase::AdapterDispatch,
            reason: "diagnostic adapter timing is included in native conversion".to_string(),
        });

        let started = Instant::now();
        let conversion_result = converter
            .convert_payload(&metadata, files.files(), format, &original_checksum)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::CcsEmission,
            duration_ms: started.elapsed().as_millis(),
        });

        info!(
            "Conversion complete: scriptlet_fidelity={}",
            conversion_result.scriptlet_metadata.scriptlet_fidelity
        );

        Ok(ParsedConversion {
            metadata,
            format,
            original_checksum: original_checksum.to_prefixed_string(),
            conversion_result,
            repo_pkg,
            phase_timings,
            skipped_phases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::production_rust_sources;
    use super::ConversionService;

    #[test]
    fn remi_server_conversion_paths_do_not_block_on_async_work() {
        for (relative_path, source) in production_rust_sources("src/server") {
            assert!(
                !source.contains(".block_on("),
                "{} must not call Handle::block_on in production Remi server paths",
                relative_path.display()
            );
        }
    }

    #[test]
    fn conversion_route_resolves_repository_feed() {
        let feed =
            ConversionService::public_feed_for_route("fedora").expect("fedora repository feed");
        assert_eq!(feed.id(), "fedora-44");
    }
}
