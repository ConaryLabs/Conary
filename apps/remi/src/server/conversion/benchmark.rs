// apps/remi/src/server/conversion/benchmark.rs
//! Offline immutable-authority conversion benchmark operator.

mod process;
mod public_projection;
mod report;

use super::{
    CONVERSION_BENCHMARK_SCHEMA_V8, ConversionBenchmarkAuthority,
    ConversionBenchmarkCatalogAuthority, ConversionBenchmarkCatalogQuery,
    ConversionBenchmarkCatalogReopen, ConversionBenchmarkCatalogSetup, ConversionBenchmarkConfig,
    ConversionBenchmarkEnvironment, ConversionBenchmarkEvidence, ConversionBenchmarkOutcome,
    ConversionBenchmarkOutputProof, ConversionBenchmarkProcessUsage, ConversionBenchmarkReportV8,
    ConversionBenchmarkRootIdentity, ConversionBenchmarkSelectionKind, ConversionBenchmarkSetup,
    ConversionBenchmarkSubject, ConversionBenchmarkView, ConversionBenchmarkViews,
    ConversionService,
};
use crate::server::catalog_authority::{CatalogAuthority, ProfileRevisionSelection};
use crate::server::config::RemiConfig;
use crate::server::database_writer::DatabaseWriter;
use crate::server::profile_catalog::ProfileCatalog;
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, anyhow, bail, ensure};
use conary_core::db::models::{RemiCatalogPhysicalAttestation, RemiRuntimeSession};
use conary_core::repository::catalog::{
    CatalogPackageRecordV1, CatalogReader, PORTABLE_CHUNK_SIZE_V1, PortableVfsMetricsV1,
    ProfileRevisionV2, SourceSnapshotV1,
};
use process::ProcessUsageProbe;
use public_projection::{PUBLIC_REPORT_FILE_NAME, publish_and_reopen_public_report};
use report::{publish_and_reopen_report, validate_report};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const REPORT_FILE_NAME: &str = "conversion-benchmark-v8.json";

