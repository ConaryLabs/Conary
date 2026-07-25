// conary-core/src/ccs/native_transaction/tests/debian/lifecycle/basic.rs

//! Debian happy-path install and upgrade invocation coverage.

use super::*;

#[test]
fn upgrade_uses_policy_defined_maintainer_script_arguments() {
    let installing = bundle(
        SourceFormat::Deb,
        vec![
            deb_entry(
                "deb:preinst",
                LifecyclePath::PreUpgrade,
                DebControlMember::Preinst,
            ),
            deb_entry(
                "deb:postinst",
                LifecyclePath::PostUpgrade,
                DebControlMember::Postinst,
            ),
        ],
    );
    let removing = bundle(
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
        &NativeTransactionState {
            deb_package_states: vec![deb_package_state(
                "owner",
                "1",
                None,
                DebPackageState::Installed,
            )],
            deb_change_indices: BTreeSet::from([0]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(
        event_stages_and_args(&plan),
        vec![
            (
                NativeEventStage::DebPreRemoveUpgrade,
                vec!["upgrade".to_string(), "2".to_string()]
            ),
            (
                NativeEventStage::PackagePreInstall,
                vec!["upgrade".to_string(), "1".to_string(), "2".to_string()]
            ),
            (
                NativeEventStage::DebPostRemoveUpgrade,
                vec!["upgrade".to_string(), "2".to_string()]
            ),
            (
                NativeEventStage::DebPostInstall,
                vec!["configure".to_string(), "1".to_string()]
            ),
        ]
    );
}

#[test]
fn fresh_install_omits_absent_configure_version_argument() {
    let installing = bundle(
        SourceFormat::Deb,
        vec![deb_entry(
            "deb:postinst",
            LifecyclePath::PostInstall,
            DebControlMember::Postinst,
        )],
    );
    let change = NativeTransactionChange {
        package_name: "owner".to_string(),
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
    };

    let plan = plan_native_transaction(
        &[NativeBundleView {
            package_name: "owner",
            package_version: "2",
            instances_after: 1,
            role: NativeBundleRole::Installing,
            bundle: &installing,
        }],
        &[change],
        &NativeTransactionState {
            deb_change_indices: BTreeSet::from([0]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(plan.events[0].stage, NativeEventStage::DebPostInstall);
    assert_eq!(plan.events[0].args, vec!["configure"]);
}

#[test]
fn config_preconfiguration_runs_before_preinst_with_installed_version() {
    let installing = bundle(
        SourceFormat::Deb,
        vec![
            deb_entry(
                "deb:config",
                // CCS keeps the broad lifecycle phase compact; the exact
                // preconfiguration boundary comes from DebControlMember.
                LifecyclePath::PostInstall,
                DebControlMember::Config,
            ),
            deb_entry(
                "deb:preinst",
                LifecyclePath::PreUpgrade,
                DebControlMember::Preinst,
            ),
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
        &NativeTransactionState {
            deb_package_states: vec![deb_package_state(
                "owner",
                "1",
                None,
                DebPackageState::Installed,
            )],
            deb_change_indices: BTreeSet::from([0]),
            ..NativeTransactionState::default()
        },
    )
    .unwrap();

    assert_eq!(
        event_stages_and_args(&plan),
        vec![
            (
                NativeEventStage::DebPreConfigure,
                vec!["configure".to_string(), "1".to_string()]
            ),
            (
                NativeEventStage::PackagePreInstall,
                vec!["upgrade".to_string(), "1".to_string(), "2".to_string()]
            ),
        ]
    );
    let rendered = plan
        .graph
        .steps
        .iter()
        .filter_map(|step| match *step {
            NativeTransactionStep::RunEvent { event_index } => Some(plan.events[event_index].stage),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rendered,
        [
            NativeEventStage::DebPreConfigure,
            NativeEventStage::PackagePreInstall,
        ]
    );
}
