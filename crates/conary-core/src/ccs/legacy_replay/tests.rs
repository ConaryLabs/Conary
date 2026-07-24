// conary-core/src/ccs/legacy_replay/tests.rs

use super::*;
use crate::ccs::legacy_scriptlets::{
    DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
    LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy, PublicationStatus,
    ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility, TransactionOrder,
    VersionScheme,
};
use crate::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, MatrixPreflightRequirements, TargetCompatibilityMatrix,
    TargetCompatibilityMatrixEntry, TargetSelector, TargetSelectorArch, TargetSelectorRelease,
};
use crate::hash;
use crate::repository::distro::{ReplayTarget, source_target_from_bundle};
use crate::scriptlet::SandboxMode;
use std::collections::BTreeMap;

fn target() -> ReplayTarget<'static> {
    ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    }
}

fn policy_input() -> LegacyReplayPolicyInput<'static> {
    LegacyReplayPolicyInput {
        replay_enabled: false,
        foreign_replay_override: false,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
        host_policy: HostForeignReplayPolicy::Strict,
        target: target(),
        compatibility_matrix: TargetCompatibilityMatrix::production_default(),
        compatibility_environment: CompatibilityPreflightEnvironment::default(),
    }
}

fn fedora45_to_fedora44_entry(id: &str) -> TargetCompatibilityMatrixEntry {
    TargetCompatibilityMatrixEntry {
        id: id.to_string(),
        source: TargetSelector {
            format: "rpm".to_string(),
            distro: "fedora".to_string(),
            release: TargetSelectorRelease::Exact("45".to_string()),
            arch: TargetSelectorArch::Exact("x86_64".to_string()),
        },
        target: TargetSelector {
            format: "rpm".to_string(),
            distro: "fedora".to_string(),
            release: TargetSelectorRelease::Exact("44".to_string()),
            arch: TargetSelectorArch::Exact("x86_64".to_string()),
        },
        requirements: MatrixPreflightRequirements::default(),
        digest: Some("sha256:test-fedora45-to-44".to_string()),
        rationale: "synthetic planner fixture".to_string(),
    }
}

fn policy_with_fedora_matrix() -> LegacyReplayPolicyInput<'static> {
    let mut input = policy_input();
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora45_to_fedora44_entry(
            "test-fedora45-to-fedora44",
        )]);
    input
}

fn fedora44_to_ubuntu2604_entry(id: &str) -> TargetCompatibilityMatrixEntry {
    let mut entry = fedora45_to_fedora44_entry(id);
    entry.source.release = TargetSelectorRelease::Exact("44".to_string());
    entry.target.format = "deb".to_string();
    entry.target.distro = "ubuntu".to_string();
    entry.target.release = TargetSelectorRelease::Exact("26.04".to_string());
    entry
}

fn entry(id: &str, phase: LifecyclePath, decision: ScriptletDecision) -> LegacyScriptletEntry {
    let body = format!("echo {id}\n");
    LegacyScriptletEntry {
        id: id.to_string(),
        native_slot: id.to_string(),
        phase,
        lifecycle_paths: vec!["fixture".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: hash::sha256_prefixed(body.as_bytes()),
        body,
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "default".to_string(),
            before: Vec::new(),
            after: Vec::new(),
            extra: BTreeMap::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        decision,
        reason_code: "fixture".to_string(),
        human_reason: None,
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        effects: Vec::new(),
        unknown_command_evidence: Vec::new(),
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

fn bundle_with_entries(entries: Vec<LegacyScriptletEntry>) -> LegacyScriptletBundle {
    let mut decision_counts = DecisionCounts::default();
    for entry in &entries {
        match &entry.decision {
            ScriptletDecision::Replaced => decision_counts.replaced += 1,
            ScriptletDecision::Legacy => decision_counts.legacy += 1,
            ScriptletDecision::Blocked => decision_counts.blocked += 1,
            ScriptletDecision::Review => decision_counts.review += 1,
            ScriptletDecision::Unknown(value) => {
                *decision_counts.extra.entry(value.clone()).or_default() += 1;
            }
        }
    }

    LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 2,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: "fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "0.0.0".to_string(),
        conversion_policy: "fixture".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: Some(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        ),
        target_compatibility: TargetCompatibility::SourceNative,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status: PublicationStatus::Public,
        scriptlet_fidelity: ScriptletFidelity::Mixed,
        decision_counts,
        unsupported_class_counts: BTreeMap::new(),
        security_policy_intents: Vec::new(),
        entries,
        extra: BTreeMap::new(),
    }
}

#[test]
fn arch_source_release_none_normalizes_to_rolling() {
    let mut bundle = bundle_with_entries(Vec::new());
    bundle.source_format = SourceFormat::Arch;
    bundle.source_family = "arch".to_string();
    bundle.source_distro = Some("arch".to_string());
    bundle.source_release = None;
    bundle.source_arch = Some("x86_64".to_string());

    assert_eq!(
        source_target_from_bundle(&bundle).to_id(),
        "arch/arch/rolling/x86_64"
    );
}

#[test]
fn review_blocked_and_unknown_entries_refuse_admission_anywhere_in_bundle() {
    for (decision, expected) in [
        (
            ScriptletDecision::Review,
            LegacyReplayRefusalKind::ReviewEntry,
        ),
        (
            ScriptletDecision::Blocked,
            LegacyReplayRefusalKind::BlockedEntry,
        ),
        (
            ScriptletDecision::Unknown("mystery".to_string()),
            LegacyReplayRefusalKind::UnknownDecision,
        ),
    ] {
        let bundle =
            bundle_with_entries(vec![entry("future", LifecyclePath::PostRemove, decision)]);

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &policy_input(),
            )
            .expect("plan"),
            expected,
        );
    }
}

