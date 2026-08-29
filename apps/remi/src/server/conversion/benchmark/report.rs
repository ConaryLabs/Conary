// apps/remi/src/server/conversion/benchmark/report.rs
//! Strict schema-v3 validation and durable report publication.

use super::{
    CONVERSION_BENCHMARK_SCHEMA_V3, ConversionBenchmarkCatalogAuthority,
    ConversionBenchmarkCatalogReopen, ConversionBenchmarkCatalogSetup, ConversionBenchmarkEvidence,
    ConversionBenchmarkOutcome, ConversionBenchmarkOutputProof, ConversionBenchmarkProcessUsage,
    ConversionBenchmarkReportV3, PORTABLE_CHUNK_SIZE_V1, PortableVfsMetricsV1, PublishedInode,
    REPORT_FILE_NAME, conversion_core_duration, rollback_failed_publication, sync_parent,
    validate_sha256,
};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::repository::catalog::{portable_chunk_count_v1, portable_manifest_size_v1};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(super) fn validate_report(report: &ConversionBenchmarkReportV3) -> Result<()> {
    ensure!(
        report.schema_version == CONVERSION_BENCHMARK_SCHEMA_V3,
        "conversion benchmark schema {} is unsupported",
        report.schema_version
    );
    ensure!(
        !report.repetitions.is_empty(),
        "conversion benchmark report has no repetitions"
    );
    validate_catalog_authority(&report.authority.profile, "profile")?;
    validate_catalog_authority(&report.authority.source, "source")?;
    validate_process_usage(&report.setup.prepare, "setup prepare")?;
    validate_catalog_setup(&report.setup.profile, &report.authority.profile, "profile")?;
    validate_catalog_setup(&report.setup.source, &report.authority.source, "source")?;
    validate_process_usage(&report.setup.finalize, "setup finalize")?;
    validate_sha256(&report.subject.package_key_sha256, "package key SHA-256")?;
    validate_sha256(
        &report.subject.source_artifact_sha256,
        "source artifact SHA-256",
    )?;
    ensure!(
        report.authority.source_profile
            == conary_core::repository::supported_profiles::profile_by_id(
                &report.authority.source_profile,
            )
            .map(|profile| profile.id())
            .unwrap_or_default(),
        "benchmark report names an unknown source profile"
    );

    let mut roles = BTreeSet::new();
    for root in &report.environment.roots {
        ensure!(
            roles.insert(root.role.as_str()),
            "repeated root role '{}'",
            root.role
        );
        ensure!(!root.path.is_empty(), "root '{}' has no path", root.role);
    }

    let mut cold_output = None;
    for (index, repetition) in report.repetitions.iter().enumerate() {
        ensure!(
            repetition.iteration == index + 1,
            "conversion benchmark repetitions are not sequential"
        );
        validate_process_usage(
            &repetition.process,
            &format!("repetition {}", repetition.iteration),
        )?;
        match &repetition.outcome {
            ConversionBenchmarkOutcome::Success {
                cache_state,
                timing,
                output,
            } => {
                let expected_cache = if cold_output.is_some() { "hot" } else { "cold" };
                validate_successful_repetition(
                    report,
                    repetition,
                    cache_state,
                    timing,
                    output,
                    expected_cache,
                    cold_output.as_ref(),
                )?;
                if cold_output.is_none() {
                    cold_output = Some(output.clone());
                }
            }
            ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
                cache_state,
                timing,
                error,
            } => {
                validate_terminal_failure(index, report.repetitions.len(), repetition, error)?;
                let expected_cache = if cold_output.is_some() { "hot" } else { "cold" };
                validate_completed_conversion(
                    report,
                    repetition,
                    cache_state,
                    timing,
                    expected_cache,
                )?;
            }
            ConversionBenchmarkOutcome::Failure { error } => {
                validate_terminal_failure(index, report.repetitions.len(), repetition, error)?;
                ensure!(
                    !repetition.views.conversion_core.executed
                        && repetition.views.conversion_core.duration_ms == 0
                        && !repetition.views.end_to_end.executed
                        && repetition.views.end_to_end.duration_ms == 0,
                    "failed benchmark iteration {} claims an executed timing view",
                    repetition.iteration
                );
            }
        }
    }
    Ok(())
}

fn validate_terminal_failure(
    index: usize,
    repetition_count: usize,
    repetition: &ConversionBenchmarkEvidence,
    error: &str,
) -> Result<()> {
    ensure!(
        index + 1 == repetition_count,
        "failed benchmark iteration {} is not terminal",
        repetition.iteration
    );
    ensure!(
        !error.trim().is_empty(),
        "benchmark iteration {} has an empty failure",
        repetition.iteration
    );
    Ok(())
}

