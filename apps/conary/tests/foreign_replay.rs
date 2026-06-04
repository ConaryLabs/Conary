// apps/conary/tests/foreign_replay.rs

mod common;

use common::legacy_scriptlet_fixtures::{LegacyBundleFixture, synthetic_legacy_bundle};
use conary_core::ccs::legacy_replay::{
    HostForeignReplayPolicy, LegacyReplayLifecycle, LegacyReplayPolicyInput, LegacyReplayPreflight,
    LegacyReplayRefusalKind, plan_legacy_replay,
};
use conary_core::ccs::legacy_scriptlets::{ForeignReplayPolicy, TargetCompatibility};
use conary_core::ccs::target_compatibility::{
    CompatibilityPreflightEnvironment, MatrixPreflightRequirements, TargetCompatibilityMatrix,
    TargetCompatibilityMatrixEntry, TargetSelector, TargetSelectorArch, TargetSelectorRelease,
};
use conary_core::repository::distro::ReplayTarget;
use conary_core::scriptlet::SandboxMode;

#[test]
fn strict_host_refuses_foreign_replay_even_with_operator_override() {
    let bundle = foreign_legacy_bundle(ForeignReplayPolicy::Permissive);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Strict;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan replay");

    assert_refused(
        preflight,
        LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
    );
}

#[test]
fn guarded_host_allows_guarded_bundle_with_explicit_overrides() {
    let bundle = foreign_legacy_bundle(ForeignReplayPolicy::Guarded);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan replay");

    let LegacyReplayPreflight::RequiresReplay(plan) = preflight else {
        panic!("expected accepted foreign replay plan");
    };
    assert_eq!(plan.target_id, "rpm/fedora/44/x86_64");
    assert_eq!(plan.source_target_id, "rpm/fedora/45/x86_64");
    assert!(plan.raw_replay_required);
}

#[test]
fn permissive_host_allows_permissive_bundle_with_explicit_overrides() {
    let bundle = foreign_legacy_bundle(ForeignReplayPolicy::Permissive);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Permissive;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan replay");

    assert!(matches!(
        preflight,
        LegacyReplayPreflight::RequiresReplay(_)
    ));
}

#[test]
fn family_compatible_without_matrix_refuses_before_foreign_policy() {
    let bundle = foreign_legacy_bundle(ForeignReplayPolicy::Guarded);
    let mut input = policy_input();
    input.compatibility_matrix = TargetCompatibilityMatrix::production_default();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;

    let preflight = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan replay");

    assert_refused(
        preflight,
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}

fn foreign_legacy_bundle(
    policy: ForeignReplayPolicy,
) -> conary_core::ccs::legacy_scriptlets::LegacyScriptletBundle {
    let mut bundle = synthetic_legacy_bundle(LegacyBundleFixture::SameSourceLegacyPostInstall)
        .expect("legacy bundle fixture");
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.allowed_targets.clear();
    bundle.foreign_replay_policy = policy;
    bundle.validate().expect("foreign replay fixture validates");
    bundle
}

fn policy_input() -> LegacyReplayPolicyInput<'static> {
    LegacyReplayPolicyInput {
        replay_enabled: false,
        foreign_replay_override: false,
        no_scripts: false,
        requested_sandbox_mode: SandboxMode::Always,
        host_policy: HostForeignReplayPolicy::Strict,
        target: ReplayTarget {
            format: "rpm",
            distro: "fedora",
            release: "44",
            arch: "x86_64",
        },
        compatibility_matrix: synthetic_foreign_matrix(),
        compatibility_environment: CompatibilityPreflightEnvironment::default(),
    }
}

fn synthetic_foreign_matrix() -> TargetCompatibilityMatrix {
    TargetCompatibilityMatrix::for_testing(vec![TargetCompatibilityMatrixEntry {
        id: "test-fedora45-to-fedora44".to_string(),
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
        digest: Some("sha256:test-fedora45-to-fedora44".to_string()),
        rationale: "synthetic foreign replay test entry".to_string(),
    }])
}

fn assert_refused(preflight: LegacyReplayPreflight, expected: LegacyReplayRefusalKind) {
    let LegacyReplayPreflight::Refused(refusal) = preflight else {
        panic!("expected refusal");
    };
    assert_eq!(refusal.kind, expected);
}
