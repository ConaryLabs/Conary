// conary-core/src/ccs/native_transaction/tests.rs
use super::*;
use crate::ccs::native_lifecycle::{
    ArchHookActionMetadata, ArchHookMetadata, ArchHookOperation, ArchHookTriggerMetadata,
    ArchHookTriggerType, ArchHookWhen, ArchInstallMetadata, ArchInstallSelection, DebControlMember,
    DebMaintainerArgumentMetadata, DebMaintainerArgumentValue, DebMaintainerInvocationMetadata,
    DebMaintainerMetadata, DebMaintainerMode, DebTriggerAwaitMode, DebTriggerDirective,
    DebTriggerMetadata, LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation,
    NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata, RpmTriggerAction,
    RpmTriggerKind, RpmTriggerMetadata, RpmTriggerTargetConstraint, ScriptletFidelity,
    SourceFormat, TransactionOrder, VersionScheme,
};
use std::collections::BTreeSet;

#[path = "tests/arch_deconfiguration.rs"]
mod arch_deconfiguration;

fn bundle(format: SourceFormat, entries: Vec<NativeLifecycleEntry>) -> NativeLifecycleBundle {
    let mut entries = entries;
    let version_scheme = match &format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
    };
    if format == SourceFormat::Rpm {
        for entry in &mut entries {
            entry.rpm_runtime.get_or_insert_with(|| RpmRuntimeMetadata {
                program: RpmProgram::External,
                body_transforms: Vec::new(),
                critical: true,
                criticality: RpmCriticality::SlotDefault,
                raw_flags: 0,
                unknown_flags: 0,
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            });
        }
    }
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: crate::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: format,
        source_family: "fixture".to_string(),
        source_profile: None,
        source_release: None,
        source_arch: None,
        source_package: "owner".to_string(),
        source_version: "1".to_string(),
        source_checksum: None,
        version_scheme,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "test".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries,
    }
}

