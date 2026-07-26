// conary-core/src/ccs/native_transaction/tests/debian/refcount.rs

use super::*;

#[test]
fn multiarch_fresh_install_refcounts_follow_each_dpkg_execution_boundary() {
    let mut amd64 = bundle(
        SourceFormat::Deb,
        vec![
            deb_entry(
                "deb:preinst",
                LifecyclePath::PreInstall,
                DebControlMember::Preinst,
            ),
            deb_entry(
                "deb:postinst",
                LifecyclePath::PostInstall,
                DebControlMember::Postinst,
            ),
        ],
    );
    amd64.source_arch = Some("amd64".to_string());
    let mut arm64 = amd64.clone();
    arm64.source_arch = Some("arm64".to_string());
    let changes = [
        NativeTransactionChange {
            package_name: "owner".to_string(),
            old_arch: None,
            new_arch: Some("amd64".to_string()),
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::new(),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        },
        NativeTransactionChange {
            package_name: "owner".to_string(),
            old_arch: None,
            new_arch: Some("arm64".to_string()),
            old_version: None,
            new_version: Some("1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::new(),
            instances_before: 1,
            instances_after: 2,
            transaction_index: 1,
        },
    ];
    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "owner",
                package_version: "1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &amd64,
            },
            NativeBundleView {
                package_name: "owner",
                package_version: "1",
                instances_after: 2,
                role: NativeBundleRole::Installing,
                bundle: &arm64,
            },
        ],
        &changes,
        &NativeTransactionState {
            deb_change_indices: BTreeSet::from([0, 1]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .filter(|event| event.stage == NativeEventStage::PackagePreInstall)
            .map(|event| (
                event.owner_arch.as_deref().unwrap(),
                event.deb_package_refcount.unwrap(),
            ))
            .collect::<Vec<_>>(),
        [("amd64", 1), ("arm64", 2)]
    );
    assert_eq!(
        plan.events
            .iter()
            .filter(|event| event.stage == NativeEventStage::DebPostInstall)
            .map(|event| event.deb_package_refcount.unwrap())
            .collect::<Vec<_>>(),
        [2, 2]
    );
}

#[test]
fn ordinary_remove_keeps_config_files_instance_in_dpkg_refcount() {
    let mut removing = bundle(
        SourceFormat::Deb,
        vec![
            deb_entry(
                "deb:prerm",
                LifecyclePath::PreRemove,
                DebControlMember::Prerm,
            ),
            deb_entry(
                "deb:postrm",
                LifecyclePath::PostRemove,
                DebControlMember::Postrm,
            ),
        ],
    );
    removing.source_arch = Some("amd64".to_string());
    let change = NativeTransactionChange {
        package_name: "owner".to_string(),
        old_arch: Some("amd64".to_string()),
        new_arch: None,
        old_version: Some("1".to_string()),
        new_version: None,
        operation: NativeTransactionOperation::Remove,
        old_paths: BTreeSet::new(),
        new_paths: BTreeSet::new(),
        instances_before: 2,
        instances_after: 1,
        transaction_index: 0,
    };
    let state = NativeTransactionState {
        deb_package_states: vec![
            NativeDebPackageState {
                package_name: "owner".to_string(),
                package_version: "1".to_string(),
                source_arch: Some("amd64".to_string()),
                lifecycle_state: DebPackageState::Installed,
                pending_triggers: Vec::new(),
                awaited_packages: Vec::new(),
            },
            NativeDebPackageState {
                package_name: "owner".to_string(),
                package_version: "1".to_string(),
                source_arch: Some("arm64".to_string()),
                lifecycle_state: DebPackageState::ConfigFiles,
                pending_triggers: Vec::new(),
                awaited_packages: Vec::new(),
            },
        ],
        deb_change_indices: BTreeSet::from([0]),
        ..NativeTransactionState::default()
    };
    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "owner",
            package_version: "1",
            instances_after: 1,
            role: NativeBundleRole::Removing,
            bundle: &removing,
        }],
        &[change],
        &state,
    )
    .unwrap();

    assert_eq!(
        plan.events
            .iter()
            .filter_map(|event| event.deb_package_refcount)
            .collect::<Vec<_>>(),
        [2, 2]
    );
}
