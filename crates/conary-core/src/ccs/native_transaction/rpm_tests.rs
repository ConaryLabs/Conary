// conary-core/src/ccs/native_transaction/rpm_tests.rs

use super::*;
use crate::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
    NativeLifecycleEntryKind, RpmBodyTransform, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    RpmSysusersDirective, RpmSysusersMetadata, RpmTriggerAction, RpmTriggerKind,
    RpmTriggerMetadata, RpmTriggerTargetConstraint, ScriptletFidelity, SourceFormat,
    TransactionOrder, VersionScheme as LifecycleVersionScheme,
};
use std::collections::BTreeSet;

fn runtime(program: RpmProgram, critical: bool) -> RpmRuntimeMetadata {
    RpmRuntimeMetadata {
        program,
        body_transforms: Vec::new(),
        critical,
        criticality: if critical {
            RpmCriticality::SlotDefault
        } else {
            RpmCriticality::WarningOnly
        },
        raw_flags: 0,
        unknown_flags: 0,
        install_prefixes: Vec::new(),
        macro_context: Default::default(),
        header_context: Default::default(),
        package_rpm_version: None,
    }
}

fn entry(id: &str, phase: LifecyclePath, runtime: RpmRuntimeMetadata) -> NativeLifecycleEntry {
    let body = "exit 0\n".to_string();
    let interpreter = match &runtime.program {
        RpmProgram::EmbeddedLua => "<lua>",
        RpmProgram::External => "/bin/sh",
        RpmProgram::Sysusers => "/usr/bin/systemd-sysusers",
    };
    NativeLifecycleEntry {
        id: id.to_string(),
        native_slot: id.to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: phase.clone(),
        lifecycle_paths: vec![phase.as_str().to_string()],
        interpreter: interpreter.to_string(),
        interpreter_args: Vec::new(),
        body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
        body,
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "before-payload".to_string(),
            ..TransactionOrder::default()
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(runtime),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn bundle(entries: Vec<NativeLifecycleEntry>) -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "rpm".to_string(),
        source_distro: None,
        source_release: None,
        source_arch: None,
        source_package: "owner".to_string(),
        source_version: "1".to_string(),
        source_checksum: None,
        version_scheme: LifecycleVersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "native-lifecycle".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries,
    }
}

fn install_change() -> NativeTransactionChange {
    NativeTransactionChange {
        package_name: "owner".to_string(),
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
    }
}

fn installing_view(bundle: &NativeLifecycleBundle) -> NativeBundleView<'_> {
    NativeBundleView {
        package_name: "owner",
        package_version: "1",
        instances_after: 1,
        role: NativeBundleRole::Installing,
        bundle,
    }
}

fn trigger_entry(
    id: &str,
    kind: RpmTriggerKind,
    action: RpmTriggerAction,
    target: &str,
) -> NativeLifecycleEntry {
    let mut entry = entry(
        id,
        LifecyclePath::PostInstall,
        runtime(RpmProgram::External, false),
    );
    entry.rpm_trigger = Some(RpmTriggerMetadata {
        kind,
        target_constraints: vec![RpmTriggerTargetConstraint {
            package: target.to_string(),
            action,
            operator: None,
            version: None,
            raw_flags: 0,
        }],
        priority: Some(100_000),
        path_prefixes: matches!(kind, RpmTriggerKind::File | RpmTriggerKind::TransactionFile)
            .then(|| vec![target.to_string()])
            .unwrap_or_default(),
    });
    entry
}

#[allow(clippy::too_many_arguments)]
fn change(
    package_name: &str,
    old_version: Option<&str>,
    new_version: Option<&str>,
    old_paths: &[&str],
    new_paths: &[&str],
    instances_before: u32,
    instances_after: u32,
    transaction_index: usize,
) -> NativeTransactionChange {
    NativeTransactionChange {
        package_name: package_name.to_string(),
        old_arch: None,
        new_arch: None,
        old_version: old_version.map(str::to_string),
        new_version: new_version.map(str::to_string),
        operation: match (old_version, new_version) {
            (None, Some(_)) => NativeTransactionOperation::Install,
            (Some(_), Some(_)) => NativeTransactionOperation::Upgrade,
            (Some(_), None) => NativeTransactionOperation::Remove,
            (None, None) => panic!("test change needs an old or new version"),
        },
        old_paths: old_paths.iter().map(|path| (*path).to_string()).collect(),
        new_paths: new_paths.iter().map(|path| (*path).to_string()).collect(),
        instances_before,
        instances_after,
        transaction_index,
    }
}

