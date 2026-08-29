// apps/remi/src/server/conversion_timing.rs
//! Timing evidence for Remi package conversion.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use conary_core::filesystem::VerifiedObjectBatchMetrics;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionSourceIdentity {
    pub source_profile: String,
    pub version: String,
    pub architecture: Option<String>,
    pub checksum: String,
    pub declared_size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionR2Work {
    pub head_requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub put_requests: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionWorkMetrics {
    pub downloaded_bytes: u64,
    pub admitted_local_bytes: u64,
    pub repository_checksum_bytes_hashed: u64,
    pub source_artifact_bytes: u64,
    pub source_bytes_hashed: u64,
    pub ccs_output_bytes: u64,
    pub signed_object_count: u64,
    pub signed_object_bytes: u64,
    pub cas_incoming_bytes_hashed: u64,
    pub cas_persistent_bytes_written: u64,
    pub cas_objects_hashed: u64,
    pub cas_hits: u64,
    pub cas_misses: u64,
    pub cas_race_losers: u64,
    pub cas_object_syncs: u64,
    pub cas_shard_syncs: u64,
    pub cas_root_syncs: u64,
    pub cas_canonical_bytes_reread: u64,
    pub r2: ConversionR2Work,
}

impl ConversionWorkMetrics {
    pub fn record_cas(&mut self, metrics: VerifiedObjectBatchMetrics) {
        self.cas_incoming_bytes_hashed = metrics.incoming_bytes_hashed;
        self.cas_persistent_bytes_written = metrics.persistent_bytes_written;
        self.cas_objects_hashed = metrics.objects_hashed;
        self.cas_hits = metrics.hits;
        self.cas_misses = metrics.misses;
        self.cas_race_losers = metrics.race_losers;
        self.cas_object_syncs = metrics.object_syncs;
        self.cas_shard_syncs = metrics.shard_syncs;
        self.cas_root_syncs = metrics.root_syncs;
        self.cas_canonical_bytes_reread = metrics.canonical_bytes_reread;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionPhase {
    PackageLookup,
    LocalArtifactAdmission,
    Download,
    Checksum,
    CacheLookup,
    ArchiveExtraction,
    NativeShellAstExtraction,
    AdapterDispatch,
    CcsEmission,
    TransportVerification,
    CasWrite,
    R2WriteThrough,
    Persistence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionPhaseTiming {
    pub phase: ConversionPhase,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionSkippedPhase {
    pub phase: ConversionPhase,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionTimingReport {
    pub distro: String,
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ConversionSourceIdentity>,
    pub phases: Vec<ConversionPhaseTiming>,
    pub skipped_phases: Vec<ConversionSkippedPhase>,
    pub work: ConversionWorkMetrics,
    pub total_ms: u128,
    pub success: bool,
    #[serde(skip, default = "Instant::now")]
    started_at: Instant,
}

impl ConversionTimingReport {
    pub fn new(distro: &str, package: &str, version: Option<&str>) -> Self {
        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(ToString::to_string),
            source: None,
            phases: Vec::new(),
            skipped_phases: Vec::new(),
            work: ConversionWorkMetrics::default(),
            total_ms: 0,
            success: false,
            started_at: Instant::now(),
        }
    }

    pub fn record(&mut self, phase: ConversionPhase, duration: Duration) {
        self.phases.push(ConversionPhaseTiming {
            phase,
            duration_ms: duration.as_millis(),
        });
    }

    pub fn record_skipped(&mut self, phase: ConversionPhase, reason: impl Into<String>) {
        self.skipped_phases.push(ConversionSkippedPhase {
            phase,
            reason: reason.into(),
        });
    }

    pub fn finish(&mut self, success: bool) {
        self.success = success;
        self.total_ms = self.started_at.elapsed().as_millis();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn timing_report_serializes_phase_durations() {
        let mut report = ConversionTimingReport::new("fedora", "nginx", Some("1.28.0"));
        report.record(ConversionPhase::PackageLookup, Duration::from_millis(11));
        report.record(ConversionPhase::Download, Duration::from_millis(22));
        report.record_skipped(ConversionPhase::R2WriteThrough, "r2 store not configured");
        report.finish(true);

        let value = serde_json::to_value(&report).expect("timing report serializes");
        assert_eq!(value["distro"], json!("fedora"));
        assert_eq!(value["package"], json!("nginx"));
        assert_eq!(value["version"], json!("1.28.0"));
        assert_eq!(value["success"], json!(true));
        assert_eq!(value["work"]["downloaded_bytes"], json!(0));
        assert_eq!(value["phases"][0]["phase"], json!("package_lookup"));
        assert_eq!(value["phases"][0]["duration_ms"], json!(11));
        assert_eq!(value["phases"][1]["phase"], json!("download"));
        assert_eq!(value["phases"][1]["duration_ms"], json!(22));
        assert_eq!(
            value["skipped_phases"][0]["phase"],
            json!("r2_write_through")
        );
        assert_eq!(
            value["skipped_phases"][0]["reason"],
            json!("r2 store not configured")
        );
    }
}