fn entry(id: &str) -> NativeLifecycleEntry {
    let body = "exit 0\n".to_string();
    NativeLifecycleEntry {
        id: id.to_string(),
        native_slot: id.to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::Trigger,
        lifecycle_paths: vec!["trigger".to_string()],
        interpreter: "/bin/sh".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: crate::hash::sha256_prefixed(body.as_bytes()),
        body,
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "trigger".to_string(),
            ..TransactionOrder::default()
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: None,
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn lifecycle_entry(id: &str, phase: LifecyclePath) -> NativeLifecycleEntry {
    let mut entry = entry(id);
    entry.phase = phase;
    entry.lifecycle_paths = vec![phase.as_str().to_string()];
    entry
}

fn deb_entry(
    id: &str,
    phase: LifecyclePath,
    control_member: DebControlMember,
) -> NativeLifecycleEntry {
    let mut entry = lifecycle_entry(id, phase);
    if control_member == DebControlMember::Triggers {
        entry.kind = NativeLifecycleEntryKind::ControlArtifact;
        entry.interpreter = "package-manager-control-artifact".to_string();
    }
    let modes = match control_member {
        DebControlMember::Config => vec![DebMaintainerMode::Configure],
        DebControlMember::Preinst => {
            vec![DebMaintainerMode::Install, DebMaintainerMode::Upgrade]
        }
        DebControlMember::Postinst => {
            vec![DebMaintainerMode::Configure, DebMaintainerMode::Triggered]
        }
        DebControlMember::Prerm | DebControlMember::Postrm => {
            vec![DebMaintainerMode::Remove, DebMaintainerMode::Upgrade]
        }
        DebControlMember::Triggers => Vec::new(),
    };
    entry.deb_maintainer = Some(DebMaintainerMetadata {
        control_member,
        invocations: modes
            .into_iter()
            .map(|mode| {
                let mut arguments = vec![DebMaintainerArgumentMetadata {
                    index: 1,
                    name: "action".to_string(),
                    value: DebMaintainerArgumentValue::Action,
                    literal: None,
                    required: true,
                }];
                let values = match (&control_member, &mode) {
                    (DebControlMember::Preinst, DebMaintainerMode::Install) => vec![
                        ("old-version", DebMaintainerArgumentValue::OldVersion, false),
                        ("new-version", DebMaintainerArgumentValue::NewVersion, false),
                    ],
                    (DebControlMember::Preinst, DebMaintainerMode::Upgrade) => vec![
                        ("old-version", DebMaintainerArgumentValue::OldVersion, true),
                        ("new-version", DebMaintainerArgumentValue::NewVersion, true),
                    ],
                    (
                        DebControlMember::Config | DebControlMember::Postinst,
                        DebMaintainerMode::Configure,
                    ) => vec![(
                        "most-recently-configured-version",
                        DebMaintainerArgumentValue::MostRecentlyConfiguredVersion,
                        true,
                    )],
                    (
                        DebControlMember::Prerm | DebControlMember::Postrm,
                        DebMaintainerMode::Upgrade,
                    ) => vec![("new-version", DebMaintainerArgumentValue::NewVersion, true)],
                    _ => Vec::new(),
                };
                arguments.extend(values.into_iter().enumerate().map(
                    |(offset, (name, value, required))| DebMaintainerArgumentMetadata {
                        index: offset + 2,
                        name: name.to_string(),
                        value,
                        literal: None,
                        required,
                    },
                ));
                DebMaintainerInvocationMetadata {
                    mode,
                    arguments,
                    lifecycle_paths: vec![entry.phase],
                }
            })
            .collect(),
        ..DebMaintainerMetadata::default()
    });
    entry
}

fn deb_triggers_entry(declarations: Vec<DebTriggerMetadata>) -> NativeLifecycleEntry {
    let mut entry = deb_entry(
        "deb:triggers",
        LifecyclePath::Trigger,
        DebControlMember::Triggers,
    );
    entry.body = declarations
        .iter()
        .map(|declaration| declaration.raw_line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    entry.body.push('\n');
    entry.body_sha256 = crate::hash::sha256_prefixed(entry.body.as_bytes());
    entry
        .deb_maintainer
        .as_mut()
        .expect("Debian triggers metadata")
        .trigger_declarations = declarations;
    entry
}

fn add_deb_invocation(
    entry: &mut NativeLifecycleEntry,
    mode: DebMaintainerMode,
    values: Vec<(&str, DebMaintainerArgumentValue, bool)>,
) {
    let metadata = entry
        .deb_maintainer
        .as_mut()
        .expect("Debian entry metadata");
    let mut arguments = vec![DebMaintainerArgumentMetadata {
        index: 1,
        name: "action".to_string(),
        value: DebMaintainerArgumentValue::Action,
        literal: None,
        required: true,
    }];
    arguments.extend(
        values
            .into_iter()
            .enumerate()
            .map(
                |(offset, (name, value, required))| DebMaintainerArgumentMetadata {
                    index: offset + 2,
                    name: name.to_string(),
                    value,
                    literal: None,
                    required,
                },
            ),
    );
    metadata.invocations.push(DebMaintainerInvocationMetadata {
        mode,
        arguments,
        lifecycle_paths: vec![LifecyclePath::PostRemove],
    });
}

fn recovery_path(
    recovery: &DebRecoveryResult,
    outcomes: &[bool],
) -> (Vec<Vec<String>>, DebRecoveryDisposition) {
    let mut events = Vec::new();
    let mut current = recovery;
    let mut outcomes = outcomes.iter();
    loop {
        match current {
            DebRecoveryResult::Disposition(disposition) => return (events, *disposition),
            DebRecoveryResult::Run(node) => {
                events.push(node.event.args.clone());
                current = if *outcomes.next().expect("recovery outcome") {
                    &node.on_success
                } else {
                    &node.on_failure
                };
            }
            DebRecoveryResult::PersistState(transition) => {
                current = &transition.next;
            }
        }
    }
}

fn arch_entry(id: &str, phase: LifecyclePath) -> NativeLifecycleEntry {
    let called_function = match &phase {
        LifecyclePath::PreInstall => "pre_install",
        LifecyclePath::PostInstall => "post_install",
        LifecyclePath::PreUpgrade => "pre_upgrade",
        LifecyclePath::PostUpgrade => "post_upgrade",
        LifecyclePath::PreRemove => "pre_remove",
        LifecyclePath::PostRemove => "post_remove",
        unsupported => panic!("unsupported Arch lifecycle test phase {unsupported:?}"),
    };
    let mut entry = lifecycle_entry(id, phase);
    entry.arch_install = Some(ArchInstallMetadata {
        install_digest: entry.body_sha256.clone(),
        called_function: called_function.to_string(),
        selection: ArchInstallSelection::LibalpmGrepV1,
    });
    entry
}

fn arch_hook_entry(
    id: &str,
    hook_path: &str,
    when: ArchHookWhen,
    argv: &[&str],
    depends: &[&str],
) -> NativeLifecycleEntry {
    let mut entry = entry(id);
    entry.kind = NativeLifecycleEntryKind::ControlArtifact;
    entry.interpreter = "package-manager-control-artifact".to_string();
    entry.arch_hook = Some(ArchHookMetadata {
        hook_path: hook_path.to_string(),
        triggers: vec![ArchHookTriggerMetadata {
            operations: vec![
                ArchHookOperation::Install,
                ArchHookOperation::Upgrade,
                ArchHookOperation::Remove,
            ],
            trigger_type: ArchHookTriggerType::Path,
            targets: vec!["usr/share/cache/*".to_string()],
        }],
        action: ArchHookActionMetadata {
            description: None,
            when,
            exec: argv.join(" "),
            argv: argv.iter().map(|value| (*value).to_string()).collect(),
            depends: depends
                .iter()
                .map(|dependency| (*dependency).to_string())
                .collect(),
            abort_on_fail: when == ArchHookWhen::PreTransaction,
            needs_targets: true,
        },
    });
    entry
}

fn upgrade_change() -> NativeTransactionChange {
    NativeTransactionChange {
        package_name: "owner".to_string(),
        old_arch: None,
        new_arch: None,
        old_version: Some("1".to_string()),
        new_version: Some("2".to_string()),
        operation: NativeTransactionOperation::Upgrade,
        old_paths: BTreeSet::from(["usr/bin/owner".to_string()]),
        new_paths: BTreeSet::from(["usr/bin/owner".to_string()]),
        instances_before: 1,
        instances_after: 1,
        transaction_index: 0,
    }
}

fn event_stages_and_args(plan: &NativeTransactionPlan) -> Vec<(NativeEventStage, Vec<String>)> {
    plan.events
        .iter()
        .map(|event| (event.stage, event.args.clone()))
        .collect()
}

mod debian;

#[test]
fn rpm_upgrade_uses_typed_transaction_and_package_lifecycle_order() {
    let installing = bundle(
        SourceFormat::Rpm,
        vec![
            lifecycle_entry("rpm:%pretrans", LifecyclePath::PreTransaction),
            lifecycle_entry("rpm:%pre", LifecyclePath::PreUpgrade),
            lifecycle_entry("rpm:%post", LifecyclePath::PostUpgrade),
            lifecycle_entry("rpm:%posttrans", LifecyclePath::PostTransaction),
        ],
    );
    let removing = bundle(
        SourceFormat::Rpm,
        vec![
            lifecycle_entry("rpm:%preuntrans", LifecyclePath::PreUntransaction),
            lifecycle_entry("rpm:%preun", LifecyclePath::PreRemove),
            lifecycle_entry("rpm:%postun", LifecyclePath::PostRemove),
            lifecycle_entry("rpm:%postuntrans", LifecyclePath::PostUntransaction),
        ],
    );

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "owner",
                package_version: "2",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &installing,
            },
            NativeBundleView {
                package_name: "owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Removing,
                bundle: &removing,
            },
        ],
        &[upgrade_change()],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(
        event_stages_and_args(&plan),
        vec![
            (NativeEventStage::RpmPreTransaction, vec!["2".to_string()]),
            (NativeEventStage::RpmPreUnTransaction, vec!["1".to_string()]),
            (NativeEventStage::PackagePreInstall, vec!["2".to_string()]),
            (NativeEventStage::PackagePostInstall, vec!["2".to_string()]),
            (NativeEventStage::PackagePreRemove, vec!["1".to_string()]),
            (NativeEventStage::PackagePostRemove, vec!["1".to_string()]),
            (NativeEventStage::RpmPostTransaction, vec!["2".to_string()]),
            (
                NativeEventStage::RpmPostUnTransaction,
                vec!["1".to_string()]
            ),
        ]
    );
}

#[test]
fn rpm_trigger_stages_match_rpm_priority_classes_and_postun_order() {
    assert_eq!(
        rpm_stage(
            RpmTriggerKind::File,
            RpmTriggerAction::Install,
            Some(10_000)
        )
        .unwrap(),
        NativeEventStage::RpmFileTriggerInstallHigh
    );
    assert_eq!(
        rpm_stage(RpmTriggerKind::File, RpmTriggerAction::Install, Some(9_999)).unwrap(),
        NativeEventStage::RpmFileTriggerInstallLow
    );
    assert_eq!(
        rpm_stage(
            RpmTriggerKind::Package,
            RpmTriggerAction::PostUninstall,
            None
        )
        .unwrap(),
        NativeEventStage::RpmTriggerPostUninstall
    );
    assert!(NativeEventStage::PackagePostRemove < NativeEventStage::RpmTriggerPostUninstall);
    assert!(
        NativeEventStage::RpmTriggerPostUninstall
            < NativeEventStage::RpmFileTriggerPostUninstallLow
    );
}

#[test]
fn arch_upgrade_uses_new_then_old_version_arguments() {
    let installing = bundle(
        SourceFormat::Arch,
        vec![
            arch_entry("arch:pre_upgrade", LifecyclePath::PreUpgrade),
            arch_entry("arch:post_upgrade", LifecyclePath::PostUpgrade),
        ],
    );

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "owner",
            package_version: "2",
            instances_after: 1,
            role: NativeBundleRole::Installing,
            bundle: &installing,
        }],
        &[upgrade_change()],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(
        event_stages_and_args(&plan),
        vec![
            (
                NativeEventStage::PackagePreInstall,
                vec!["2".to_string(), "1".to_string()]
            ),
            (
                NativeEventStage::PackagePostInstall,
                vec!["2".to_string(), "1".to_string()]
            ),
        ]
    );
}

