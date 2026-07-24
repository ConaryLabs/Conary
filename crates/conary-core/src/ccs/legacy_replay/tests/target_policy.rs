// conary-core/src/ccs/legacy_replay/tests/target_policy.rs

use super::*;

#[test]
fn target_mismatch_refuses_source_native_raw_replay() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "45",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::TargetMismatch,
    );
}

#[test]
fn unknown_target_release_refuses_source_native_raw_replay() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "unknown",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::TargetMismatch,
    );
}

#[test]
fn same_source_raw_replay_does_not_need_foreign_override() {
    let bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;
    input.host_policy = HostForeignReplayPolicy::Strict;
    input.foreign_replay_override = false;

    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        &["post"],
    );
}

#[test]
fn old_upgrade_remove_lifecycle_selects_installed_bundle_remove_entries() {
    let bundle = bundle_with_entries(vec![
        entry(
            "old-pre-remove",
            LifecyclePath::PreRemove,
            ScriptletDecision::Legacy,
        ),
        entry(
            "old-post-remove",
            LifecyclePath::PostRemove,
            ScriptletDecision::Legacy,
        ),
    ]);
    let mut input = policy_input();
    input.replay_enabled = true;

    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::UpgradeOldPreRemove,
            &input,
        )
        .expect("plan"),
        &["old-pre-remove"],
    );
    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::UpgradeOldPostRemove,
            &input,
        )
        .expect("plan"),
        &["old-post-remove"],
    );
}

#[test]
fn rollback_lifecycle_refuses_when_replay_is_unavailable() {
    let bundle = bundle_with_entries(vec![entry(
        "rollback-post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    let mut input = policy_input();
    input.replay_enabled = true;

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::RollbackRestore,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::RollbackReplayUnavailable,
    );
}

#[test]
fn target_compatibility_review_blocked_and_unknown_refuse_replay() {
    for (compatibility, expected) in [
        (
            TargetCompatibility::ReviewRequired,
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
        ),
        (
            TargetCompatibility::Blocked,
            LegacyReplayRefusalKind::TargetCompatibilityBlocked,
        ),
        (
            TargetCompatibility::Unknown("future".to_string()),
            LegacyReplayRefusalKind::TargetCompatibilityReviewRequired,
        ),
    ] {
        let mut bundle = bundle_with_entries(vec![entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        )]);
        bundle.target_compatibility = compatibility;
        let mut input = policy_input();
        input.replay_enabled = true;

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
fn family_compatible_without_matrix_entry_refuses() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;

    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}

#[test]
fn family_compatible_with_matrix_entry_records_decision() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;

    let mut input = policy_with_fedora_matrix();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;

    let LegacyReplayPreflight::RequiresReplay(plan) = plan_legacy_replay(
        Some(&bundle),
        LegacyReplayLifecycle::FreshInstallPost,
        &input,
    )
    .expect("plan") else {
        panic!("expected accepted replay plan");
    };

    assert_eq!(plan.compatibility_decision.decision, "accepted");
    assert_eq!(
        plan.compatibility_decision.reason_code,
        "compatibility-matrix-entry-accepted"
    );
    assert_eq!(
        plan.compatibility_decision.matrix_entry_id.as_deref(),
        Some("test-fedora45-to-fedora44")
    );
    assert!(plan.compatibility_decision.override_required);
    assert!(plan.compatibility_decision.override_used);
}

#[test]
fn allowed_targets_do_not_substitute_for_family_compatible_matrix_entry() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
    bundle
        .allowed_targets
        .push("rpm/fedora/44/x86_64".to_string());

    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::CompatibilityMatrixEntryMissing,
    );
}

#[test]
fn no_scripts_refusal_still_precedes_matrix_lookup() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.source_release = Some("45".to_string());
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;

    let mut input = policy_input();
    input.replay_enabled = true;
    input.no_scripts = true;
    input.target = ReplayTarget {
        format: "rpm",
        distro: "fedora",
        release: "44",
        arch: "x86_64",
    };

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
fn foreign_replay_policy_and_host_policy_fail_closed() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Deny;
    let mut input = policy_input();
    input.replay_enabled = true;
    input.target = ReplayTarget {
        format: "deb",
        distro: "ubuntu",
        release: "26.04",
        arch: "x86_64",
    };
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora44_to_ubuntu2604_entry(
            "test-fedora44-to-ubuntu2604",
        )]);
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Permissive;

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayDeniedByBundle,
    );

    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
    input.host_policy = HostForeignReplayPolicy::Strict;
    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
    );

    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.foreign_replay_override = false;
    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayOverrideRequired,
    );

    input.foreign_replay_override = true;
    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        &["post"],
    );

    input.host_policy = HostForeignReplayPolicy::Permissive;
    input.foreign_replay_override = false;
    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayOverrideRequired,
    );

    input.foreign_replay_override = true;
    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        &["post"],
    );
}

