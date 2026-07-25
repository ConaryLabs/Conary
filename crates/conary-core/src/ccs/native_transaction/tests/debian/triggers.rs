// conary-core/src/ccs/native_transaction/tests/debian/triggers.rs

use super::*;

#[test]
fn trigger_await_modes_create_edges_and_one_policy_argv_batch() {
    let activation = deb_triggers_entry(vec![
        DebTriggerMetadata {
            directive: DebTriggerDirective::Activate,
            trigger_name: "awaited-cache".to_string(),
            await_mode: DebTriggerAwaitMode::Await,
            raw_line: "activate-await awaited-cache".to_string(),
        },
        DebTriggerMetadata {
            directive: DebTriggerDirective::Activate,
            trigger_name: "late-cache".to_string(),
            await_mode: DebTriggerAwaitMode::NoAwait,
            raw_line: "activate-noawait late-cache".to_string(),
        },
    ]);
    let activating = bundle(SourceFormat::Deb, vec![activation]);

    let postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
    );
    let triggers = deb_triggers_entry(vec![
        DebTriggerMetadata {
            directive: DebTriggerDirective::Interest,
            trigger_name: "awaited-cache".to_string(),
            await_mode: DebTriggerAwaitMode::Await,
            raw_line: "interest-await awaited-cache".to_string(),
        },
        DebTriggerMetadata {
            directive: DebTriggerDirective::Interest,
            trigger_name: "late-cache".to_string(),
            await_mode: DebTriggerAwaitMode::Await,
            raw_line: "interest-await late-cache".to_string(),
        },
    ]);
    let interested = bundle(SourceFormat::Deb, vec![postinst, triggers]);

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "activator",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &activating,
            },
            NativeBundleView {
                package_name: "cache-owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installed,
                bundle: &interested,
            },
        ],
        &[NativeTransactionChange {
            package_name: "activator".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::new(),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        }],
        &NativeTransactionState {
            deb_package_states: vec![deb_package_state(
                "cache-owner",
                "1",
                None,
                DebPackageState::Installed,
            )],
            deb_change_indices: BTreeSet::from([0]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].stage,
        NativeEventStage::DebAwaitedTriggerProcessing
    );
    assert_eq!(
        plan.events[0].matched_targets,
        vec!["awaited-cache", "late-cache"]
    );
    assert_eq!(
        plan.events[0].args,
        vec!["triggered", "awaited-cache late-cache"]
    );
    assert_eq!(plan.deb.trigger_activations.len(), 4);
    assert_eq!(
        plan.deb
            .trigger_activations
            .iter()
            .map(|activation| activation.boundary)
            .collect::<Vec<_>>(),
        vec![
            DebTriggerActivationBoundary::BeforePayloadEvents,
            DebTriggerActivationBoundary::BeforePayloadEvents,
            DebTriggerActivationBoundary::BeforeConfigure,
            DebTriggerActivationBoundary::BeforeConfigure,
        ]
    );
    assert!(
        plan.deb
            .trigger_activations
            .iter()
            .any(|activation| activation.trigger_name == "awaited-cache" && activation.awaited)
    );
    assert!(
        plan.deb
            .trigger_activations
            .iter()
            .any(|activation| { activation.trigger_name == "late-cache" && !activation.awaited })
    );
}