fn validate_completed_conversion(
    report: &ConversionBenchmarkReportV3,
    repetition: &ConversionBenchmarkEvidence,
    cache_state: &str,
    timing: &crate::server::conversion_timing::ConversionTimingReport,
    expected_cache: &str,
) -> Result<()> {
    ensure!(
        cache_state == expected_cache,
        "benchmark iteration {} is '{}'; expected '{}'",
        repetition.iteration,
        cache_state,
        expected_cache
    );
    ensure!(
        timing.success,
        "completed benchmark iteration {} carries failed timing evidence",
        repetition.iteration
    );
    let profile = conary_core::repository::supported_profiles::profile_by_id(
        &report.authority.source_profile,
    )
    .context("validated benchmark source profile disappeared")?;
    ensure!(
        timing.distro == profile.remi_route_slug()
            && timing.package == report.subject.name
            && timing.version.as_deref() == Some(report.subject.version.as_str()),
        "completed benchmark timing route or package identity contradicts report subject"
    );
    let source = timing
        .source
        .as_ref()
        .context("completed benchmark timing omitted source identity")?;
    ensure!(
        source.source_profile == report.authority.source_profile
            && source.version == report.subject.version
            && source.architecture == report.subject.architecture
            && source.checksum == report.subject.repository_checksum
            && source.declared_size_bytes == report.subject.source_size_bytes,
        "completed benchmark timing contradicts report subject"
    );

    let core_duration = conversion_core_duration(timing)?;
    ensure!(
        repetition.views.end_to_end.executed
            && repetition.views.end_to_end.duration_ms == timing.total_ms,
        "benchmark iteration {} end-to-end view contradicts timing total",
        repetition.iteration
    );
    ensure!(
        repetition.views.conversion_core.duration_ms == core_duration
            && core_duration <= timing.total_ms,
        "benchmark iteration {} conversion-core view contradicts phase timings",
        repetition.iteration
    );

    if expected_cache == "cold" {
        ensure!(
            repetition.views.conversion_core.executed,
            "cold benchmark did not execute conversion core"
        );
        let work = &timing.work;
        ensure!(
            work.admitted_local_bytes == report.subject.source_size_bytes
                && work.repository_checksum_bytes_hashed == report.subject.source_size_bytes
                && work.source_artifact_bytes == report.subject.source_size_bytes
                && work.source_bytes_hashed == report.subject.source_size_bytes,
            "cold benchmark source-artifact work contradicts the exact subject size"
        );
    } else {
        ensure!(
            !repetition.views.conversion_core.executed && core_duration == 0,
            "hot benchmark executed conversion core"
        );
        ensure!(
            timing.work == crate::server::conversion_timing::ConversionWorkMetrics::default(),
            "hot benchmark recorded conversion or persistence work: {:#?}",
            timing.work
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_successful_repetition(
    report: &ConversionBenchmarkReportV3,
    repetition: &ConversionBenchmarkEvidence,
    cache_state: &str,
    timing: &crate::server::conversion_timing::ConversionTimingReport,
    output: &ConversionBenchmarkOutputProof,
    expected_cache: &str,
    cold_output: Option<&ConversionBenchmarkOutputProof>,
) -> Result<()> {
    validate_completed_conversion(report, repetition, cache_state, timing, expected_cache)?;

    validate_sha256(&output.ccs_sha256, "output CCS SHA-256")?;
    validate_sha256(&output.transport_sha256, "transport SHA-256")?;
    validate_sha256(
        &output.signed_object_set_sha256,
        "signed object set SHA-256",
    )?;
    ensure!(output.ccs_size_bytes > 0, "benchmark output CCS is empty");
    ensure!(
        output.independent_transport_reopen_bytes == output.ccs_size_bytes
            && output.independent_complete_archive_hash_bytes == output.ccs_size_bytes,
        "independent output reopen/hash bytes differ from the exact CCS size"
    );

    if expected_cache == "cold" {
        let work = &timing.work;
        ensure!(
            work.ccs_output_bytes == output.ccs_size_bytes
                && work.immediate_converter_reopen_ccs_bytes == output.ccs_size_bytes
                && work.independent_transport_reopen_ccs_bytes == output.ccs_size_bytes
                && work.complete_archive_hash_bytes == output.ccs_size_bytes
                && work.complete_archive_copy_bytes == output.ccs_size_bytes,
            "cold benchmark conversion work contradicts the exact output CCS size"
        );
        ensure!(
            work.signed_object_count == output.signed_object_count
                && work.signed_object_bytes == output.signed_object_bytes
                && work.immediate_converter_reopen_object_bytes_hashed
                    == output.signed_object_bytes
                && work.independent_transport_reopen_object_bytes_hashed
                    == output.signed_object_bytes,
            "cold benchmark conversion work contradicts the signed object set"
        );
    }

    if let Some(cold) = cold_output {
        ensure!(
            output.ccs_sha256 == cold.ccs_sha256
                && output.ccs_size_bytes == cold.ccs_size_bytes
                && output.transport_sha256 == cold.transport_sha256
                && output.signed_object_set_sha256 == cold.signed_object_set_sha256
                && output.signed_object_count == cold.signed_object_count
                && output.signed_object_bytes == cold.signed_object_bytes,
            "hot benchmark output identity or byte geometry differs from the cold output"
        );
    }
    Ok(())
}

fn validate_catalog_setup(
    setup: &ConversionBenchmarkCatalogSetup,
    authority: &ConversionBenchmarkCatalogAuthority,
    label: &str,
) -> Result<()> {
    validate_catalog_reopen(&setup.reopen, authority, label)?;
    validate_process_usage(&setup.query.process, &format!("{label} authority query"))?;
    validate_vfs_metrics(&setup.query.vfs, label, false)
}

fn validate_catalog_authority(
    authority: &ConversionBenchmarkCatalogAuthority,
    label: &str,
) -> Result<()> {
    validate_sha256(
        &authority.resource_sha256,
        &format!("{label} resource SHA-256"),
    )?;
    validate_sha256(
        &authority.artifact_sha256,
        &format!("{label} catalog artifact SHA-256"),
    )?;
    validate_sha256(
        &authority.logical_digest_sha256,
        &format!("{label} catalog logical digest"),
    )?;
    validate_sha256(
        &authority.portable_manifest_sha256,
        &format!("{label} portable manifest SHA-256"),
    )?;
    ensure!(
        authority.portable_chunk_size == PORTABLE_CHUNK_SIZE_V1,
        "{label} portable chunk size {} differs from schema-v1 size {PORTABLE_CHUNK_SIZE_V1}",
        authority.portable_chunk_size
    );
    let chunk_count = portable_chunk_count_v1(authority.artifact_bytes)
        .map_err(|error| anyhow!("derive {label} portable chunk count: {error}"))?;
    ensure!(
        authority.portable_chunk_count == chunk_count,
        "{label} portable chunk count differs from exact artifact geometry"
    );
    let manifest_bytes = portable_manifest_size_v1(chunk_count)
        .map_err(|error| anyhow!("derive {label} portable manifest size: {error}"))?;
    ensure!(
        authority.portable_manifest_bytes == manifest_bytes,
        "{label} portable manifest size differs from exact chunk geometry"
    );
    Ok(())
}

fn validate_catalog_reopen(
    reopen: &ConversionBenchmarkCatalogReopen,
    authority: &ConversionBenchmarkCatalogAuthority,
    label: &str,
) -> Result<()> {
    let verification = &reopen.verification;
    ensure!(
        verification.catalog_bytes == authority.artifact_bytes,
        "{label} reopen evidence names the wrong catalog size"
    );
    ensure!(
        verification.portable_manifest_validation_passes == 1
            && verification.portable_manifest_validation_bytes == authority.portable_manifest_bytes,
        "{label} reopen did not perform exactly one attested portable-manifest validation"
    );
    ensure!(
        verification.userspace_sha256_passes == 0 && verification.userspace_sha256_bytes == 0,
        "{label} registered reopen performed a complete userspace artifact hash"
    );
    ensure!(
        verification.sqlite_integrity_passes == 0
            && verification.sqlite_integrity_bytes_covered == 0,
        "{label} registered reopen performed a complete SQLite integrity scan"
    );
    ensure!(
        verification.logical_replay_passes == 0 && verification.logical_replay_wall_us == 0,
        "{label} registered reopen replayed logical catalog rows"
    );
    ensure!(
        verification.stored_binding_checks == 1,
        "{label} registered reopen did not perform one exact stored-binding check"
    );
    validate_process_usage(&reopen.process, &format!("{label} registered reopen"))?;
    validate_vfs_metrics(&reopen.vfs, label, true)
}

fn validate_vfs_metrics(
    vfs: &PortableVfsMetricsV1,
    label: &str,
    require_authentication: bool,
) -> Result<()> {
    ensure!(
        vfs.read_calls > 0
            && vfs.requested_bytes > 0
            && vfs.returned_bytes > 0
            && vfs.chunk_accesses > 0,
        "{label} VFS evidence recorded no demanded read work"
    );
    let accounted_accesses = vfs
        .cache_hits
        .checked_add(vfs.cache_misses)
        .context("portable VFS chunk-access counter overflow")?;
    ensure!(
        vfs.chunk_accesses == accounted_accesses,
        "{label} VFS chunk-access counters are inconsistent"
    );
    ensure!(
        vfs.cache_misses == vfs.authenticated_chunks,
        "{label} VFS authenticated-chunk count differs from cache misses"
    );
    ensure!(
        vfs.carrier_bytes_requested == vfs.authenticated_bytes,
        "{label} VFS carrier and authenticated byte counters disagree"
    );
    ensure!(
        vfs.returned_bytes <= vfs.requested_bytes && vfs.short_reads <= vfs.read_calls,
        "{label} VFS read/short-read counters are inconsistent"
    );
    if require_authentication {
        ensure!(
            vfs.authenticated_chunks > 0 && vfs.authenticated_bytes > 0,
            "{label} fresh registered reopen authenticated no carrier chunks"
        );
    }
    ensure!(
        vfs.integrity_failures == 0,
        "{label} VFS evidence recorded an integrity failure"
    );
    Ok(())
}

fn validate_process_usage(usage: &ConversionBenchmarkProcessUsage, label: &str) -> Result<()> {
    ensure!(
        usage.process_lifetime_peak_rss_bytes >= usage.rss_start_bytes
            && usage.process_lifetime_peak_rss_bytes >= usage.rss_end_bytes,
        "{label} process lifetime RSS peak is below an endpoint sample"
    );
    ensure!(
        usage.runnable_threads_start <= usage.thread_count_start
            && usage.runnable_threads_end <= usage.thread_count_end,
        "{label} runnable-thread evidence exceeds process thread count"
    );
    Ok(())
}

pub(super) fn publish_and_reopen_report(
    path: &Path,
    report: &ConversionBenchmarkReportV3,
) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(anyhow!(
                "benchmark report already exists: {}",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .context("benchmark report path has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspect benchmark report parent {}", parent.display()))?;
    ensure!(
        parent_metadata.file_type().is_dir() && !parent_metadata.file_type().is_symlink(),
        "benchmark report parent must be a plain directory"
    );

    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    let temporary =
        path.with_file_name(format!(".{REPORT_FILE_NAME}.{}.tmp", uuid::Uuid::new_v4()));
    let mut linked_inode = None;
    let publication = (|| -> Result<PublishedInode> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        output.set_permissions(fs::Permissions::from_mode(0o600))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        let staged_metadata = output.metadata()?;
        let staged_inode = PublishedInode::from_metadata(&staged_metadata);

        match fs::hard_link(&temporary, path) {
            Ok(()) => linked_inode = Some(staged_inode),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(anyhow!(
                    "benchmark report already exists: {}",
                    path.display()
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let published_metadata = fs::symlink_metadata(path)?;
        ensure!(
            published_metadata.file_type().is_file()
                && published_metadata.dev() == staged_metadata.dev()
                && published_metadata.ino() == staged_metadata.ino()
                && published_metadata.nlink() == 2
                && published_metadata.mode() & 0o7777 == 0o600,
            "published benchmark report is not the private staged file"
        );
        fs::remove_file(&temporary)?;
        let final_metadata = fs::symlink_metadata(path)?;
        ensure!(
            final_metadata.file_type().is_file()
                && final_metadata.dev() == staged_metadata.dev()
                && final_metadata.ino() == staged_metadata.ino()
                && final_metadata.nlink() == 1
                && final_metadata.mode() & 0o7777 == 0o600,
            "published benchmark report retained an unexpected link"
        );
        sync_parent(path)?;
        Ok(staged_inode)
    })();
    let published_inode = match publication {
        Ok(published_inode) => published_inode,
        Err(error) => {
            rollback_failed_publication(path, &temporary, linked_inode);
            return Err(error);
        }
    };

    let reopen = (|| -> Result<()> {
        let named_metadata = fs::symlink_metadata(path)?;
        let mut reopened_input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let opened_metadata = reopened_input.metadata()?;
        ensure!(
            named_metadata.file_type().is_file()
                && named_metadata.dev() == opened_metadata.dev()
                && named_metadata.ino() == opened_metadata.ino()
                && opened_metadata.nlink() == 1
                && opened_metadata.mode() & 0o7777 == 0o600,
            "published benchmark report changed before durable reopen"
        );
        let read_limit = u64::try_from(bytes.len())
            .context("published benchmark report size exceeds u64")?
            .saturating_add(1);
        let mut reopened_bytes = Vec::with_capacity(bytes.len());
        (&mut reopened_input)
            .take(read_limit)
            .read_to_end(&mut reopened_bytes)?;
        let current_metadata = fs::symlink_metadata(path)?;
        ensure!(
            current_metadata.file_type().is_file()
                && current_metadata.dev() == opened_metadata.dev()
                && current_metadata.ino() == opened_metadata.ino()
                && current_metadata.nlink() == 1
                && current_metadata.mode() & 0o7777 == 0o600,
            "published benchmark report changed during durable reopen"
        );
        ensure!(
            reopened_bytes == bytes,
            "reopened conversion benchmark report changed bytes"
        );
        let reopened: ConversionBenchmarkReportV3 = serde_json::from_slice(&reopened_bytes)
            .context("strictly reopen published conversion benchmark schema v3")?;
        validate_report(&reopened)?;
        ensure!(
            serde_json::to_value(&reopened)? == serde_json::to_value(report)?,
            "reopened conversion benchmark report changed value"
        );
        Ok(())
    })();
    if reopen.is_err() {
        rollback_failed_publication(path, &temporary, Some(published_inode));
    }
    reopen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::conversion::{
        ConversionBenchmarkAuthority, ConversionBenchmarkCatalogQuery,
        ConversionBenchmarkEnvironment, ConversionBenchmarkSelectionKind, ConversionBenchmarkSetup,
        ConversionBenchmarkSubject, ConversionBenchmarkView, ConversionBenchmarkViews,
    };
    use crate::server::conversion_timing::{
        ConversionPhase, ConversionSourceIdentity, ConversionTimingReport,
    };
    use conary_core::repository::catalog::CatalogVerificationEvidenceV1;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    fn valid_process_usage() -> ConversionBenchmarkProcessUsage {
        ConversionBenchmarkProcessUsage {
            rss_start_bytes: 1,
            rss_end_bytes: 1,
            process_lifetime_peak_rss_bytes: 1,
            thread_count_start: 1,
            thread_count_end: 1,
            ..ConversionBenchmarkProcessUsage::default()
        }
    }

    fn valid_catalog_authority() -> ConversionBenchmarkCatalogAuthority {
        ConversionBenchmarkCatalogAuthority {
            resource_sha256: "a".repeat(64),
            artifact_sha256: "b".repeat(64),
            artifact_bytes: 1,
            logical_digest_sha256: "c".repeat(64),
            portable_manifest_sha256: "d".repeat(64),
            portable_manifest_bytes: portable_manifest_size_v1(1).unwrap(),
            portable_chunk_size: PORTABLE_CHUNK_SIZE_V1,
            portable_chunk_count: 1,
        }
    }

    fn valid_reopen_vfs() -> PortableVfsMetricsV1 {
        PortableVfsMetricsV1 {
            read_calls: 1,
            requested_bytes: 1,
            returned_bytes: 1,
            chunk_accesses: 1,
            cache_misses: 1,
            carrier_bytes_requested: 1,
            authenticated_chunks: 1,
            authenticated_bytes: 1,
            ..PortableVfsMetricsV1::default()
        }
    }

    fn valid_query_vfs() -> PortableVfsMetricsV1 {
        PortableVfsMetricsV1 {
            read_calls: 1,
            requested_bytes: 1,
            returned_bytes: 1,
            chunk_accesses: 1,
            cache_hits: 1,
            ..PortableVfsMetricsV1::default()
        }
    }

    fn valid_catalog_setup() -> ConversionBenchmarkCatalogSetup {
        let authority = valid_catalog_authority();
        let verification = CatalogVerificationEvidenceV1 {
            catalog_bytes: authority.artifact_bytes,
            portable_manifest_validation_passes: 1,
            portable_manifest_validation_bytes: authority.portable_manifest_bytes,
            stored_binding_checks: 1,
            ..CatalogVerificationEvidenceV1::default()
        };
        ConversionBenchmarkCatalogSetup {
            reopen: ConversionBenchmarkCatalogReopen {
                process: valid_process_usage(),
                verification,
                vfs: valid_reopen_vfs(),
            },
            query: ConversionBenchmarkCatalogQuery {
                process: valid_process_usage(),
                vfs: valid_query_vfs(),
            },
        }
    }

    fn valid_output() -> ConversionBenchmarkOutputProof {
        ConversionBenchmarkOutputProof {
            ccs_sha256: "3".repeat(64),
            ccs_size_bytes: 23,
            transport_sha256: "4".repeat(64),
            signed_object_set_sha256: "5".repeat(64),
            signed_object_count: 2,
            signed_object_bytes: 17,
            independent_transport_reopen_ms: 1,
            independent_transport_reopen_bytes: 23,
            independent_complete_archive_hash_ms: 1,
            independent_complete_archive_hash_bytes: 23,
        }
    }

    fn valid_timing(cold: bool) -> ConversionTimingReport {
        let mut timing = ConversionTimingReport::new("fedora", "fixture", Some("1"));
        timing.source = Some(ConversionSourceIdentity {
            source_profile: "fedora-44".to_string(),
            version: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            checksum: "repository-checksum".to_string(),
            declared_size_bytes: 5,
        });
        timing.success = true;
        if cold {
            timing.record(
                ConversionPhase::NativeArchiveParseAndSpool,
                Duration::from_millis(7),
            );
            timing.work.admitted_local_bytes = 5;
            timing.work.repository_checksum_bytes_hashed = 5;
            timing.work.source_artifact_bytes = 5;
            timing.work.source_bytes_hashed = 5;
            timing.work.ccs_output_bytes = 23;
            timing.work.immediate_converter_reopen_ccs_bytes = 23;
            timing.work.independent_transport_reopen_ccs_bytes = 23;
            timing.work.complete_archive_hash_bytes = 23;
            timing.work.complete_archive_copy_bytes = 23;
            timing.work.signed_object_count = 2;
            timing.work.signed_object_bytes = 17;
            timing.work.immediate_converter_reopen_object_bytes_hashed = 17;
            timing.work.independent_transport_reopen_object_bytes_hashed = 17;
            timing.total_ms = 11;
        } else {
            timing.total_ms = 3;
        }
        timing
    }

    fn valid_report() -> ConversionBenchmarkReportV3 {
        let output = valid_output();
        ConversionBenchmarkReportV3 {
            schema_version: CONVERSION_BENCHMARK_SCHEMA_V3,
            environment: ConversionBenchmarkEnvironment {
                hardware_label: "fixture".to_string(),
                remi_version: "0".to_string(),
                source_commit: "fixture".to_string(),
                source_dirty: false,
                binary_path: "/fixture/remi".to_string(),
                binary_sha256: "6".repeat(64),
                os_release: "fixture".to_string(),
                kernel_release: "fixture".to_string(),
                cpu_model: "fixture".to_string(),
                logical_cpus: 1,
                memory_bytes: 1,
                roots: Vec::new(),
            },
            authority: ConversionBenchmarkAuthority {
                selection_kind: ConversionBenchmarkSelectionKind::Active,
                source_profile: "fedora-44".to_string(),
                profile: valid_catalog_authority(),
                source: valid_catalog_authority(),
                source_identity: "source".to_string(),
                repository_identity: "repository".to_string(),
                source_parser_config_sha256: "7".repeat(64),
                source_trust_policy_sha256: "8".repeat(64),
                authenticated_metadata_objects: 1,
            },
            setup: ConversionBenchmarkSetup {
                prepare: valid_process_usage(),
                profile: valid_catalog_setup(),
                source: valid_catalog_setup(),
                finalize: valid_process_usage(),
            },
            subject: ConversionBenchmarkSubject {
                package_key_sha256: "1".repeat(64),
                name: "fixture".to_string(),
                version: "1".to_string(),
                package_release: "1".to_string(),
                architecture: Some("x86_64".to_string()),
                repository_checksum: "repository-checksum".to_string(),
                source_size_bytes: 5,
                source_artifact_sha256: "2".repeat(64),
            },
            repetitions: vec![
                ConversionBenchmarkEvidence {
                    iteration: 1,
                    process: valid_process_usage(),
                    views: ConversionBenchmarkViews {
                        conversion_core: ConversionBenchmarkView {
                            executed: true,
                            duration_ms: 7,
                        },
                        end_to_end: ConversionBenchmarkView {
                            executed: true,
                            duration_ms: 11,
                        },
                    },
                    outcome: ConversionBenchmarkOutcome::Success {
                        cache_state: "cold".to_string(),
                        timing: Box::new(valid_timing(true)),
                        output: output.clone(),
                    },
                },
                ConversionBenchmarkEvidence {
                    iteration: 2,
                    process: valid_process_usage(),
                    views: ConversionBenchmarkViews {
                        conversion_core: ConversionBenchmarkView {
                            executed: false,
                            duration_ms: 0,
                        },
                        end_to_end: ConversionBenchmarkView {
                            executed: true,
                            duration_ms: 3,
                        },
                    },
                    outcome: ConversionBenchmarkOutcome::Success {
                        cache_state: "hot".to_string(),
                        timing: Box::new(valid_timing(false)),
                        output,
                    },
                },
            ],
        }
    }

    #[test]
    fn accepts_exact_registered_reopen_and_query_counters() {
        validate_catalog_setup(
            &valid_catalog_setup(),
            &valid_catalog_authority(),
            "fixture",
        )
        .unwrap();
    }

    #[test]
    fn accepts_fully_bound_cold_and_hot_repetition_evidence() {
        validate_report(&valid_report()).unwrap();
    }

    #[test]
    fn raw_report_publication_never_overwrites_existing_targets() {
        let root = tempfile::tempdir().unwrap();
        let regular_path = root.path().join("existing-raw.json");
        fs::write(&regular_path, b"existing raw evidence").unwrap();
        let error = publish_and_reopen_report(&regular_path, &valid_report())
            .expect_err("existing raw report must not be replaced");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&regular_path).unwrap(), b"existing raw evidence");

        let dangling_path = root.path().join("dangling-raw.json");
        symlink("missing-raw-target", &dangling_path).unwrap();
        let error = publish_and_reopen_report(&dangling_path, &valid_report())
            .expect_err("dangling raw target must not be replaced");
        assert!(error.to_string().contains("already exists"));
        assert!(
            fs::symlink_metadata(&dangling_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&dangling_path).unwrap(),
            Path::new("missing-raw-target")
        );
    }

    #[test]
    fn terminal_independent_reopen_failure_retains_validated_conversion_evidence() {
        let mut report = valid_report();
        let cold_success = report.repetitions[0].clone();
        let ConversionBenchmarkOutcome::Success {
            cache_state,
            timing,
            ..
        } = cold_success.outcome.clone()
        else {
            unreachable!()
        };
        report.repetitions[0].outcome =
            ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
                cache_state,
                timing,
                error: "independent benchmark output reopen failed: fixture corruption".to_string(),
            };
        report.repetitions[1] = cold_success;
        report.repetitions[1].iteration = 2;
        let error = validate_report(&report).expect_err("nonterminal failure must be rejected");
        assert_eq!(
            error.to_string(),
            "failed benchmark iteration 1 is not terminal"
        );

        report.repetitions.truncate(1);
        validate_report(&report).expect("terminal reopen failure is valid typed evidence");
        let mut tampered = report.clone();
        tampered.repetitions[0].views.end_to_end.duration_ms += 1;
        assert!(
            validate_report(&tampered).is_err(),
            "retained conversion views must remain bound to timing evidence"
        );

        let root = tempfile::tempdir().expect("create report publication root");
        let path = root.path().join(REPORT_FILE_NAME);
        publish_and_reopen_report(&path, &report)
            .expect("publish and strictly reopen terminal reopen-failure report");
        assert!(path.is_file());
    }

    #[test]
    fn terminal_conversion_failure_requires_unexecuted_views() {
        let mut report = valid_report();
        report.repetitions.truncate(1);
        report.repetitions[0].views = ConversionBenchmarkViews {
            conversion_core: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
            end_to_end: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
        };
        report.repetitions[0].outcome = ConversionBenchmarkOutcome::Failure {
            error: "fixture conversion failure".to_string(),
        };
        validate_report(&report).expect("terminal conversion failure is valid typed evidence");
        report.repetitions[0].views.end_to_end.executed = true;
        assert!(validate_report(&report).is_err());
    }

    #[test]
    fn terminal_hot_reopen_failure_requires_exact_completed_conversion_evidence() {
        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success {
            cache_state,
            timing,
            ..
        } = report.repetitions[1].outcome.clone()
        else {
            unreachable!()
        };
        report.repetitions[1].outcome =
            ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
                cache_state,
                timing,
                error: "independent benchmark output reopen failed: fixture corruption".to_string(),
            };
        validate_report(&report).expect("terminal hot reopen failure retains exact evidence");

        let mut wrong_cache = report.clone();
        let ConversionBenchmarkOutcome::IndependentOutputReopenFailure { cache_state, .. } =
            &mut wrong_cache.repetitions[1].outcome
        else {
            unreachable!()
        };
        *cache_state = "cold".to_string();
        assert!(validate_report(&wrong_cache).is_err());

        let mut failed_timing = report;
        let ConversionBenchmarkOutcome::IndependentOutputReopenFailure { timing, .. } =
            &mut failed_timing.repetitions[1].outcome
        else {
            unreachable!()
        };
        timing.success = false;
        assert!(validate_report(&failed_timing).is_err());
    }

    #[test]
    fn rejects_views_or_output_bytes_that_contradict_measured_evidence() {
        let mut report = valid_report();
        report.repetitions[0].views.end_to_end.duration_ms += 1;
        assert!(validate_report(&report).is_err());

        let mut report = valid_report();
        report.repetitions[0].views.conversion_core.duration_ms += 1;
        assert!(validate_report(&report).is_err());

        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success { output, .. } = &mut report.repetitions[0].outcome
        else {
            unreachable!()
        };
        output.independent_complete_archive_hash_bytes -= 1;
        assert!(validate_report(&report).is_err());
    }

    #[test]
    fn rejects_hot_output_or_cold_work_that_changes_exact_ccs_identity() {
        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success { output, .. } = &mut report.repetitions[1].outcome
        else {
            unreachable!()
        };
        output.ccs_sha256 = "9".repeat(64);
        assert!(validate_report(&report).is_err());

        let mut report = valid_report();
        let ConversionBenchmarkOutcome::Success { timing, .. } = &mut report.repetitions[0].outcome
        else {
            unreachable!()
        };
        timing.work.complete_archive_hash_bytes -= 1;
        assert!(validate_report(&report).is_err());
    }

    #[test]
    fn rejects_full_scan_registered_reopen_counters() {
        let mut setup = valid_catalog_setup();
        setup.reopen.verification.userspace_sha256_passes = 1;
        setup.reopen.verification.userspace_sha256_bytes = 1;
        assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

        let mut setup = valid_catalog_setup();
        setup.reopen.verification.sqlite_integrity_passes = 1;
        setup.reopen.verification.sqlite_integrity_bytes_covered = 1;
        assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

        let mut setup = valid_catalog_setup();
        setup.reopen.verification.logical_replay_passes = 1;
        assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());
    }

    #[test]
    fn rejects_wrong_proof_geometry_and_vfs_counters() {
        let mut authority = valid_catalog_authority();
        authority.portable_chunk_count = 2;
        assert!(validate_catalog_authority(&authority, "fixture").is_err());

        let mut setup = valid_catalog_setup();
        setup.reopen.vfs.cache_misses = 0;
        assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());

        let mut setup = valid_catalog_setup();
        setup.query.vfs.integrity_failures = 1;
        assert!(validate_catalog_setup(&setup, &valid_catalog_authority(), "fixture").is_err());
    }

    #[test]
    fn strict_schema_rejects_unknown_top_level_fields() {
        let process = serde_json::to_value(ConversionBenchmarkProcessUsage::default()).unwrap();
        let verification = serde_json::to_value(CatalogVerificationEvidenceV1::default()).unwrap();
        let vfs = serde_json::to_value(PortableVfsMetricsV1::default()).unwrap();
        let reopen = serde_json::json!({
            "process": process,
            "verification": verification,
            "vfs": vfs
        });
        let catalog = serde_json::json!({
            "resource_sha256": "a".repeat(64),
            "artifact_sha256": "b".repeat(64),
            "artifact_bytes": 1,
            "logical_digest_sha256": "c".repeat(64),
            "portable_manifest_sha256": "d".repeat(64),
            "portable_manifest_bytes": 1,
            "portable_chunk_size": PORTABLE_CHUNK_SIZE_V1,
            "portable_chunk_count": 1
        });
        let mut value = serde_json::json!({
            "schema_version": CONVERSION_BENCHMARK_SCHEMA_V3,
            "environment": {
                "hardware_label": "fixture",
                "remi_version": "0",
                "source_commit": "fixture",
                "source_dirty": false,
                "binary_path": "/fixture/remi",
                "binary_sha256": "a".repeat(64),
                "os_release": "fixture",
                "kernel_release": "fixture",
                "cpu_model": "fixture",
                "logical_cpus": 1,
                "memory_bytes": 1,
                "roots": []
            },
            "authority": {
                "selection_kind": "active",
                "source_profile": "fedora-44",
                "profile": catalog.clone(),
                "source": catalog,
                "source_identity": "fixture",
                "repository_identity": "fixture",
                "source_parser_config_sha256": "e".repeat(64),
                "source_trust_policy_sha256": "f".repeat(64),
                "authenticated_metadata_objects": 1
            },
            "subject": {
                "package_key_sha256": "1".repeat(64),
                "name": "fixture",
                "version": "1",
                "package_release": "1",
                "architecture": "x86_64",
                "repository_checksum": "sha256:fixture",
                "source_size_bytes": 1,
                "source_artifact_sha256": "2".repeat(64)
            },
            "setup": {
                "prepare": process.clone(),
                "profile": {
                    "reopen": reopen.clone(),
                    "query": {
                        "process": process.clone(),
                        "vfs": vfs.clone()
                    }
                },
                "source": {
                    "reopen": reopen,
                    "query": {
                        "process": process.clone(),
                        "vfs": vfs
                    }
                },
                "finalize": process
            },
            "repetitions": []
        });
        value
            .as_object_mut()
            .unwrap()
            .insert("legacy_v2_field".to_string(), serde_json::Value::Bool(true));
        let error = serde_json::from_value::<ConversionBenchmarkReportV3>(value).unwrap_err();
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}