#[test]
fn rpm_regular_events_do_not_delegate_transaction_failure_policy() {
    let bundle = bundle(vec![
        entry(
            "rpm:%pre",
            LifecyclePath::PreInstall,
            runtime(RpmProgram::External, true),
        ),
        entry(
            "rpm:%post",
            LifecyclePath::PostInstall,
            runtime(RpmProgram::External, false),
        ),
    ]);

    let plan = plan_native_transaction(
        &[installing_view(&bundle)],
        &[install_change()],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(plan.events.len(), 2);
    assert_eq!(plan.events[0].stage, NativeEventStage::PackagePreInstall);
    assert_eq!(plan.events[1].stage, NativeEventStage::PackagePostInstall);
}

#[test]
fn rpm_sysusers_is_an_exact_pre_payload_compatibility_command() {
    let mut sysusers = entry(
        "rpm:%sysusers:0",
        LifecyclePath::RpmSysusers,
        runtime(RpmProgram::Sysusers, true),
    );
    sysusers.interpreter = "/usr/bin/systemd-sysusers".to_string();
    sysusers.rpm_sysusers = Some(RpmSysusersMetadata {
        source_path: Some("/usr/lib/sysusers.d/owner.conf".to_string()),
        lines: vec!["u! owner - \"Owner service\" /var/lib/owner".to_string()],
        directives: vec![RpmSysusersDirective::User {
            name: "owner".to_string(),
            id: None,
            description: Some("Owner service".to_string()),
            home: Some("/var/lib/owner".to_string()),
            shell: None,
            locked: true,
        }],
    });

    let bundle = bundle(vec![sysusers]);
    let plan = plan_native_transaction(
        &[installing_view(&bundle)],
        &[install_change()],
        &NativeTransactionState::default(),
    )
    .unwrap();
    let event = plan.events.first().expect("sysusers event");

    assert_eq!(event.stage, NativeEventStage::RpmSysusers);
    assert_eq!(
        event.program,
        NativeEventProgram::Command {
            argv: vec![
                "/usr/bin/systemd-sysusers".to_string(),
                "--replace".to_string(),
                "/usr/lib/sysusers.d/owner.conf".to_string(),
                "-".to_string(),
            ]
        }
    );
    assert_eq!(
        event.stdin,
        b"u! owner - \"Owner service\" /var/lib/owner\n"
    );
}

#[test]
fn rpm_owned_body_engines_are_planned_for_typed_runtime_execution() {
    let mut transformed = runtime(RpmProgram::External, false);
    transformed.body_transforms = vec![RpmBodyTransform::MacroExpand];
    let transformed_bundle = bundle(vec![entry(
        "rpm:%post",
        LifecyclePath::PostInstall,
        transformed,
    )]);
    let transformed_plan = plan_native_transaction(
        &[installing_view(&transformed_bundle)],
        &[install_change()],
        &NativeTransactionState::default(),
    )
    .expect("transformed RPM is planned");
    assert_eq!(transformed_plan.events.len(), 1);

    let lua_bundle = bundle(vec![entry(
        "rpm:%post",
        LifecyclePath::PostInstall,
        runtime(RpmProgram::EmbeddedLua, false),
    )]);
    let lua_plan = plan_native_transaction(
        &[installing_view(&lua_bundle)],
        &[install_change()],
        &NativeTransactionState::default(),
    )
    .expect("embedded Lua RPM is planned");
    assert_eq!(lua_plan.events.len(), 1);
}

#[test]
fn transaction_path_delta_preserves_old_new_added_retained_and_removed_sets() {
    let change = change(
        "payload",
        Some("1"),
        Some("2"),
        &["usr/lib/old", "usr/lib/retained"],
        &["usr/lib/new", "usr/lib/retained"],
        1,
        1,
        7,
    );

    assert_eq!(
        change.old_paths,
        BTreeSet::from(["usr/lib/old".to_string(), "usr/lib/retained".to_string()])
    );
    assert_eq!(
        change.new_paths,
        BTreeSet::from(["usr/lib/new".to_string(), "usr/lib/retained".to_string()])
    );
    assert_eq!(
        change.added_paths(),
        BTreeSet::from(["usr/lib/new".to_string()])
    );
    assert_eq!(
        change.retained_paths(),
        BTreeSet::from(["usr/lib/retained".to_string()])
    );
    assert_eq!(
        change.removed_paths(),
        BTreeSet::from(["usr/lib/old".to_string()])
    );
}

#[test]
fn installed_rpm_trigger_owner_reacts_to_changed_target_with_exact_counts() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%triggerin:target",
        RpmTriggerKind::Package,
        RpmTriggerAction::Install,
        "target",
    )]);
    let target_change = change("target", None, Some("2"), &[], &[], 2, 3, 4);

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "5",
            instances_after: 2,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[target_change],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].rpm_trigger_owner,
        Some(RpmTriggerOwner::InstalledDatabase)
    );
    assert_eq!(plan.events[0].args, ["2", "3"]);
    assert_eq!(plan.events[0].matched_targets, ["target"]);
}

