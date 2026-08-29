// apps/remi/src/server/conversion/types.rs
//! Public DTOs emitted by Remi conversion workflows.

use crate::server::conversion_timing::ConversionTimingReport;
use conary_core::ccs::convert::ScriptletBundleSummary;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const CONVERSION_BENCHMARK_SCHEMA_V2: u32 = 2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionBenchmarkSelectionKind {
    Active,
    Pinned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkAuthority {
    pub selection_kind: ConversionBenchmarkSelectionKind,
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub profile_catalog_sha256: String,
    pub profile_catalog_bytes: u64,
    pub profile_logical_digest_sha256: String,
    pub source_snapshot_sha256: String,
    pub source_identity: String,
    pub repository_identity: String,
    pub source_parser_config_sha256: String,
    pub source_trust_policy_sha256: String,
    pub authenticated_metadata_objects: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkSubject {
    pub package_key_sha256: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: Option<String>,
    pub repository_checksum: String,
    pub source_size_bytes: u64,
    pub source_artifact_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkRootIdentity {
    pub role: String,
    pub path: String,
    pub device_id: u64,
    pub filesystem_type: String,
    pub block_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkEnvironment {
    pub hardware_label: String,
    pub remi_version: String,
    pub source_commit: String,
    pub source_dirty: bool,
    pub binary_path: String,
    pub binary_sha256: String,
    pub os_release: String,
    pub kernel_release: String,
    pub cpu_model: String,
    pub logical_cpus: usize,
    pub memory_bytes: u64,
    pub roots: Vec<ConversionBenchmarkRootIdentity>,
}

impl ConversionBenchmarkEnvironment {
    pub fn capture(
        hardware_label: String,
        binary_path: &Path,
        binary_sha256: String,
        roots: Vec<ConversionBenchmarkRootIdentity>,
    ) -> Self {
        Self {
            hardware_label,
            remi_version: env!("CARGO_PKG_VERSION").to_string(),
            source_commit: option_env!("CONARY_GIT_COMMIT")
                .unwrap_or("unknown")
                .to_string(),
            source_dirty: option_env!("CONARY_GIT_DIRTY") == Some("true"),
            binary_path: binary_path.display().to_string(),
            binary_sha256,
            os_release: os_release(),
            kernel_release: read_trimmed("/proc/sys/kernel/osrelease"),
            cpu_model: cpu_model(),
            logical_cpus: std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            memory_bytes: memory_bytes(),
            roots,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConversionBenchmarkConfig {
    pub source_config_path: PathBuf,
    pub work_root: PathBuf,
    pub output_path: PathBuf,
    pub source_profile: String,
    pub profile_revision_sha256: Option<String>,
    pub package_key_sha256: String,
    pub source_artifact: PathBuf,
    pub hardware_label: String,
    pub iterations: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkProcessUsage {
    pub wall_time_us: u64,
    pub user_cpu_us: u64,
    pub system_cpu_us: u64,
    pub peak_rss_bytes: u64,
    pub minor_faults: u64,
    pub major_faults: u64,
    pub block_input_operations: u64,
    pub block_output_operations: u64,
    pub voluntary_context_switches: u64,
    pub involuntary_context_switches: u64,
    pub maximum_thread_count: u64,
    pub maximum_runnable_threads: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkView {
    pub executed: bool,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkViews {
    pub conversion_core: ConversionBenchmarkView,
    pub end_to_end: ConversionBenchmarkView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkOutputProof {
    pub ccs_sha256: String,
    pub ccs_size_bytes: u64,
    pub transport_sha256: String,
    pub signed_object_set_sha256: String,
    pub signed_object_count: u64,
    pub signed_object_bytes: u64,
    pub independent_transport_reopen_ms: u128,
    pub independent_transport_reopen_bytes: u64,
    pub independent_complete_archive_hash_ms: u128,
    pub independent_complete_archive_hash_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionBenchmarkOutcome {
    Success {
        cache_state: String,
        timing: Box<ConversionTimingReport>,
        output: ConversionBenchmarkOutputProof,
    },
    Failure {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkEvidence {
    pub iteration: usize,
    pub process: ConversionBenchmarkProcessUsage,
    pub views: ConversionBenchmarkViews,
    pub outcome: ConversionBenchmarkOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionBenchmarkReportV2 {
    pub schema_version: u32,
    pub environment: ConversionBenchmarkEnvironment,
    pub authority: ConversionBenchmarkAuthority,
    pub subject: ConversionBenchmarkSubject,
    pub repetitions: Vec<ConversionBenchmarkEvidence>,
}

fn read_trimmed(path: &str) -> String {
    fs::read_to_string(path)
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn os_release() -> String {
    let Ok(contents) = fs::read_to_string("/etc/os-release") else {
        return "unknown".to_string();
    };
    contents
        .lines()
        .find_map(|line| {
            line.strip_prefix("PRETTY_NAME=")
                .map(|value| value.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn cpu_model() -> String {
    let Ok(contents) = fs::read_to_string("/proc/cpuinfo") else {
        return "unknown".to_string();
    };
    contents
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.trim() == "model name")
                .map(|(_, value)| value.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn memory_bytes() -> u64 {
    let Ok(contents) = fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    contents
        .lines()
        .find_map(|line| {
            let value = line.strip_prefix("MemTotal:")?.trim();
            let kibibytes = value.strip_suffix("kB")?.trim().parse::<u64>().ok()?;
            kibibytes.checked_mul(1024)
        })
        .unwrap_or(0)
}

/// Result of a server-side conversion.
#[derive(Debug)]
pub struct ServerConversionResult {
    pub name: String,
    pub version: String,
    pub source_profile: Option<String>,
    pub transport: conary_core::ccs::CcsTransportEnvelopeV1,
    pub total_size: u64,
    pub content_hash: String,
    pub ccs_path: PathBuf,
    pub cache_state: String,
    pub scriptlets: ScriptletPackageMetadata,
    pub timing: Option<ConversionTimingReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptletPackageMetadata {
    pub scriptlet_fidelity: String,
    pub evidence_digest: Option<String>,
}

impl From<&ScriptletBundleSummary> for ScriptletPackageMetadata {
    fn from(summary: &ScriptletBundleSummary) -> Self {
        Self {
            scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
            evidence_digest: summary.evidence_digest.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use std::time::Duration;

    fn test_transport() -> conary_core::ccs::CcsTransportEnvelopeV1 {
        conary_core::ccs::CcsTransportEnvelopeV1 {
            schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: String::new(),
            signature_json: "{}".to_string(),
            debug_toml_base64: None,
            build_attestation_json: None,
            foreign_conversion_boundary_json: None,
            objects: Vec::new(),
        }
    }

    #[test]
    fn server_conversion_result_can_carry_timing_report() {
        use crate::server::conversion_timing::{ConversionPhase, ConversionTimingReport};

        let mut timing = ConversionTimingReport::new("fedora", "nginx", None);
        timing.record(ConversionPhase::PackageLookup, Duration::from_millis(7));
        timing.finish(true);

        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
            source_profile: Some("fedora-44".to_string()),
            transport: test_transport(),
            total_size: 0,
            content_hash: "sha256:test".to_string(),
            ccs_path: PathBuf::from("/tmp/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            timing: Some(timing),
        };

        assert_eq!(result.timing.unwrap().phases[0].duration_ms, 7);
    }

    #[test]
    fn benchmark_environment_captures_reproducibility_identity() {
        let executable = std::env::current_exe().unwrap();
        let environment = ConversionBenchmarkEnvironment::capture(
            "fixture-runner".to_string(),
            &executable,
            "a".repeat(64),
            Vec::new(),
        );
        assert_eq!(environment.hardware_label, "fixture-runner");
        assert_eq!(environment.remi_version, env!("CARGO_PKG_VERSION"));
        assert!(!environment.source_commit.is_empty());
        assert!(environment.logical_cpus > 0);
    }

    #[test]
    fn test_server_conversion_result_debug() {
        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            source_profile: Some("fedora-44".to_string()),
            transport: test_transport(),
            total_size: 1024,
            content_hash: "sha256:deadbeef".to_string(),
            ccs_path: PathBuf::from("/data/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            timing: None,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("nginx"));
    }
}
