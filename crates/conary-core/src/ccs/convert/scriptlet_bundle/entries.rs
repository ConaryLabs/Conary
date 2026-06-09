// conary-core/src/ccs/convert/scriptlet_bundle/entries.rs

use super::classification::{classification_entries_for, classify_entry};
use super::format_metadata::project_format_metadata;
use super::native_contracts::{
    encoded_native_body, flat_transaction_order, native_invocation, native_lifecycle_paths,
    native_scriptlet_kind, native_transaction_order, non_empty_or_default,
    phase_from_native_lifecycle, phase_from_scriptlet_phase,
};
use super::types::ScriptletBundleInput;
use crate::ccs::convert::effects::ScriptletClassificationReport;
use crate::ccs::legacy_scriptlets::{LegacyScriptletEntry, NativeInvocation};
use crate::packages::native_abi::{NativeScriptletEntry, NativeScriptletSupport};
use crate::packages::traits::Scriptlet;
use std::collections::BTreeMap;

pub(super) fn build_entries(
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<Vec<LegacyScriptletEntry>> {
    if !input.source_metadata.native_scriptlet_abi.is_empty() {
        input
            .source_metadata
            .native_scriptlet_abi
            .iter()
            .map(|entry| build_native_entry(entry, input.classification))
            .collect()
    } else {
        input
            .source_metadata
            .scriptlets
            .iter()
            .enumerate()
            .map(|(index, scriptlet)| build_flat_entry(index, scriptlet, input.classification))
            .collect()
    }
}

fn build_flat_entry(
    index: usize,
    scriptlet: &Scriptlet,
    report: &ScriptletClassificationReport,
) -> anyhow::Result<LegacyScriptletEntry> {
    let id = format!("scriptlet:{index}:{}", scriptlet.phase);
    let phase = phase_from_scriptlet_phase(scriptlet.phase);
    let lifecycle_paths = vec![phase.as_str().to_string()];
    let classifications = classification_entries_for(report, &id);
    let outcome = classify_entry(&classifications, &NativeScriptletSupport::Parsed);
    let body_bytes = scriptlet.content.as_bytes();

    Ok(LegacyScriptletEntry {
        id,
        native_slot: scriptlet.phase.to_string(),
        phase,
        lifecycle_paths,
        interpreter: non_empty_or_default(&scriptlet.interpreter, "/bin/sh"),
        interpreter_args: scriptlet
            .flags
            .as_deref()
            .map(|flags| flags.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
        body_sha256: crate::hash::sha256_prefixed(body_bytes),
        body: scriptlet.content.clone(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: flat_transaction_order(scriptlet.phase),
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: outcome.decision,
        reason_code: outcome.reason_code,
        human_reason: None,
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: outcome.effects,
        unknown_commands: outcome.unknown_commands,
        blocked_classes: outcome.blocked_classes,
        rpm_trigger: None,
        deb_maintainer: None,
        arch_install: None,
        residual_replay: None,
        extra: BTreeMap::new(),
    })
}

fn build_native_entry(
    native: &NativeScriptletEntry,
    report: &ScriptletClassificationReport,
) -> anyhow::Result<LegacyScriptletEntry> {
    let classifications = classification_entries_for(report, &native.id);
    let outcome = classify_entry(&classifications, &native.support);
    let phase = phase_from_native_lifecycle(native.primary_lifecycle);
    let lifecycle_paths = native_lifecycle_paths(native);
    let (body, body_encoding) = encoded_native_body(&native.body);
    let mut extra = BTreeMap::from([(
        "native_scriptlet_kind".to_string(),
        toml::Value::String(native_scriptlet_kind(native.kind).to_string()),
    )]);
    let (rpm_trigger, deb_maintainer, arch_install) = project_format_metadata(native, &mut extra);

    Ok(LegacyScriptletEntry {
        id: native.id.clone(),
        native_slot: native.native_slot.clone(),
        phase,
        lifecycle_paths,
        interpreter: native
            .interpreter
            .clone()
            .unwrap_or_else(|| "package-manager-control-artifact".to_string()),
        interpreter_args: native.interpreter_args.clone(),
        body_sha256: native.body.sha256.clone(),
        body,
        body_encoding,
        native_invocation: native_invocation(&native.invocation),
        transaction_order: native_transaction_order(&native.order),
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision: outcome.decision,
        reason_code: outcome.reason_code,
        human_reason: None,
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: outcome.effects,
        unknown_commands: outcome.unknown_commands,
        blocked_classes: outcome.blocked_classes,
        rpm_trigger,
        deb_maintainer,
        arch_install,
        residual_replay: None,
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        bundle_for_metadata, complete_effect, native_entry_with_body, package_metadata,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        ForeignReplayPolicy, PublicationPolicy, PublicationStatus, ScriptletDecision,
        ScriptletFidelity, TargetCompatibility,
    };
    use crate::packages::native_abi::NativeScriptletSupport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    #[test]
    fn flattened_scriptlet_with_complete_effect_builds_replaced_entry() {
        let mut metadata = package_metadata("flat", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "/sbin/ldconfig\n".to_string(),
            flags: None,
        });
        let files = Vec::new();
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Known {
                reason_code: "dynamic-linker-cache-complete".to_string(),
                effects: vec![complete_effect("dynamic-linker-cache", "ldconfig")],
            },
        );

        let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

        assert_eq!(build.bundle.entries.len(), 1);
        let entry = &build.bundle.entries[0];
        assert_eq!(entry.decision.as_str(), "replaced");
        assert_eq!(entry.reason_code, "dynamic-linker-cache-complete");
        assert_eq!(entry.effects.len(), 1);
        assert_eq!(entry.body, "/sbin/ldconfig\n");
        build.bundle.validate().unwrap();
    }

    #[test]
    fn native_abi_binary_body_is_base64_encoded_and_validates() {
        let mut metadata = package_metadata("native-bin", "1.0");
        metadata
            .native_scriptlet_abi
            .push(native_entry_with_body(vec![0xff, 0x00, 0x01]));
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();

        let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.body_encoding.as_deref(), Some("base64"));
        assert_eq!(
            entry.body_sha256,
            crate::hash::sha256_prefixed(&[0xff, 0x00, 0x01])
        );
        build.bundle.validate().unwrap();
    }

    #[test]
    fn unknown_classification_becomes_source_native_legacy_replay_entry() {
        let mut metadata = package_metadata("unknown", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "custom-helper --do-thing\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Legacy);
        assert_eq!(entry.reason_code, "unknown-command");
        assert_eq!(entry.unknown_commands, vec!["custom-helper"]);
        assert_eq!(build.bundle.decision_counts.legacy, 1);
        assert_eq!(
            build.bundle.scriptlet_fidelity,
            ScriptletFidelity::LegacyReplay
        );
        assert_eq!(
            build.bundle.target_compatibility,
            TargetCompatibility::SourceNative
        );
        assert_eq!(
            build.bundle.foreign_replay_policy,
            ForeignReplayPolicy::Deny
        );
        assert_eq!(
            build.bundle.publication_policy,
            PublicationPolicy::LocalOnly
        );
        assert_eq!(
            build.bundle.publication_status,
            PublicationStatus::LocalOnly
        );
        assert_ne!(build.bundle.publication_status, PublicationStatus::Public);
        assert_eq!(build.summary.scriptlet_fidelity, "legacy-replay");
        assert_eq!(build.summary.target_compatibility, "source-native");
        assert_eq!(build.summary.publication_status, "local-only");
        assert_eq!(build.summary.decision_counts.legacy, 1);
        assert_eq!(build.summary.unknown_commands, vec!["custom-helper"]);
    }

    #[test]
    fn review_classification_becomes_private_review_entry() {
        let mut metadata = package_metadata("review", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "systemctl restart demo.service\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Review {
                reason_code: "review-class-systemd-runtime-action".to_string(),
                class_id: Some("systemd-runtime-action".to_string()),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Review);
        assert_eq!(entry.reason_code, "review-class-systemd-runtime-action");
        assert_eq!(build.bundle.decision_counts.review, 1);
        assert_eq!(
            build.bundle.scriptlet_fidelity,
            ScriptletFidelity::ReviewRequired
        );
        assert_eq!(
            build.bundle.target_compatibility,
            TargetCompatibility::ReviewRequired
        );
        assert_eq!(
            build.bundle.publication_status,
            PublicationStatus::PrivateReview
        );
        assert_eq!(
            build.summary.review_reason_codes,
            vec!["review-class-systemd-runtime-action"]
        );
    }

    #[test]
    fn blocked_classification_becomes_blocked_entry() {
        let mut metadata = package_metadata("blocked", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "curl https://example.invalid\n".to_string(),
            flags: None,
        });
        let mut classification = ScriptletClassificationReport::default();
        classification.push(
            "scriptlet:0:post-install",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
            },
        );

        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();
        let entry = &build.bundle.entries[0];

        assert_eq!(entry.decision, ScriptletDecision::Blocked);
        assert_eq!(entry.reason_code, "blocked-class-network");
        assert_eq!(entry.blocked_classes, vec!["network"]);
        assert_eq!(
            build.summary.blocked_reason_codes,
            vec!["blocked-class-network"]
        );
        assert_eq!(build.summary.blocked_classes, vec!["network"]);
        assert_eq!(build.summary.publication_status, "blocked");
    }

    #[test]
    fn native_deferred_and_unpreservable_support_drive_decisions() {
        let mut metadata = package_metadata("native-support", "1.0");
        let mut deferred = native_entry_with_body(b"echo deferred\n".to_vec());
        deferred.id = "rpm:%verify".to_string();
        deferred.native_slot = "%verify".to_string();
        deferred.support = NativeScriptletSupport::DeferredReview {
            reason_code: "rpm-verify-scriptlet-deferred".to_string(),
        };
        let mut unpreservable = native_entry_with_body(b"echo nope\n".to_vec());
        unpreservable.id = "rpm:%postun".to_string();
        unpreservable.native_slot = "%postun".to_string();
        unpreservable.support = NativeScriptletSupport::Unpreservable {
            reason_code: "native-abi-parser-limitation".to_string(),
        };
        metadata.native_scriptlet_abi = vec![deferred, unpreservable];

        let build =
            bundle_for_metadata(&metadata, &[], &ScriptletClassificationReport::default()).unwrap();

        let deferred = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "rpm:%verify")
            .unwrap();
        let unpreservable = build
            .bundle
            .entries
            .iter()
            .find(|entry| entry.id == "rpm:%postun")
            .unwrap();
        assert_eq!(deferred.decision, ScriptletDecision::Review);
        assert_eq!(deferred.reason_code, "rpm-verify-scriptlet-deferred");
        assert_eq!(unpreservable.decision, ScriptletDecision::Blocked);
        assert_eq!(unpreservable.reason_code, "native-abi-parser-limitation");
    }
}