#[test]
fn changed_rpm_trigger_owner_reacts_to_already_installed_target() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%triggerin:target",
        RpmTriggerKind::Package,
        RpmTriggerAction::Install,
        "target",
    )]);
    let owner_change = change("owner", None, Some("1"), &[], &[], 0, 1, 3);

    let plan = plan_native_transaction(
        &[installing_view(&owner_bundle)],
        &[owner_change],
        &NativeTransactionState {
            installed_packages_before: vec![
                NativeInstalledPackage {
                    package_name: "target".to_string(),
                    package_version: "1".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
                NativeInstalledPackage {
                    package_name: "target".to_string(),
                    package_version: "2".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
            ],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].rpm_trigger_owner,
        Some(RpmTriggerOwner::Transaction)
    );
    assert_eq!(plan.events[0].args, ["1", "2"]);
    assert_eq!(plan.events[0].matched_targets, ["target"]);
}

#[test]
fn rpm_upgrade_trigger_counts_preserve_prein_and_in_database_boundaries() {
    let owner_bundle = bundle(vec![
        trigger_entry(
            "rpm:%triggerprein:target",
            RpmTriggerKind::Package,
            RpmTriggerAction::PreInstall,
            "target",
        ),
        trigger_entry(
            "rpm:%triggerin:target",
            RpmTriggerKind::Package,
            RpmTriggerAction::Install,
            "target",
        ),
    ]);
    let target_change = change("target", Some("1"), Some("2"), &[], &[], 1, 1, 4);

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "5",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[target_change],
        &NativeTransactionState {
            installed_packages_before: vec![
                NativeInstalledPackage {
                    package_name: "trigger-owner".to_string(),
                    package_version: "5".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
                NativeInstalledPackage {
                    package_name: "target".to_string(),
                    package_version: "1".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
            ],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .map(|event| (event.stage, event.args.clone()))
            .collect::<Vec<_>>(),
        [
            (
                NativeEventStage::RpmTriggerPreInstall,
                vec!["1".to_string(), "1".to_string()]
            ),
            (
                NativeEventStage::RpmTriggerInstall,
                vec!["1".to_string(), "2".to_string()]
            ),
        ]
    );
}

#[test]
fn changed_upgrade_trigger_owner_uses_pre_and_installed_side_counts() {
    let owner_bundle = bundle(vec![
        trigger_entry(
            "rpm:%triggerprein:target",
            RpmTriggerKind::Package,
            RpmTriggerAction::PreInstall,
            "target",
        ),
        trigger_entry(
            "rpm:%triggerin:target",
            RpmTriggerKind::Package,
            RpmTriggerAction::Install,
            "target",
        ),
    ]);
    let owner_change = change("owner", Some("1"), Some("2"), &[], &[], 1, 1, 3);

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "owner",
            package_version: "2",
            instances_after: 1,
            role: NativeBundleRole::Installing,
            bundle: &owner_bundle,
        }],
        &[owner_change],
        &NativeTransactionState {
            installed_packages_before: vec![
                NativeInstalledPackage {
                    package_name: "owner".to_string(),
                    package_version: "1".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
                NativeInstalledPackage {
                    package_name: "target".to_string(),
                    package_version: "1".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
                NativeInstalledPackage {
                    package_name: "target".to_string(),
                    package_version: "2".to_string(),
                    package_arch: None,
                    paths: BTreeSet::new(),
                },
            ],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .map(|event| (event.stage, event.args.clone()))
            .collect::<Vec<_>>(),
        [
            (
                NativeEventStage::RpmTriggerPreInstall,
                vec!["1".to_string(), "2".to_string()]
            ),
            (
                NativeEventStage::RpmTriggerInstall,
                vec!["2".to_string(), "2".to_string()]
            ),
        ]
    );
}

#[test]
fn rpm_file_trigger_upgrade_uses_new_paths_for_install_and_old_only_for_uninstall() {
    let owner_bundle = bundle(vec![
        trigger_entry(
            "rpm:%filetriggerin:usr-lib",
            RpmTriggerKind::File,
            RpmTriggerAction::Install,
            "/usr/lib",
        ),
        trigger_entry(
            "rpm:%filetriggerun:usr-lib",
            RpmTriggerKind::File,
            RpmTriggerAction::Uninstall,
            "/usr/lib",
        ),
    ]);
    let target_change = change(
        "target",
        Some("1"),
        Some("2"),
        &["usr/lib/old", "usr/lib/retained"],
        &["usr/lib/new", "usr/lib/retained"],
        1,
        1,
        0,
    );

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[target_change],
        &NativeTransactionState::default(),
    )
    .unwrap();

    let install = plan
        .events
        .iter()
        .find(|event| event.stage == NativeEventStage::RpmFileTriggerInstallHigh)
        .unwrap();
    assert_eq!(install.stdin, b"/usr/lib/new\n/usr/lib/retained\n");
    let uninstall = plan
        .events
        .iter()
        .find(|event| event.stage == NativeEventStage::RpmFileTriggerUninstallHigh)
        .unwrap();
    assert_eq!(uninstall.stdin, b"/usr/lib/old\n");
}

#[test]
fn rpm_transaction_file_install_keeps_database_and_transaction_owner_passes_exact() {
    let database_bundle = bundle(vec![trigger_entry(
        "rpm:%transfiletriggerin:database",
        RpmTriggerKind::TransactionFile,
        RpmTriggerAction::Install,
        "/usr/lib",
    )]);
    let transaction_bundle = bundle(vec![trigger_entry(
        "rpm:%transfiletriggerin:transaction",
        RpmTriggerKind::TransactionFile,
        RpmTriggerAction::Install,
        "/usr/lib",
    )]);
    let target_change = change("target", None, Some("1"), &[], &["usr/lib/new"], 0, 1, 0);
    let owner_change = change("transaction-owner", None, Some("1"), &[], &[], 0, 1, 1);
    let state = NativeTransactionState {
        installed_packages_before: vec![NativeInstalledPackage {
            package_name: "database-owner".to_string(),
            package_version: "1".to_string(),
            package_arch: None,
            paths: BTreeSet::new(),
        }],
        installed_paths_after: BTreeSet::from([
            "usr/lib/new".to_string(),
            "usr/lib/unrelated".to_string(),
        ]),
        ..NativeTransactionState::default()
    };

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "database-owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installed,
                bundle: &database_bundle,
            },
            NativeBundleView {
                package_name: "transaction-owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &transaction_bundle,
            },
        ],
        &[target_change, owner_change],
        &state,
    )
    .unwrap();

    assert_eq!(plan.events.len(), 2);
    let database = &plan.events[0];
    assert_eq!(
        database.stage,
        NativeEventStage::RpmDatabaseTransactionFileTriggerInstall
    );
    assert_eq!(
        database.rpm_trigger_owner,
        Some(RpmTriggerOwner::InstalledDatabase)
    );
    assert_eq!(database.stdin, b"/usr/lib/new\n");

    let transaction = &plan.events[1];
    assert_eq!(
        transaction.stage,
        NativeEventStage::RpmTransactionFileTriggerInstall
    );
    assert_eq!(
        transaction.rpm_trigger_owner,
        Some(RpmTriggerOwner::Transaction)
    );
    assert_eq!(transaction.stdin, b"/usr/lib/new\n/usr/lib/unrelated\n");
}

