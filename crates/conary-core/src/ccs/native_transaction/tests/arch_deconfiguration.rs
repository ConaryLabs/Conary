// conary-core/src/ccs/native_transaction/tests/arch_deconfiguration.rs

//! Source-family isolation for Debian state-only transaction elements.

use super::*;

#[test]
fn debian_deconfiguration_does_not_activate_alpm_package_or_path_hooks() {
    let mut hook = arch_hook_entry(
        "arch:deconfiguration-isolation",
        "/usr/share/libalpm/hooks/50-deconfiguration-isolation.hook",
        ArchHookWhen::PostTransaction,
        &["/usr/bin/record", "unexpected"],
        &[],
    );
    hook.arch_hook.as_mut().unwrap().triggers = vec![
        ArchHookTriggerMetadata {
            operations: vec![ArchHookOperation::Upgrade],
            trigger_type: ArchHookTriggerType::Package,
            targets: vec!["dependent".to_string()],
        },
        ArchHookTriggerMetadata {
            operations: vec![ArchHookOperation::Upgrade],
            trigger_type: ArchHookTriggerType::Path,
            targets: vec!["usr/share/cache/retained".to_string()],
        },
    ];
    let hook_bundle = bundle(SourceFormat::Arch, vec![hook]);
    let changes = [
        NativeTransactionChange {
            package_name: "incoming".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("2".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::new(),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        },
        NativeTransactionChange {
            package_name: "dependent".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: Some("1".to_string()),
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::DeconfigureInFavour {
                installing_transaction_index: 0,
                removing_conflict_transaction_index: None,
            },
            old_paths: BTreeSet::from(["usr/share/cache/retained".to_string()]),
            new_paths: BTreeSet::from(["usr/share/cache/retained".to_string()]),
            instances_before: 1,
            instances_after: 1,
            transaction_index: 1,
        },
    ];

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "hook-owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Installed,
            bundle: &hook_bundle,
        }],
        &changes,
        &NativeTransactionState::default(),
    )
    .unwrap();

    assert!(
        plan.events.is_empty(),
        "a Debian package-state transition must not masquerade as an ALPM upgrade"
    );
}
