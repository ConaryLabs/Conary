// apps/remi/src/server/conversion/workflow.rs
//! Cold/hot package conversion workflow orchestration.

use super::ConversionService;
use super::lookup::{PackageDownloadRequest, PinnedConversionSource};
use super::persistence::PersistConversionInput;
use crate::server::catalog_authority::ProfileRevisionSelection;
use crate::server::conversion_timing::{
    ConversionNestedPhase, ConversionNestedPhaseTiming, ConversionPhase, ConversionPhaseTiming,
    ConversionSourceIdentity, ConversionTimingReport,
};
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, NativePackageConverter};
use conary_core::db::models::RepositoryPackage;
use conary_core::repository::catalog::CatalogPackageRecordV1;
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
    nested_phase_timings: Vec<ConversionNestedPhaseTiming>,
    native_parse: conary_core::packages::NativePackageParseMetrics,
    native_payload_entries: u64,
    native_payload_regular_files: u64,
    native_payload_declared_bytes: u64,
}

enum ConversionSourceSelection {
    Active,
    Pinned(ProfileRevisionSelection),
    Exact {
        selection: ProfileRevisionSelection,
        package: Box<CatalogPackageRecordV1>,
    },
}

enum ConversionArtifactSelection {
    Download,
    AuthenticatedLocal(PathBuf),
}