/// Run one network-independent conversion benchmark against a coherent copy
/// of the deployed operational database and the deployed immutable catalogs.
/// All benchmark mutation remains below the newly-created work root.
pub async fn run_conversion_benchmark_from_config(
    remi_config: &RemiConfig,
    config: ConversionBenchmarkConfig,
) -> Result<ConversionBenchmarkReportV8> {
    let prepare_probe = ProcessUsageProbe::start()?;
    validate_request(&config)?;
    remi_config.validate()?;
    let server = remi_config.to_server_config()?;
    let runtime_lock = acquire_benchmark_runtime_storage(remi_config, &server)?;
    let repository_keys_dir = server
        .release_publish
        .repository_keys_dir
        .clone()
        .context("release_publish.repository_keys_dir is required for conversion benchmark")?;

    let live_root = canonical_existing(remi_config.storage_root(), "live Remi storage root")?;
    let work_root = create_isolated_work_root(&config.work_root, &live_root)?;
    let expected_output = work_root.join(REPORT_FILE_NAME);
    ensure!(
        absolute_path(&config.output_path)? == expected_output,
        "conversion benchmark output must be the work-root-owned path {}",
        expected_output.display()
    );

    let metadata_dir = work_root.join("metadata");
    let chunk_dir = work_root.join("chunks");
    let cache_dir = work_root.join("cache");
    let source_dir = work_root.join("source");
    for directory in [&metadata_dir, &chunk_dir, &cache_dir, &source_dir] {
        fs::create_dir(directory)
            .with_context(|| format!("create benchmark directory {}", directory.display()))?;
    }

    let benchmark_db = metadata_dir.join("conary.db");
    snapshot_operational_database(&server.db_path, &benchmark_db)?;
    reset_benchmark_runtime_state(&benchmark_db)?;

    let staged_source = source_dir.join("artifact.native");
    stage_source_artifact(&config.source_artifact, &staged_source)?;

    let catalog_authority = CatalogAuthority::from_paths(
        benchmark_db.clone(),
        server.catalog_dir.clone(),
        DatabaseWriter::default(),
    );
    let (selection_kind, selection) = resolve_selection(
        &catalog_authority,
        &config.source_profile,
        config.profile_revision_sha256.as_deref(),
    )?;
    let prepare_process = prepare_probe.finish()?;
    let resolved = resolve_subject(&catalog_authority, &selection, &config.package_key_sha256)?;
    let finalize_probe = ProcessUsageProbe::start()?;
    admit_staged_subject(&staged_source, &resolved.package)?;
    let source_artifact_sha256 = sha256_file(&staged_source)?;

    let authority = benchmark_authority(
        selection_kind,
        &selection,
        &resolved.profile_manifest,
        &resolved.profile_physical_attestation,
        &resolved.source_snapshot,
        &resolved.source_physical_attestation,
    )?;
    let subject = benchmark_subject(&resolved.package, source_artifact_sha256);

    let binary_path = std::env::current_exe()
        .context("resolve running Remi benchmark executable")?
        .canonicalize()
        .context("canonicalize running Remi benchmark executable")?;
    let binary_sha256 = sha256_file(&binary_path)?;
    let environment = ConversionBenchmarkEnvironment::capture(
        config.hardware_label,
        &binary_path,
        binary_sha256,
        environment_roots(&[
            ("source_config", &config.source_config_path),
            ("source_database", &server.db_path),
            ("source_catalogs", &server.catalog_dir),
            ("repository_keys", &repository_keys_dir),
            ("operator_source_artifact", &config.source_artifact),
            ("work_root", &work_root),
            ("benchmark_database", &benchmark_db),
            ("benchmark_chunks", &chunk_dir),
            ("benchmark_cache", &cache_dir),
            ("staged_source_artifact", &staged_source),
        ])?,
    );

    let archive_cpu = conary_core::ccs::CcsArchiveCpuAdmission::for_current_process();
    let service = ConversionService::new(chunk_dir, cache_dir, benchmark_db, None)
        .with_catalog_authority(catalog_authority)
        .with_archive_cpu_admission(archive_cpu)
        .with_repository_keys_dir(Some(repository_keys_dir.clone()));
    let signing_key = Arc::new(load_role_key(
        &repository_keys_dir,
        &selection.source_profile,
        RepositorySigningRole::Targets,
    )?);
    let finalize_process = finalize_probe.finish()?;
    let setup = ConversionBenchmarkSetup {
        prepare: prepare_process,
        profile: resolved.profile_setup.clone(),
        source: resolved.source_setup.clone(),
        finalize: finalize_process,
    };

    let mut repetitions = Vec::with_capacity(config.iterations);
    for iteration in 1..=config.iterations {
        let evidence = run_iteration(
            &service,
            &selection,
            &resolved.package,
            &staged_source,
            Arc::clone(&signing_key),
            iteration,
        )
        .await?;
        let failed = !matches!(
            &evidence.outcome,
            ConversionBenchmarkOutcome::Success { .. }
        );
        repetitions.push(evidence);
        if failed {
            break;
        }
    }

    let report = ConversionBenchmarkReportV8 {
        schema_version: CONVERSION_BENCHMARK_SCHEMA_V8,
        environment,
        authority,
        setup,
        subject,
        repetitions,
    };
    validate_report(&report)?;
    publish_and_reopen_report(&expected_output, &report)?;
    if report.repetitions.iter().all(|repetition| {
        matches!(
            &repetition.outcome,
            ConversionBenchmarkOutcome::Success { .. }
        )
    }) {
        publish_and_reopen_public_report(
            &expected_output,
            &work_root.join(PUBLIC_REPORT_FILE_NAME),
        )?;
    }
    drop(runtime_lock);
    Ok(report)
}

