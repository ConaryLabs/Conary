// conary-core/src/ccs/native_transaction/tests/eopkg.rs

use super::*;

#[test]
fn transaction_runs_usysconf_once_after_all_payload_changes() {
    let first = bundle(SourceFormat::Eopkg, Vec::new());
    let second = bundle(SourceFormat::Eopkg, Vec::new());
    let changes = [
        NativeTransactionChange {
            package_name: "owner".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("1-1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/bin/owner".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 0,
        },
        NativeTransactionChange {
            package_name: "other".to_string(),
            old_arch: None,
            new_arch: None,
            old_version: None,
            new_version: Some("2-1".to_string()),
            operation: NativeTransactionOperation::Install,
            old_paths: BTreeSet::new(),
            new_paths: BTreeSet::from(["usr/bin/other".to_string()]),
            instances_before: 0,
            instances_after: 1,
            transaction_index: 1,
        },
    ];
    let plan = plan_native_transaction(
        &[
            NativeBundleView {
                package_name: "owner",
                package_version: "1-1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &first,
            },
            NativeBundleView {
                package_name: "other",
                package_version: "2-1",
                instances_after: 1,
                role: NativeBundleRole::Installing,
                bundle: &second,
            },
        ],
        &changes,
        &NativeTransactionState::default(),
    )
    .unwrap();

    let events = plan
        .events_at(NativeEventStage::EopkgSystemConfiguration)
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].placement, NativeEventPlacement::TransactionAfter);
    assert_eq!(
        events[0].program,
        NativeEventProgram::Command {
            argv: vec!["/usr/sbin/usysconf".to_string(), "run".to_string()]
        }
    );
    assert_eq!(
        events[0].matched_targets,
        vec!["owner".to_string(), "other".to_string()]
    );
}
