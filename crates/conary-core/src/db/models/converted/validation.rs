// conary-core/src/db/models/converted/validation.rs

use crate::ccs::convert::ScriptletBundleSummary;
use crate::error::Result;

pub(super) fn bare_sha256(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
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
