// apps/remi/src/server/conversion/benchmark/report.rs
//! Strict schema-v4 validation and durable report publication.

use super::{
    CONVERSION_BENCHMARK_SCHEMA_V4, ConversionBenchmarkCatalogAuthority,
    ConversionBenchmarkCatalogReopen, ConversionBenchmarkCatalogSetup, ConversionBenchmarkEvidence,
    ConversionBenchmarkOutcome, ConversionBenchmarkOutputProof, ConversionBenchmarkProcessUsage,
    ConversionBenchmarkReportV4, PORTABLE_CHUNK_SIZE_V1, PortableVfsMetricsV1, PublishedInode,
    REPORT_FILE_NAME, conversion_core_duration, rollback_failed_publication, sync_parent,
    validate_sha256,
};
use crate::server::conversion_timing::{ConversionPhase, DURABLE_CAS_FUSED_SKIP_REASON};
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::repository::catalog::{portable_chunk_count_v1, portable_manifest_size_v1};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const HOT_EXECUTED_PHASES: [ConversionPhase; 2] =
    [ConversionPhase::PackageLookup, ConversionPhase::CacheLookup];

const HOT_SKIPPED_PHASES: [ConversionPhase; 18] = [
    ConversionPhase::LocalArtifactAdmission,
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
    ConversionPhase::NativeProvenanceProjection,
    ConversionPhase::CompleteArchiveCopy,
    ConversionPhase::IndependentTransportReopen,
    ConversionPhase::CompleteArchiveHash,
    ConversionPhase::DurableCasIngestion,
    ConversionPhase::R2WriteThrough,
    ConversionPhase::DatabasePersistence,
];

pub(super) fn validate_report(report: &ConversionBenchmarkReportV4) -> Result<()> {
    ensure!(
        report.schema_version == CONVERSION_BENCHMARK_SCHEMA_V4,
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
    report: &ConversionBenchmarkReportV4,
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
        validate_cold_fused_cas(timing)?;
    } else {
        ensure!(
            !repetition.views.conversion_core.executed && core_duration == 0,
            "hot benchmark executed conversion core"
        );
        ensure!(
            timing
                .phases
                .iter()
                .map(|phase| phase.phase)
                .eq(HOT_EXECUTED_PHASES),
            "hot benchmark executed phases differ from the exact cache-hit path"
        );
        ensure!(
            timing.nested_phases.is_empty(),
            "hot benchmark recorded nested conversion work"
        );
        ensure!(
            timing
                .skipped_phases
                .iter()
                .map(|phase| phase.phase)
                .eq(HOT_SKIPPED_PHASES),
            "hot benchmark skipped phases differ from the exact cache-hit path"
        );
        ensure!(
            timing.work == crate::server::conversion_timing::ConversionWorkMetrics::default(),
            "hot benchmark recorded conversion or persistence work: {:#?}",
            timing.work
        );
    }
    Ok(())
}

fn validate_cold_fused_cas(
    timing: &crate::server::conversion_timing::ConversionTimingReport,
) -> Result<()> {
    let work = &timing.work;
    let expected_cas_barriers = u64::from(work.signed_object_count > 0);
    ensure!(
        work.independent_transport_reopen_object_bytes_hashed == work.signed_object_bytes
            && work.cas_incoming_bytes_hashed == work.signed_object_bytes
            && work.cas_persistent_bytes_written == work.signed_object_bytes
            && work.cas_objects_hashed == work.signed_object_count
            && work.cas_hits == 0
            && work.cas_misses == work.signed_object_count
            && work.cas_race_losers == 0
            && work.cas_staged_data_barriers == expected_cas_barriers
            && work.cas_canonical_name_barriers == expected_cas_barriers
            && work.cas_canonical_bytes_reread == 0,
        "cold benchmark did not stream its exact signed object set once into the empty durable CAS"
    );
    let mut fused_phases = timing
        .phases
        .iter()
        .filter(|phase| phase.phase == ConversionPhase::IndependentTransportReopen);
    let fused_phase = fused_phases
        .next()
        .context("cold benchmark omitted fused independent reopen into durable CAS")?;
    ensure!(
        fused_phases.next().is_none()
            && !timing
                .skipped_phases
                .iter()
                .any(|phase| phase.phase == ConversionPhase::IndependentTransportReopen)
            && !timing
                .phases
                .iter()
                .any(|phase| phase.phase == ConversionPhase::DurableCasIngestion)
            && timing
                .skipped_phases
                .iter()
                .filter(|phase| phase.phase == ConversionPhase::DurableCasIngestion)
                .all(|phase| phase.reason == DURABLE_CAS_FUSED_SKIP_REASON)
            && timing
                .skipped_phases
                .iter()
                .filter(|phase| phase.phase == ConversionPhase::DurableCasIngestion)
                .count()
                == 1,
        "cold benchmark did not record one fused independent reopen into durable CAS"
    );
    ensure!(
        fused_phase.duration_ms <= timing.total_ms,
        "cold benchmark fused independent reopen duration exceeds timing total"
    );
    let archive_pipeline = [
        ConversionPhase::CompleteArchiveCopy,
        ConversionPhase::IndependentTransportReopen,
        ConversionPhase::CompleteArchiveHash,
    ];
    ensure!(
        timing
            .phases
            .iter()
            .filter(|phase| archive_pipeline.contains(&phase.phase))
            .map(|phase| phase.phase)
            .eq(archive_pipeline)
            && !timing
                .skipped_phases
                .iter()
                .any(|phase| archive_pipeline.contains(&phase.phase)),
        "cold benchmark archive copy, fused reopen, and canonical hash phases are not one ordered executed pipeline"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_successful_repetition(
    report: &ConversionBenchmarkReportV4,
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
                && work.ccs_output_bytes_hashed == output.ccs_size_bytes
                && work.independent_transport_reopen_ccs_bytes == output.ccs_size_bytes
                && work.complete_archive_hash_bytes == output.ccs_size_bytes
                && work.complete_archive_copy_bytes == output.ccs_size_bytes,
            "cold benchmark conversion work contradicts the exact output CCS size"
        );
        ensure!(
            work.signed_object_count == output.signed_object_count
                && work.signed_object_bytes == output.signed_object_bytes
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
    report: &ConversionBenchmarkReportV4,
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
        let reopened: ConversionBenchmarkReportV4 = serde_json::from_slice(&reopened_bytes)
            .context("strictly reopen published conversion benchmark schema v4")?;
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
mod tests;