#[test]
fn rpm_transaction_file_trigger_runs_once_with_prefix_matched_stdin() {
    let mut trigger_entry = entry("rpm:transfiletriggerin");
    trigger_entry.rpm_trigger = Some(RpmTriggerMetadata {
        kind: RpmTriggerKind::TransactionFile,
        target_constraints: vec![RpmTriggerTargetConstraint {
            package: "/usr/share/mime".to_string(),
            action: RpmTriggerAction::Install,
            operator: None,
            version: None,
            raw_flags: 0,
        }],
        priority: Some(100_000),
        path_prefixes: vec!["/usr/share/mime".to_string()],
    });
    let bundle = bundle(SourceFormat::Rpm, vec![trigger_entry]);
    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "shared-mime-info",
            package_version: "2",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &bundle,
        }],
        &[
            NativeTransactionChange {
                package_name: "icons-a".to_string(),
                old_arch: None,
                new_arch: None,
                old_version: None,
                new_version: Some("1".to_string()),
                operation: NativeTransactionOperation::Install,
                old_paths: BTreeSet::new(),
                new_paths: BTreeSet::from(["usr/share/mime/a.xml".to_string()]),
                instances_before: 0,
                instances_after: 1,
                transaction_index: 0,
            },
            NativeTransactionChange {
                package_name: "icons-b".to_string(),
                old_arch: None,
                new_arch: None,
                old_version: None,
                new_version: Some("1".to_string()),
                operation: NativeTransactionOperation::Install,
                old_paths: BTreeSet::new(),
                new_paths: BTreeSet::from(["/usr/share/mime/b.xml".to_string()]),
                instances_before: 0,
                instances_after: 1,
                transaction_index: 1,
            },
        ],
        &NativeTransactionState {
            installed_paths_after: BTreeSet::from([
                "/usr/share/mime/a.xml".to_string(),
                "/usr/share/mime/b.xml".to_string(),
                "/usr/share/mime/existing.xml".to_string(),
            ]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].stage,
        NativeEventStage::RpmDatabaseTransactionFileTriggerInstall
    );
    assert_eq!(plan.events[0].args, vec!["1"]);
    assert_eq!(
        plan.events[0].stdin,
        b"/usr/share/mime/a.xml\n/usr/share/mime/b.xml\n"
    );
}