#[test]
fn file_triggers_activate_from_created_updated_and_deleted_archive_paths() {
    let postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
    );
    let triggers = deb_triggers_entry(vec![DebTriggerMetadata {
        directive: DebTriggerDirective::Interest,
        trigger_name: "/usr/share/icons".to_string(),
        await_mode: DebTriggerAwaitMode::Await,
        raw_line: "interest-await /usr/share/icons".to_string(),
    }]);
    let interested = bundle(SourceFormat::Deb, vec![postinst, triggers]);

    let changes = vec![
        NativeTransactionChange {
            package_name: "created".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/share/icons/new.png".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        },
        NativeTransactionChange {
            package_name: "updated".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: Some("1".to_string()),
            new_version: Some("2".to_string()),
            operation: NativeTransactionOperation::Upgrade,
            old_paths: BTreeSet::from(["/usr/share/icons/changed.png".to_string()]),
            new_paths: BTreeSet::from(["usr/share/icons/changed.png".to_string()]),
            instances_before: 1,
            instances_after: 1,
            transaction_index: 1,
        },
        NativeTransactionChange {
            package_name: "deleted".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: Some("1".to_string()),
            new_version: None,
            operation: NativeTransactionOperation::Remove,
            old_paths: BTreeSet::from(["usr/share/icons/old.png".to_string()]),
            new_paths: BTreeSet::new(),
            instances_before: 1,
            instances_after: 0,
            transaction_index: 2,
        },
        NativeTransactionChange {
            package_name: "sibling".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/share/icons-theme/not-a-match".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 3,
        },
    ];
    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "icon-cache",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &interested,
        }],
        &changes,
        &NativeTransactionState {
            deb_package_states: vec![deb_package_state(
                "icon-cache",
                "1",
                None,
                DebPackageState::Installed,
            )],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].stage, NativeEventStage::DebTriggerProcessing);
    assert_eq!(plan.events[0].args, vec!["triggered", "/usr/share/icons"]);
    assert_eq!(
        plan.deb
            .trigger_activations
            .iter()
            .map(|activation| (
                activation.triggering.package_name.as_str(),
                activation.triggering.package_version.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![("created", "1"), ("updated", "2"), ("deleted", "1")]
    );
    assert!(
        plan.deb
            .trigger_activations
            .iter()
            .all(|activation| !activation.awaited)
    );
}

#[test]
fn trigger_interest_becomes_active_only_after_its_package_is_configured() {
    let postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
    );
    let triggers = deb_triggers_entry(vec![DebTriggerMetadata {
        directive: DebTriggerDirective::Interest,
        trigger_name: "/usr/share/icons".to_string(),
        await_mode: DebTriggerAwaitMode::Await,
        raw_line: "interest-await /usr/share/icons".to_string(),
    }]);
    let incoming_interest = bundle(SourceFormat::Deb, vec![postinst, triggers]);
    let changes = [
        NativeTransactionChange {
            package_name: "early".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/share/icons/early.png".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        },
        NativeTransactionChange {
            package_name: "icon-cache".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::new(),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 1,
        },
        NativeTransactionChange {
            package_name: "late".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/share/icons/late.png".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 2,
        },
    ];

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "icon-cache",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installing,
            bundle: &incoming_interest,
        }],
        &changes,
        &NativeTransactionState {
            deb_change_indices: BTreeSet::from([1]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.deb.trigger_activations.len(), 1);
    assert_eq!(plan.deb.trigger_activations[0].transaction_index, 2);
    assert_eq!(
        plan.deb.trigger_activations[0].boundary,
        DebTriggerActivationBoundary::BeforePayloadMutation
    );
    assert!(!plan.deb.trigger_activations[0].awaited);
}

