// crates/conary-core/src/db/models/converted/validation.rs

use crate::ccs::convert::ScriptletBundleSummary;
use crate::error::Result;

pub(super) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if !crate::hash::is_canonical_sha256(value) {
        return Err(crate::Error::ConfigError(format!(
            "{label} must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

pub(super) fn default_scriptlet_summary_json() -> String {
    serde_json::to_string(&ScriptletBundleSummary::default())
        .expect("default lifecycle summary must serialize")
}

pub(super) fn validate_current_scriptlet_summary(summary: &ScriptletBundleSummary) -> Result<()> {
    if !matches!(
        summary.scriptlet_fidelity.as_str(),
        "native-free" | "native-lifecycle"
    ) {
        return Err(crate::Error::InternalError(format!(
            "unsupported current lifecycle fidelity {:?}",
            summary.scriptlet_fidelity
        )));
    }
    Ok(())
}