#[test]
fn alpm_hook_uses_ordered_inversion_and_formal_argv() {
    let mut hook_entry = arch_hook_entry(
        "arch:hook",
        "/usr/share/libalpm/hooks/90-cache.hook",
        ArchHookWhen::PostTransaction,
        &["/usr/bin/cache", "--refresh"],
        &[],
    );
    hook_entry.arch_hook.as_mut().unwrap().triggers = vec![ArchHookTriggerMetadata {
        operations: vec![ArchHookOperation::Install],
        trigger_type: ArchHookTriggerType::Path,
        targets: vec![
            "usr/share/cache/*".to_string(),
            "!usr/share/cache/private/*".to_string(),
        ],
    }];
    let bundle = bundle(SourceFormat::Arch, vec![hook_entry]);
    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "cache-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &bundle,
        }],
        &[NativeTransactionChange {
            package_name: "data".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from([
                "usr/share/cache/public/a".to_string(),
                "usr/share/cache/private/key".to_string(),
            ]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        }],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(
        plan.events[0].matched_targets,
        vec!["usr/share/cache/public/a"]
    );
    assert_eq!(
        plan.events[0].program,
        NativeEventProgram::Command {
            argv: vec!["/usr/bin/cache".to_string(), "--refresh".to_string()]
        }
    );
}