#[test]
fn rpm_file_trigger_prefix_uses_exact_rpm_byte_prefix_semantics() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%filetriggerin:usr-lib",
        RpmTriggerKind::File,
        RpmTriggerAction::Install,
        "/usr/lib",
    )]);
    let target_change = change("target", None, Some("1"), &[], &["usr/libfoo"], 0, 1, 0);

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[target_change],
        &NativeTransactionState {
            installed_packages_before: vec![NativeInstalledPackage {
                package_name: "trigger-owner".to_string(),
                package_version: "1".to_string(),
                package_arch: None,
                paths: BTreeSet::new(),
            }],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].stdin, b"/usr/libfoo\n");
}

#[test]
fn rpm_file_trigger_trailing_slash_remains_significant() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%filetriggerin:usr-lib-slash",
        RpmTriggerKind::File,
        RpmTriggerAction::Install,
        "/usr/lib/",
    )]);
    let target_change = change(
        "target",
        None,
        Some("1"),
        &[],
        &["usr/libfoo", "usr/lib/child"],
        0,
        1,
        0,
    );

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[target_change],
        &NativeTransactionState {
            installed_packages_before: vec![NativeInstalledPackage {
                package_name: "trigger-owner".to_string(),
                package_version: "1".to_string(),
                package_arch: None,
                paths: BTreeSet::new(),
            }],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].stdin, b"/usr/lib/child\n");
}

