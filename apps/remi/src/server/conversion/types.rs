// apps/remi/src/server/conversion/types.rs
//! Public DTOs emitted by Remi conversion workflows.

use crate::server::conversion_timing::ConversionTimingReport;
use conary_core::ccs::convert::ScriptletBundleSummary;
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

#[derive(Debug, Serialize)]
pub struct ConversionBenchmarkEvidence {
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub cache_state: String,
    pub r2_configured: bool,
    pub timing: Option<ConversionTimingReport>,
    pub converted: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
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
            timing: Some(timing),
        };

        assert_eq!(result.timing.unwrap().phases[0].duration_ms, 7);
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
            timing: None,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("nginx"));
    }
}