struct ConversionRequest<'a> {
    distro: &'a str,
    package_name: &'a str,
    version: Option<&'a str>,
    architecture: Option<&'a str>,
    selection: ConversionSourceSelection,
    artifact: ConversionArtifactSelection,
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
        self.convert_package_with_selection_async(
            distro,
            package_name,
            version,
            architecture,
            ConversionSourceSelection::Active,
            ConversionArtifactSelection::Download,
        )
        .await
    }

    pub(crate) async fn convert_package_from_selection_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        selection: ProfileRevisionSelection,
    ) -> Result<super::ServerConversionResult> {
        self.convert_package_with_selection_async(
            distro,
            package_name,
            version,
            architecture,
            ConversionSourceSelection::Pinned(selection),
            ConversionArtifactSelection::Download,
        )
        .await
    }

    pub(crate) async fn convert_catalog_package_from_selection_async(
        &self,
        distro: &str,
        package: CatalogPackageRecordV1,
        selection: ProfileRevisionSelection,
    ) -> Result<super::ServerConversionResult> {
        let name = package.name.clone();
        let version = package.version.clone();
        let architecture = package.architecture.clone();
        self.convert_package_with_selection_async(
            distro,
            &name,
            Some(&version),
            architecture.as_deref(),
            ConversionSourceSelection::Exact {
                selection,
                package: Box::new(package),
            },
            ConversionArtifactSelection::Download,
        )
        .await
    }

    pub(crate) async fn convert_benchmark_catalog_package_from_selection_async(
        &self,
        package: CatalogPackageRecordV1,
        selection: ProfileRevisionSelection,
        source_artifact: PathBuf,
    ) -> Result<super::ServerConversionResult> {
        let profile =
            conary_core::repository::supported_profiles::profile_by_id(&selection.source_profile)
                .ok_or_else(|| {
                anyhow!(
                    "benchmark profile '{}' is not a known source profile",
                    selection.source_profile
                )
            })?;
        let name = package.name.clone();
        let version = package.version.clone();
        let architecture = package.architecture.clone();
        self.convert_package_with_selection_async(
            profile.remi_route_slug(),
            &name,
            Some(&version),
            architecture.as_deref(),
            ConversionSourceSelection::Exact {
                selection,
                package: Box::new(package),
            },
            ConversionArtifactSelection::AuthenticatedLocal(source_artifact),
        )
        .await
    }

    async fn convert_package_with_selection_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
        selection: ConversionSourceSelection,
        artifact: ConversionArtifactSelection,
    ) -> Result<super::ServerConversionResult> {
        let mut timing = ConversionTimingReport::new(distro, package_name, version);
        let result = self
            .convert_package_async_inner(
                ConversionRequest {
                    distro,
                    package_name,
                    version,
                    architecture,
                    selection,
                    artifact,
                },
                &mut timing,
            )
            .await;

        match result {
            Ok(mut result) => {
                Self::record_result_output_work(
                    &mut timing,
                    &result.cache_state,
                    result.total_size,
                    result.transport.objects.iter().map(|object| object.size),
                )?;
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

    fn record_result_output_work(
        timing: &mut ConversionTimingReport,
        cache_state: &str,
        total_size: u64,
        object_sizes: impl IntoIterator<Item = u64>,
    ) -> Result<()> {
        if cache_state == "hot" {
            return Ok(());
        }

        timing.work.ccs_output_bytes = total_size;
        let mut signed_object_count = 0_u64;
        let mut signed_object_bytes = 0_u64;
        for size in object_sizes {
            signed_object_count = signed_object_count
                .checked_add(1)
                .context("signed conversion object count overflow")?;
            signed_object_bytes = signed_object_bytes
                .checked_add(size)
                .context("signed conversion object byte count overflow")?;
        }
        timing.work.signed_object_count = signed_object_count;
        timing.work.signed_object_bytes = signed_object_bytes;
        Ok(())
    }

    async fn convert_package_async_inner(
        &self,
        request: ConversionRequest<'_>,
        timing: &mut ConversionTimingReport,
    ) -> Result<super::ServerConversionResult> {
        let ConversionRequest {
            distro,
            package_name,
            version,
            architecture,
            selection,
            artifact,
        } = request;
        info!(
            "Converting package: {}:{} (version: {:?})",
            distro, package_name, version
        );

        let started = Instant::now();
        let source_feed = match &selection {
            ConversionSourceSelection::Exact { selection, .. } => {
                let profile = conary_core::repository::supported_profiles::profile_by_id(
                    &selection.source_profile,
                )
                .ok_or_else(|| {
                    anyhow!(
                        "unsupported exact source profile '{}'",
                        selection.source_profile
                    )
                })?;
                if profile.remi_route_slug() != distro {
                    return Err(anyhow!(
                        "selected catalog profile '{}' uses route '{}' instead of '{}'",
                        profile.id(),
                        profile.remi_route_slug(),
                        distro
                    ));
                }
                profile
            }
            _ => Self::public_feed_for_route(distro)?,
        };
        let source = match selection {
            ConversionSourceSelection::Exact { selection, package } => {
                if selection.source_profile != source_feed.id() {
                    return Err(anyhow!(
                        "selected catalog profile '{}' does not match route '{}' profile '{}'",
                        selection.source_profile,
                        distro,
                        source_feed.id()
                    ));
                }
                self.find_exact_package_for_selected_revision_async(selection, *package)
                    .await?
            }
            ConversionSourceSelection::Pinned(selection) => {
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
            ConversionSourceSelection::Active => {
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
        let authenticated_local_artifact = matches!(
            &artifact,
            ConversionArtifactSelection::AuthenticatedLocal(_)
        );
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
            if authenticated_local_artifact {
                timing.record_skipped(
                    ConversionPhase::LocalArtifactAdmission,
                    "exact cache hit; local source artifact did not need admission",
                );
            }
            Self::record_cache_hit_skips(timing);
            return Ok(existing);
        }
        timing.record(ConversionPhase::CacheLookup, started.elapsed());

        let local_artifact = match artifact {
            ConversionArtifactSelection::Download => None,
            ConversionArtifactSelection::AuthenticatedLocal(path) => {
                let started = Instant::now();
                let path = Self::admit_local_source_artifact(&path, &source.repo_pkg)?;
                timing.record(ConversionPhase::LocalArtifactAdmission, started.elapsed());
                timing.work.admitted_local_bytes = u64::try_from(source.repo_pkg.size)
                    .context("repository package size is negative")?;
                timing.work.repository_checksum_bytes_hashed = timing.work.admitted_local_bytes;
                Some(path)
            }
        };

        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;

        let uses_local_artifact = local_artifact.is_some();
        let (source, pkg_path) = if let Some(path) = local_artifact {
            timing.record_skipped(
                ConversionPhase::Download,
                "authenticated local source artifact; network transfer did not run",
            );
            (source, path)
        } else {
            let started = Instant::now();
            let downloaded = self
                .download_package_async(PackageDownloadRequest {
                    source,
                    dest_dir: temp_dir.path(),
                })
                .await
                .map_err(|e| anyhow!("Failed to download package: {}", e))?;
            timing.record(ConversionPhase::Download, started.elapsed());
            downloaded
        };
        let source_artifact_bytes = tokio::fs::metadata(&pkg_path).await?.len();
        timing.work.source_artifact_bytes = source_artifact_bytes;
        if !uses_local_artifact {
            timing.work.downloaded_bytes = source_artifact_bytes;
            timing.work.repository_checksum_bytes_hashed = source_artifact_bytes;
        }
        Self::record_source_identity(timing, source.source_profile(), &source.repo_pkg)?;
        info!("Downloaded to: {:?}", pkg_path);

        let checksum_path = pkg_path.clone();
        let started = Instant::now();
        let artifact_sha256 =
            tokio::task::spawn_blocking(move || Self::calculate_sha256(&checksum_path))
                .await
                .map_err(|e| anyhow!("checksum task panicked: {e}"))??;
        timing.record(ConversionPhase::Checksum, started.elapsed());
        timing.work.source_bytes_hashed = source_artifact_bytes;

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
        timing
            .nested_phases
            .extend(parsed.nested_phase_timings.clone());
        timing.work.native_payload_entries = parsed.native_payload_entries;
        timing.work.native_payload_regular_files = parsed.native_payload_regular_files;
        timing.work.native_payload_declared_bytes = parsed.native_payload_declared_bytes;
        timing.work.record_native_parse(&parsed.native_parse);
        timing
            .work
            .record_native_conversion(&parsed.conversion_result.metrics);

        let stored_transport = self
            .store_transport_with_timing(&parsed.conversion_result, source_feed.id())
            .await?;
        timing.record(
            ConversionPhase::IndependentTransportReopen,
            stored_transport.verification_duration,
        );
        timing.record(
            ConversionPhase::DurableCasIngestion,
            stored_transport.cas_duration,
        );
        timing.work.independent_transport_reopen_ccs_bytes = timing.work.ccs_output_bytes;
        let reopened_object_bytes =
            stored_transport
                .transport
                .objects
                .iter()
                .try_fold(0_u64, |total, object| {
                    total
                        .checked_add(object.size)
                        .context("signed conversion object byte count overflow")
                })?;
        timing.work.immediate_converter_reopen_object_bytes_hashed = reopened_object_bytes;
        timing.work.independent_transport_reopen_object_bytes_hashed = reopened_object_bytes;
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
        let persisted = tokio::task::spawn_blocking(move || {
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
        .map_err(|e| anyhow!("conversion persistence task panicked: {e}"))??;
        timing.record(
            ConversionPhase::CompleteArchiveHash,
            persisted.metrics.complete_archive_hash,
        );
        timing.record(
            ConversionPhase::CompleteArchiveCopy,
            persisted.metrics.complete_archive_copy,
        );
        timing.record(
            ConversionPhase::DatabasePersistence,
            persisted.metrics.database_persistence,
        );
        timing.work.complete_archive_hash_bytes = persisted.metrics.complete_archive_hash_bytes;
        timing.work.complete_archive_copy_bytes = persisted.metrics.complete_archive_copy_bytes;
        Ok(persisted.result)
    }

    fn record_cache_hit_skips(timing: &mut ConversionTimingReport) {
        for phase in [
            ConversionPhase::Download,
            ConversionPhase::Checksum,
            ConversionPhase::NativeArchiveParseAndSpool,
            ConversionPhase::ArtifactIdentityAndAuthorityValidation,
            ConversionPhase::MetadataLifecycleAndAuthorityProjection,
            ConversionPhase::PayloadReferenceDerivation,
            ConversionPhase::OutputWorkspacePreparation,
            ConversionPhase::ControlProjectionAndSigning,
            ConversionPhase::PayloadObjectEmission,
            ConversionPhase::ArchiveAssemblyAndGzip,
            ConversionPhase::ImmediateConverterReopen,
            ConversionPhase::NativeProvenanceProjection,
            ConversionPhase::IndependentTransportReopen,
            ConversionPhase::DurableCasIngestion,
            ConversionPhase::R2WriteThrough,
            ConversionPhase::CompleteArchiveHash,
            ConversionPhase::CompleteArchiveCopy,
            ConversionPhase::DatabasePersistence,
        ] {
            timing.record_skipped(phase, "cache hit; phase did not run");
        }
    }

    fn admit_local_source_artifact(
        path: &std::path::Path,
        package: &RepositoryPackage,
    ) -> Result<PathBuf> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect benchmark source artifact {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(anyhow!(
                "benchmark source artifact {} must be a regular non-symlink file",
                path.display()
            ));
        }
        let expected_size =
            u64::try_from(package.size).context("repository package size is negative")?;
        if metadata.len() != expected_size {
            return Err(anyhow!(
                "benchmark source artifact {} has {} bytes; immutable catalog requires {}",
                path.display(),
                metadata.len(),
                expected_size
            ));
        }
        conary_core::repository::verify_checksum(path, &package.checksum).with_context(|| {
            format!(
                "authenticate benchmark source artifact {} against immutable catalog checksum {}",
                path.display(),
                package.checksum
            )
        })?;
        path.canonicalize()
            .with_context(|| format!("canonicalize benchmark source artifact {}", path.display()))
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

        let started = Instant::now();
        let (metadata, files, format, native_parse) =
            self.parse_package(&pkg_path, source_profile)?;
        phase_timings.push(ConversionPhaseTiming {
            phase: ConversionPhase::NativeArchiveParseAndSpool,
            duration_ms: started.elapsed().as_millis(),
        });
        let native_payload_entries =
            u64::try_from(files.files().len()).context("native payload entry count exceeds u64")?;
        let native_payload_regular_files = u64::try_from(
            files
                .files()
                .iter()
                .filter(|file| file.content_authority.is_some())
                .count(),
        )
        .context("native regular payload file count exceeds u64")?;
        let native_payload_declared_bytes =
            files.files().iter().try_fold(0_u64, |total, file| {
                total
                    .checked_add(
                        file.content_authority
                            .as_ref()
                            .map_or(0, |content| content.size),
                    )
                    .context("native declared payload byte count overflow")
            })?;

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
            phase: ConversionPhase::ArtifactIdentityAndAuthorityValidation,
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

        let conversion_result = converter
            .convert_payload(&metadata, files.files(), format, &artifact_sha256)
            .map_err(|e| anyhow!("Conversion failed: {}", e))?;
        let metrics = &conversion_result.metrics;
        phase_timings.extend([
            ConversionPhaseTiming {
                phase: ConversionPhase::MetadataLifecycleAndAuthorityProjection,
                duration_ms: metrics
                    .metadata_lifecycle_and_authority_projection
                    .as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::PayloadReferenceDerivation,
                duration_ms: metrics.payload_reference_derivation.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::OutputWorkspacePreparation,
                duration_ms: metrics.output_workspace_preparation.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::ControlProjectionAndSigning,
                duration_ms: metrics.ccs_write.control_projection_and_signing.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::PayloadObjectEmission,
                duration_ms: metrics.ccs_write.payload_object_emission.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::ArchiveAssemblyAndGzip,
                duration_ms: metrics.ccs_write.archive_assembly_and_gzip.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::ImmediateConverterReopen,
                duration_ms: metrics.immediate_converter_reopen.as_millis(),
            },
            ConversionPhaseTiming {
                phase: ConversionPhase::NativeProvenanceProjection,
                duration_ms: metrics.native_provenance_projection.as_millis(),
            },
        ]);
        let nested_phase_timings = vec![ConversionNestedPhaseTiming {
            phase: ConversionNestedPhase::TemporaryObjectStaging,
            included_in: ConversionPhase::PayloadObjectEmission,
            duration_ms: metrics.ccs_write.temporary_object_staging.as_millis(),
        }];

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
            nested_phase_timings,
            native_parse,
            native_payload_entries,
            native_payload_regular_files,
            native_payload_declared_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::production_rust_sources;
    use super::ConversionService;
    use crate::server::conversion_timing::{ConversionTimingReport, ConversionWorkMetrics};

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
    fn exact_hot_result_records_no_conversion_output_work() {
        let mut hot = ConversionTimingReport::new("arch", "fixture", None);
        ConversionService::record_result_output_work(&mut hot, "hot", 23, [7, 11])
            .expect("record exact-hot output");
        assert_eq!(hot.work, ConversionWorkMetrics::default());

        let mut cold = ConversionTimingReport::new("arch", "fixture", None);
        ConversionService::record_result_output_work(&mut cold, "cold", 23, [7, 11])
            .expect("record cold output");
        assert_eq!(cold.work.ccs_output_bytes, 23);
        assert_eq!(cold.work.signed_object_count, 2);
        assert_eq!(cold.work.signed_object_bytes, 18);
    }
}
