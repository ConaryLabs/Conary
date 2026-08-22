// apps/remi/src/server/conversion/workflow.rs
//! Cold/hot package conversion workflow orchestration.

use super::ConversionService;
use super::lookup::{PackageDownloadRequest, PinnedConversionSource};
use super::persistence::PersistConversionInput;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionSourceIdentity,
    ConversionTimingReport,
};
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, NativePackageConverter};
use conary_core::db::models::{RemiActiveProfileRevision, RepositoryPackage};
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;

struct ParsedConversion {
    metadata: ForeignConversionInput,
    format: &'static str,
    source_checksum: String,
    conversion_result: ConversionResult,
    source: PinnedConversionSource,
    phase_timings: Vec<ConversionPhaseTiming>,
    skipped_phases: Vec<ConversionSkippedPhase>,
}

impl ConversionService {
    fn record_source_identity(
        timing: &mut ConversionTimingReport,
        source_profile: &str,
        repo_pkg: &RepositoryPackage,
    ) -> Result<()> {
        timing.source = Some(ConversionSourceIdentity {
            source_profile: source_profile.to_string(),
            version: repo_pkg.version.clone(),
            architecture: repo_pkg.architecture.clone(),
            checksum: repo_pkg.checksum.clone(),
            declared_size_bytes: u64::try_from(repo_pkg.size)
                .context("repository package size is negative")?,
        });
        Ok(())
    }

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
        self.convert_package_with_selection_async(distro, package_name, version, architecture, None)
            .await
    }

    pub(crate) async fn convert_package_from_selection_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        selection: RemiActiveProfileRevision,
    ) -> Result<super::ServerConversionResult> {
        self.convert_package_with_selection_async(
            distro,
            package_name,
            version,
            architecture,
            Some(selection),
        )
        .await
    }

    async fn convert_package_with_selection_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        selection: Option<RemiActiveProfileRevision>,
    ) -> Result<super::ServerConversionResult> {
        let mut timing = ConversionTimingReport::new(distro, package_name, version);
        let result = self
            .convert_package_async_inner(
                distro,
                package_name,
                version,
                architecture,
                selection,
                &mut timing,
            )
            .await;

        match result {
            Ok(mut result) => {
                timing.work.ccs_output_bytes = result.total_size;
                timing.work.signed_object_count = result.transport.objects.len() as u64;
                timing.work.signed_object_bytes = result
                    .transport
                    .objects
                    .iter()
                    .map(|object| object.size)
                    .sum();
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
        selection: Option<RemiActiveProfileRevision>,
        timing: &mut ConversionTimingReport,
    ) -> Result<super::ServerConversionResult> {
        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        let started = Instant::now();
        let source_feed = Self::public_feed_for_route(distro)?;
        let source = match selection {
            Some(selection) => {
                if selection.source_profile != source_feed.id() {
                    return Err(anyhow!(
                        "selected catalog profile '{}' does not match route '{}' profile '{}'",
                        selection.source_profile,
                        distro,
                        source_feed.id()
                    ));
                }
                self.find_package_for_selected_revision_async(
                    selection,
                    package_name,
                    version,
                    architecture,
                )
                .await?
            }
            None => {
                self.find_package_for_conversion_async(
                    source_feed.id(),
                    package_name,
                    version,
                    architecture,
                )
                .await?
            }
        };
        timing.record(ConversionPhase::PackageLookup, started.elapsed());
        Self::record_source_identity(timing, source.source_profile(), &source.repo_pkg)?;
        if timing.version.is_none() {
            timing.version = Some(source.repo_pkg.version.clone());
        }
        info!(
            "Found package: {} {} from repo {}",
            source.repo_pkg.name, source.repo_pkg.version, source.repository_id
        );

        let source_checksum = source.repo_pkg.checksum.clone();
        let profile_revision_sha256 = source.profile_revision_sha256().to_string();
        let started = Instant::now();
        if let Some(existing) = self
            .cached_conversion_result_async(
                source_feed.id(),
                &source.repo_pkg,
                &source_checksum,
                &profile_revision_sha256,
            )
            .await?
        {
            timing.record(ConversionPhase::CacheLookup, started.elapsed());
            Self::record_cache_hit_skips(timing);
            return Ok(existing);
        }
        timing.record(ConversionPhase::CacheLookup, started.elapsed());

        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;

        let started = Instant::now();
        let (source, pkg_path) = self
            .download_package_async(PackageDownloadRequest {
                source,
                dest_dir: temp_dir.path(),
            })
            .await
            .map_err(|e| anyhow!("Failed to download package: {}", e))?;
        timing.record(ConversionPhase::Download, started.elapsed());
        timing.work.downloaded_bytes = tokio::fs::metadata(&pkg_path).await?.len();
        Self::record_source_identity(timing, source.source_profile(), &source.repo_pkg)?;
        info!("Downloaded to: {:?}", pkg_path);

        let checksum_path = pkg_path.clone();
        let started = Instant::now();
        let artifact_sha256 =
            tokio::task::spawn_blocking(move || Self::calculate_sha256(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());
        timing.work.source_bytes_hashed = timing.work.downloaded_bytes;

        let parse_service = self.clone();
        let source_profile = source.source_profile().to_string();
        let output_dir = temp_dir.path().join("output");
        let parsed = tokio::task::spawn_blocking(move || {
            parse_service.parse_and_convert_package(
                &source_profile,
                source,
                pkg_path,
                output_dir,
                artifact_sha256,
            )
        })
        .await
        .map_err(|e| anyhow!("conversion task panicked: {e}"))??;
        timing.phases.extend(parsed.phase_timings.clone());
        timing.skipped_phases.extend(parsed.skipped_phases.clone());

        let stored_transport = self
            .store_transport_with_timing(&parsed.conversion_result, source_feed.id())
            .await?;
        timing.record(
            ConversionPhase::TransportVerification,
            stored_transport.verification_duration,
        );
        timing.record(ConversionPhase::CasWrite, stored_transport.cas_duration);
        timing.work.record_cas(stored_transport.cas_metrics);
        timing.work.r2 = stored_transport.r2_work;
        if let Some(duration) = stored_transport.r2_duration {
            timing.record(ConversionPhase::R2WriteThrough, duration);
        } else {
            timing.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        }
        info!(
            "Stored {} signed CCS objects",
            stored_transport.transport.objects.len()
        );

        let persist_service = self.clone();
        let source_profile_owned = parsed.source.source_profile().to_string();
        let started = Instant::now();
        tokio::task::spawn_blocking(move || {
            persist_service.persist_conversion_result(PersistConversionInput {
                source_profile: source_profile_owned,
                metadata: parsed.metadata,
                format: parsed.format,
                source_checksum: parsed.source_checksum,
                conversion_result: parsed.conversion_result,
                source: parsed.source,
                profile_revision_sha256,
                transport: stored_transport.transport,
            })
        })
        .await
        .map_err(|e| anyhow!("conversion persistence task panicked: {e}"))?
        .inspect(|_| timing.record(ConversionPhase::Persistence, started.elapsed()))
    }

    fn record_cache_hit_skips(timing: &mut ConversionTimingReport) {
        for phase in [
            ConversionPhase::Download,
            ConversionPhase::Checksum,
            ConversionPhase::ArchiveExtraction,
            ConversionPhase::NativeShellAstExtraction,
            ConversionPhase::AdapterDispatch,
            ConversionPhase::CcsEmission,
            ConversionPhase::TransportVerification,
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
        source: PinnedConversionSource,
        pkg_path: PathBuf,
        output_dir: PathBuf,
        artifact_sha256: conary_core::hash::Hash,
    ) -> Result<ParsedConversion> {
        let repo_pkg = &source.repo_pkg;
        let mut phase_timings = Vec::new();
        let mut skipped_phases = Vec::new();

        let started = Instant::now();
        let (metadata, files, format) = self.parse_package(&pkg_path, source_profile)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::ArchiveExtraction,
            duration_ms: started.elapsed().as_millis(),
        });

        let started = Instant::now();
        Self::validate_repository_identity(&metadata, repo_pkg)?;
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
            source,
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