#[test]
fn alpm_upgrade_path_hooks_receive_added_retained_and_removed_candidates_by_operation() {
    let mut install = arch_hook_entry(
        "arch:install-path",
        "/usr/share/libalpm/hooks/10-install.hook",
        ArchHookWhen::PostTransaction,
        &["/usr/bin/record", "install"],
        &[],
    );
    install.arch_hook.as_mut().unwrap().triggers[0].operations = vec![ArchHookOperation::Install];
    install.arch_hook.as_mut().unwrap().triggers[0].targets =
        vec!["usr/share/cache/new".to_string()];

    let mut upgrade = arch_hook_entry(
        "arch:upgrade-path",
        "/usr/share/libalpm/hooks/20-upgrade.hook",
        ArchHookWhen::PostTransaction,
        &["/usr/bin/record", "upgrade"],
        &[],
    );
    upgrade.arch_hook.as_mut().unwrap().triggers[0].operations = vec![ArchHookOperation::Upgrade];
    upgrade.arch_hook.as_mut().unwrap().triggers[0].targets =
        vec!["usr/share/cache/retained".to_string()];

    let mut remove = arch_hook_entry(
        "arch:remove-path",
        "/usr/share/libalpm/hooks/30-remove.hook",
        ArchHookWhen::PostTransaction,
        &["/usr/bin/record", "remove"],
        &[],
    );
    remove.arch_hook.as_mut().unwrap().triggers[0].operations = vec![ArchHookOperation::Remove];
    remove.arch_hook.as_mut().unwrap().triggers[0].targets =
        vec!["usr/share/cache/old".to_string()];

    let hook_bundle = bundle(SourceFormat::Arch, vec![install, upgrade, remove]);
    let change = NativeTransactionChange {
        package_name: "cache-data".to_string(),
        old_arch: None,
        new_arch: None,
        old_version: Some("1".to_string()),
        new_version: Some("2".to_string()),
        operation: NativeTransactionOperation::Upgrade,
        old_paths: BTreeSet::from([
            "usr/share/cache/old".to_string(),
            "usr/share/cache/retained".to_string(),
        ]),
        new_paths: BTreeSet::from([
            "usr/share/cache/new".to_string(),
            "usr/share/cache/retained".to_string(),
        ]),
        instances_before: 1,
        instances_after: 1,
        transaction_index: 0,
    };

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "hook-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &hook_bundle,
        }],
        &[change],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .map(|event| event.matched_targets.clone())
            .collect::<Vec<_>>(),
        vec![
            vec!["usr/share/cache/new".to_string()],
            vec!["usr/share/cache/retained".to_string()],
            vec!["usr/share/cache/old".to_string()],
        ]
    );
}

#[test]
fn alpm_pre_transaction_snapshot_uses_old_hook_during_upgrade() {
    let old = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:old-hook",
            "/usr/share/libalpm/hooks/40-cache.hook",
            ArchHookWhen::PreTransaction,
            &["/usr/bin/old-cache"],
            &[],
        )],
    );
    let new = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:new-hook",
            "/usr/share/libalpm/hooks/40-cache.hook",
            ArchHookWhen::PreTransaction,
            &["/usr/bin/new-cache"],
            &[],
        )],
    );

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Removing,
                bundle: &old,
            },
            NativeBundleView {
                package_name: "owner",
                package_version: "2",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &new,
            },
        ],
        &[NativeTransactionChange {
            package_name: "owner".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: Some("1".to_string()),
            new_version: Some("2".to_string()),
            operation: NativeTransactionOperation::Upgrade,
            old_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
            new_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
            instances_before: 1,
            instances_after: 1,
            transaction_index: 0,
        }],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].owner_version, "1");
    assert_eq!(
        plan.events[0].program,
        NativeEventProgram::Command {
            argv: vec!["/usr/bin/old-cache".to_string()]
        }
    );
}

