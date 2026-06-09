// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod classification;
mod format_metadata;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};

use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry,
    NativeInvocation, SourceFormat, VersionScheme,
};
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::{NativeScriptletEntry, NativeScriptletSupport};
use crate::packages::traits::Scriptlet;
use std::collections::{BTreeMap, BTreeSet};

use classification::{classification_entries_for, classify_entry};
use format_metadata::project_format_metadata;
use native_contracts::{
    encoded_native_body, flat_transaction_order, native_invocation, native_lifecycle_paths,
    native_scriptlet_kind, native_transaction_order, non_empty_or_default,
    phase_from_native_lifecycle, phase_from_scriptlet_phase,
};
use summary::{aggregate_status, decision_counts, summary_from_bundle};

pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let format = source_format(input.source_format)?;
    let source_distro = input.source_distro.unwrap_or("unknown").to_string();
    let source_release = input.source_release.unwrap_or("unknown").to_string();
    let source_arch = input
        .source_arch
        .or(input.source_metadata.architecture.as_deref())
        .unwrap_or("unknown")
        .to_string();
    let source_checksum = input
        .source_checksum
        .filter(|checksum| valid_prefixed_sha256(checksum))
        .map(str::to_string);

    let entries = build_entries(&input)?;
    let decision_counts = decision_counts(&entries);
    let (scriptlet_fidelity, target_compatibility, publication_policy, publication_status) =
        aggregate_status(&entries, &decision_counts);

    let mut bundle = LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: format.clone(),
        source_family: source_family(&format).to_string(),
        source_distro: Some(source_distro),
        source_release: Some(source_release),
        source_arch: Some(source_arch),
        source_package: input.source_metadata.name.clone(),
        source_version: input.source_metadata.version.clone(),
        source_checksum,
        version_scheme: version_scheme(&format),
        conversion_tool: input.conversion_tool.to_string(),
        conversion_tool_version: input.conversion_tool_version.to_string(),
        conversion_policy: "passive-scriptlet-bundle-goal4".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: None,
        target_compatibility,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy,
        publication_status,
        scriptlet_fidelity,
        decision_counts,
        unsupported_class_counts: input.classification.unsupported_class_counts.clone(),
        entries,
        extra: BTreeMap::new(),
    };

    let digest = evidence_digest(&bundle, &input)?;
    bundle.evidence_digest = Some(digest.clone());
    for entry in &mut bundle.entries {
        entry.evidence_digest = Some(digest.clone());
    }
    bundle.validate()?;

    Ok(ScriptletBundleBuild {
        summary: summary_from_bundle(&bundle, Some(digest)),
        bundle,
    })
}

fn source_format(value: &str) -> anyhow::Result<SourceFormat> {
    match value {
        "rpm" => Ok(SourceFormat::Rpm),
        "deb" => Ok(SourceFormat::Deb),
        "arch" => Ok(SourceFormat::Arch),
        other => anyhow::bail!("unsupported scriptlet source format '{other}'"),
    }
}

fn source_family(format: &SourceFormat) -> &'static str {
    match format {
        SourceFormat::Rpm => "rpm",
        SourceFormat::Deb => "deb",
        SourceFormat::Arch => "arch",
        SourceFormat::Unknown(_) => "unknown",
    }
}

fn version_scheme(format: &SourceFormat) -> VersionScheme {
    match format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
        SourceFormat::Unknown(_) => VersionScheme::Semver,
    }
}

fn valid_prefixed_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn build_entries(input: &ScriptletBundleInput<'_>) -> anyhow::Result<Vec<LegacyScriptletEntry>> {
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

fn evidence_digest(
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
    use super::test_support::{
        bundle_for_metadata, complete_effect, known_report_with_effect, native_entry_with_body,
        package_metadata,
    };
    use super::*;
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        EffectReplacement, ForeignReplayPolicy, PublicationPolicy, PublicationStatus,
        ScriptletDecision, ScriptletFidelity, TargetCompatibility,
    };
    use crate::packages::native_abi::NativeScriptletSupport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    #[test]
    fn native_free_input_builds_zero_entry_bundle() {
        let metadata = package_metadata("native-free", "1.0");
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();

        let build = build_legacy_scriptlet_bundle(ScriptletBundleInput {
            source_metadata: &metadata,
            final_metadata: &metadata,
            source_files: &files,
            final_files: &files,
            source_format: "rpm",
            source_distro: Some("fedora-44"),
            source_release: Some("44"),
            source_arch: Some("x86_64"),
            source_checksum: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            classification: &classification,
            conversion_tool: "remi",
            conversion_tool_version: "0.1.0",
        })
        .unwrap();

        assert!(build.bundle.entries.is_empty());
        assert_eq!(build.bundle.scriptlet_fidelity.as_str(), "native-free");
        assert_eq!(
            build.bundle.target_compatibility.as_str(),
            "conary-portable"
        );
        assert_eq!(
            build.bundle.publication_policy.as_str(),
            "public-if-no-blocked"
        );
        assert_eq!(build.bundle.publication_status.as_str(), "public");
        assert_eq!(build.bundle.decision_counts.total(), 0);
        assert_eq!(build.summary.scriptlet_fidelity, "native-free");
        assert_eq!(build.summary.target_compatibility, "conary-portable");
        assert_eq!(build.summary.publication_status, "public");
        assert!(
            build
                .summary
                .evidence_digest
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        build.bundle.validate().unwrap();
    }

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
    fn tampered_body_after_build_fails_strict_bundle_validation() {
        let mut metadata = package_metadata("tamper", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PreInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo ok\n".to_string(),
            flags: None,
        });
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();
        let mut build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

        build.bundle.entries[0].body.push_str("tampered\n");

        assert!(build.bundle.validate().is_err());
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
