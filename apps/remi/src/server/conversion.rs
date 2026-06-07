// apps/remi/src/server/conversion.rs
//! Package conversion service for the Remi server
//!
//! Downloads legacy packages from upstream repositories and converts them
//! to CCS format, storing chunks in the CAS.

mod metadata;
#[cfg(test)]
mod test_support;
mod types;

use crate::server::R2Store;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::publication::{
    PublicationDecision, PublicationRefusal, ReviewArtifactInput, ServerConversionOutcome,
    classify_converted_package, decision_refusal, write_review_artifact,
};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::{
    ConversionOptions, ConversionResult, LegacyConverter, ScriptletBundleSummary,
};
use conary_core::db::models::{ConvertedPackage, RepositoryPackage, RepositoryProvide};
use conary_core::packages::common::PackageMetadata;
use conary_core::repository::download_package;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tracing::{debug, info};

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

struct PackageDownloadRefresh<'a> {
    distro: &'a str,
    package_name: &'a str,
    version: Option<&'a str>,
    architecture: Option<&'a str>,
    repo_pkg: RepositoryPackage,
    dest_dir: &'a Path,
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

struct StoredChunks {
    chunk_hashes: Vec<String>,
    cas_duration: Duration,
    r2_duration: Option<Duration>,
}

struct PersistConversionInput {
    distro: String,
    metadata: PackageMetadata,
    format: &'static str,
    original_checksum: String,
    conversion_result: ConversionResult,
    repo_pkg: RepositoryPackage,
    chunk_hashes: Vec<String>,
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

    fn ensure_package_name_not_critical(package_name: &str) -> Result<()> {
        if conary_core::critical_packages::is_critical_package_name(package_name) {
            anyhow::bail!(
                "Refusing to convert critical system package '{}'",
                package_name
            );
        }
        Ok(())
    }