#[test]
fn rpm_file_trigger_rejects_non_absolute_source_prefix() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%filetriggerin:relative",
        RpmTriggerKind::File,
        RpmTriggerAction::Install,
        "usr/lib",
    )]);

    let error = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[change("target", None, Some("1"), &[], &[], 0, 1, 0)],
        &NativeTransactionState::default(),
    )
    .unwrap_err();

    let error_chain = format!("{error:#}");
    assert!(error_chain.contains("must begin with '/'"), "{error_chain}");
}

#[test]
fn rpm_trigger_events_follow_source_transaction_order_not_input_or_package_order() {
    let owner_bundle = bundle(vec![trigger_entry(
        "rpm:%triggerin:target",
        RpmTriggerKind::Package,
        RpmTriggerAction::Install,
        "target",
    )]);
    let later = change("target", None, Some("2"), &[], &[], 1, 2, 9);
    let earlier = change("target", None, Some("1"), &[], &[], 0, 1, 2);

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "trigger-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &owner_bundle,
        }],
        &[later, earlier],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .map(|event| event.args[1].as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
}

#[test]
fn rpm_transaction_graph_interleaves_each_payload_with_its_lifecycle() {
    let package_a = bundle(vec![
        entry(
            "rpm:a:%pre",
            LifecyclePath::PreInstall,
            runtime(RpmProgram::External, true),
        ),
        entry(
            "rpm:a:%post",
            LifecyclePath::PostInstall,
            runtime(RpmProgram::External, false),
        ),
    ]);
    let package_b = bundle(vec![
        entry(
            "rpm:b:%pre",
            LifecyclePath::PreInstall,
            runtime(RpmProgram::External, true),
        ),
        entry(
            "rpm:b:%post",
            LifecyclePath::PostInstall,
            runtime(RpmProgram::External, false),
        ),
    ]);
    let changes = [
        change("package-a", None, Some("1"), &[], &["usr/lib/a"], 0, 1, 0),
        change("package-b", None, Some("1"), &[], &["usr/lib/b"], 0, 1, 1),
    ];

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "package-a",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &package_a,
            },
            NativeBundleView {
                package_name: "package-b",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &package_b,
            },
        ],
        &changes,
        &NativeTransactionState {
            installed_paths_after: BTreeSet::from([
                "usr/lib/a".to_string(),
                "usr/lib/b".to_string(),
            ]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    let trigger_boundaries = plan
        .graph
        .steps
        .iter()
        .filter_map(|step| match *step {
            NativeTransactionStep::PersistDebTriggerActivations {
                transaction_index,
                boundary,
                ..
            } => Some((transaction_index, boundary)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        trigger_boundaries,
        [
            (0, DebTriggerActivationBoundary::BeforePayloadEvents),
            (0, DebTriggerActivationBoundary::BeforePayloadMutation),
            (
                0,
                DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
            ),
            (0, DebTriggerActivationBoundary::BeforeConfigure),
            (1, DebTriggerActivationBoundary::BeforePayloadEvents),
            (1, DebTriggerActivationBoundary::BeforePayloadMutation),
            (
                1,
                DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
            ),
            (1, DebTriggerActivationBoundary::BeforeConfigure),
        ]
    );
    assert!(plan.deb.trigger_activations.is_empty());
    assert!(plan.deb.trigger_unconfigurations.is_empty());
    assert!(plan.deb.trigger_batches.is_empty());

    let rendered = plan
        .graph
        .steps
        .iter()
        .filter_map(|step| match *step {
            NativeTransactionStep::RunEvent { event_index } => {
                let event = &plan.events[event_index];
                Some(format!("{}:{:?}", event.owner_package, event.stage))
            }
            NativeTransactionStep::PersistDebTriggerActivations { .. } => None,
            NativeTransactionStep::ApplyPayload { change_index } => {
                Some(format!("apply:{}", changes[change_index].package_name))
            }
            NativeTransactionStep::FinalizeOldPayload { change_index } => {
                Some(format!("finalize:{}", changes[change_index].package_name))
            }
            NativeTransactionStep::PurgeConfigFiles { .. } => {
                unreachable!("RPM transactions do not carry Debian config-purge boundaries")
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "package-a:PackagePreInstall",
            "apply:package-a",
            "package-a:PackagePostInstall",
            "finalize:package-a",
            "package-b:PackagePreInstall",
            "apply:package-b",
            "package-b:PackagePostInstall",
            "finalize:package-b",
        ]
    );
}
