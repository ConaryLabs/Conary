// apps/remi/src/server/conversion/benchmark/public_projection.rs
//! Sanitized, byte-bound projection of one successful schema-v3 benchmark.

use super::{
    ConversionBenchmarkAuthority, ConversionBenchmarkOutcome, ConversionBenchmarkOutputProof,
    ConversionBenchmarkProcessUsage, ConversionBenchmarkReportV3, ConversionBenchmarkSetup,
    ConversionBenchmarkSubject, ConversionBenchmarkViews, PublishedInode, report::validate_report,
    rollback_failed_publication, sync_parent, validate_sha256,
};
use crate::server::conversion_timing::{
    ConversionNestedPhaseTiming, ConversionPhase, ConversionPhaseTiming, ConversionSourceIdentity,
    ConversionWorkMetrics,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(super) const PUBLIC_REPORT_FILE_NAME: &str = "conversion-benchmark-public-v1.json";
const PUBLIC_REPORT_SCHEMA_V1: u32 = 1;
const MAX_REPORT_BYTES: u64 = 64 * 1024 * 1024;
const PRIVATE_FILE_MODE: u32 = 0o600;
const EXPECTED_ROOT_ROLES: [&str; 10] = [
    "source_config",
    "source_database",
    "source_catalogs",
    "repository_keys",
    "operator_source_artifact",
    "work_root",
    "benchmark_database",
    "benchmark_chunks",
    "benchmark_cache",
    "staged_source_artifact",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicRawReportBinding {
    schema_version: u32,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicBenchmarkRootIdentity {
    role: String,
    filesystem_type: String,
    block_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicBenchmarkEnvironment {
    hardware_label: String,
    remi_version: String,
    source_commit: String,
    source_dirty: bool,
    binary_sha256: String,
    os_release: String,
    kernel_release: String,
    cpu_model: String,
    logical_cpus: usize,
    memory_bytes: u64,
    roots: Vec<PublicBenchmarkRootIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicConversionTiming {
    distro: String,
    package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ConversionSourceIdentity>,
    phases: Vec<ConversionPhaseTiming>,
    nested_phases: Vec<ConversionNestedPhaseTiming>,
    skipped_phases: Vec<ConversionPhase>,
    work: ConversionWorkMetrics,
    #[serde(with = "crate::server::conversion_timing::json_u128")]
    total_ms: u128,
    success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublicBenchmarkRepetition {
    iteration: usize,
    cache_state: String,
    process: ConversionBenchmarkProcessUsage,
    views: ConversionBenchmarkViews,
    timing: PublicConversionTiming,
    output: ConversionBenchmarkOutputProof,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConversionBenchmarkPublicReportV1 {
    schema_version: u32,
    raw_report: PublicRawReportBinding,
    environment: PublicBenchmarkEnvironment,
    authority: ConversionBenchmarkAuthority,
    setup: ConversionBenchmarkSetup,
    subject: ConversionBenchmarkSubject,
    repetitions: Vec<PublicBenchmarkRepetition>,
}

pub(super) fn publish_and_reopen_public_report(raw_path: &Path, public_path: &Path) -> Result<()> {
    let (raw_bytes, raw_report) = read_and_validate_raw_report(raw_path)?;
    let projected = project_report(&raw_bytes, &raw_report)?;
    validate_public_report(&projected)?;
    publish_create_new(public_path, &projected)?;
    Ok(())
}

fn read_and_validate_raw_report(path: &Path) -> Result<(Vec<u8>, ConversionBenchmarkReportV3)> {
    let bytes = read_regular_nofollow(path, "raw conversion benchmark")?;
    let report: ConversionBenchmarkReportV3 = serde_json::from_slice(&bytes)
        .context("strictly decode raw conversion benchmark schema v3")?;
    validate_report(&report).context("validate raw conversion benchmark schema v3")?;
    Ok((bytes, report))
}

fn project_report(
    raw_bytes: &[u8],
    raw: &ConversionBenchmarkReportV3,
) -> Result<ConversionBenchmarkPublicReportV1> {
    ensure!(
        !raw.environment.source_dirty,
        "dirty source identity cannot be published as public benchmark evidence"
    );
    validate_git_commit(&raw.environment.source_commit)?;
    ensure!(
        !is_unknown(&raw.authority.source_identity)
            && !is_unknown(&raw.authority.repository_identity),
        "unknown benchmark authority identity cannot be published"
    );
    validate_sha256(&raw.environment.binary_sha256, "benchmark binary SHA-256")?;

    let repetitions = raw
        .repetitions
        .iter()
        .map(|repetition| {
            let ConversionBenchmarkOutcome::Success {
                cache_state,
                timing,
                output,
            } = &repetition.outcome
            else {
                bail!(
                    "benchmark repetition {} is not successful and cannot be published",
                    repetition.iteration
                );
            };
            ensure!(
                timing.success,
                "successful benchmark repetition {} carries unsuccessful timing",
                repetition.iteration
            );
            Ok(PublicBenchmarkRepetition {
                iteration: repetition.iteration,
                cache_state: cache_state.clone(),
                process: repetition.process.clone(),
                views: repetition.views.clone(),
                timing: PublicConversionTiming {
                    distro: timing.distro.clone(),
                    package: timing.package.clone(),
                    version: timing.version.clone(),
                    source: timing.source.clone(),
                    phases: timing.phases.clone(),
                    nested_phases: timing.nested_phases.clone(),
                    skipped_phases: timing
                        .skipped_phases
                        .iter()
                        .map(|skipped| skipped.phase)
                        .collect(),
                    work: timing.work.clone(),
                    total_ms: timing.total_ms,
                    success: timing.success,
                },
                output: output.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let raw_size = u64::try_from(raw_bytes.len()).context("raw report size exceeds u64")?;
    Ok(ConversionBenchmarkPublicReportV1 {
        schema_version: PUBLIC_REPORT_SCHEMA_V1,
        raw_report: PublicRawReportBinding {
            schema_version: raw.schema_version,
            sha256: conary_core::hash::sha256(raw_bytes),
            size_bytes: raw_size,
        },
        environment: PublicBenchmarkEnvironment {
            hardware_label: raw.environment.hardware_label.clone(),
            remi_version: raw.environment.remi_version.clone(),
            source_commit: raw.environment.source_commit.clone(),
            source_dirty: raw.environment.source_dirty,
            binary_sha256: raw.environment.binary_sha256.clone(),
            os_release: raw.environment.os_release.clone(),
            kernel_release: raw.environment.kernel_release.clone(),
            cpu_model: raw.environment.cpu_model.clone(),
            logical_cpus: raw.environment.logical_cpus,
            memory_bytes: raw.environment.memory_bytes,
            roots: raw
                .environment
                .roots
                .iter()
                .map(|root| PublicBenchmarkRootIdentity {
                    role: root.role.clone(),
                    filesystem_type: root.filesystem_type.clone(),
                    block_size: root.block_size,
                })
                .collect(),
        },
        authority: raw.authority.clone(),
        setup: raw.setup.clone(),
        subject: raw.subject.clone(),
        repetitions,
    })
}

fn validate_public_report(report: &ConversionBenchmarkPublicReportV1) -> Result<()> {
    ensure!(
        report.schema_version == PUBLIC_REPORT_SCHEMA_V1,
        "unsupported public conversion benchmark schema {}",
        report.schema_version
    );
    ensure!(
        report.raw_report.schema_version == super::CONVERSION_BENCHMARK_SCHEMA_V3,
        "public benchmark does not bind raw schema v3"
    );
    validate_sha256(&report.raw_report.sha256, "raw benchmark report SHA-256")?;
    ensure!(
        report.raw_report.size_bytes > 0 && report.raw_report.size_bytes <= MAX_REPORT_BYTES,
        "raw benchmark report size is outside the public schema bound"
    );
    ensure!(
        !report.environment.source_dirty,
        "public benchmark carries a dirty source identity"
    );
    validate_git_commit(&report.environment.source_commit)?;
    validate_sha256(
        &report.environment.binary_sha256,
        "public benchmark binary SHA-256",
    )?;
    ensure!(
        !report.environment.hardware_label.is_empty()
            && report.environment.hardware_label.trim() == report.environment.hardware_label
            && report.environment.hardware_label.len() <= 128,
        "public benchmark hardware label is not a bounded canonical label"
    );
    ensure!(
        !report.environment.remi_version.is_empty()
            && report.environment.logical_cpus > 0
            && report.environment.memory_bytes > 0,
        "public benchmark environment identity is incomplete"
    );

    let expected_roles = EXPECTED_ROOT_ROLES.into_iter().collect::<BTreeSet<_>>();
    let mut actual_roles = BTreeSet::new();
    for root in &report.environment.roots {
        ensure!(
            actual_roles.insert(root.role.as_str()),
            "public benchmark repeats root role '{}'",
            root.role
        );
        ensure!(
            root.filesystem_type.starts_with("0x")
                && root.filesystem_type.len() > 2
                && root.filesystem_type[2..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                && root.block_size > 0,
            "public benchmark root '{}' has invalid filesystem geometry",
            root.role
        );
    }
    ensure!(
        actual_roles == expected_roles,
        "public benchmark root roles differ from the schema-v1 environment identity"
    );

    ensure!(
        !report.repetitions.is_empty(),
        "public benchmark has no successful repetitions"
    );
    for (index, repetition) in report.repetitions.iter().enumerate() {
        let expected_cache = if index == 0 { "cold" } else { "hot" };
        ensure!(
            repetition.iteration == index + 1 && repetition.cache_state == expected_cache,
            "public benchmark repetition sequence or cache state is invalid"
        );
        ensure!(
            repetition.timing.success,
            "public benchmark repetition {} has unsuccessful timing",
            repetition.iteration
        );
    }

    let value = serde_json::to_value(report)?;
    validate_public_strings(&value, "$")
}

fn validate_git_commit(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            && value == value.to_ascii_lowercase(),
        "benchmark source commit must be one exact lowercase Git commit identity"
    );
    Ok(())
}

fn is_unknown(value: &str) -> bool {
    value.trim().is_empty() || value.eq_ignore_ascii_case("unknown")
}

fn validate_public_strings(value: &Value, location: &str) -> Result<()> {
    match value {
        Value::String(value) => validate_public_string(value, location),
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_public_strings(value, &format!("{location}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_public_strings(value, &format!("{location}.{key}"))?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_public_string(value: &str, location: &str) -> Result<()> {
    ensure!(
        value.len() <= 4096 && !value.chars().any(char::is_control),
        "unsafe control or unbounded string at {location}"
    );
    ensure!(
        !value.contains('/') && !value.contains('\\'),
        "path or URL-like string is forbidden at {location}"
    );
    let lowercase = value.to_ascii_lowercase();
    const CREDENTIAL_MARKERS: [&str; 13] = [
        "authorization:",
        "authorization=",
        "bearer ",
        "password:",
        "password=",
        "passwd:",
        "passwd=",
        "token:",
        "token=",
        "secret:",
        "secret=",
        "api_key=",
        "private_key=",
    ];
    ensure!(
        !CREDENTIAL_MARKERS
            .iter()
            .any(|marker| lowercase.contains(marker)),
        "credential-like string is forbidden at {location}"
    );
    Ok(())
}

fn read_regular_nofollow(path: &Path, label: &str) -> Result<Vec<u8>> {
    let named_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        named_metadata.file_type().is_file() && !named_metadata.file_type().is_symlink(),
        "{label} {} must be a regular non-symlink file",
        path.display()
    );
    ensure!(
        named_metadata.mode() & 0o7777 == PRIVATE_FILE_MODE && named_metadata.nlink() == 1,
        "{label} {} must be private mode 0600 with one link",
        path.display()
    );
    ensure!(
        named_metadata.len() > 0 && named_metadata.len() <= MAX_REPORT_BYTES,
        "{label} {} is outside the bounded report size",
        path.display()
    );

    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open {label} {} without following links", path.display()))?;
    let opened_metadata = input.metadata()?;
    ensure!(
        opened_metadata.file_type().is_file()
            && opened_metadata.dev() == named_metadata.dev()
            && opened_metadata.ino() == named_metadata.ino()
            && opened_metadata.mode() & 0o7777 == PRIVATE_FILE_MODE
            && opened_metadata.nlink() == 1,
        "{label} {} changed while opening",
        path.display()
    );

    let capacity = usize::try_from(opened_metadata.len()).context("report size exceeds usize")?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut input)
        .take(MAX_REPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read exact {label} bytes from {}", path.display()))?;
    let final_metadata = input.metadata()?;
    let final_size = u64::try_from(bytes.len()).context("read report size exceeds u64")?;
    ensure!(
        final_size == opened_metadata.len()
            && final_size == final_metadata.len()
            && final_size <= MAX_REPORT_BYTES,
        "{label} {} changed size while reading",
        path.display()
    );
    let current_metadata = fs::symlink_metadata(path)?;
    ensure!(
        current_metadata.file_type().is_file()
            && current_metadata.dev() == opened_metadata.dev()
            && current_metadata.ino() == opened_metadata.ino()
            && current_metadata.mode() & 0o7777 == PRIVATE_FILE_MODE
            && current_metadata.nlink() == 1,
        "{label} {} changed while reading",
        path.display()
    );
    Ok(bytes)
}

fn publish_create_new(path: &Path, report: &ConversionBenchmarkPublicReportV1) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("public benchmark report already exists: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .context("public benchmark report path has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent).with_context(|| {
        format!(
            "inspect public benchmark report parent {}",
            parent.display()
        )
    })?;
    ensure!(
        parent_metadata.file_type().is_dir() && !parent_metadata.file_type().is_symlink(),
        "public benchmark report parent must be a plain directory"
    );
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    let temporary = parent.join(format!(
        ".{PUBLIC_REPORT_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut linked_inode = None;
    let publication = (|| -> Result<PublishedInode> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE_MODE)
            .open(&temporary)
            .with_context(|| {
                format!(
                    "create private public-report staging file {}",
                    temporary.display()
                )
            })?;
        output.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        let staged_metadata = output.metadata()?;
        let staged_inode = PublishedInode::from_metadata(&staged_metadata);

        match fs::hard_link(&temporary, path) {
            Ok(()) => linked_inode = Some(staged_inode),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("public benchmark report already exists: {}", path.display())
            }
            Err(error) => return Err(error.into()),
        }
        let published_metadata = fs::symlink_metadata(path)?;
        ensure!(
            published_metadata.file_type().is_file()
                && published_metadata.dev() == staged_metadata.dev()
                && published_metadata.ino() == staged_metadata.ino()
                && published_metadata.nlink() == 2
                && published_metadata.mode() & 0o7777 == PRIVATE_FILE_MODE,
            "published public benchmark report is not the private staged file"
        );
        fs::remove_file(&temporary)?;
        let final_metadata = fs::symlink_metadata(path)?;
        ensure!(
            final_metadata.file_type().is_file()
                && final_metadata.dev() == staged_metadata.dev()
                && final_metadata.ino() == staged_metadata.ino()
                && final_metadata.nlink() == 1
                && final_metadata.mode() & 0o7777 == PRIVATE_FILE_MODE,
            "published public benchmark report retained an unexpected link"
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
        let reopened_bytes = read_regular_nofollow(path, "public conversion benchmark")?;
        ensure!(
            reopened_bytes == bytes,
            "reopened public conversion benchmark report changed bytes"
        );
        let reopened: ConversionBenchmarkPublicReportV1 =
            serde_json::from_slice(&reopened_bytes)
                .context("strictly reopen published public conversion benchmark schema v1")?;
        validate_public_report(&reopened)?;
        ensure!(
            reopened == *report,
            "reopened public conversion benchmark report changed value"
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
        CONVERSION_BENCHMARK_SCHEMA_V3, ConversionBenchmarkCatalogAuthority,
        ConversionBenchmarkCatalogQuery, ConversionBenchmarkCatalogReopen,
        ConversionBenchmarkCatalogSetup, ConversionBenchmarkEnvironment,
        ConversionBenchmarkEvidence, ConversionBenchmarkRootIdentity,
        ConversionBenchmarkSelectionKind, ConversionBenchmarkView,
    };
    use crate::server::conversion_timing::{ConversionSourceIdentity, ConversionTimingReport};
    use conary_core::repository::catalog::{
        CatalogVerificationEvidenceV1, PORTABLE_CHUNK_SIZE_V1, PortableVfsMetricsV1,
        portable_manifest_size_v1,
    };
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    const PRIVATE_PATH_SENTINEL: &str = "/private/operator/benchmark-sentinel";

    fn valid_process_usage() -> ConversionBenchmarkProcessUsage {
        ConversionBenchmarkProcessUsage {
            wall_time_us: 19,
            user_cpu_us: 7,
            system_cpu_us: 3,
            rss_start_bytes: 11,
            rss_end_bytes: 13,
            process_lifetime_peak_rss_bytes: 17,
            minor_faults: 23,
            major_faults: 29,
            block_input_operations: 31,
            block_output_operations: 37,
            logical_read_bytes: 41,
            logical_write_bytes: 43,
            read_syscalls: 47,
            write_syscalls: 53,
            storage_read_bytes: 59,
            storage_write_bytes: 61,
            cancelled_write_bytes: 0,
            voluntary_context_switches: 67,
            involuntary_context_switches: 71,
            thread_count_start: 2,
            thread_count_end: 2,
            runnable_threads_start: 1,
            runnable_threads_end: 1,
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
            read_calls: 2,
            requested_bytes: 2,
            returned_bytes: 2,
            chunk_accesses: 2,
            cache_hits: 1,
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
        ConversionBenchmarkCatalogSetup {
            reopen: ConversionBenchmarkCatalogReopen {
                process: valid_process_usage(),
                verification: CatalogVerificationEvidenceV1 {
                    catalog_bytes: authority.artifact_bytes,
                    portable_manifest_validation_passes: 1,
                    portable_manifest_validation_bytes: authority.portable_manifest_bytes,
                    stored_binding_checks: 1,
                    ..CatalogVerificationEvidenceV1::default()
                },
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
            independent_transport_reopen_ms: 5,
            independent_transport_reopen_bytes: 23,
            independent_complete_archive_hash_ms: 7,
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
            timing.record_skipped(
                ConversionPhase::Download,
                format!("artifact supplied from {PRIVATE_PATH_SENTINEL}"),
            );
            timing.work.admitted_local_bytes = 5;
            timing.work.repository_checksum_bytes_hashed = 5;
            timing.work.source_artifact_bytes = 5;
            timing.work.source_bytes_hashed = 5;
            timing.work.native_archive_entries_traversed = 79;
            timing.work.native_decompressed_archive_bytes_read = 83;
            timing.work.payload_reference_bytes_hashed = 89;
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
                hardware_label: "production-xfs".to_string(),
                remi_version: "0.1.0".to_string(),
                source_commit: "e".repeat(40),
                source_dirty: false,
                binary_path: format!("{PRIVATE_PATH_SENTINEL}/bin/remi"),
                binary_sha256: "6".repeat(64),
                os_release: "Production Linux".to_string(),
                kernel_release: "6.12.0-production".to_string(),
                cpu_model: "Fixture CPU".to_string(),
                logical_cpus: 32,
                memory_bytes: 64 * 1024 * 1024 * 1024,
                roots: EXPECTED_ROOT_ROLES
                    .into_iter()
                    .map(|role| ConversionBenchmarkRootIdentity {
                        role: role.to_string(),
                        path: format!("{PRIVATE_PATH_SENTINEL}/{role}"),
                        device_id: 99,
                        filesystem_type: "0x58465342".to_string(),
                        block_size: 4096,
                    })
                    .collect(),
            },
            authority: ConversionBenchmarkAuthority {
                selection_kind: ConversionBenchmarkSelectionKind::Active,
                source_profile: "fedora-44".to_string(),
                profile: valid_catalog_authority(),
                source: valid_catalog_authority(),
                source_identity: "fedora-project".to_string(),
                repository_identity: "fedora-44-everything-x86_64".to_string(),
                source_parser_config_sha256: "7".repeat(64),
                source_trust_policy_sha256: "8".repeat(64),
                authenticated_metadata_objects: 2,
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

    fn publish_raw(root: &Path, report: &ConversionBenchmarkReportV3) -> std::path::PathBuf {
        let raw_path = root.join(super::super::REPORT_FILE_NAME);
        super::super::report::publish_and_reopen_report(&raw_path, report)
            .expect("publish raw benchmark fixture");
        raw_path
    }

    fn assert_no_key(value: &Value, forbidden: &str) {
        match value {
            Value::Array(values) => {
                for value in values {
                    assert_no_key(value, forbidden);
                }
            }
            Value::Object(values) => {
                assert!(!values.contains_key(forbidden), "found key {forbidden}");
                for value in values.values() {
                    assert_no_key(value, forbidden);
                }
            }
            _ => {}
        }
    }

    fn assert_publication_rejected(report: &ConversionBenchmarkReportV3) -> String {
        let root = tempfile::tempdir().expect("create rejected-publication root");
        let raw_path = publish_raw(root.path(), report);
        let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);
        let error = publish_and_reopen_public_report(&raw_path, &public_path)
            .expect_err("invalid raw report must not produce a public projection");
        assert!(!public_path.exists());
        error.to_string()
    }

    #[test]
    fn projection_omits_private_fields_and_preserves_exact_evidence() {
        let root = tempfile::tempdir().expect("create publication root");
        let report = valid_report();
        let raw_path = publish_raw(root.path(), &report);
        let raw_bytes = fs::read(&raw_path).expect("read raw report bytes");
        let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);

        publish_and_reopen_public_report(&raw_path, &public_path)
            .expect("publish public benchmark projection");

        assert_eq!(
            fs::metadata(&raw_path).unwrap().permissions().mode() & 0o777,
            PRIVATE_FILE_MODE
        );
        assert_eq!(
            fs::metadata(&public_path).unwrap().permissions().mode() & 0o777,
            PRIVATE_FILE_MODE
        );
        let public_bytes = fs::read(&public_path).expect("read public report bytes");
        let public_text = std::str::from_utf8(&public_bytes).unwrap();
        assert!(!public_text.contains(PRIVATE_PATH_SENTINEL));
        let value: Value = serde_json::from_slice(&public_bytes).unwrap();
        for forbidden in ["binary_path", "path", "device_id", "reason", "error"] {
            assert_no_key(&value, forbidden);
        }
        assert_eq!(value["schema_version"], PUBLIC_REPORT_SCHEMA_V1);
        assert_eq!(
            value["raw_report"]["sha256"],
            conary_core::hash::sha256(&raw_bytes)
        );
        assert_eq!(
            value["raw_report"]["size_bytes"],
            u64::try_from(raw_bytes.len()).unwrap()
        );
        assert_eq!(
            value["authority"],
            serde_json::to_value(&report.authority).unwrap()
        );
        assert_eq!(value["setup"], serde_json::to_value(&report.setup).unwrap());
        assert_eq!(
            value["subject"],
            serde_json::to_value(&report.subject).unwrap()
        );
        for (index, repetition) in report.repetitions.iter().enumerate() {
            let projected = &value["repetitions"][index];
            assert_eq!(
                projected["process"],
                serde_json::to_value(&repetition.process).unwrap()
            );
            assert_eq!(
                projected["views"],
                serde_json::to_value(&repetition.views).unwrap()
            );
            let ConversionBenchmarkOutcome::Success { timing, output, .. } = &repetition.outcome
            else {
                unreachable!()
            };
            assert_eq!(
                projected["timing"]["phases"],
                serde_json::to_value(&timing.phases).unwrap()
            );
            assert_eq!(
                projected["timing"]["nested_phases"],
                serde_json::to_value(&timing.nested_phases).unwrap()
            );
            assert_eq!(
                projected["timing"]["work"],
                serde_json::to_value(&timing.work).unwrap()
            );
            assert_eq!(projected["output"], serde_json::to_value(output).unwrap());
        }
        assert_eq!(
            value["repetitions"][0]["timing"]["skipped_phases"],
            serde_json::json!(["download"])
        );
        assert_eq!(
            value["environment"]["roots"][0],
            serde_json::json!({
                "role": "source_config",
                "filesystem_type": "0x58465342",
                "block_size": 4096,
            })
        );
    }

    #[test]
    fn exact_raw_byte_tamper_changes_public_binding() {
        let report = valid_report();
        let first = tempfile::tempdir().unwrap();
        let first_raw = publish_raw(first.path(), &report);
        let first_public = first.path().join(PUBLIC_REPORT_FILE_NAME);
        publish_and_reopen_public_report(&first_raw, &first_public).unwrap();

        let second = tempfile::tempdir().unwrap();
        let second_raw = publish_raw(second.path(), &report);
        let mut raw_output = OpenOptions::new().append(true).open(&second_raw).unwrap();
        raw_output.write_all(b" ").unwrap();
        raw_output.sync_all().unwrap();
        let second_public = second.path().join(PUBLIC_REPORT_FILE_NAME);
        publish_and_reopen_public_report(&second_raw, &second_public).unwrap();

        let first_value: Value = serde_json::from_slice(&fs::read(first_public).unwrap()).unwrap();
        let second_value: Value =
            serde_json::from_slice(&fs::read(second_public).unwrap()).unwrap();
        assert_ne!(
            first_value["raw_report"]["sha256"],
            second_value["raw_report"]["sha256"]
        );
        assert_eq!(
            second_value["raw_report"]["size_bytes"].as_u64().unwrap(),
            first_value["raw_report"]["size_bytes"].as_u64().unwrap() + 1
        );
    }

    #[test]
    fn raw_report_must_remain_private_and_unaliased() {
        let mode_root = tempfile::tempdir().unwrap();
        let mode_raw = publish_raw(mode_root.path(), &valid_report());
        fs::set_permissions(&mode_raw, fs::Permissions::from_mode(0o640)).unwrap();
        let mode_public = mode_root.path().join(PUBLIC_REPORT_FILE_NAME);
        let error = publish_and_reopen_public_report(&mode_raw, &mode_public)
            .expect_err("non-private raw report must not be projected");
        assert!(error.to_string().contains("private mode 0600"));
        assert!(!mode_public.exists());

        let link_root = tempfile::tempdir().unwrap();
        let link_raw = publish_raw(link_root.path(), &valid_report());
        fs::hard_link(&link_raw, link_root.path().join("raw-alias.json")).unwrap();
        let link_public = link_root.path().join(PUBLIC_REPORT_FILE_NAME);
        let error = publish_and_reopen_public_report(&link_raw, &link_public)
            .expect_err("hard-linked raw report must not be projected");
        assert!(error.to_string().contains("with one link"));
        assert!(!link_public.exists());
    }

    #[test]
    fn public_report_parent_must_be_a_plain_directory() {
        let root = tempfile::tempdir().unwrap();
        let raw_path = publish_raw(root.path(), &valid_report());
        let actual_parent = root.path().join("actual-public-parent");
        fs::create_dir(&actual_parent).unwrap();
        let linked_parent = root.path().join("linked-public-parent");
        symlink(&actual_parent, &linked_parent).unwrap();
        let public_path = linked_parent.join(PUBLIC_REPORT_FILE_NAME);

        let error = publish_and_reopen_public_report(&raw_path, &public_path)
            .expect_err("symlinked public-report parent must be rejected");
        assert!(error.to_string().contains("plain directory"));
        assert!(!actual_parent.join(PUBLIC_REPORT_FILE_NAME).exists());
    }

    #[test]
    fn dirty_unknown_and_failed_reports_are_not_public() {
        let mut dirty = valid_report();
        dirty.environment.source_dirty = true;
        assert!(assert_publication_rejected(&dirty).contains("dirty source identity"));

        let mut unknown = valid_report();
        unknown.environment.source_commit = "unknown".to_string();
        assert!(assert_publication_rejected(&unknown).contains("source commit"));

        let mut unknown_authority = valid_report();
        unknown_authority.authority.source_identity = "unknown".to_string();
        assert!(assert_publication_rejected(&unknown_authority).contains("authority identity"));

        let mut failed = valid_report();
        failed.repetitions.truncate(1);
        failed.repetitions[0].views = ConversionBenchmarkViews {
            conversion_core: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
            end_to_end: ConversionBenchmarkView {
                executed: false,
                duration_ms: 0,
            },
        };
        failed.repetitions[0].outcome = ConversionBenchmarkOutcome::Failure {
            error: format!("failed while reading {PRIVATE_PATH_SENTINEL}"),
        };
        assert!(assert_publication_rejected(&failed).contains("not successful"));
    }

    #[test]
    fn unsafe_retained_strings_are_rejected_recursively() {
        for unsafe_value in [
            "/etc/private-host",
            "https://private.example.invalid/evidence",
            "token=private-credential",
            r"C:\private\benchmark",
        ] {
            let mut report = valid_report();
            report.environment.os_release = unsafe_value.to_string();
            assert!(
                assert_publication_rejected(&report).contains("forbidden"),
                "unsafe public string was accepted: {unsafe_value}"
            );
        }
    }

    #[test]
    fn public_report_publication_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let raw_path = publish_raw(root.path(), &valid_report());
        let public_path = root.path().join(PUBLIC_REPORT_FILE_NAME);
        fs::write(&public_path, b"existing-public-evidence").unwrap();

        let error = publish_and_reopen_public_report(&raw_path, &public_path)
            .expect_err("existing public report must not be replaced");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&public_path).unwrap(), b"existing-public-evidence");

        let dangling_path = root.path().join("dangling-public.json");
        symlink("missing-public-target", &dangling_path).unwrap();
        let error = publish_and_reopen_public_report(&raw_path, &dangling_path)
            .expect_err("dangling public-report target must not be replaced");
        assert!(error.to_string().contains("already exists"));
        assert!(
            fs::symlink_metadata(&dangling_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&dangling_path).unwrap(),
            Path::new("missing-public-target")
        );
    }
}
