// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

use crate::ccs::convert::effects::ScriptletClassificationReport;
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::ExtractedFile;
use serde::{Deserialize, Serialize};

pub struct ScriptletBundleInput<'a> {
    pub source_metadata: &'a PackageMetadata,
    pub final_metadata: &'a PackageMetadata,
    pub source_files: &'a [ExtractedFile],
    pub final_files: &'a [ExtractedFile],
    pub source_format: &'a str,
    pub source_distro: Option<&'a str>,
    pub source_release: Option<&'a str>,
    pub source_arch: Option<&'a str>,
    pub source_checksum: Option<&'a str>,
    pub classification: &'a ScriptletClassificationReport,
    pub conversion_tool: &'a str,
    pub conversion_tool_version: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptletBundleBuild {
    pub bundle: LegacyScriptletBundle,
    pub summary: ScriptletBundleSummary,
}

/// Internal conversion summary. Do not serialize directly in public API responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptletBundleSummary {
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
    #[serde(default, skip_serializing)]
    pub review_artifact_path: Option<String>,
}

impl Default for ScriptletBundleSummary {
    fn default() -> Self {
        Self {
            scriptlet_fidelity: "unknown".to_string(),
            target_compatibility: "unknown".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: None,
            curation_evidence_digest: None,
            decision_counts: ScriptletDecisionCountsSummary::default(),
            blocked_reason_codes: Vec::new(),
            review_reason_codes: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            review_artifact_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScriptletDecisionCountsSummary {
    pub replaced: u32,
    pub legacy: u32,
    pub blocked: u32,
    pub review: u32,
}

pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let _ = input;
    anyhow::bail!("legacy scriptlet bundle builder is not wired yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scriptlet_bundle_summary_defaults_match_legacy_rows() {
        let summary = ScriptletBundleSummary::default();

        assert_eq!(summary.scriptlet_fidelity, "unknown");
        assert_eq!(summary.target_compatibility, "unknown");
        assert_eq!(summary.publication_status, "public");
        assert_eq!(summary.evidence_digest, None);
        assert_eq!(summary.curation_evidence_digest, None);
        assert_eq!(
            summary.decision_counts,
            ScriptletDecisionCountsSummary::default()
        );
        assert!(summary.blocked_reason_codes.is_empty());
        assert!(summary.review_reason_codes.is_empty());
        assert!(summary.unknown_commands.is_empty());
        assert!(summary.blocked_classes.is_empty());
        assert_eq!(summary.review_artifact_path, None);
    }

    #[test]
    fn scriptlet_bundle_summary_does_not_serialize_review_artifact_path() {
        let summary = ScriptletBundleSummary {
            review_artifact_path: Some("/tmp/private-review-secret".to_string()),
            ..ScriptletBundleSummary::default()
        };

        let json = serde_json::to_string(&summary).unwrap();

        assert!(!json.contains("review_artifact_path"));
        assert!(!json.contains("private-review-secret"));
    }
}