#[test]
fn future_lifecycle_legacy_entry_is_not_selected_for_current_install() {
    let bundle = bundle_with_entries(vec![entry(
        "future-remove",
        LifecyclePath::PostRemove,
        ScriptletDecision::Legacy,
    )]);

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &policy_input(),
    )
    .expect("plan");

    assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
}

#[test]
fn no_bundle_keeps_no_scripts_native_free() {
    let mut input = policy_input();
    input.no_scripts = true;

    let preflight =
        plan_legacy_replay(None, LegacyReplayLifecycle::FreshInstallPost, &input).expect("plan");

    assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
}

#[test]
fn native_free_bundle_is_allowed_with_no_scripts() {
    let bundle = bundle_with_entries(Vec::new());
    let mut input = policy_input();
    input.no_scripts = true;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan");

    assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
}

#[test]
fn no_scripts_future_lifecycle_legacy_entry_is_not_selected_for_current_install() {
    let bundle = bundle_with_entries(vec![entry(
        "future-remove",
        LifecyclePath::PostRemove,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.no_scripts = true;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan");

    assert_eq!(preflight, LegacyReplayPreflight::NativeFree);
}

#[test]
fn selected_legacy_entry_requires_feature_gate() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &policy_input(),
        )
        .expect("plan"),
        LegacyReplayRefusalKind::LegacyReplayFeatureDisabled,
    );
}

#[test]
fn no_scripts_refuses_selected_required_legacy_replay() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.no_scripts = true;

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::NoScriptsWouldSkipRequiredReplay,
    );
}

#[test]
fn replaced_entries_never_schedule_raw_replay() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Replaced,
    )]);

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &policy_input(),
    )
    .expect("plan");

    let LegacyReplayPreflight::FullyReplaced(plan) = preflight else {
        panic!("expected fully replaced plan");
    };
    assert!(!plan.raw_replay_required);
    assert!(plan.lifecycle_entries.is_empty());
}

#[test]
fn no_scripts_replaced_only_bundle_suppresses_ccs_hooks_in_plan() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Replaced,
    )]);
    let mut input = policy_input();
    input.no_scripts = true;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan");

    let LegacyReplayPreflight::FullyReplaced(plan) = preflight else {
        panic!("expected fully replaced plan");
    };
    assert!(!plan.ccs_hooks_allowed);
    assert!(!plan.raw_replay_required);
    assert!(plan.lifecycle_entries.is_empty());
}

#[test]
fn review_and_blocked_entries_refuse_even_with_no_scripts() {
    for (decision, expected) in [
        (
            ScriptletDecision::Review,
            LegacyReplayRefusalKind::ReviewEntry,
        ),
        (
            ScriptletDecision::Blocked,
            LegacyReplayRefusalKind::BlockedEntry,
        ),
    ] {
        let bundle =
            bundle_with_entries(vec![entry("future", LifecyclePath::PostRemove, decision)]);
        let mut input = policy_input();
        input.no_scripts = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            expected,
        );
    }
}

#[test]
fn scriptlet_fidelity_legacy_replay_does_not_override_entry_decisions() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Replaced,
    )]);
    bundle.scriptlet_fidelity = ScriptletFidelity::LegacyReplay;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &policy_input(),
    )
    .expect("plan");

    assert!(matches!(preflight, LegacyReplayPreflight::FullyReplaced(_)));
}

#[test]
fn upgrade_lifecycle_selection_uses_upgrade_slots_and_fallbacks() {
    let direct = bundle_with_entries(vec![entry(
        "pre-upgrade",
        LifecyclePath::PreUpgrade,
        ScriptletDecision::Legacy,
    )]);
    let fallback = bundle_with_entries(vec![entry(
        "pre-install",
        LifecyclePath::PreInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;

    assert_plan_entry_ids(
        plan_legacy_replay(Some(&direct), LegacyReplayLifecycle::UpgradeNewPre, &input)
            .expect("plan"),
        &["pre-upgrade"],
    );
    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&fallback),
            LegacyReplayLifecycle::UpgradeNewPre,
            &input,
        )
        .expect("plan"),
        &["pre-install"],
    );
}

#[test]
fn raw_trigger_replay_is_refused() {
    let bundle = bundle_with_entries(vec![entry(
        "trigger",
        LifecyclePath::Trigger,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::TriggerReplayUnsupported,
    );
}

#[path = "tests/target_policy.rs"]
mod target_policy;
use target_policy::{assert_plan_entry_ids, assert_refused};