fn acquire_benchmark_runtime_storage(
    remi_config: &RemiConfig,
    server: &crate::server::ServerConfig,
) -> Result<crate::server::runtime_lock::RuntimeRootLock> {
    crate::server::acquire_existing_runtime_storage(remi_config, server).context(
        "acquire exclusive Remi runtime-root authority for benchmark; the service must be stopped",
    )
}

fn validate_request(config: &ConversionBenchmarkConfig) -> Result<()> {
    ensure!(config.iterations > 0, "--iterations must be at least 1");
    ensure!(config.iterations <= 100, "--iterations must not exceed 100");
    ensure!(
        !config.hardware_label.trim().is_empty()
            && config.hardware_label.trim() == config.hardware_label,
        "--hardware-label must be a nonempty canonical label"
    );
    validate_sha256(&config.package_key_sha256, "--package-key")?;
    if let Some(revision) = &config.profile_revision_sha256 {
        validate_sha256(revision, "--revision")?;
    }
    let profile =
        conary_core::repository::supported_profiles::profile_by_id(&config.source_profile)
            .ok_or_else(|| anyhow!("unknown exact source profile '{}'", config.source_profile))?;
    ensure!(
        profile.id() == config.source_profile,
        "--profile must be an exact canonical profile ID"
    );
    Ok(())
}

fn create_isolated_work_root(requested: &Path, live_root: &Path) -> Result<PathBuf> {
    let work_root = absolute_path(requested)?;
    let live_root = canonical_existing(live_root, "live Remi storage root")?;
    ensure!(
        !work_root.starts_with(&live_root) && !live_root.starts_with(&work_root),
        "benchmark work root {} overlaps live Remi storage root {}",
        work_root.display(),
        live_root.display()
    );
    fs::create_dir(&work_root).with_context(|| {
        format!(
            "create new benchmark work root {}; it must not already exist",
            work_root.display()
        )
    })?;
    work_root
        .canonicalize()
        .context("canonicalize benchmark work root")
}

fn snapshot_operational_database(source: &Path, destination: &Path) -> Result<()> {
    let source = canonical_regular_file(source, "live Remi database")?;
    ensure!(!destination.exists(), "benchmark database already exists");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(&source, flags)
        .with_context(|| format!("open live Remi database {} read-only", source.display()))?;
    connection
        .backup(rusqlite::MAIN_DB, destination, None)
        .with_context(|| {
            format!(
                "snapshot live Remi database {} into isolated benchmark database {}",
                source.display(),
                destination.display()
            )
        })?;
    File::open(destination)?.sync_all()?;
    sync_parent(destination)
}

fn reset_benchmark_runtime_state(database: &Path) -> Result<()> {
    let mut connection = crate::server::open_runtime_db(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM remi_profile_revision_pins WHERE owner_kind IN ('reader', 'conversion')",
        [],
    )?;
    transaction.execute(
        "DELETE FROM converted_packages WHERE artifact_kind = 'repository'",
        [],
    )?;
    transaction.execute("DELETE FROM chunk_access", [])?;
    transaction.commit()?;
    RemiRuntimeSession::begin(&connection, unix_seconds()?)?;
    Ok(())
}

fn stage_source_artifact(source: &Path, destination: &Path) -> Result<()> {
    let source = canonical_regular_file(source, "benchmark source artifact")?;
    let mut input = File::open(&source)
        .with_context(|| format!("open benchmark source artifact {}", source.display()))?;
    let opened = input.metadata()?;
    let inspected = fs::symlink_metadata(&source)?;
    ensure!(
        opened.dev() == inspected.dev() && opened.ino() == inspected.ino(),
        "benchmark source artifact changed while it was opened"
    );
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("create staged source artifact {}", destination.display()))?;
    let copied = std::io::copy(&mut input, &mut output)?;
    ensure!(
        copied == opened.len(),
        "benchmark source artifact changed size while being staged"
    );
    output.sync_all()?;
    drop(output);
    sync_parent(destination)
}

