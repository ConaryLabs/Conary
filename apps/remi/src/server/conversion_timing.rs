// apps/remi/src/server/conversion_timing.rs
//! Timing evidence for Remi package conversion.

use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionPhase {
    PackageLookup,
    Download,
    Checksum,
    CacheLookup,
    ArchiveExtraction,
    NativeMetadataExtraction,
    Capture,
    AdapterDispatch,
    Chunking,
    CasWrite,
    R2WriteThrough,
    Persistence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionPhaseTiming {
    pub phase: ConversionPhase,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionSkippedPhase {
    pub phase: ConversionPhase,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionTimingReport {
    pub distro: String,
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub phases: Vec<ConversionPhaseTiming>,
    pub skipped_phases: Vec<ConversionSkippedPhase>,
    pub total_ms: u128,
    pub success: bool,
    #[serde(skip)]
    started_at: Instant,
}

impl ConversionTimingReport {
    pub fn new(distro: &str, package: &str, version: Option<&str>) -> Self {
        Self {
            distro: distro.to_string(),
            package: package.to_string(),
            version: version.map(ToString::to_string),
            phases: Vec::new(),
            skipped_phases: Vec::new(),
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