#[test]
fn alpm_post_transaction_snapshot_uses_new_hook_during_upgrade() {
    let old = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:old-hook",
            "/usr/share/libalpm/hooks/40-cache.hook",
            ArchHookWhen::PostTransaction,
            &["/usr/bin/old-cache"],
            &[],
        )],
    );
    let new = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:new-hook",
            "/usr/share/libalpm/hooks/40-cache.hook",
            ArchHookWhen::PostTransaction,
            &["/usr/bin/new-cache"],
            &[],
        )],
    );

    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Removing,
                bundle: &old,
            },
            NativeBundleView {
                package_name: "owner",
                package_version: "2",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &new,
            },
        ],
        &[NativeTransactionChange {
            package_name: "owner".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: Some("1".to_string()),
            new_version: Some("2".to_string()),
            operation: NativeTransactionOperation::Upgrade,
            old_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
            new_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
            instances_before: 1,
            instances_after: 1,
            transaction_index: 0,
        }],
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert_eq!(plan.events.len(), 1);
    assert_eq!(plan.events[0].owner_version, "2");
    assert_eq!(
        plan.events[0].program,
        NativeEventProgram::Command {
            argv: vec!["/usr/bin/new-cache".to_string()]
        }
    );
}

#[test]
fn alpm_hook_rejects_target_policy_paths_from_source_lifecycle_state() {
    let local = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:local-hook",
            "/etc/pacman.d/hooks/40-cache.hook",
            ArchHookWhen::PostTransaction,
            &["/usr/local/bin/local-cache"],
            &[],
        )],
    );

    let error = plan_native_transaction(
        &[NativeBundleView {
            package_name: "local-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &local,
        }],
        &[NativeTransactionChange {
            package_name: "data".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        }],
        &NativeTransactionState::default(),
    )
    .unwrap_err();

    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("/usr/share/libalpm/hooks/<basename>.hook"),
        "{error_chain}"
    );
}

#[test]
fn alpm_hook_depends_uses_declared_capability_and_arch_version_ordering() {
    let hook = bundle(
        SourceFormat::Arch,
        vec![arch_hook_entry(
            "arch:versioned-hook",
            "/usr/share/libalpm/hooks/40-cache.hook",
            ArchHookWhen::PostTransaction,
            &["/usr/bin/cache"],
            &["cache-provider>=2:1.0-1"],
        )],
    );
    let bundles = [NativeBundleView {
        package_name: "hook-owner",
        package_version: "1",
        instances_after: 1,
        role: NativeBundleRole::Installed,
        bundle: &hook,
    }];
    let changes = [NativeTransactionChange {
        package_name: "data".to_string(),
        old_arch: None,
        new_arch: None,
        old_version: None,
        new_version: Some("1".to_string()),
        operation: NativeTransactionOperation::Install,
        old_paths: BTreeSet::new(),
        new_paths: BTreeSet::from(["usr/share/cache/index".to_string()]),
        instances_before: 0,
        instances_after: 1,
        transaction_index: 0,
    }];

    let matching = plan_native_transaction(
        &bundles,
        &changes,
        &NativeTransactionState {
            installed_capabilities_after: vec![NativeInstalledCapability {
                name: "cache-provider".to_string(),
                version: Some("2:1.0-2".to_string()),
                version_scheme: crate::repository::versioning::VersionScheme::Arch,
            }],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();
    assert_eq!(matching.events.len(), 1);

    let foreign_version_contract = plan_native_transaction(
        &bundles,
        &changes,
        &NativeTransactionState {
            installed_capabilities_after: vec![NativeInstalledCapability {
                name: "cache-provider".to_string(),
                version: Some("2:1.0-2".to_string()),
                version_scheme: crate::repository::versioning::VersionScheme::Rpm,
            }],
            ..NativeTransactionState::default()
        },
    )
    .unwrap();
    assert!(foreign_version_contract.events.is_empty());
}