fn resolve_selection(
    authority: &CatalogAuthority,
    source_profile: &str,
    revision: Option<&str>,
) -> Result<(ConversionBenchmarkSelectionKind, ProfileRevisionSelection)> {
    match revision {
        Some(profile_revision_sha256) => Ok((
            ConversionBenchmarkSelectionKind::Pinned,
            ProfileRevisionSelection {
                source_profile: source_profile.to_string(),
                profile_revision_sha256: profile_revision_sha256.to_string(),
            },
        )),
        None => Ok((
            ConversionBenchmarkSelectionKind::Active,
            authority.active_profile_selection(source_profile)?,
        )),
    }
}

struct ResolvedBenchmarkSubject {
    profile_manifest: ProfileRevisionV2,
    profile_physical_attestation: RemiCatalogPhysicalAttestation,
    source_snapshot: SourceSnapshotV1,
    source_physical_attestation: RemiCatalogPhysicalAttestation,
    package: CatalogPackageRecordV1,
    profile_setup: ConversionBenchmarkCatalogSetup,
    source_setup: ConversionBenchmarkCatalogSetup,
}

fn resolve_subject(
    authority: &CatalogAuthority,
    selection: &ProfileRevisionSelection,
    package_key_sha256: &str,
) -> Result<ResolvedBenchmarkSubject> {
    let profile_reopen_probe = ProcessUsageProbe::start()?;
    let pinned = authority.open_selected_profile(selection)?;
    let (profile_verification, profile_reopen_vfs) = {
        let reader = pinned.reader();
        catalog_reopen_snapshot(&reader)?
    };
    let profile_reopen_process = profile_reopen_probe.finish()?;
    let profile_reopen = ConversionBenchmarkCatalogReopen {
        process: profile_reopen_process,
        verification: profile_verification,
        vfs: profile_reopen_vfs,
    };

    let profile_query_probe = ProcessUsageProbe::start()?;
    let package = ProfileCatalog::new(&pinned)
        .find_package_record_by_key(package_key_sha256)?
        .ok_or_else(|| {
            anyhow!(
                "package key {} is absent from profile '{}' revision {}",
                package_key_sha256,
                selection.source_profile,
                selection.profile_revision_sha256
            )
        })?;
    ensure!(
        package.source_profile == selection.source_profile,
        "benchmark package source profile contradicts selected authority"
    );
    let profile_query_vfs = {
        let reader = pinned.reader();
        reader
            .portable_vfs_metrics()
            .context("registered benchmark profile query lost its portable VFS metrics")?
    };
    let profile_query_process = profile_query_probe.finish()?;
    let profile_query =
        catalog_query_evidence(profile_reopen.vfs, profile_query_vfs, profile_query_process)?;

    let source_reopen_probe = ProcessUsageProbe::start()?;
    let source = authority.open_source_catalog_for_package(&pinned, &package)?;
    let (source_verification, source_reopen_vfs) = {
        let reader = source.reader();
        catalog_reopen_snapshot(&reader)?
    };
    let source_reopen_process = source_reopen_probe.finish()?;
    let source_reopen = ConversionBenchmarkCatalogReopen {
        process: source_reopen_process,
        verification: source_verification,
        vfs: source_reopen_vfs,
    };

    let source_query_probe = ProcessUsageProbe::start()?;
    let source_snapshot = authority.source_snapshot_for_package(&pinned, &package)?;
    ensure!(
        source_snapshot == *source.manifest(),
        "benchmark source snapshot changed after its measured registered reopen"
    );
    let source_query_vfs = {
        let reader = source.reader();
        reader
            .portable_vfs_metrics()
            .context("registered benchmark source query lost its portable VFS metrics")?
    };
    let source_query_process = source_query_probe.finish()?;
    let source_query =
        catalog_query_evidence(source_reopen.vfs, source_query_vfs, source_query_process)?;
    Ok(ResolvedBenchmarkSubject {
        profile_manifest: pinned.manifest().clone(),
        profile_physical_attestation: pinned.physical_attestation().clone(),
        source_snapshot,
        source_physical_attestation: source.physical_attestation().clone(),
        package,
        profile_setup: ConversionBenchmarkCatalogSetup {
            reopen: profile_reopen,
            query: profile_query,
        },
        source_setup: ConversionBenchmarkCatalogSetup {
            reopen: source_reopen,
            query: source_query,
        },
    })
}

