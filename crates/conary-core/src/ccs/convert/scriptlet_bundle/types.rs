// conary-core/src/ccs/convert/scriptlet_bundle/types.rs

use crate::ccs::native_lifecycle::NativeLifecycleBundle;
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
    pub conversion_tool: &'a str,
    pub conversion_tool_version: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptletBundleBuild {
    pub bundle: NativeLifecycleBundle,
    pub summary: ScriptletBundleSummary,
}

/// Internal conversion summary. Do not serialize directly in public API responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptletBundleSummary {
    pub scriptlet_fidelity: String,
    pub evidence_digest: Option<String>,
}

impl Default for ScriptletBundleSummary {
    fn default() -> Self {
        Self {
            scriptlet_fidelity: "native-free".to_string(),
            evidence_digest: None,
        }
    }
}
