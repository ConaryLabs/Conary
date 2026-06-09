// conary-core/src/ccs/convert/scriptlet_bundle/types.rs

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
