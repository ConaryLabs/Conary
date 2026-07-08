// conary-core/src/ccs/convert/scriptlet_bundle/summary.rs

use super::super::public_policy;
use super::types::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use crate::ccs::legacy_scriptlets::{
    DecisionCounts, LegacyScriptletBundle, LegacyScriptletEntry, PublicationPolicy,
    PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
};
use crate::ccs::v2::validation::TargetProfileQuery;
use crate::ccs::security_policy::SecurityPolicyIntent;
use std::collections::BTreeSet;

impl ScriptletBundleSummary {
    pub fn from_bundle(bundle: &LegacyScriptletBundle, evidence_digest: Option<String>) -> Self {
        summary_from_bundle(bundle, evidence_digest)
    }
}

pub(super) fn summary_from_bundle(
    bundle: &LegacyScriptletBundle,
    evidence_digest: Option<String>,
) -> ScriptletBundleSummary {
    let blocked_reason_codes = sorted_entry_reason_codes(bundle, "blocked");
    let target_profile = public_policy_target_profile(bundle);
    let target_profile_query =
        target_profile.map(|profile| profile as &dyn TargetProfileQuery);
    let mut review_reason_codes = sorted_entry_reason_codes(bundle, "review");
    review_reason_codes.extend(public_policy_review_reason_codes(
        bundle,
        target_profile_query,
    ));
    review_reason_codes.sort();
    review_reason_codes.dedup();
    let unknown_commands = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.unknown_commands.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let blocked_classes = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.blocked_classes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let boot_security_intents = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.boot_security_intents.iter().cloned())
        .collect::<Vec<_>>();
    let security_policy_intents = merged_security_policy_intents(bundle);
    let recomputed_counts = decision_counts(&bundle.entries);
    let (scriptlet_fidelity, target_compatibility, _publication_policy, publication_status) =
        if bundle.entries.is_empty() {
            (
                bundle.scriptlet_fidelity.clone(),
                bundle.target_compatibility.clone(),
                bundle.publication_policy.clone(),
                bundle.publication_status.clone(),
            )
        } else {
            aggregate_status(&bundle.entries, &recomputed_counts, target_profile_query)
        };

    ScriptletBundleSummary {
        scriptlet_fidelity: scriptlet_fidelity.as_str().to_string(),
        target_compatibility: target_compatibility.as_str().to_string(),
        publication_status: publication_status.as_str().to_string(),
        evidence_digest,
        curation_evidence_digest: None,
        decision_counts: ScriptletDecisionCountsSummary {
            replaced: recomputed_counts.replaced,
            legacy: recomputed_counts.legacy,
            blocked: recomputed_counts.blocked,
            review: recomputed_counts.review,
        },
        blocked_reason_codes,
        review_reason_codes,
        unknown_commands,
        blocked_classes,
        boot_security_intents,
        security_policy_intents,
        review_artifact_path: None,
    }
}

fn merged_security_policy_intents(bundle: &LegacyScriptletBundle) -> Vec<SecurityPolicyIntent> {
    let mut intents = bundle.security_policy_intents.clone();
    for intent in bundle
        .entries
        .iter()
        .flat_map(|entry| entry.security_policy_intents.iter())
    {
        if !intents.contains(intent) {
            intents.push(intent.clone());
        }
    }
    intents
}

