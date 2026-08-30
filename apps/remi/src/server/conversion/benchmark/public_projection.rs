// apps/remi/src/server/conversion/benchmark/public_projection.rs
//! Sanitized, byte-bound projection of one successful schema-v4 benchmark.

use super::{
    ConversionBenchmarkAuthority, ConversionBenchmarkOutcome, ConversionBenchmarkOutputProof,
    ConversionBenchmarkProcessUsage, ConversionBenchmarkReportV4, ConversionBenchmarkSetup,
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

pub(super) const PUBLIC_REPORT_FILE_NAME: &str = "conversion-benchmark-public-v2.json";
const PUBLIC_REPORT_SCHEMA_V2: u32 = 2;
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
struct ConversionBenchmarkPublicReportV2 {
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

fn read_and_validate_raw_report(path: &Path) -> Result<(Vec<u8>, ConversionBenchmarkReportV4)> {
    let bytes = read_regular_nofollow(path, "raw conversion benchmark")?;
    let report: ConversionBenchmarkReportV4 = serde_json::from_slice(&bytes)
        .context("strictly decode raw conversion benchmark schema v4")?;
    validate_report(&report).context("validate raw conversion benchmark schema v4")?;
    Ok((bytes, report))
}

fn project_report(
    raw_bytes: &[u8],
    raw: &ConversionBenchmarkReportV4,
) -> Result<ConversionBenchmarkPublicReportV2> {
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
    Ok(ConversionBenchmarkPublicReportV2 {
        schema_version: PUBLIC_REPORT_SCHEMA_V2,
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

fn validate_public_report(report: &ConversionBenchmarkPublicReportV2) -> Result<()> {
    ensure!(
        report.schema_version == PUBLIC_REPORT_SCHEMA_V2,
        "unsupported public conversion benchmark schema {}",
        report.schema_version
    );
    ensure!(
        report.raw_report.schema_version == super::CONVERSION_BENCHMARK_SCHEMA_V4,
        "public benchmark does not bind raw schema v4"
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
        "public benchmark root roles differ from the schema-v2 environment identity"
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

fn publish_create_new(path: &Path, report: &ConversionBenchmarkPublicReportV2) -> Result<()> {
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
        let reopened: ConversionBenchmarkPublicReportV2 =
            serde_json::from_slice(&reopened_bytes)
                .context("strictly reopen published public conversion benchmark schema v2")?;
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
mod tests;