fn admit_staged_subject(path: &Path, package: &CatalogPackageRecordV1) -> Result<()> {
    let path = canonical_regular_file(path, "staged benchmark source artifact")?;
    let size = fs::metadata(&path)?.len();
    ensure!(
        size == package.size,
        "staged source artifact has {size} bytes; immutable catalog package {} requires {}",
        package.package_key_sha256,
        package.size
    );
    conary_core::repository::verify_checksum(&path, &package.checksum).with_context(|| {
        format!(
            "authenticate staged source artifact against immutable package {} checksum {}",
            package.package_key_sha256, package.checksum
        )
    })
}

fn benchmark_authority(
    selection_kind: ConversionBenchmarkSelectionKind,
    selection: &ProfileRevisionSelection,
    profile: &ProfileRevisionV2,
    profile_physical_attestation: &RemiCatalogPhysicalAttestation,
    source: &SourceSnapshotV1,
    source_physical_attestation: &RemiCatalogPhysicalAttestation,
) -> Result<ConversionBenchmarkAuthority> {
    let profile_digest = profile.manifest_sha256()?;
    ensure!(
        profile_digest == selection.profile_revision_sha256,
        "selected profile revision digest changed while binding benchmark authority"
    );
    let source_snapshot_sha256 = source.manifest_sha256()?;
    Ok(ConversionBenchmarkAuthority {
        selection_kind,
        source_profile: selection.source_profile.clone(),
        profile: benchmark_catalog_authority(
            selection.profile_revision_sha256.clone(),
            &profile.catalog,
            &profile.logical_digest_sha256,
            profile_physical_attestation,
        ),
        source: benchmark_catalog_authority(
            source_snapshot_sha256,
            &source.catalog,
            &source.logical_digest_sha256,
            source_physical_attestation,
        ),
        source_identity: source.source_identity.clone(),
        repository_identity: source.repository_identity.clone(),
        source_parser_config_sha256: source.provenance.parser_config_sha256.clone(),
        source_trust_policy_sha256: source.provenance.trust_policy_sha256.clone(),
        authenticated_metadata_objects: source.authenticated_objects.len() as u64,
    })
}

fn benchmark_catalog_authority(
    resource_sha256: String,
    artifact: &conary_core::repository::catalog::CatalogArtifactV1,
    logical_digest_sha256: &str,
    physical_attestation: &RemiCatalogPhysicalAttestation,
) -> ConversionBenchmarkCatalogAuthority {
    ConversionBenchmarkCatalogAuthority {
        resource_sha256,
        artifact_sha256: artifact.sha256.clone(),
        artifact_bytes: artifact.size,
        logical_digest_sha256: logical_digest_sha256.to_string(),
        portable_manifest_sha256: physical_attestation.portable_manifest.sha256.clone(),
        portable_manifest_bytes: physical_attestation.portable_manifest.size,
        portable_chunk_size: physical_attestation.chunk_size,
        portable_chunk_count: physical_attestation.chunk_count,
    }
}

fn catalog_reopen_snapshot(
    reader: &CatalogReader,
) -> Result<(
    conary_core::repository::catalog::CatalogVerificationEvidenceV1,
    PortableVfsMetricsV1,
)> {
    let vfs = reader
        .portable_vfs_metrics()
        .context("registered benchmark catalog did not use the portable authenticated VFS")?;
    Ok((*reader.verification_evidence(), vfs))
}

fn catalog_query_evidence(
    before: PortableVfsMetricsV1,
    after: PortableVfsMetricsV1,
    process: ConversionBenchmarkProcessUsage,
) -> Result<ConversionBenchmarkCatalogQuery> {
    Ok(ConversionBenchmarkCatalogQuery {
        process,
        vfs: portable_vfs_delta(after, before)?,
    })
}