fn sorted_entry_reason_codes(bundle: &LegacyScriptletBundle, decision: &str) -> Vec<String> {
    bundle
        .entries
        .iter()
        .filter(|entry| entry.decision.as_str() == decision)
        .map(|entry| entry.reason_code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn public_policy_target_profile(
    bundle: &LegacyScriptletBundle,
) -> Option<&'static crate::repository::supported_profiles::SupportedProfile> {
    bundle
        .extra
        .get("public_policy_target_profile_id")
        .and_then(toml::Value::as_str)
        .and_then(crate::repository::supported_profiles::profile_by_public_id)
}

fn public_policy_review_reason_codes(
    bundle: &LegacyScriptletBundle,
    profile: Option<&dyn TargetProfileQuery>,
) -> Vec<String> {
    bundle
        .entries
        .iter()
        .flat_map(|entry| public_policy::entry_public_policy_review_reasons(entry, profile))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn decision_counts(entries: &[LegacyScriptletEntry]) -> DecisionCounts {
    let mut counts = DecisionCounts::default();
    for entry in entries {
        match entry.decision {
            ScriptletDecision::Replaced => counts.replaced += 1,
            ScriptletDecision::Legacy => counts.legacy += 1,
            ScriptletDecision::Blocked => counts.blocked += 1,
            ScriptletDecision::Review => counts.review += 1,
            ScriptletDecision::Unknown(_) => {}
        }
    }
    counts
}

pub(super) fn aggregate_status(
    entries: &[LegacyScriptletEntry],
    counts: &DecisionCounts,
    profile: Option<&dyn TargetProfileQuery>,
) -> (
    ScriptletFidelity,
    TargetCompatibility,
    PublicationPolicy,
    PublicationStatus,
) {
    if entries.is_empty() {
        return (
            ScriptletFidelity::NativeFree,
            TargetCompatibility::ConaryPortable,
            PublicationPolicy::PublicIfNoBlocked,
            PublicationStatus::Public,
        );
    }
    if counts.blocked > 0 {
        return (
            ScriptletFidelity::Blocked,
            TargetCompatibility::Blocked,
            PublicationPolicy::Blocked,
            PublicationStatus::Blocked,
        );
    }
    if counts.review > 0 {
        return (
            ScriptletFidelity::ReviewRequired,
            TargetCompatibility::ReviewRequired,
            PublicationPolicy::PrivateReview,
            PublicationStatus::PrivateReview,
        );
    }
    if counts.legacy > 0 {
        return (
            ScriptletFidelity::LegacyReplay,
            TargetCompatibility::SourceNative,
            PublicationPolicy::LocalOnly,
            PublicationStatus::LocalOnly,
        );
    }
    let public_policy_review_required = entries
        .iter()
        .any(|entry| !public_policy::entry_public_policy_review_reasons(entry, profile).is_empty());
    if public_policy_review_required {
        return (
            ScriptletFidelity::FullyReplaced,
            TargetCompatibility::ConaryPortable,
            PublicationPolicy::PrivateReview,
            PublicationStatus::PrivateReview,
        );
    }
    (
        ScriptletFidelity::FullyReplaced,
        TargetCompatibility::ConaryPortable,
        PublicationPolicy::PublicIfNoBlocked,
        PublicationStatus::Public,
    )
}

#[cfg(test)]
mod tests {
    use super::aggregate_status;
    use super::super::test_support::{bundle_for_metadata, package_metadata};
    use super::super::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use crate::ccs::v2::validation::TargetProfileQuery;
    use crate::ccs::convert::effects::ScriptletClassificationReport;
    use crate::ccs::legacy_scriptlets::{
        DecisionCounts, EffectConfidence, EffectReplacement, EffectSource, ForeignReplayPolicy,
        LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, LegacyScriptletEntry, LifecyclePath,
        NativeInvocation, PublicationPolicy, PublicationStatus, ScriptletDecision, ScriptletEffect,
        ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder, VersionScheme,
    };
    use crate::ccs::security_policy::{
        SECURITY_POLICY_INTENT_SCHEMA_V1, SecurityPolicyFallback, SecurityPolicyIntent,
        SecurityPolicyPayloadEvidence, SecurityPolicyProvider, SecurityPolicyReconciliation,
        SecurityPolicyReconciliationState, SecurityPolicyRequirements, SecurityPolicyScope,
        SecurityPolicySource,
    };
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
    use std::collections::BTreeMap;

    fn test_intent(id: &str, operation: &str) -> SecurityPolicyIntent {
        SecurityPolicyIntent {
            schema: SECURITY_POLICY_INTENT_SCHEMA_V1.to_string(),
            id: id.to_string(),
            source: SecurityPolicySource::default(),
            provider: SecurityPolicyProvider::Selinux,
            operation: operation.to_string(),
            scope: SecurityPolicyScope::default(),
            desired_state: BTreeMap::new(),
            requirements: SecurityPolicyRequirements::default(),
            fallback: SecurityPolicyFallback::Dormant,
            payload_evidence: SecurityPolicyPayloadEvidence::default(),
            reconciliation: SecurityPolicyReconciliation {
                state: SecurityPolicyReconciliationState::Pending,
                reason: None,
                target_provider: None,
            },
            extra: BTreeMap::new(),
        }
    }

    fn file_capability_effect(capability: &str) -> ScriptletEffect {
        let mut extra = BTreeMap::new();
        extra.insert(
            "capabilities".to_string(),
            toml::Value::Array(vec![toml::Value::String(capability.to_string())]),
        );

        ScriptletEffect {
            kind: "file-capability".to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Declared,
            replacement: EffectReplacement::Complete,
            adapter_id: Some("file-capability/v1".to_string()),
            adapter_digest: None,
            command: Some("setcap".to_string()),
            args: vec![format!("{capability}=+ep"), "/usr/bin/test".to_string()],
            path: Some("/usr/bin/test".to_string()),
            reason_code: Some("helper-complete-file-capability".to_string()),
            extra,
        }
    }

    fn replaced_file_capability_entry(capability: &str) -> LegacyScriptletEntry {
        let body = format!("setcap {capability}=+ep /usr/bin/test\n");

        LegacyScriptletEntry {
            id: "scriptlet:0:post-install".to_string(),
            native_slot: "%post".to_string(),
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                ..TransactionOrder::default()
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Replaced,
            reason_code: "helper-complete-file-capability".to_string(),
            human_reason: None,
            evidence_digest: Some(crate::hash::sha256_prefixed(
                format!("file-capability:{capability}").as_bytes(),
            )),
            source_evidence_refs: Vec::new(),
            effects: vec![file_capability_effect(capability)],
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            boot_security_intents: Vec::new(),
            security_policy_intents: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn replaced_sysctl_entry(key: &str) -> LegacyScriptletEntry {
        let body = format!("sysctl -w {key}=1\n");
        let mut extra = BTreeMap::new();
        extra.insert("key".to_string(), toml::Value::String(key.to_string()));

        LegacyScriptletEntry {
            id: "scriptlet:0:post-install".to_string(),
            native_slot: "%post".to_string(),
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["post-install".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                ..TransactionOrder::default()
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Replaced,
            reason_code: "helper-complete-sysctl".to_string(),
            human_reason: None,
            evidence_digest: Some(crate::hash::sha256_prefixed(format!("sysctl:{key}").as_bytes())),
            source_evidence_refs: Vec::new(),
            effects: vec![ScriptletEffect {
                kind: "sysctl-setting".to_string(),
                source: EffectSource::StaticSignal,
                confidence: EffectConfidence::Declared,
                replacement: EffectReplacement::Complete,
                adapter_id: Some("sysctl/v1".to_string()),
                adapter_digest: None,
                command: Some("sysctl".to_string()),
                args: vec!["-w".to_string(), format!("{key}=1")],
                path: None,
                reason_code: Some("helper-complete-sysctl".to_string()),
                extra,
            }],
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            boot_security_intents: Vec::new(),
            security_policy_intents: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn stale_public_sysctl_bundle(key: &str) -> LegacyScriptletBundle {
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "sysctl-target".to_string(),
            source_version: "1.0".to_string(),
            source_checksum: Some(crate::hash::sha256_prefixed(b"sysctl-target-source")),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "stale-converter".to_string(),
            conversion_tool_version: "0.0.0".to_string(),
            conversion_policy: "stale-public-policy".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(crate::hash::sha256_prefixed(b"sysctl-target-evidence")),
            target_compatibility: TargetCompatibility::ConaryPortable,
            allowed_targets: Vec::new(),
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::FullyReplaced,
            decision_counts: DecisionCounts {
                replaced: 1,
                ..DecisionCounts::default()
            },
            unsupported_class_counts: BTreeMap::new(),
            security_policy_intents: Vec::new(),
            entries: vec![replaced_sysctl_entry(key)],
            extra: BTreeMap::new(),
        }
    }

    fn stale_public_file_capability_bundle(capability: &str) -> LegacyScriptletBundle {
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "high-risk-file-capability".to_string(),
            source_version: "1.0".to_string(),
            source_checksum: Some(crate::hash::sha256_prefixed(b"high-risk-source")),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "stale-converter".to_string(),
            conversion_tool_version: "0.0.0".to_string(),
            conversion_policy: "stale-public-policy".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(crate::hash::sha256_prefixed(b"high-risk-evidence")),
            target_compatibility: TargetCompatibility::ConaryPortable,
            allowed_targets: Vec::new(),
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::FullyReplaced,
            decision_counts: DecisionCounts {
                replaced: 1,
                ..DecisionCounts::default()
            },
            unsupported_class_counts: BTreeMap::new(),
            security_policy_intents: Vec::new(),
            entries: vec![replaced_file_capability_entry(capability)],
            extra: BTreeMap::new(),
        }
    }

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
        assert!(summary.boot_security_intents.is_empty());
        assert!(summary.security_policy_intents.is_empty());
        assert_eq!(summary.review_artifact_path, None);
    }

    #[test]
    fn scriptlet_bundle_summary_defaults_include_empty_security_policy_intents() {
        let summary = ScriptletBundleSummary::default();

        assert!(summary.security_policy_intents.is_empty());
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

    #[test]
    fn scriptlet_bundle_summary_from_bundle_is_public_api() {
        let metadata = package_metadata("public-api", "1.0");
        let classification = ScriptletClassificationReport::default();
        let build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

        let summary = ScriptletBundleSummary::from_bundle(
            &build.bundle,
            Some(crate::hash::sha256_prefixed(b"x")),
        );

        assert_eq!(
            summary.publication_status,
            build.bundle.publication_status.as_str()
        );
        assert_eq!(
            summary.evidence_digest,
            Some(crate::hash::sha256_prefixed(b"x"))
        );
        assert_eq!(summary.review_artifact_path, None);
    }

    #[test]
    fn scriptlet_bundle_summary_from_bundle_recomputes_high_risk_file_capability_public_policy() {
        let bundle = stale_public_file_capability_bundle("cap_sys_admin");
        bundle.validate().expect("fixture bundle is valid");

        let summary = ScriptletBundleSummary::from_bundle(
            &bundle,
            Some(crate::hash::sha256_prefixed(b"reconstructed-evidence")),
        );

        assert_eq!(summary.scriptlet_fidelity, "fully-replaced");
        assert_eq!(summary.target_compatibility, "conary-portable");
        assert_eq!(summary.publication_status, "private-review");
        assert_eq!(summary.decision_counts.replaced, 1);
        assert_eq!(summary.decision_counts.review, 0);
        assert_eq!(
            summary.review_reason_codes,
            vec!["public-policy-file-capability-private-review".to_string()]
        );
        assert_eq!(
            summary.evidence_digest,
            Some(crate::hash::sha256_prefixed(b"reconstructed-evidence"))
        );
    }

    #[test]
    fn aggregate_status_applies_target_aware_sysctl_public_policy() {
        let counts = DecisionCounts {
            replaced: 1,
            ..DecisionCounts::default()
        };
        let kernel_example_entry = replaced_sysctl_entry("kernel.example");
        let ip_forward_entry = replaced_sysctl_entry("net.ipv4.ip_forward");
        let fedora = crate::repository::supported_profiles::profile_by_public_id("fedora-44")
            .expect("fedora profile");

        let public = aggregate_status(
            &[kernel_example_entry],
            &counts,
            Some(fedora as &dyn TargetProfileQuery),
        );
        assert_eq!(public.3.as_str(), "public");

        let private = aggregate_status(
            &[ip_forward_entry],
            &counts,
            Some(fedora as &dyn TargetProfileQuery),
        );
        assert_eq!(private.3.as_str(), "private-review");

        let missing = aggregate_status(
            &[replaced_sysctl_entry("kernel.example")],
            &counts,
            None,
        );
        assert_eq!(missing.3.as_str(), "private-review");
    }

    #[test]
    fn scriptlet_bundle_summary_recomputes_sysctl_public_policy_without_target_profile_id() {
        let bundle = stale_public_sysctl_bundle("kernel.example");
        bundle.validate().expect("fixture bundle is valid");

        let summary = ScriptletBundleSummary::from_bundle(
            &bundle,
            Some(crate::hash::sha256_prefixed(b"reconstructed-sysctl-evidence")),
        );

        assert_eq!(summary.publication_status, "private-review");
        assert!(
            summary
                .review_reason_codes
                .contains(&"public-policy-sysctl-target-profile-unsupported".to_string())
        );
    }

    #[test]
    fn scriptlet_bundle_summary_recomputes_sysctl_public_policy_for_unknown_target_profile_id() {
        let mut bundle = stale_public_sysctl_bundle("kernel.example");
        bundle.extra.insert(
            "public_policy_target_profile_id".to_string(),
            toml::Value::String("unknown-distro".to_string()),
        );
        bundle.validate().expect("fixture bundle is valid");

        let summary =
            ScriptletBundleSummary::from_bundle(&bundle, bundle.evidence_digest.clone());
        assert_eq!(summary.publication_status, "private-review");
        assert!(
            summary
                .review_reason_codes
                .contains(&"public-policy-sysctl-target-profile-unsupported".to_string())
        );
    }

    #[test]
    fn scriptlet_bundle_summary_preserves_entry_only_intents_alongside_bundle_mirrors() {
        let mut metadata = package_metadata("security-policy-summary", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo summary\n".to_string(),
            flags: None,
        });
        let classification = ScriptletClassificationReport::default();
        let mut build = bundle_for_metadata(&metadata, &[], &classification).unwrap();

        let mirrored_intent = test_intent(
            "scriptlet:0:post-install:selinux-label-refresh",
            "label-refresh",
        );
        let entry_only_intent =
            test_intent("scriptlet:0:post-install:selinux-boolean", "boolean-set");

        build.bundle.security_policy_intents = vec![mirrored_intent.clone()];
        build.bundle.entries[0].security_policy_intents =
            vec![mirrored_intent, entry_only_intent.clone()];

        let summary = ScriptletBundleSummary::from_bundle(
            &build.bundle,
            build.bundle.evidence_digest.clone(),
        );

        assert_eq!(summary.security_policy_intents.len(), 2);
        assert!(
            summary
                .security_policy_intents
                .iter()
                .any(|intent| intent.id == "scriptlet:0:post-install:selinux-label-refresh")
        );
        assert!(
            summary
                .security_policy_intents
                .iter()
                .any(|intent| intent == &entry_only_intent)
        );
    }
}
