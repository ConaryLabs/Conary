// conary-core/src/ccs/convert/scriptlet_bundle/digest.rs

use super::types::ScriptletBundleInput;
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::NativeScriptletSupport;
use std::collections::BTreeSet;

pub(super) fn evidence_digest(
    bundle: &LegacyScriptletBundle,
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<String> {
    let digest_doc = serde_json::json!({
        "schema": "conary-scriptlet-evidence-v1",
        "source_format": bundle.source_format.as_str(),
        "source_distro": bundle.source_distro.as_deref(),
        "source_release": bundle.source_release.as_deref(),
        "source_arch": bundle.source_arch.as_deref(),
        "source_package": &bundle.source_package,
        "source_version": &bundle.source_version,
        "source_checksum": bundle.source_checksum.as_deref(),
        "native_entries": sorted_native_digest_entries(input.source_metadata),
        "flat_entries": sorted_flat_digest_entries(input.source_metadata),
        "classification_counts": {
            "known": input.classification.known_count,
            "unknown": input.classification.unknown_count,
            "review": input.classification.review_count,
            "blocked": input.classification.blocked_count,
        },
        "classification_reasons": sorted_classification_reasons(input.classification),
        "classification_evidence": sorted_classification_evidence(input.classification),
        "entry_decisions": sorted_entry_decision_digest(bundle),
        "decision_counts": {
            "replaced": bundle.decision_counts.replaced,
            "legacy": bundle.decision_counts.legacy,
            "blocked": bundle.decision_counts.blocked,
            "review": bundle.decision_counts.review,
        },
        "scriptlet_fidelity": bundle.scriptlet_fidelity.as_str(),
        "target_compatibility": bundle.target_compatibility.as_str(),
        "publication_status": bundle.publication_status.as_str(),
    });
    let canonical = crate::json::canonical_json(&digest_doc)
        .map_err(|error| anyhow::anyhow!("failed to canonicalize scriptlet evidence: {error}"))?;
    let mut bytes = b"conary-scriptlet-evidence-v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(crate::hash::sha256_prefixed(&bytes))
}

fn sorted_native_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    let mut entries = metadata
        .native_scriptlet_abi
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "slot": &entry.native_slot,
                "body_sha256": &entry.body.sha256,
                "support": native_support_digest(&entry.support),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    entries
}

fn sorted_flat_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    if !metadata.native_scriptlet_abi.is_empty() {
        return Vec::new();
    }
    metadata
        .scriptlets
        .iter()
        .enumerate()
        .map(|(index, scriptlet)| {
            serde_json::json!({
                "id": format!("scriptlet:{index}:{}", scriptlet.phase),
                "phase": scriptlet.phase.to_string(),
                "body_sha256": crate::hash::sha256_prefixed(scriptlet.content.as_bytes()),
            })
        })
        .collect()
}

fn sorted_classification_reasons(report: &ScriptletClassificationReport) -> Vec<String> {
    report
        .entries
        .iter()
        .map(|entry| match &entry.classification {
            ScriptletClassification::Known { reason_code, .. }
            | ScriptletClassification::Unknown { reason_code, .. }
            | ScriptletClassification::Review { reason_code, .. }
            | ScriptletClassification::Blocked { reason_code, .. } => reason_code.clone(),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn sorted_classification_evidence(
    report: &ScriptletClassificationReport,
) -> Vec<serde_json::Value> {
    let mut values = report
        .entries
        .iter()
        .map(|entry| match &entry.classification {
            ScriptletClassification::Known {
                reason_code,
                effects,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "known",
                "reason_code": reason_code,
                "effects": sorted_effect_digest(effects),
            }),
            ScriptletClassification::Unknown {
                command,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "unknown",
                "command": command,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Review {
                class_id,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "review",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Blocked {
                class_id,
                reason_code,
            } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "blocked",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["entry_id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["entry_id"].as_str().unwrap_or_default())
            .then_with(|| {
                left["outcome"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["outcome"].as_str().unwrap_or_default())
            })
            .then_with(|| {
                left["reason_code"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["reason_code"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_effect_digest(effects: &[ScriptletEffectEvidence]) -> Vec<serde_json::Value> {
    let mut values = effects
        .iter()
        .map(|effect| {
            serde_json::json!({
                "kind": &effect.kind,
                "replacement": effect.replacement.as_str(),
                "adapter_id": effect.adapter_id.as_deref(),
                "adapter_digest": effect.adapter_digest.as_deref(),
                "reason_code": effect.reason_code.as_deref(),
                "command": effect.command.as_deref(),
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["kind"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["kind"].as_str().unwrap_or_default())
            .then_with(|| {
                left["adapter_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["adapter_id"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_entry_decision_digest(bundle: &LegacyScriptletBundle) -> Vec<serde_json::Value> {
    let mut values = bundle
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "decision": entry.decision.as_str(),
                "reason_code": &entry.reason_code,
                "body_sha256": &entry.body_sha256,
                "unknown_commands": &entry.unknown_commands,
                "blocked_classes": &entry.blocked_classes,
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    values
}

fn native_support_digest(support: &NativeScriptletSupport) -> serde_json::Value {
    match support {
        NativeScriptletSupport::Parsed => serde_json::json!({"status": "parsed"}),
        NativeScriptletSupport::DeferredReview { reason_code } => {
            serde_json::json!({"status": "deferred-review", "reason_code": reason_code})
        }
        NativeScriptletSupport::Unpreservable { reason_code } => {
            serde_json::json!({"status": "unpreservable", "reason_code": reason_code})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        bundle_for_metadata, complete_effect, known_report_with_effect, package_metadata,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::EffectReplacement;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    #[test]
    fn digest_changes_when_classification_evidence_changes() {
        let mut metadata = package_metadata("digest", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "ldconfig\n".to_string(),
            flags: None,
        });
        let files = Vec::new();

        let base = bundle_for_metadata(
            &metadata,
            &files,
            &known_report_with_effect(complete_effect("dynamic-linker-cache", "ldconfig")),
        )
        .unwrap()
        .bundle
        .evidence_digest;
        let mut different_adapter = complete_effect("dynamic-linker-cache", "ldconfig");
        different_adapter.adapter_digest = Some(crate::hash::sha256_prefixed(b"different"));
        let adapter_digest = bundle_for_metadata(
            &metadata,
            &files,
            &known_report_with_effect(different_adapter),
        )
        .unwrap()
        .bundle
        .evidence_digest;
        let mut partial = complete_effect("dynamic-linker-cache", "ldconfig");
        partial.replacement = EffectReplacement::Partial;
        let replacement_digest =
            bundle_for_metadata(&metadata, &files, &known_report_with_effect(partial))
                .unwrap()
                .bundle
                .evidence_digest;
        let mut unknown = ScriptletClassificationReport::default();
        unknown.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );
        let unknown_digest = bundle_for_metadata(&metadata, &files, &unknown)
            .unwrap()
            .bundle
            .evidence_digest;
        let mut blocked = ScriptletClassificationReport::default();
        blocked.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
            },
        );
        let blocked_digest = bundle_for_metadata(&metadata, &files, &blocked)
            .unwrap()
            .bundle
            .evidence_digest;

        assert_ne!(base, adapter_digest);
        assert_ne!(base, replacement_digest);
        assert_ne!(base, unknown_digest);
        assert_ne!(base, blocked_digest);
    }
}