fn portable_vfs_delta(
    after: PortableVfsMetricsV1,
    before: PortableVfsMetricsV1,
) -> Result<PortableVfsMetricsV1> {
    let delta = |after: u64, before: u64, label: &str| {
        after
            .checked_sub(before)
            .with_context(|| format!("portable VFS {label} counter regressed"))
    };
    Ok(PortableVfsMetricsV1 {
        read_calls: delta(after.read_calls, before.read_calls, "read calls")?,
        requested_bytes: delta(
            after.requested_bytes,
            before.requested_bytes,
            "requested bytes",
        )?,
        returned_bytes: delta(
            after.returned_bytes,
            before.returned_bytes,
            "returned bytes",
        )?,
        chunk_accesses: delta(
            after.chunk_accesses,
            before.chunk_accesses,
            "chunk accesses",
        )?,
        cache_hits: delta(after.cache_hits, before.cache_hits, "cache hits")?,
        cache_misses: delta(after.cache_misses, before.cache_misses, "cache misses")?,
        carrier_bytes_requested: delta(
            after.carrier_bytes_requested,
            before.carrier_bytes_requested,
            "carrier bytes requested",
        )?,
        authenticated_chunks: delta(
            after.authenticated_chunks,
            before.authenticated_chunks,
            "authenticated chunks",
        )?,
        authenticated_bytes: delta(
            after.authenticated_bytes,
            before.authenticated_bytes,
            "authenticated bytes",
        )?,
        short_reads: delta(after.short_reads, before.short_reads, "short reads")?,
        integrity_failures: delta(
            after.integrity_failures,
            before.integrity_failures,
            "integrity failures",
        )?,
    })
}

fn benchmark_subject(
    package: &CatalogPackageRecordV1,
    source_artifact_sha256: String,
) -> ConversionBenchmarkSubject {
    ConversionBenchmarkSubject {
        package_key_sha256: package.package_key_sha256.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        repository_checksum: package.checksum.clone(),
        source_size_bytes: package.size,
        source_artifact_sha256,
    }
}

async fn run_iteration(
    service: &ConversionService,
    selection: &ProfileRevisionSelection,
    package: &CatalogPackageRecordV1,
    source_artifact: &Path,
    signing_key: Arc<conary_core::ccs::signing::SigningKeyPair>,
    iteration: usize,
) -> Result<ConversionBenchmarkEvidence> {
    let process_probe = ProcessUsageProbe::start()?;
    let conversion = service
        .convert_benchmark_catalog_package_from_selection_async(
            package.clone(),
            selection.clone(),
            source_artifact.to_path_buf(),
        )
        .await;

    let (views, outcome) = match conversion {
        Ok(mut result) => {
            let cache_state = result.cache_state.clone();
            let timing = result.timing.take();
            postprocess_successful_conversion(cache_state, timing, || {
                independently_reopen_output(&result, signing_key.as_ref())
            })?
        }
        Err(error) => conversion_failure_outcome(error),
    };
    let process = process_probe.finish()?;
    Ok(ConversionBenchmarkEvidence {
        iteration,
        process,
        views,
        outcome,
    })
}

fn postprocess_successful_conversion<F>(
    cache_state: String,
    timing: Option<crate::server::conversion_timing::ConversionTimingReport>,
    reopen_output: F,
) -> Result<(ConversionBenchmarkViews, ConversionBenchmarkOutcome)>
where
    F: FnOnce() -> Result<ConversionBenchmarkOutputProof>,
{
    let timing = timing.context("conversion benchmark result omitted timing evidence")?;
    let views = benchmark_views(&cache_state, &timing)?;
    let outcome = match reopen_output() {
        Ok(output) => ConversionBenchmarkOutcome::Success {
            cache_state,
            timing: Box::new(timing),
            output,
        },
        Err(error) => ConversionBenchmarkOutcome::IndependentOutputReopenFailure {
            cache_state,
            timing: Box::new(timing),
            error: format!("independent benchmark output reopen failed: {error:#}"),
        },
    };
    Ok((views, outcome))
}

