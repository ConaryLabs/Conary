// apps/remi/src/server/conversion/types.rs
//! Public DTOs emitted by Remi conversion workflows.

use crate::server::conversion_timing::ConversionTimingReport;
use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Result of a server-side conversion.
#[derive(Debug)]
pub struct ServerConversionResult {
    pub name: String,
    pub version: String,
    pub distro: String,
    pub chunk_hashes: Vec<String>,
    pub total_size: u64,
    pub content_hash: String,
    pub ccs_path: PathBuf,
    pub cache_state: String,
    pub scriptlets: ScriptletPackageMetadata,
    pub publication: Option<crate::server::publication::PublicationGateReport>,
    pub timing: Option<ConversionTimingReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptletPackageMetadata {
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub publication_status: String,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub decision_counts: ScriptletDecisionCountsSummary,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    pub review_artifact_available: bool,
}

impl From<&ScriptletBundleSummary> for ScriptletPackageMetadata {
    fn from(summary: &ScriptletBundleSummary) -> Self {
        Self {
            scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
            target_compatibility: summary.target_compatibility.clone(),
            publication_status: summary.publication_status.clone(),
            evidence_digest: summary.evidence_digest.clone(),
            curation_evidence_digest: summary.curation_evidence_digest.clone(),
            decision_counts: summary.decision_counts,
            blocked_reason_codes: summary.blocked_reason_codes.clone(),
            review_reason_codes: summary.review_reason_codes.clone(),
            unknown_commands: summary.unknown_commands.clone(),
            blocked_classes: summary.blocked_classes.clone(),
            review_artifact_available: summary.review_artifact_path.is_some(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ConversionBenchmarkEvidence {
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub scan_only: bool,
    pub cache_state: String,
    pub r2_configured: bool,
    pub timing: Option<ConversionTimingReport>,
    pub scriptlet_summary: Option<crate::server::scriptlet_corpus::ScriptletCorpusSummary>,
    pub converted: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::jobs::JobStatus;
    use crate::server::publication::ServerConversionOutcome;
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use std::time::Duration;

    #[test]
    fn server_conversion_result_can_carry_timing_report() {
        use crate::server::conversion_timing::{ConversionPhase, ConversionTimingReport};

        let mut timing = ConversionTimingReport::new("fedora", "nginx", None);
        timing.record(ConversionPhase::PackageLookup, Duration::from_millis(7));
        timing.finish(true);

        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: vec![],
            total_size: 0,
            content_hash: "sha256:test".to_string(),
            ccs_path: PathBuf::from("/tmp/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: Some(timing),
        };

        assert_eq!(result.timing.unwrap().phases[0].duration_ms, 7);
    }

    #[test]
    fn server_conversion_outcome_reports_terminal_state() {
        let result = ServerConversionResult {
            name: "pkg".to_string(),
            version: "1.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: Vec::new(),
            total_size: 0,
            content_hash: "sha256:test".to_string(),
            ccs_path: PathBuf::from("/tmp/pkg.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        };

        assert!(matches!(
            ServerConversionOutcome::Ready(result).job_status(),
            JobStatus::Ready
        ));
    }

    #[test]
    fn test_server_conversion_result_debug() {
        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: vec!["abc123".to_string()],
            total_size: 1024,
            content_hash: "sha256:deadbeef".to_string(),
            ccs_path: PathBuf::from("/data/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("nginx"));
    }
}