#[test]
fn foreign_replay_override_without_replay_enabled_is_insufficient() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Permissive;
    let mut input = policy_input();
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Permissive;
    input.target = ReplayTarget {
        format: "deb",
        distro: "ubuntu",
        release: "26.04",
        arch: "x86_64",
    };
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora44_to_ubuntu2604_entry(
            "test-fedora44-to-ubuntu2604",
        )]);

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::LegacyReplayFeatureDisabled,
    );
}

#[test]
fn guarded_host_requires_guarded_compatible_bundle_policy() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Permissive;
    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Guarded;
    input.target = ReplayTarget {
        format: "deb",
        distro: "ubuntu",
        release: "26.04",
        arch: "x86_64",
    };
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora44_to_ubuntu2604_entry(
            "test-fedora44-to-ubuntu2604",
        )]);

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayDeniedByHostPolicy,
    );

    bundle.foreign_replay_policy = ForeignReplayPolicy::Guarded;
    assert_plan_entry_ids(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        &["post"],
    );
}

#[test]
fn unknown_foreign_replay_policy_fails_closed() {
    let mut bundle = bundle_with_entries(vec![entry(
        "post",
        LifecyclePath::PostInstall,
        ScriptletDecision::Legacy,
    )]);
    bundle.target_compatibility = TargetCompatibility::FamilyCompatible;
    bundle.foreign_replay_policy = ForeignReplayPolicy::Unknown("future".to_string());
    let mut input = policy_input();
    input.replay_enabled = true;
    input.foreign_replay_override = true;
    input.host_policy = HostForeignReplayPolicy::Permissive;
    input.target = ReplayTarget {
        format: "deb",
        distro: "ubuntu",
        release: "26.04",
        arch: "x86_64",
    };
    input.compatibility_matrix =
        TargetCompatibilityMatrix::for_testing(vec![fedora44_to_ubuntu2604_entry(
            "test-fedora44-to-ubuntu2604",
        )]);

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPost,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::ForeignReplayDeniedByBundle,
    );
}

#[test]
fn replay_timeout_bounds_are_enforced() {
    for timeout_ms in [999, 300_001] {
        let mut legacy = entry(
            "post",
            LifecyclePath::PostInstall,
            ScriptletDecision::Legacy,
        );
        legacy.timeout_ms = timeout_ms;
        let bundle = bundle_with_entries(vec![legacy]);
        let mut input = policy_input();
        input.replay_enabled = true;

        assert_refused(
            plan_legacy_replay(
                Some(&bundle),
                LegacyReplayLifecycle::FreshInstallPost,
                &input,
            )
            .expect("plan"),
            LegacyReplayRefusalKind::TimeoutOutOfRange,
        );
    }
}

#[test]
fn pre_mutation_ordering_conflicts_refuse_mixed_raw_and_replaced_entries() {
    let mut raw = entry("raw", LifecyclePath::PreInstall, ScriptletDecision::Legacy);
    raw.transaction_order.after = vec!["replaced".to_string()];
    let replaced = entry(
        "replaced",
        LifecyclePath::PreInstall,
        ScriptletDecision::Replaced,
    );
    let bundle = bundle_with_entries(vec![raw, replaced]);
    let mut input = policy_input();
    input.replay_enabled = true;

    assert_refused(
        plan_legacy_replay(
            Some(&bundle),
            LegacyReplayLifecycle::FreshInstallPre,
            &input,
        )
        .expect("plan"),
        LegacyReplayRefusalKind::UnsatisfiedTransactionOrder,
    );
}

pub(super) fn assert_refused(preflight: LegacyReplayPreflight, expected: LegacyReplayRefusalKind) {
    let LegacyReplayPreflight::Refused(refusal) = preflight else {
        panic!("expected refusal");
    };
    assert_eq!(refusal.kind, expected);
}

pub(super) fn assert_plan_entry_ids(preflight: LegacyReplayPreflight, expected: &[&str]) {
    let LegacyReplayPreflight::RequiresReplay(plan) = preflight else {
        panic!("expected replay plan");
    };
    let actual: Vec<&str> = plan
        .lifecycle_entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect();
    assert_eq!(actual, expected);
}