fn conversion_failure_outcome(
    error: anyhow::Error,
) -> (ConversionBenchmarkViews, ConversionBenchmarkOutcome) {
    (
        ConversionBenchmarkViews {
            conversion_core: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
            end_to_end: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
        },
        ConversionBenchmarkOutcome::Failure {
            error: format!("{error:#}"),
        },
    )
}

fn benchmark_views(
    cache_state: &str,
    timing: &crate::server::conversion_timing::ConversionTimingReport,
) -> Result<ConversionBenchmarkViews> {
    let core_duration = conversion_core_duration(timing)?;
    let core_executed = match cache_state {
        "cold" => true,
        "hot" => false,
        other => bail!("benchmark conversion returned unknown cache state '{other}'"),
    };
    if !core_executed {
        ensure!(
            core_duration == 0,
            "exact cache hit unexpectedly executed conversion-core phases"
        );
    }
    Ok(ConversionBenchmarkViews {
        conversion_core: ConversionBenchmarkView {
            executed: core_executed,
            duration_ms: core_duration,
        },
        end_to_end: ConversionBenchmarkView {
            executed: true,
            duration_ms: timing.total_ms,
        },
    })
}

fn conversion_core_duration(
    timing: &crate::server::conversion_timing::ConversionTimingReport,
) -> Result<u128> {
    timing
        .phases
        .iter()
        .filter(|phase| {
            matches!(
                phase.phase,
                crate::server::conversion_timing::ConversionPhase::NativeArchiveParseAndSpool
                    | crate::server::conversion_timing::ConversionPhase::ArtifactIdentityAndAuthorityValidation
                    | crate::server::conversion_timing::ConversionPhase::MetadataLifecycleAndAuthorityProjection
                    | crate::server::conversion_timing::ConversionPhase::PayloadDerivationAndObjectStaging
                    | crate::server::conversion_timing::ConversionPhase::OutputWorkspacePreparation
                    | crate::server::conversion_timing::ConversionPhase::ControlProjectionAndSigning
                    | crate::server::conversion_timing::ConversionPhase::ArchiveAssemblyAndGzip
                    | crate::server::conversion_timing::ConversionPhase::NativeProvenanceProjection
            )
        })
        .try_fold(0_u128, |total, phase| {
            total
                .checked_add(phase.duration_ms)
                .context("conversion-core duration overflow")
        })
}

fn independently_reopen_output(
    result: &super::ServerConversionResult,
    signing_key: &conary_core::ccs::signing::SigningKeyPair,
) -> Result<ConversionBenchmarkOutputProof> {
    let ccs_size_bytes = fs::metadata(&result.ccs_path)?.len();
    ensure!(
        ccs_size_bytes == result.total_size,
        "independent CCS size disagrees with persisted conversion result"
    );
    let transport_reopen_started = Instant::now();
    let policy =
        conary_core::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]);
    let verified = conary_core::ccs::verify::verify_package(&result.ccs_path, &policy)?;
    let transport = conary_core::ccs::CcsTransportEnvelopeV1::from_verified_archive(&verified)?;
    let independent_transport_reopen_ms = transport_reopen_started.elapsed().as_millis();
    let expected_transport_sha256 =
        conary_core::ccs::attestation::canonical_json_hash(&result.transport)?;
    let transport_sha256 = conary_core::ccs::attestation::canonical_json_hash(&transport)?;
    ensure!(
        transport_sha256 == expected_transport_sha256,
        "independent transport envelope disagrees with conversion result"
    );
    let complete_hash_started = Instant::now();
    let ccs_sha256 = sha256_file(&result.ccs_path)?;
    let independent_complete_archive_hash_ms = complete_hash_started.elapsed().as_millis();
    ensure!(
        result.content_hash == format!("sha256:{ccs_sha256}"),
        "independent CCS digest disagrees with persisted conversion result"
    );
    let signed_object_count = transport.objects.len() as u64;
    let signed_object_bytes = transport.objects.iter().try_fold(0_u64, |total, object| {
        total
            .checked_add(object.size)
            .context("signed object byte count overflow")
    })?;
    let signed_object_set_sha256 =
        conary_core::ccs::attestation::canonical_json_hash(&transport.objects)?;
    Ok(ConversionBenchmarkOutputProof {
        ccs_sha256,
        ccs_size_bytes,
        transport_sha256: bare_sha256(&transport_sha256, "transport SHA-256")?,
        signed_object_set_sha256: bare_sha256(
            &signed_object_set_sha256,
            "signed object set SHA-256",
        )?,
        signed_object_count,
        signed_object_bytes,
        independent_transport_reopen_ms,
        independent_transport_reopen_bytes: ccs_size_bytes,
        independent_complete_archive_hash_ms,
        independent_complete_archive_hash_bytes: ccs_size_bytes,
    })
}