#[test]
fn removing_trigger_owner_is_unconfigured_before_payload_file_activations() {
    let postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
    );
    let triggers = deb_triggers_entry(vec![DebTriggerMetadata {
        directive: DebTriggerDirective::Interest,
        trigger_name: "/usr/share/icons".to_string(),
        await_mode: DebTriggerAwaitMode::Await,
        raw_line: "interest-await /usr/share/icons".to_string(),
    }]);
    let removing_interest = bundle(SourceFormat::Deb, vec![postinst, triggers]);
    let changes = [NativeTransactionChange {
        package_name: "icon-cache".to_string(),
        old_arch: None,
        new_arch: None,
        old_version: Some("1".to_string()),
        new_version: None,
        operation: NativeTransactionOperation::Remove,
        old_paths: BTreeSet::from(["usr/share/icons/owned.png".to_string()]),
        new_paths: BTreeSet::new(),
        instances_before: 1,
        instances_after: 0,
        transaction_index: 4,
    }];

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "icon-cache",
            package_version: "1",
            instances_after: 0,
            role: NativeBundleRole::Removing,
            bundle: &removing_interest,
        }],
        &changes,
        &NativeTransactionState {
            deb_package_states: vec![deb_package_state(
                "icon-cache",
                "1",
                None,
                DebPackageState::Installed,
            )],
            deb_change_indices: BTreeSet::from([4]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert!(plan.deb.trigger_activations.is_empty());
    assert!(plan.deb.trigger_batches.is_empty());
    assert_eq!(
        plan.deb.trigger_unconfigurations,
        vec![DebTriggerUnconfiguration {
            transaction_index: 4,
            boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
            owner: NativePackageIdentity::new("icon-cache", "1", None),
        }]
    );
}

#[test]
fn file_trigger_matching_uses_a_textual_path_component_boundary() {
    use crate::ccs::native_transaction::deb::deb_file_trigger_matches;

    assert!(deb_file_trigger_matches(
        "/usr/share/icons",
        "usr/share/icons"
    ));
    assert!(deb_file_trigger_matches(
        "/usr/share/icons",
        "/usr/share/icons/theme/index"
    ));
    assert!(!deb_file_trigger_matches(
        "/usr/share/icons",
        "usr/share/icons-theme/index"
    ));
}

#[test]
fn persisted_pending_triggers_resume_with_exact_await_edges() {
    let postinst = deb_entry(
        "deb:postinst",
        LifecyclePath::PostInstall,
        DebControlMember::Postinst,
    );
    let triggers = deb_triggers_entry(vec![DebTriggerMetadata {
        directive: DebTriggerDirective::Interest,
        trigger_name: "refresh-cache".to_string(),
        await_mode: DebTriggerAwaitMode::Await,
        raw_line: "interest-await refresh-cache".to_string(),
    }]);
    let interested = bundle(SourceFormat::Deb, vec![postinst, triggers]);
    let awaiter = bundle(
        SourceFormat::Deb,
        vec![deb_entry(
            "deb:postinst",
            LifecyclePath::PostInstall,
            DebControlMember::Postinst,
        )],
    );
    let state = NativeTransactionState {
        deb_package_states: vec![
            NativeDebPackageState {
                package_name: "cache-owner".to_string(),
                package_version: "1".to_string(),
                source_arch: None,
                lifecycle_state: DebPackageState::TriggersPending,
                pending_triggers: vec!["refresh-cache".to_string()],
                awaited_packages: Vec::new(),
            },
            NativeDebPackageState {
                package_name: "activator".to_string(),
                package_version: "1".to_string(),
                source_arch: None,
                lifecycle_state: DebPackageState::TriggersAwaited,
                pending_triggers: Vec::new(),
                awaited_packages: vec![NativePackageIdentity {
                    package_name: "cache-owner".to_string(),
                    package_version: "1".to_string(),
                    package_arch: None,
                }],
            },
        ],
        ..NativeTransactionState::default()
    };

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "cache-owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installed,
                bundle: &interested,
            },
            NativeBundleView {
                package_name: "activator",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installed,
                bundle: &awaiter,
            },
        ],
        &[],
        &state,
    )
    .unwrap();

    assert!(plan.deb.trigger_activations.is_empty());
    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].stage,
        NativeEventStage::DebAwaitedTriggerProcessing
    );
    assert_eq!(plan.events[0].args, vec!["triggered", "refresh-cache"]);
    assert_eq!(
        plan.deb.trigger_batches[0].awaited_by,
        vec![NativePackageIdentity {
            package_name: "activator".to_string(),
            package_version: "1".to_string(),
            package_arch: None,
        }]
    );
}

#[test]
fn persisted_pending_trigger_must_still_be_an_exact_interest() {
    let interested = bundle(
        SourceFormat::Deb,
        vec![deb_entry(
            "deb:postinst",
            LifecyclePath::PostInstall,
            DebControlMember::Postinst,
        )],
    );
    let state = NativeTransactionState {
        deb_package_states: vec![NativeDebPackageState {
            package_name: "cache-owner".to_string(),
            package_version: "1".to_string(),
            source_arch: None,
            lifecycle_state: DebPackageState::TriggersPending,
            pending_triggers: vec!["obsolete-cache".to_string()],
            awaited_packages: Vec::new(),
        }],
        ..NativeTransactionState::default()
    };

    let error = plan_native_transaction(
        &[NativeBundleView {
            package_name: "cache-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &interested,
        }],
        &[],
        &state,
    )
    .expect_err("stale trigger names must not be guessed or dropped");
    assert!(error.to_string().contains("not a current exact interest"));
}
