// conary-core/src/ccs/convert/scriptlet_bundle/digest.rs

use crate::ccs::native_lifecycle::NativeLifecycleBundle;

/// Hash the exact typed lifecycle authority, excluding the digest fields that
/// receive the result.
pub(super) fn evidence_digest(bundle: &NativeLifecycleBundle) -> anyhow::Result<String> {
    let mut authority = bundle.clone();
    authority.evidence_digest = None;
    for entry in &mut authority.entries {
        entry.evidence_digest = None;
    }
    let document = serde_json::json!({
        "schema": "conary-native-lifecycle-authority-v1",
        "authority": authority,
    });
    let canonical = crate::json::canonical_json(&document)
        .map_err(|error| anyhow::anyhow!("failed to canonicalize lifecycle authority: {error}"))?;
    let mut bytes = b"conary-native-lifecycle-authority-v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(crate::hash::sha256_prefixed(&bytes))
}