fn environment_roots(roots: &[(&str, &Path)]) -> Result<Vec<ConversionBenchmarkRootIdentity>> {
    roots
        .iter()
        .map(|(role, path)| filesystem_identity(role, path))
        .collect()
}

fn filesystem_identity(role: &str, path: &Path) -> Result<ConversionBenchmarkRootIdentity> {
    let path = canonical_existing(path, role)?;
    let metadata = fs::metadata(&path)?;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("filesystem path for role '{role}' contains NUL"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` is NUL terminated and `stats` points to writable storage.
    let status = unsafe { libc::statfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "inspect filesystem for benchmark root '{}': {}",
                role,
                path.display()
            )
        });
    }
    // SAFETY: successful `statfs` initialized the complete structure.
    let stats = unsafe { stats.assume_init() };
    let block_size =
        u64::try_from(stats.f_bsize).context("benchmark root filesystem block size is negative")?;
    Ok(ConversionBenchmarkRootIdentity {
        role: role.to_string(),
        path: path.display().to_string(),
        device_id: metadata.dev(),
        filesystem_type: format!("0x{:x}", stats.f_type),
        block_size,
    })
}

fn canonical_regular_file(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "{label} {} must be a regular non-symlink file",
        path.display()
    );
    path.canonicalize()
        .with_context(|| format!("canonicalize {label} {}", path.display()))
}

fn canonical_existing(path: &Path, label: &str) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("canonicalize {label} {}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty(),
        "benchmark path must not be empty"
    );
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent_path = absolute
        .parent()
        .context("benchmark path has no parent directory")?;
    let parent = parent_path.canonicalize().with_context(|| {
        format!(
            "canonicalize benchmark path parent {}",
            parent_path.display()
        )
    })?;
    let name = absolute
        .file_name()
        .context("benchmark path has no final component")?;
    Ok(parent.join(name))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    Ok(conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut file)?.value)
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            && value == value.to_ascii_lowercase(),
        "{label} must be one exact lowercase SHA-256 digest"
    );
    Ok(())
}

fn bare_sha256(value: &str, label: &str) -> Result<String> {
    let value = value
        .strip_prefix("sha256:")
        .with_context(|| format!("{label} lacks the exact sha256 algorithm prefix"))?;
    validate_sha256(value, label)?;
    Ok(value.to_string())
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PublishedInode {
    device: u64,
    inode: u64,
}

impl PublishedInode {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_file()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
    }
}

fn rollback_failed_publication(path: &Path, temporary: &Path, published: Option<PublishedInode>) {
    if let Some(published) = published
        && fs::symlink_metadata(path)
            .map(|metadata| published.matches(&metadata))
            .unwrap_or(false)
    {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(temporary);
    let _ = sync_parent(path);
}

fn unix_seconds() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time precedes Unix epoch")?
            .as_secs(),
    )
    .context("system time exceeds SQLite integer range")
}

#[cfg(test)]
mod tests;