    fn metadata_provides_critical_runtime(metadata: &PackageMetadata) -> Option<&str> {
        metadata
            .provides
            .iter()
            .map(|provide| provide.name.as_str())
            .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name))
    }

    fn ensure_metadata_not_critical(metadata: &PackageMetadata) -> Result<()> {
        if let Some(capability) = Self::metadata_provides_critical_runtime(metadata) {
            anyhow::bail!(
                "Refusing to convert critical runtime capability '{}' provided by package '{}'",
                capability,
                metadata.name
            );
        }
        Ok(())
    }

    fn repository_package_provides_critical_runtime(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<Option<String>> {
        let Some(repository_package_id) = repo_pkg.id else {
            return Ok(None);
        };

        let provides = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
        Ok(provides
            .into_iter()
            .map(|provide| provide.capability)
            .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name)))
    }

    fn ensure_repository_package_not_critical(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<()> {
        if let Some(capability) =
            Self::repository_package_provides_critical_runtime(conn, repo_pkg)?
        {
            anyhow::bail!(
                "Refusing to convert critical runtime capability '{}' provided by package '{}'",
                capability,
                repo_pkg.name
            );
        }
        Ok(())
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

    pub async fn benchmark_package_sample(
        &self,
        distro: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let db_path = self.db_path.clone();
        let distro = distro.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            let distro_filter = match distro.as_str() {
                "fedora" => "fedora",
                "ubuntu" => "ubuntu",
                "debian" => "debian",
                "arch" => "arch",
                _ => return Err(anyhow!("Unknown distribution: {}", distro)),
            };
            let mut stmt = conn.prepare(
                "SELECT DISTINCT rp.name
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE COALESCE(r.default_strategy_distro, rp.distro, r.name) LIKE ?1
                 AND rp.size > 0
                 ORDER BY rp.size DESC
                 LIMIT ?2",
            )?;
            let pattern = format!("{distro_filter}%");
            let names = stmt
                .query_map(rusqlite::params![pattern, limit as i64], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(names)
        })
        .await
        .map_err(|e| anyhow!("benchmark package sample task panicked: {e}"))?
    }

    pub async fn scan_package_scriptlets(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ConversionBenchmarkEvidence> {
        // Goal 0 accepts full package downloads for scriptlet-only scanning so the
        // evidence path reuses existing parsers. Before production-scale corpus
        // scans, optimize this with ranged reads for RPM headers and DEB control
        // archives.
        let repo_pkg = self
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        let cache_dir = self
            .cache_dir
            .canonicalize()
            .unwrap_or_else(|_| self.cache_dir.clone());
        let temp_dir = TempDir::new_in(&cache_dir).context("Failed to create temp directory")?;
        let (repo_pkg, pkg_path) = self
            .download_package_with_refresh_async(PackageDownloadRefresh {
                distro,
                package_name,
                version,
                architecture,
                repo_pkg,
                dest_dir: temp_dir.path(),
            })
            .await?;
        let package_version = repo_pkg.version.clone();
        let service = self.clone();
        let distro_owned = distro.to_string();
        let package_owned = package_name.to_string();
        let summary_result = tokio::task::spawn_blocking(
            move || -> Result<crate::server::scriptlet_corpus::ScriptletCorpusSummary> {
                let (mut metadata, _files, _format) =
                    service.parse_package(&pkg_path, &distro_owned)?;
                Self::apply_repository_identity(&mut metadata, &repo_pkg);
                Ok(
                    crate::server::scriptlet_corpus::ScriptletCorpusSummary::from_scriptlets(
                        &distro_owned,
                        &package_owned,
                        &metadata.scriptlets,
                    ),
                )
            },
        )
        .await
        .map_err(|e| anyhow!("scriptlet scan task panicked: {e}"))?;
        let summary = summary_result?;

        Ok(ConversionBenchmarkEvidence {
            distro: distro.to_string(),
            package: package_name.to_string(),
            version: Some(package_version),
            scan_only: true,
            cache_state: "scan-only".to_string(),
            r2_configured: self.r2_store.is_some(),
            timing: None,
            scriptlet_summary: Some(summary),
            converted: false,
            error: None,
        })
    }

    pub async fn benchmark_package_conversion(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<ConversionBenchmarkEvidence> {
        match self
            .convert_package_async(distro, package_name, version, architecture)
            .await
        {
            Ok(outcome) => {
                let result = outcome.into_result();
                Ok(ConversionBenchmarkEvidence {
                    distro: distro.to_string(),
                    package: package_name.to_string(),
                    version: Some(result.version),
                    scan_only: false,
                    cache_state: result.cache_state,
                    r2_configured: self.r2_store.is_some(),
                    timing: result.timing,
                    scriptlet_summary: None,
                    converted: true,
                    error: None,
                })
            }
            Err(err) => Ok(ConversionBenchmarkEvidence {
                distro: distro.to_string(),
                package: package_name.to_string(),
                version: version.map(ToString::to_string),
                scan_only: false,
                cache_state: "error".to_string(),
                r2_configured: self.r2_store.is_some(),
                timing: None,
                scriptlet_summary: None,
                converted: false,
                error: Some(err.to_string()),
            }),
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

    async fn find_package_for_conversion_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<RepositoryPackage> {
        let service = self.clone();
        let distro = distro.to_string();
        let package_name = package_name.to_string();
        let version = version.map(ToString::to_string);
        let architecture = architecture.map(ToString::to_string);

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&service.db_path)?;
            let repo_pkg = service.find_package(
                &conn,
                &distro,
                &package_name,
                version.as_deref(),
                architecture.as_deref(),
            )?;
            Self::ensure_repository_package_not_critical(&conn, &repo_pkg)?;
            Ok(repo_pkg)
        })
        .await
        .map_err(|e| anyhow!("package lookup task panicked: {e}"))?
    }

    async fn cached_conversion_result_async(
        &self,
        distro: &str,
        repo_pkg: &RepositoryPackage,
        original_checksum: &str,
    ) -> Result<Option<ServerConversionOutcome>> {
        let service = self.clone();
        let distro = distro.to_string();
        let repo_pkg = repo_pkg.clone();
        let original_checksum = original_checksum.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&service.db_path)?;
            let Some(existing) = ConvertedPackage::find_by_checksum(&conn, &original_checksum)?
            else {
                return Ok(None);
            };

            let ccs_filename = Self::safe_ccs_filename_with_arch(
                &repo_pkg.name,
                &repo_pkg.version,
                repo_pkg.architecture.as_deref(),
            )?;
            let ccs_path = service.cache_dir.join("packages").join(&ccs_filename);
            if !existing.needs_reconversion() && ccs_path.exists() {
                info!(
                    "Package already converted (checksum: {})",
                    original_checksum
                );
                return service
                    .build_result_from_existing(&existing, &distro, &repo_pkg)
                    .map(Some);
            }

            info!(
                "Stale conversion record (CCS file missing or needs reconversion), re-converting"
            );
            ConvertedPackage::delete_by_checksum(&conn, &original_checksum)?;
            Ok(None)
        })
        .await
        .map_err(|e| anyhow!("conversion cache lookup task panicked: {e}"))?
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

    fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<ServerConversionOutcome> {
        let PersistConversionInput {
            distro,
            metadata,
            format,
            original_checksum,
            conversion_result,
            repo_pkg,
            chunk_hashes,
        } = input;

        let conn = conary_core::db::open(&self.db_path)?;
        let ccs_path = conversion_result
            .package_path
            .as_ref()
            .ok_or_else(|| anyhow!("No CCS package path"))?;

        let content_hash = Self::calculate_checksum(ccs_path)?;
        let total_size = std::fs::metadata(ccs_path)?.len();

        let package_architecture = repo_pkg
            .architecture
            .clone()
            .or_else(|| metadata.architecture.clone());
        let ccs_filename = Self::safe_ccs_filename_with_arch(
            &metadata.name,
            &metadata.version,
            package_architecture.as_deref(),
        )?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(ccs_path, &final_ccs_path)?;

        let mut converted = ConvertedPackage::new_server(
            distro.clone(),
            metadata.name.clone(),
            metadata.version.clone(),
            format.to_string(),
            original_checksum,
            conversion_result.fidelity.level.to_string(),
            &chunk_hashes,
            total_size as i64,
            content_hash.clone(),
            final_ccs_path.to_string_lossy().to_string(),
        );
        converted.detected_hooks = Some(serde_json::to_string(&conversion_result.detected_hooks)?);
        converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
        converted.package_architecture = package_architecture;
        let decision = classify_converted_package(&converted);
        if let Some(refusal) = decision_refusal(decision) {
            let mut report = match refusal {
                PublicationRefusal::ReviewRequired(report)
                | PublicationRefusal::Blocked(report) => report,
            };
            report.review_artifact_available = true;
            let conversion_fidelity = conversion_result.fidelity.level.to_string();
            let artifact_path = write_review_artifact(
                &self.cache_dir,
                ReviewArtifactInput {
                    distro: &distro,
                    package: &metadata.name,
                    version: &metadata.version,
                    architecture: converted.package_architecture.as_deref(),
                    original_format: &conversion_result.original_format,
                    conversion_fidelity: &conversion_fidelity,
                    conversion_version: conary_core::db::models::CONVERSION_VERSION,
                    ccs_content_hash: &content_hash,
                    ccs_total_size: total_size,
                    publication: report,
                },
            )?;
            let mut summary = conversion_result.scriptlet_metadata.clone();
            summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
            converted.set_scriptlet_metadata(&summary)?;
        }
        converted.insert(&conn)?;

        info!(
            "Recorded conversion in database (distro={}, name={}, version={})",
            distro, metadata.name, metadata.version
        );

        let result = ServerConversionResult {
            name: metadata.name,
            version: metadata.version,
            distro: distro.clone(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&converted.scriptlet_summary()),
            publication: None,
            timing: None,
        };
        Ok(Self::outcome_from_converted_result(&converted, result))
    }

    /// Find a package in the repository metadata.
    ///
    /// When `version` is `Some`, returns the exact match. When `None`, fetches
    /// all candidates and picks the latest using scheme-aware version comparison
    /// (RPM/Debian/Arch) instead of lexicographic ordering.
    fn find_package(
        &self,
        conn: &rusqlite::Connection,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<RepositoryPackage> {
        use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
        use conary_core::repository::distro::{flavor_from_distro_name, flavor_to_version_scheme};
        use conary_core::repository::versioning::compare_repo_versions;

        let flavor = flavor_from_distro_name(distro)
            .ok_or_else(|| anyhow!("Unknown distribution: {}", distro))?;
        let repo_pattern = match flavor {
            RepositoryDependencyFlavor::Rpm => "fedora%",
            RepositoryDependencyFlavor::Deb => "ubuntu%",
            RepositoryDependencyFlavor::Arch => "arch%",
        };
        let scheme = flavor_to_version_scheme(flavor);

        let row_mapper = |row: &rusqlite::Row| {
            Ok(RepositoryPackage {
                id: row.get(0)?,
                repository_id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                architecture: row.get(4)?,
                description: row.get(5)?,
                checksum: row.get(6)?,
                size: row.get(7)?,
                download_url: row.get(8)?,
                dependencies: row.get(9)?,
                metadata: row.get(10)?,
                synced_at: row.get(11)?,
                is_security_update: row.get(12)?,
                severity: row.get(13)?,
                cve_ids: row.get(14)?,
                advisory_id: row.get(15)?,
                advisory_url: row.get(16)?,
                distro: None,
                version_scheme: None,
                canonical_id: None,
            })
        };

        // When a specific version is requested, use a simple exact-match query.
        if let Some(ver) = version {
            if let Some(arch) = architecture {
                let mut stmt = conn.prepare(
                    "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                            rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                            rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                            rp.cve_ids, rp.advisory_id, rp.advisory_url
                     FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1
                     AND r.name LIKE ?2
                     AND rp.version = ?3
                     AND rp.architecture = ?4
                     AND rp.size > 0
                     LIMIT 1",
                )?;

                return stmt
                    .query_row(
                        rusqlite::params![package_name, repo_pattern, ver, arch],
                        row_mapper,
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            anyhow!(
                                "Package '{}' version '{}' arch '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                                package_name, ver, arch, distro
                            )
                        }
                        _ => anyhow!("Database error: {}", e),
                    });
            }

            let mut stmt = conn.prepare(
                "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                        rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                        rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                        rp.cve_ids, rp.advisory_id, rp.advisory_url
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.name = ?1
                 AND r.name LIKE ?2
                 AND rp.version = ?3
                 AND rp.size > 0
                 LIMIT 1",
            )?;

            return stmt
                .query_row(rusqlite::params![package_name, repo_pattern, ver], row_mapper)
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        anyhow!(
                            "Package '{}' version '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                            package_name, ver, distro
                        )
                    }
                    _ => anyhow!("Database error: {}", e),
                });
        }

        // No version specified: fetch all candidates and pick the latest using
        // scheme-aware comparison instead of lexicographic ORDER BY.
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                    rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                    rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                    rp.cve_ids, rp.advisory_id, rp.advisory_url
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             WHERE rp.name = ?1
             AND r.name LIKE ?2
             AND (?3 IS NULL OR rp.architecture = ?3)
             AND rp.size > 0",
        )?;

        let candidates: Vec<RepositoryPackage> = stmt
            .query_map(
                rusqlite::params![package_name, repo_pattern, architecture],
                row_mapper,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| anyhow!("Database error: {}", e))?;

        if candidates.is_empty() {
            return Err(anyhow!(
                "Package '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                package_name,
                distro
            ));
        }

        // Pick the latest version using scheme-aware comparison.
        // unwrap is safe because we checked candidates is non-empty above.
        let latest = candidates
            .into_iter()
            .max_by(|a, b| {
                compare_repo_versions(scheme, &a.version, &b.version)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        Ok(latest)
    }

    async fn download_package_with_refresh_async(
        &self,
        request: PackageDownloadRefresh<'_>,
    ) -> Result<(RepositoryPackage, PathBuf)> {
        let PackageDownloadRefresh {
            distro,
            package_name,
            version,
            architecture,
            repo_pkg,
            dest_dir,
        } = request;
        match download_package(&repo_pkg, dest_dir).await {
            Ok(path) => return Ok((repo_pkg, path)),
            Err(err) if !Self::is_upstream_not_found(&err) => return Err(err.into()),
            Err(err) => {
                info!(
                    "Download for {}:{} hit upstream 404 ({}), refreshing repo {} once",
                    distro, package_name, err, repo_pkg.repository_id
                );
            }
        }

        let db_path = self.db_path.clone();
        let repo_id = repo_pkg.repository_id;
        let repo = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            conary_core::db::models::Repository::find_by_id(&conn, repo_id)?
                .ok_or_else(|| anyhow!("Repository {} not found during refresh", repo_id))
        })
        .await
        .map_err(|e| anyhow!("repository refresh lookup task panicked: {e}"))??;
        let repo_name = repo.name.clone();
        conary_core::repository::sync_repository_from_db_path(self.db_path.clone(), repo)
            .await
            .map_err(|e| anyhow!("Repository refresh failed for {}: {}", repo_name, e))?;

        let refreshed_pkg = self
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        let path = download_package(&refreshed_pkg, dest_dir)
            .await
            .map_err(|e| anyhow!("Retry after refresh failed: {}", e))?;
        Ok((refreshed_pkg, path))
    }

    fn is_upstream_not_found(err: &conary_core::Error) -> bool {
        match err {
            conary_core::Error::DownloadError(message) => {
                message.contains("HTTP 404") || message.contains("404 Not Found")
            }
            _ => false,
        }
    }

    /// Store blobs from conversion result in CAS
    #[cfg(test)]
    async fn store_chunks(&self, result: &ConversionResult) -> Result<Vec<String>> {
        Ok(self.store_chunks_with_timing(result).await?.chunk_hashes)
    }

    async fn store_chunks_with_timing(&self, result: &ConversionResult) -> Result<StoredChunks> {
        let mut chunk_hashes = Vec::new();
        let mut cas_duration = Duration::default();
        let mut r2_duration = self.r2_store.as_ref().map(|_| Duration::default());
        let objects_dir = self.chunk_dir.join("objects");

        // Get blobs from the build result (chunks or whole files)
        for (hash, data) in &result.build_result.blobs {
            let cas_started = Instant::now();
            let (prefix, rest) = hash.split_at(2.min(hash.len()));
            let chunk_path = objects_dir.join(prefix).join(rest);

            // Skip if chunk already exists (content-addressed = immutable)
            if chunk_path.exists() {
                cas_duration += cas_started.elapsed();
                debug!("Chunk {} already exists", hash);
                chunk_hashes.push(hash.clone());
                continue;
            }

            // Create parent directory
            if let Some(parent) = chunk_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("Failed to create chunk directory")?;
            }

            // Write chunk atomically
            let temp_path = chunk_path.with_extension("tmp");
            tokio::fs::write(&temp_path, data)
                .await
                .context("Failed to write chunk")?;
            tokio::fs::rename(&temp_path, &chunk_path)
                .await
                .context("Failed to rename chunk")?;
            cas_duration += cas_started.elapsed();

            // R2 write-through: upload to Cloudflare R2 in parallel
            if let Some(ref r2) = self.r2_store {
                let r2_started = Instant::now();
                if let Err(e) = r2.put_chunk(hash, data).await {
                    tracing::warn!("R2 write-through failed for chunk {}: {}", hash, e);
                } else {
                    debug!("R2 write-through: uploaded chunk {}", hash);
                }
                if let Some(total) = &mut r2_duration {
                    *total += r2_started.elapsed();
                }
            }

            debug!("Stored chunk: {} ({} bytes)", hash, data.len());
            chunk_hashes.push(hash.clone());
        }

        Ok(StoredChunks {
            chunk_hashes,
            cas_duration,
            r2_duration,
        })
    }

    /// Calculate SHA-256 checksum of a file
    fn calculate_checksum(path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        Ok(conary_core::hash::sha256_reader_hex(&mut file)?)
    }

    /// Build result from existing conversion record
    fn build_result_from_existing(
        &self,
        existing: &ConvertedPackage,
        distro: &str,
        repo_pkg: &RepositoryPackage,
    ) -> Result<ServerConversionOutcome> {
        // Use server-side fields from ConvertedPackage if available, else fallback to repo_pkg
        let name = existing
            .package_name
            .clone()
            .unwrap_or_else(|| repo_pkg.name.clone());
        let version = existing
            .package_version
            .clone()
            .unwrap_or_else(|| repo_pkg.version.clone());

        // Prefer stored CCS path if available
        let ccs_path = if let Some(stored_path) = &existing.ccs_path {
            PathBuf::from(stored_path)
        } else {
            let ccs_filename = Self::safe_ccs_filename_with_arch(
                &name,
                &version,
                existing.package_architecture.as_deref(),
            )?;
            self.cache_dir.join("packages").join(&ccs_filename)
        };

        // Parse chunk hashes from JSON if stored
        let chunk_hashes: Vec<String> = existing
            .chunk_hashes_json
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();

        let scriptlet_summary = existing.scriptlet_summary();

        let result = ServerConversionResult {
            name,
            version,
            distro: existing
                .distro
                .clone()
                .unwrap_or_else(|| distro.to_string()),
            chunk_hashes,
            total_size: u64::try_from(existing.total_size.unwrap_or(repo_pkg.size)).unwrap_or(0),
            content_hash: existing
                .content_hash
                .clone()
                .unwrap_or_else(|| existing.original_checksum.clone()),
            ccs_path,
            cache_state: "hot".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
            publication: None,
            timing: None,
        };
        Ok(Self::outcome_from_converted_result(existing, result))
    }

    fn outcome_from_converted_result(
        converted: &ConvertedPackage,
        mut result: ServerConversionResult,
    ) -> ServerConversionOutcome {
        match classify_converted_package(converted) {
            PublicationDecision::Ready => ServerConversionOutcome::Ready(result),
            PublicationDecision::ReviewRequired(report) => {
                result.publication = Some(report);
                ServerConversionOutcome::ReviewRequired(result)
            }
            PublicationDecision::Blocked(report) => {
                result.publication = Some(report);
                ServerConversionOutcome::Blocked(result)
            }
        }
    }

    /// Build a package from a recipe URL
    ///
    /// 1. Fetch the recipe from the URL
    /// 2. Parse and validate the recipe
    /// 3. Cook it using the Kitchen (with isolation)
    /// 4. Store chunks in CAS
    /// 5. Return the result
    pub async fn build_from_recipe(&self, recipe_url: &str) -> Result<ServerConversionResult> {
        use conary_core::recipe::{Kitchen, KitchenConfig, parse_recipe};

        info!("Building package from recipe: {}", recipe_url);

        // Step 1: Fetch recipe content
        let recipe_content = Self::fetch_url(recipe_url).await?;
        info!("Fetched recipe ({} bytes)", recipe_content.len());

        // Step 2: Parse and validate recipe
        let recipe =
            parse_recipe(&recipe_content).map_err(|e| anyhow!("Failed to parse recipe: {}", e))?;

        info!(
            "Recipe: {} version {}",
            recipe.package.name, recipe.package.version
        );

        // Step 3: Cook the recipe
        let temp_dir =
            TempDir::new_in(&self.cache_dir).context("Failed to create temp directory")?;

        let config = KitchenConfig {
            source_cache: self.cache_dir.join("sources"),
            use_isolation: true, // Always use isolation on server
            ..Default::default()
        };

        let kitchen = Kitchen::new(config);
        let cook_result = kitchen
            .cook(&recipe, temp_dir.path())
            .map_err(|e| anyhow!("Recipe cooking failed: {}", e))?;

        info!(
            "Cooked: {} ({} warnings)",
            cook_result.package_path.display(),
            cook_result.warnings.len()
        );

        // Step 4: Store chunks
        let ccs_data = tokio::fs::read(&cook_result.package_path)
            .await
            .context("Failed to read cooked CCS package")?;

        let content_hash = conary_core::hash::sha256(&ccs_data);

        // Copy CCS package to persistent location
        let ccs_filename = Self::safe_ccs_filename(&recipe.package.name, &recipe.package.version)?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&cook_result.package_path, &final_ccs_path).await?;

        // Extract chunk hashes from the CCS package
        // For now, we'll just report the package itself
        let chunk_hashes = vec![content_hash.clone()];
        let total_size = ccs_data.len() as u64;

        Ok(ServerConversionResult {
            name: recipe.package.name,
            version: recipe.package.version,
            distro: "recipe".to_string(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "recipe".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        })
    }

    /// Fetch content from a URL (with security validation)
    ///
    /// SECURITY: This function validates URLs and blocks requests to:
    /// - Private IP ranges (10.x, 172.16-31.x, 192.168.x, 127.x)
    /// - Link-local addresses (169.254.x)
    /// - Loopback addresses
    /// - IPv6 private/local addresses
    ///
    /// This prevents SSRF attacks where a malicious recipe URL could be used
    /// to probe internal services.
    async fn fetch_url(url: &str) -> Result<String> {
        // Parse URL to validate scheme and extract host
        let parsed_url =
            url::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;

        // Only allow https (http redirects to https, but we reject http-only)
        let scheme = parsed_url.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(anyhow!("Only http/https URLs are allowed, got: {}", scheme));
        }

        // Extract host
        let host = parsed_url
            .host_str()
            .ok_or_else(|| anyhow!("URL '{}' has no host", url))?;

        // Check for prohibited hosts
        Self::validate_host(host)?;

        // Resolve DNS and validate the resolved IP. We pin the validated IP
        // using reqwest's `resolve()` so that reqwest connects to the exact
        // address we checked, closing the DNS rebinding TOCTOU gap.
        let port = parsed_url
            .port()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        let resolved_ips: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(format!("{host}:{port}"))
                .await
                .map_err(|e| anyhow!("Failed to resolve '{}': {}", host, e))?
                .collect();

        // Check all resolved IPs - if ANY is private, reject
        for addr in &resolved_ips {
            Self::validate_ip(&addr.ip())?;
        }

        // Pin the first validated IP so reqwest uses it instead of
        // re-resolving (which could return a different, malicious IP).
        let pinned_ip = resolved_ips
            .first()
            .ok_or_else(|| anyhow!("DNS resolution for '{}' returned no addresses", host))?;

        // SECURITY: Disable automatic redirects entirely AND pin the
        // resolved IP. This closes both the redirect-based and
        // DNS-rebinding SSRF vectors.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve(host, *pinned_ip)
            .user_agent("conary-remi/0.1")
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch '{}': {}", url, e))?;

        // Reject redirects -- the response status will be 3xx if the server
        // tried to redirect. Return a clear error instead of following.
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<unknown>");
            return Err(anyhow!(
                "URL '{}' returned redirect ({}) to '{}'. \
                 Redirects are rejected to prevent SSRF.",
                url,
                response.status().as_u16(),
                location
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP {} fetching '{}': {}",
                response.status().as_u16(),
                url,
                response.status().canonical_reason().unwrap_or("Unknown")
            ));
        }

        response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response body: {}", e))
    }

    /// Validate a hostname is not a private/internal address
    fn validate_host(host: &str) -> Result<()> {
        // Check for localhost aliases
        let lower_host = host.to_lowercase();
        if lower_host == "localhost"
            || lower_host.ends_with(".localhost")
            || lower_host == "127.0.0.1"
            || lower_host == "::1"
            || lower_host == "0.0.0.0"
        {
            return Err(anyhow!("Localhost URLs are not allowed"));
        }

        // Check for AWS/cloud metadata endpoints
        if lower_host == "169.254.169.254"
            || lower_host.contains("metadata")
            || lower_host == "metadata.google.internal"
        {
            return Err(anyhow!("Cloud metadata endpoints are not allowed"));
        }

        // Check for internal domain suffixes
        let internal_suffixes = [".internal", ".local", ".lan", ".home", ".corp"];
        for suffix in internal_suffixes {
            if lower_host.ends_with(suffix) {
                return Err(anyhow!("Internal domain '{}' is not allowed", host));
            }
        }

        Ok(())
    }

    /// Validate an IP address is not private/internal
    fn validate_ip(ip: &std::net::IpAddr) -> Result<()> {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();

                // Loopback: 127.0.0.0/8
                if octets[0] == 127 {
                    return Err(anyhow!("Loopback addresses are not allowed"));
                }

                // Private: 10.0.0.0/8
                if octets[0] == 10 {
                    return Err(anyhow!("Private IP range 10.x.x.x is not allowed"));
                }

                // Private: 172.16.0.0/12
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return Err(anyhow!("Private IP range 172.16-31.x.x is not allowed"));
                }

                // Private: 192.168.0.0/16
                if octets[0] == 192 && octets[1] == 168 {
                    return Err(anyhow!("Private IP range 192.168.x.x is not allowed"));
                }

                // Link-local: 169.254.0.0/16 (includes AWS metadata)
                if octets[0] == 169 && octets[1] == 254 {
                    return Err(anyhow!("Link-local addresses are not allowed"));
                }

                // Broadcast: 255.255.255.255
                if octets == [255, 255, 255, 255] {
                    return Err(anyhow!("Broadcast addresses are not allowed"));
                }

                // Unspecified: 0.0.0.0
                if octets == [0, 0, 0, 0] {
                    return Err(anyhow!("Unspecified addresses are not allowed"));
                }

                Ok(())
            }
            std::net::IpAddr::V6(ipv6) => {
                // Loopback: ::1
                if ipv6.is_loopback() {
                    return Err(anyhow!("IPv6 loopback is not allowed"));
                }

                // Unspecified: ::
                if ipv6.is_unspecified() {
                    return Err(anyhow!("IPv6 unspecified is not allowed"));
                }

                // Private/ULA: fc00::/7
                let segments = ipv6.segments();
                if (segments[0] & 0xfe00) == 0xfc00 {
                    return Err(anyhow!("IPv6 unique local addresses are not allowed"));
                }

                // Link-local: fe80::/10
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return Err(anyhow!("IPv6 link-local addresses are not allowed"));
                }

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use conary_core::ccs::convert::ScriptletDecisionCountsSummary;
    use conary_core::db::models::{ConvertedPackage, RepositoryPackage, RepositoryProvide};
    use conary_core::packages::common::PackageMetadata;
    use conary_core::packages::traits::{Dependency, DependencyType};
    use std::path::Path;

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

    // --- calculate_checksum tests ---

    #[test]
    fn test_calculate_checksum_valid_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.pkg");
        std::fs::write(&file_path, b"hello world").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            checksum,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_calculate_checksum_empty_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.pkg");
        std::fs::write(&file_path, b"").unwrap();

        let checksum = ConversionService::calculate_checksum(&file_path).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_calculate_checksum_missing_file() {
        let result = ConversionService::calculate_checksum(Path::new("/nonexistent/file.pkg"));
        assert!(result.is_err());
    }

    // --- find_package tests ---

    #[test]
    fn test_find_package_found() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora", "nginx", None, None)
            .unwrap();
        assert_eq!(pkg.name, "nginx");
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0", 1100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora", "nginx", Some("1.24.0"), None)
            .unwrap();
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version_and_architecture() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");

        let mut i686 = RepositoryPackage::new(
            repo_id,
            "glib2".to_string(),
            "2.86.0-2.fc44".to_string(),
            "sha256:glib2-i686".to_string(),
            1024,
            "https://example.com/glib2-2.86.0-2.fc44.i686.rpm".to_string(),
        );
        i686.architecture = Some("i686".to_string());
        i686.insert(&conn).unwrap();

        let mut x86_64 = RepositoryPackage::new(
            repo_id,
            "glib2".to_string(),
            "2.86.0-2.fc44".to_string(),
            "sha256:glib2-x86_64".to_string(),
            2048,
            "https://example.com/glib2-2.86.0-2.fc44.x86_64.rpm".to_string(),
        );
        x86_64.architecture = Some("x86_64".to_string());
        x86_64.insert(&conn).unwrap();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(
                &conn,
                "fedora",
                "glib2",
                Some("2.86.0-2.fc44"),
                Some("x86_64"),
            )
            .unwrap();
        assert_eq!(pkg.architecture.as_deref(), Some("x86_64"));
        assert!(pkg.download_url.ends_with(".x86_64.rpm"));
    }

    #[test]
    fn test_find_package_not_found() {
        let (temp_file, conn) = create_test_db();
        insert_repo(&conn, "fedora-base", "fedora");

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "fedora", "nonexistent", None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
        assert!(err_msg.contains("repo-sync"));
    }

    #[test]
    fn test_find_package_unknown_distro() {
        let (temp_file, conn) = create_test_db();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "gentoo", "nginx", None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown distribution"));
    }

    #[test]
    fn test_find_package_arch_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, repo_id, "pacman", "6.0.0", 800);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "arch", "pacman", None, None)
            .unwrap();
        assert_eq!(pkg.name, "pacman");
    }

    #[test]
    fn test_find_package_ubuntu_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
        insert_package(&conn, repo_id, "libc6", "2.38-1", 2048);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "ubuntu", "libc6", None, None)
            .unwrap();
        assert_eq!(pkg.name, "libc6");
    }

    #[test]
    fn test_find_package_debian_is_not_supported_distro() {
        let (temp_file, conn) = create_test_db();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let err = service
            .find_package(&conn, "debian", "apt", None, None)
            .expect_err("debian is not a supported Remi distro")
            .to_string();
        assert!(err.contains("Unknown distribution"));
    }

    // --- build_result_from_existing tests ---

    #[test]
    fn test_build_result_from_existing_with_server_fields() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "nginx".to_string(),
            "1.24.0".to_string(),
            "rpm".to_string(),
            "sha256:orig".to_string(),
            "high".to_string(),
            &["chunk1".to_string(), "chunk2".to_string()],
            2048,
            "sha256:content_abc".to_string(),
            "/data/nginx.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "nginx", None, None)
            .unwrap();

        let outcome = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();
        let result = outcome.result();

        assert_eq!(result.name, "nginx");
        assert_eq!(result.version, "1.24.0");
        assert_eq!(result.distro, "fedora");
        assert_eq!(result.chunk_hashes, vec!["chunk1", "chunk2"]);
        assert_eq!(result.total_size, 2048);
        assert_eq!(result.content_hash, "sha256:content_abc");
        assert_eq!(result.ccs_path, PathBuf::from("/data/nginx.ccs"));
    }

    #[test]
    fn test_build_result_from_existing_without_chunk_hashes() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "curl", "8.5.0", 512);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        // Create a converted package with no chunk_hashes_json
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "curl".to_string(),
            "8.5.0".to_string(),
            "rpm".to_string(),
            "sha256:curl-orig".to_string(),
            "high".to_string(),
            &[],
            512,
            "sha256:curl-content".to_string(),
            "/data/curl.ccs".to_string(),
        );
        converted.insert(&conn).unwrap();

        let existing = ConvertedPackage::find_by_checksum(&conn, "sha256:curl-orig")
            .unwrap()
            .unwrap();

        let repo_pkg = service
            .find_package(&conn, "fedora", "curl", None, None)
            .unwrap();

        let outcome = service
            .build_result_from_existing(&existing, "fedora", &repo_pkg)
            .unwrap();
        let result = outcome.result();

        // Should return empty chunk list, not panic
        assert!(result.chunk_hashes.is_empty());
    }

    #[test]
    fn persisted_goal8a_golden_outcomes_respect_publication_gate() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let service = ConversionService::new(chunk_dir, cache_dir.clone(), db_path.clone(), None);

        let mut native_free = goal8a_scriptlet_summary("native-free", "source-native", "public");
        native_free.decision_counts = ScriptletDecisionCountsSummary::default();

        let mut fully_replaced =
            goal8a_scriptlet_summary("fully-replaced", "source-native", "public");
        fully_replaced.decision_counts = ScriptletDecisionCountsSummary {
            replaced: 2,
            ..ScriptletDecisionCountsSummary::default()
        };

        let mut legacy_replay =
            goal8a_scriptlet_summary("legacy-replay", "source-native", "private-review");
        legacy_replay.decision_counts = ScriptletDecisionCountsSummary {
            legacy: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        legacy_replay
            .review_reason_codes
            .push("legacy-replay-required".to_string());

        let mut review_required =
            goal8a_scriptlet_summary("review-required", "review-required", "private-review");
        review_required.decision_counts = ScriptletDecisionCountsSummary {
            review: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        review_required
            .review_reason_codes
            .push("review-class-deb-trigger".to_string());
        review_required
            .unknown_commands
            .push("custom-helper".to_string());

        let mut blocked = goal8a_scriptlet_summary("blocked", "blocked", "blocked");
        blocked.decision_counts = ScriptletDecisionCountsSummary {
            blocked: 1,
            ..ScriptletDecisionCountsSummary::default()
        };
        blocked
            .blocked_reason_codes
            .push("blocked-class-package-manager-recursion".to_string());
        blocked
            .blocked_classes
            .push("package-manager-recursion".to_string());

        let cases = [
            ("goal8a-native-free", native_free, "ready", true),
            ("goal8a-fully-replaced", fully_replaced, "ready", true),
            (
                "goal8a-legacy-replay",
                legacy_replay,
                "review-required",
                false,
            ),
            (
                "goal8a-review-required",
                review_required,
                "review-required",
                false,
            ),
            ("goal8a-blocked", blocked, "blocked", false),
        ];

        for (index, (name, summary, expected_outcome, public_ready)) in cases.iter().enumerate() {
            let output_ccs = temp.path().join("out").join(format!("{name}.ccs"));
            std::fs::create_dir_all(output_ccs.parent().unwrap()).unwrap();
            std::fs::write(&output_ccs, format!("ccs payload {name}")).unwrap();

            let metadata = PackageMetadata::new(
                PathBuf::from(format!("/tmp/{name}.rpm")),
                (*name).to_string(),
                "1.0".to_string(),
            );
            let mut result = make_conversion_result(Default::default());
            result.package_path = Some(output_ccs);
            result.scriptlet_metadata = summary.clone();

            let mut repo_pkg = RepositoryPackage::new(
                index as i64 + 1,
                (*name).to_string(),
                "1.0".to_string(),
                format!("sha256:{name}-source"),
                11,
                format!("https://example.invalid/{name}.rpm"),
            );
            repo_pkg.architecture = Some("x86_64".to_string());

            let outcome = service
                .persist_conversion_result(PersistConversionInput {
                    distro: "fedora".to_string(),
                    metadata,
                    format: "rpm",
                    original_checksum: format!("sha256:{name}-original"),
                    conversion_result: result,
                    repo_pkg: repo_pkg.clone(),
                    chunk_hashes: vec![format!("sha256:{name}-chunk")],
                })
                .unwrap();
            let observed_outcome = match &outcome {
                ServerConversionOutcome::Ready(_) => "ready",
                ServerConversionOutcome::ReviewRequired(_) => "review-required",
                ServerConversionOutcome::Blocked(_) => "blocked",
            };
            assert_eq!(observed_outcome, *expected_outcome, "{name}");

            let server_result = outcome.result();
            assert_eq!(
                server_result.scriptlets.scriptlet_fidelity, summary.scriptlet_fidelity,
                "{name}"
            );
            assert_eq!(
                server_result.scriptlets.publication_status, summary.publication_status,
                "{name}"
            );
            assert_eq!(server_result.publication.is_none(), *public_ready, "{name}");
            assert_eq!(
                server_result.scriptlets.review_artifact_available, !*public_ready,
                "{name}"
            );
            let public_metadata_json = serde_json::to_string(&server_result.scriptlets).unwrap();
            assert!(!public_metadata_json.contains("review_artifact_path"));
            assert!(!public_metadata_json.contains(cache_dir.to_str().unwrap()));
            if let Some(report) = &server_result.publication {
                let report_json = serde_json::to_string(report).unwrap();
                assert!(report.evidence_digest.is_some(), "{name}");
                assert!(!report_json.contains("review_artifact_path"));
                assert!(!report_json.contains(cache_dir.to_str().unwrap()));
                assert!(!report_json.contains("legacy_scriptlets"));
            }
        }

        let conn = conary_core::db::open(&db_path).unwrap();
        let candidates =
            ConvertedPackage::find_publication_candidates(&conn, "fedora", None).unwrap();
        assert_eq!(candidates.len(), cases.len());

        let public_ready_names: std::collections::BTreeSet<_> = candidates
            .iter()
            .filter(|converted| converted.is_scriptlet_public_ready())
            .map(|converted| converted.package_name.as_deref().unwrap())
            .collect();
        assert_eq!(
            public_ready_names,
            std::collections::BTreeSet::from(["goal8a-fully-replaced", "goal8a-native-free"])
        );

        for (name, summary, _expected_outcome, public_ready) in cases {
            let converted = ConvertedPackage::find_by_package_identity_with_arch(
                &conn,
                "fedora",
                name,
                Some("1.0"),
                Some("x86_64"),
            )
            .unwrap()
            .unwrap();
            assert_eq!(converted.scriptlet_fidelity, summary.scriptlet_fidelity);
            assert_eq!(converted.publication_status, summary.publication_status);
            assert_eq!(
                converted.is_scriptlet_public_ready(),
                public_ready,
                "{name}"
            );
            assert_eq!(
                converted.review_artifact_path.is_some(),
                !public_ready,
                "{name}"
            );
        }
    }

    #[test]
    fn persisted_conversion_records_scriptlet_metadata() {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        conary_core::db::init(&db_path).unwrap();
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let output_ccs = temp.path().join("out/test.ccs");
        std::fs::create_dir_all(output_ccs.parent().unwrap()).unwrap();
        std::fs::write(&output_ccs, b"ccs payload").unwrap();
        let service = ConversionService::new(chunk_dir, cache_dir, db_path.clone(), None);
        let metadata = PackageMetadata::new(
            PathBuf::from("/tmp/test.rpm"),
            "test".to_string(),
            "1.0".to_string(),
        );
        let mut result = make_conversion_result(Default::default());
        result.package_path = Some(output_ccs);
        result.scriptlet_metadata = ScriptletBundleSummary {
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            publication_status: "private-review".to_string(),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(b"remi-scriptlets")),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            review_reason_codes: vec!["review-class-debconf".to_string()],
            unknown_commands: vec!["custom-helper".to_string()],
            blocked_classes: vec!["network".to_string()],
            review_artifact_path: Some("/tmp/review-artifact.json".to_string()),
            ..ScriptletBundleSummary::default()
        };
        let mut repo_pkg = RepositoryPackage::new(
            1,
            "test".to_string(),
            "1.0".to_string(),
            "sha256:repo".to_string(),
            11,
            "https://example.invalid/test.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        let input = PersistConversionInput {
            distro: "fedora".to_string(),
            metadata,
            format: "rpm",
            original_checksum: "sha256:source".to_string(),
            conversion_result: result,
            repo_pkg: repo_pkg.clone(),
            chunk_hashes: vec!["sha256:chunk".to_string()],
        };

        let server_outcome = service.persist_conversion_result(input).unwrap();
        let server_result = server_outcome.result();

        assert_eq!(
            server_result.scriptlets.scriptlet_fidelity,
            "review-required"
        );
        assert!(server_result.scriptlets.review_artifact_available);
        let conn = conary_core::db::open(&db_path).unwrap();
        let converted = ConvertedPackage::find_by_package_identity_with_arch(
            &conn,
            "fedora",
            "test",
            Some("1.0"),
            Some("x86_64"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(converted.scriptlet_fidelity, "review-required");
        assert_eq!(converted.publication_status, "private-review");
        assert_eq!(
            converted.blocked_reason_codes_json,
            "[\"blocked-class-network\"]"
        );

        let hot = service
            .build_result_from_existing(&converted, "fedora", &repo_pkg)
            .unwrap();
        assert_eq!(
            hot.result().scriptlets.scriptlet_fidelity,
            "review-required"
        );
        assert!(hot.result().scriptlets.review_artifact_available);
    }

    #[tokio::test]
    async fn test_store_chunks_writes_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert("abcdef1234567890".to_string(), b"chunk data one".to_vec());
        blobs.insert("1234567890abcdef".to_string(), b"chunk data two".to_vec());

        let result = make_conversion_result(blobs);
        let hashes = service.store_chunks(&result).await.unwrap();
        assert_eq!(hashes.len(), 2);

        // Verify files were written to correct paths
        for hash in &hashes {
            let (prefix, rest) = hash.split_at(2);
            let chunk_path = chunk_dir.join("objects").join(prefix).join(rest);
            assert!(
                chunk_path.exists(),
                "Chunk file should exist at {:?}",
                chunk_path
            );
        }
    }

    #[tokio::test]
    async fn test_store_chunks_idempotent() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir.clone(),
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let mut blobs = std::collections::HashMap::new();
        blobs.insert("aabbccdd11223344".to_string(), b"some data".to_vec());

        let result = make_conversion_result(blobs.clone());

        // Store twice - should not error
        let hashes1 = service.store_chunks(&result).await.unwrap();

        let result2 = make_conversion_result(blobs);
        let hashes2 = service.store_chunks(&result2).await.unwrap();
        assert_eq!(hashes1, hashes2);
    }

    #[tokio::test]
    async fn test_store_chunks_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let chunk_dir = temp_dir.path().join("chunks");
        std::fs::create_dir_all(&chunk_dir).unwrap();

        let service = ConversionService::new(
            chunk_dir,
            temp_dir.path().to_path_buf(),
            PathBuf::from("/tmp/nonexistent.db"),
            None,
        );

        let result = make_conversion_result(std::collections::HashMap::new());
        let hashes = service.store_chunks(&result).await.unwrap();
        assert!(hashes.is_empty());
    }

    // --- validate_host tests ---

    #[test]
    fn test_validate_host_allows_public() {
        assert!(ConversionService::validate_host("remi.conary.io").is_ok());
        assert!(ConversionService::validate_host("github.com").is_ok());
        assert!(ConversionService::validate_host("example.com").is_ok());
    }

    #[test]
    fn test_validate_host_blocks_localhost() {
        assert!(ConversionService::validate_host("localhost").is_err());
        assert!(ConversionService::validate_host("LOCALHOST").is_err());
        assert!(ConversionService::validate_host("sub.localhost").is_err());
        assert!(ConversionService::validate_host("127.0.0.1").is_err());
        assert!(ConversionService::validate_host("::1").is_err());
        assert!(ConversionService::validate_host("0.0.0.0").is_err());
    }

    #[test]
    fn test_validate_host_blocks_cloud_metadata() {
        assert!(ConversionService::validate_host("169.254.169.254").is_err());
        assert!(ConversionService::validate_host("metadata.google.internal").is_err());
    }

    #[test]
    fn test_validate_host_blocks_internal_domains() {
        assert!(ConversionService::validate_host("server.internal").is_err());
        assert!(ConversionService::validate_host("mybox.local").is_err());
        assert!(ConversionService::validate_host("router.lan").is_err());
        assert!(ConversionService::validate_host("nas.home").is_err());
        assert!(ConversionService::validate_host("ldap.corp").is_err());
    }

    // --- validate_ip tests ---

    #[test]
    fn test_validate_ip_allows_public_ipv4() {
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "46.4.33.93".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_loopback() {
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "127.0.0.2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_10() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "10.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_172() {
        let ip: std::net::IpAddr = "172.16.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "172.31.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        // 172.15.x.x is NOT private
        let ip: std::net::IpAddr = "172.15.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        // 172.32.x.x is NOT private
        let ip: std::net::IpAddr = "172.32.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_private_192_168() {
        let ip: std::net::IpAddr = "192.168.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "192.168.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_link_local() {
        let ip: std::net::IpAddr = "169.254.169.254".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "169.254.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_broadcast() {
        let ip: std::net::IpAddr = "255.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_unspecified() {
        let ip: std::net::IpAddr = "0.0.0.0".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_allows_public_ipv6() {
        let ip: std::net::IpAddr = "2a01:4f8:221:350b::2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_loopback() {
        let ip: std::net::IpAddr = "::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_unspecified() {
        let ip: std::net::IpAddr = "::".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_ula() {
        let ip: std::net::IpAddr = "fc00::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "fd12:3456:789a::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_link_local() {
        let ip: std::net::IpAddr = "fe80::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
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

    #[test]
    fn test_find_package_maps_distro_to_repo_pattern() {
        // Verify all supported distros can resolve to their repo patterns.
        // We insert repos with the expected naming pattern and ensure find_package
        // correctly maps distro name -> LIKE pattern.
        let (temp_file, conn) = create_test_db();

        let arch_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, arch_id, "vim", "9.0", 500);

        let fed_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, fed_id, "vim", "9.0", 500);

        let ubuntu_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
        insert_package(&conn, ubuntu_id, "vim", "9.0", 500);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        assert!(
            service
                .find_package(&conn, "arch", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "fedora", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "ubuntu", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "debian", "vim", None, None)
                .is_err()
        );
    }

    #[test]
    fn test_critical_packages_blocked() {
        for package_name in [
            "glibc",
            "systemd",
            "openssl-libs",
            "sudo",
            "coreutils",
            "ca-certificates",
        ] {
            assert!(ConversionService::ensure_package_name_not_critical(package_name).is_err());
        }
    }

    #[test]
    fn shared_critical_package_names_are_refused_by_conversion_guard() {
        for package_name in ["bash", "filesystem", "setup", "GLIBC"] {
            let err = ConversionService::ensure_package_name_not_critical(package_name)
                .expect_err("critical package should be refused")
                .to_string();
            assert!(err.contains("Refusing to convert critical system package"));
            assert!(err.contains(package_name));
        }

        ConversionService::ensure_package_name_not_critical("nginx").unwrap();
    }

    #[test]
    fn metadata_provides_critical_runtime_capabilities_are_detected() {
        let mut metadata = PackageMetadata::new(
            PathBuf::from("/tmp/alt-libc.rpm"),
            "alt-libc".to_string(),
            "1.0".to_string(),
        );
        metadata.provides.push(Dependency {
            name: "libc.so.6()(64bit)".to_string(),
            version: None,
            dep_type: DependencyType::Runtime,
            description: None,
        });

        assert_eq!(
            ConversionService::metadata_provides_critical_runtime(&metadata),
            Some("libc.so.6()(64bit)")
        );
    }

    #[test]
    fn repository_provides_guard_blocks_cached_conversion_path() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "alt-libc", "1.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );
        let repo_pkg = service
            .find_package(&conn, "fedora", "alt-libc", None, None)
            .unwrap();
        let repo_pkg_id = repo_pkg.id.unwrap();
        RepositoryProvide::new(
            repo_pkg_id,
            "ld-linux-x86-64.so.2()(64bit)".to_string(),
            None,
            "virtual".to_string(),
            None,
        )
        .insert(&conn)
        .unwrap();

        let err = ConversionService::ensure_repository_package_not_critical(&conn, &repo_pkg)
            .expect_err("critical repository provide should be refused")
            .to_string();
        assert!(err.contains("Refusing to convert critical runtime capability"));
        assert!(err.contains("ld-linux-x86-64.so.2()(64bit)"));
    }

    #[test]
    fn test_normal_packages_not_blocked() {
        for package_name in ["nginx", "tree", "curl", "jq", "vim"] {
            ConversionService::ensure_package_name_not_critical(package_name).unwrap();
        }
    }

    #[test]
    fn test_detects_upstream_not_found_download_error() {
        let err = conary_core::Error::DownloadError(
            "HTTP 404 Not Found from https://example.com/pkg.rpm".to_string(),
        );
        assert!(ConversionService::is_upstream_not_found(&err));

        let other = conary_core::Error::DownloadError("HTTP 500".to_string());
        assert!(!ConversionService::is_upstream_not_found(&other));
    }
}
