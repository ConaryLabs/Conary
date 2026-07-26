// conary-core/src/ccs/convert/scriptlet_bundle/summary.rs

use super::types::ScriptletBundleSummary;
use crate::ccs::native_lifecycle::NativeLifecycleBundle;

pub(super) fn summary_from_bundle(bundle: &NativeLifecycleBundle) -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
        evidence_digest: bundle.evidence_digest.clone(),
    }
}

impl ScriptletBundleSummary {
    pub fn from_bundle(bundle: &NativeLifecycleBundle, digest: Option<String>) -> Self {
        Self {
            scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
            evidence_digest: digest.or_else(|| bundle.evidence_digest.clone()),
        }
    }
}
