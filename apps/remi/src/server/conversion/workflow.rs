// apps/remi/src/server/conversion/workflow.rs
//! Cold/hot package conversion workflow orchestration.

use super::ConversionService;
use super::lookup::PackageDownloadRefresh;
use super::metadata::RepositoryConversionMetadata;
use super::persistence::PersistConversionInput;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, NativePackageConverter};
use conary_core::db::models::RepositoryPackage;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;

struct ParsedConversion {
    metadata: ForeignConversionInput,
    format: &'static str,
    source_checksum: String,
    conversion_result: ConversionResult,
    repo_pkg: RepositoryPackage,
    repository_provides_digest: String,
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
        let artifact_sha256 =
            tokio::task::spawn_blocking(move || Self::calculate_sha256(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());
        let source_checksum = repo_pkg.checksum.clone();

        let started = Instant::now();
        let repository_metadata = self
            .load_repository_conversion_metadata_async(&repo_pkg)
            .await?;
        if let Some(existing) = self
            .cached_conversion_result_async(
                source_feed.id(),
                &repo_pkg,
                &source_checksum,
                &repository_metadata.digest,
            )
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
                repository_metadata,
                pkg_path,
                output_dir,
                artifact_sha256,
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
                source_checksum: parsed.source_checksum,
                conversion_result: parsed.conversion_result,
                repo_pkg: parsed.repo_pkg,
                repository_provides_digest: parsed.repository_provides_digest,
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
        repository_metadata: RepositoryConversionMetadata,
        pkg_path: PathBuf,
        output_dir: PathBuf,
        artifact_sha256: conary_core::hash::Hash,
    ) -> Result<ParsedConversion> {
        let mut phase_timings = Vec::new();
        let mut skipped_phases = Vec::new();

        let started = Instant::now();
        let (metadata, files, format) = self.parse_package(&pkg_path, source_profile)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::ArchiveExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        let started = Instant::now();
        Self::validate_repository_identity(&metadata, &repo_pkg)?;
        let capability_count = metadata.source_authority.declared_capabilities()?.len();
        info!(
            "Parsed: {} v{} ({} files, {} native provides)",
            metadata.name(),
            metadata.version(),
            files.files().len(),
            capability_count
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
            .convert_payload(&metadata, files.files(), format, &artifact_sha256)
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
            source_checksum: repo_pkg.checksum.clone(),
            conversion_result,
            repo_pkg,
            repository_provides_digest: repository_metadata.digest,
            phase_timings,
            skipped_phases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{eopkg_fixture, production_rust_sources};
    use super::{ConversionService, RepositoryConversionMetadata};
    use conary_core::ccs::SigningKeyPair;
    use conary_core::db::models::RepositoryPackage;
    use conary_core::repository::versioning::VersionScheme;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    fn eopkg_conversion_separates_source_checksum_from_ccs_sha256() {
        let fixture = eopkg_fixture();
        let root = tempfile::tempdir().unwrap();
        let keys_root = root.path().join("keys");
        let profile_dir = keys_root.join("solus");
        fs::create_dir_all(&profile_dir).unwrap();
        fs::set_permissions(&keys_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&profile_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let private = profile_dir.join("targets.private");
        let public = profile_dir.join("targets.public");
        SigningKeyPair::generate()
            .with_key_id("targets")
            .save_to_files(&private, &public)
            .unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o644)).unwrap();

        let service = ConversionService::new(
            root.path().join("chunks"),
            root.path().join("cache"),
            root.path().join("remi.db"),
            None,
        )
        .with_repository_keys_dir(Some(keys_root));
        let source_checksum = "sha1:1826421aded2a344b7864ffff2fae2430778b1f0";
        let mut package = RepositoryPackage::new(
            1,
            "demo".to_string(),
            "1.0-2".to_string(),
            VersionScheme::Eopkg,
            source_checksum.to_string(),
            fixture.as_file().metadata().unwrap().len() as i64,
            "https://example.invalid/demo.eopkg".to_string(),
        );
        package.architecture = Some("x86_64".to_string());
        package.source_profile = Some("solus".to_string());
        let artifact_sha256 = ConversionService::calculate_sha256(fixture.path()).unwrap();
        let artifact_sha256_text = artifact_sha256.to_prefixed_string();

        let parsed = service
            .parse_and_convert_package(
                "solus",
                package,
                RepositoryConversionMetadata {
                    digest: conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
                },
                fixture.path().to_path_buf(),
                root.path().join("output"),
                artifact_sha256,
            )
            .unwrap();

        assert_eq!(parsed.source_checksum, source_checksum);
        assert_eq!(
            parsed.conversion_result.original_checksum,
            artifact_sha256_text
        );
    }
}
