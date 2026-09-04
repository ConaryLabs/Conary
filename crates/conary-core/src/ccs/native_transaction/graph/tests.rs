// conary-core/src/ccs/native_transaction/graph/tests.rs

use super::*;
use crate::ccs::native_transaction::NativeTransactionOperation;

fn change(
    package_name: &str,
    old_paths: &[&str],
    new_paths: &[&str],
    transaction_index: usize,
) -> NativeTransactionChange {
    NativeTransactionChange {
        package_name: package_name.to_string(),
        old_arch: None,
        new_arch: None,
        old_version: (!old_paths.is_empty()).then(|| "1".to_string()),
        new_version: (!new_paths.is_empty()).then(|| "1".to_string()),
        operation: match (old_paths.is_empty(), new_paths.is_empty()) {
            (true, false) => NativeTransactionOperation::Install,
            (false, true) => NativeTransactionOperation::Remove,
            (false, false) => NativeTransactionOperation::Upgrade,
            (true, true) => panic!("graph test change needs a payload"),
        },
        old_paths: old_paths.iter().map(|path| (*path).to_string()).collect(),
        new_paths: new_paths.iter().map(|path| (*path).to_string()).collect(),
        instances_before: u32::from(!old_paths.is_empty()),
        instances_after: u32::from(!new_paths.is_empty()),
        transaction_index,
    }
}

#[test]
fn path_projection_removes_then_reintroduces_a_future_ownership_transfer() {
    let changes = [
        change("old-owner", &["usr/bin/tool"], &[], 0),
        change("new-owner", &[], &["/usr/bin/tool"], 1),
    ];
    let graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
            NativeTransactionStep::RunEvent { event_index: 0 },
            NativeTransactionStep::RunEvent { event_index: 1 },
            NativeTransactionStep::ApplyPayload { change_index: 1 },
            NativeTransactionStep::RunEvent { event_index: 2 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 1 },
        ],
    };

    let path_capabilities = vec![NativeTransactionPathCapabilities::default(); changes.len()];
    let projections = graph
        .path_projections(
            3,
            &changes,
            &BTreeSet::from(["usr/bin/tool".to_string()]),
            &path_capabilities,
            &BTreeSet::new(),
        )
        .unwrap();
    let NativeEventPathProjection::Projected {
        explicitly_removed_paths,
        ..
    } = projections[0].as_ref().unwrap()
    else {
        panic!("event after removal must use a projected root");
    };
    assert!(explicitly_removed_paths.contains("usr/bin/tool"));
    let NativeEventPathProjection::Projected {
        introduced_paths,
        explicitly_removed_paths,
        ..
    } = projections[2].as_ref().unwrap()
    else {
        panic!("event after replacement must use a projected root");
    };
    assert!(introduced_paths.contains("usr/bin/tool"));
    assert!(!explicitly_removed_paths.contains("usr/bin/tool"));
}

#[test]
fn path_projection_preserves_an_already_applied_ownership_transfer() {
    let changes = [
        change("new-owner", &[], &["usr/bin/tool"], 0),
        change("old-owner", &["/usr/bin/tool"], &[], 1),
    ];
    let graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
            NativeTransactionStep::ApplyPayload { change_index: 1 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 1 },
            NativeTransactionStep::RunEvent { event_index: 0 },
        ],
    };

    let path_capabilities = vec![NativeTransactionPathCapabilities::default(); changes.len()];
    let projections = graph
        .path_projections(
            1,
            &changes,
            &BTreeSet::from(["usr/bin/tool".to_string()]),
            &path_capabilities,
            &BTreeSet::new(),
        )
        .unwrap();
    let NativeEventPathProjection::Projected {
        introduced_paths,
        explicitly_removed_paths,
        ..
    } = projections[0].as_ref().unwrap()
    else {
        panic!("event after transfer must use a projected root");
    };
    assert!(introduced_paths.contains("usr/bin/tool"));
    assert!(!explicitly_removed_paths.contains("usr/bin/tool"));
}

#[test]
fn path_capability_projection_obeys_payload_and_finalization_boundaries() {
    let install_changes = [change("shell-provider", &[], &["usr/bin/sh"], 0)];
    let install_capabilities = [NativeTransactionPathCapabilities {
        old_paths: BTreeSet::new(),
        new_paths: BTreeSet::from(["/bin/sh".to_string()]),
    }];
    let install_graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::RunEvent { event_index: 0 },
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::RunEvent { event_index: 1 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
        ],
    };

    let projections = install_graph
        .path_projections(
            2,
            &install_changes,
            &BTreeSet::from(["usr/bin/sh".to_string()]),
            &install_capabilities,
            &BTreeSet::from(["bin/sh".to_string()]),
        )
        .unwrap();
    assert_eq!(projections[0], Some(NativeEventPathProjection::CurrentRoot));
    let NativeEventPathProjection::Projected {
        introduced_paths,
        introduced_path_capabilities,
        explicitly_removed_path_capabilities,
        ..
    } = projections[1].as_ref().unwrap()
    else {
        panic!("event after provider payload must use a projected root");
    };
    assert!(introduced_paths.contains("usr/bin/sh"));
    assert!(introduced_path_capabilities.contains("bin/sh"));
    assert!(explicitly_removed_path_capabilities.is_empty());

    let remove_changes = [change("shell-provider", &["usr/bin/sh"], &[], 0)];
    let remove_capabilities = [NativeTransactionPathCapabilities {
        old_paths: BTreeSet::from(["/bin/sh".to_string()]),
        new_paths: BTreeSet::new(),
    }];
    let remove_graph = NativeTransactionGraph {
        steps: vec![
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
            NativeTransactionStep::RunEvent { event_index: 0 },
        ],
    };

    let projections = remove_graph
        .path_projections(
            1,
            &remove_changes,
            &BTreeSet::new(),
            &remove_capabilities,
            &BTreeSet::new(),
        )
        .unwrap();
    let NativeEventPathProjection::Projected {
        introduced_path_capabilities,
        explicitly_removed_path_capabilities,
        ..
    } = projections[0].as_ref().unwrap()
    else {
        panic!("event after provider removal must use a projected root");
    };
    assert!(introduced_path_capabilities.is_empty());
    assert!(explicitly_removed_path_capabilities.contains("bin/sh"));
}

#[test]
fn debian_trigger_activations_surround_every_source_element_mutation() {
    let changes = [change(
        "arch-package",
        &["usr/lib/old"],
        &["usr/lib/new"],
        7,
    )];
    let graph = build(&mut Vec::new(), &changes).unwrap();

    assert_eq!(
        graph.steps,
        [
            NativeTransactionStep::PersistDebTriggerActivations {
                change_index: 0,
                transaction_index: 7,
                boundary: DebTriggerActivationBoundary::BeforePayloadEvents,
            },
            NativeTransactionStep::PersistDebTriggerActivations {
                change_index: 0,
                transaction_index: 7,
                boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
            },
            NativeTransactionStep::ApplyPayload { change_index: 0 },
            NativeTransactionStep::PersistDebTriggerActivations {
                change_index: 0,
                transaction_index: 7,
                boundary: DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
            },
            NativeTransactionStep::FinalizeOldPayload { change_index: 0 },
            NativeTransactionStep::PersistDebTriggerActivations {
                change_index: 0,
                transaction_index: 7,
                boundary: DebTriggerActivationBoundary::BeforeConfigure,
            },
        ]
    );
}

#[test]
fn debian_purge_has_a_distinct_config_boundary_between_postrm_calls() {
    let mut purge = change("deb-package", &["etc/demo.conf"], &[], 7);
    purge.operation = NativeTransactionOperation::Purge;
    let event = |stage, order_key: &str| NativeTransactionEvent {
        owner_package: "deb-package".to_string(),
        owner_version: "1".to_string(),
        owner_arch: None,
        source_format: "deb".to_string(),
        stage,
        program: super::super::NativeEventProgram::BundleEntry {
            entry_id: order_key.to_string(),
        },
        args: Vec::new(),
        stdin: Vec::new(),
        matched_targets: Vec::new(),
        rpm_trigger_owner: None,
        deb_package_refcount: None,
        order_key: order_key.to_string(),
        placement: NativeEventPlacement::TransactionElement {
            transaction_index: 7,
        },
    };
    let mut events = vec![
        event(NativeEventStage::PackagePreRemove, "prerm-remove"),
        event(NativeEventStage::PackagePostRemove, "postrm-remove"),
        event(NativeEventStage::DebPostRemovePurge, "postrm-purge"),
    ];

    let graph = build(&mut events, &[purge]).unwrap();
    let rendered = graph
        .steps
        .iter()
        .map(|step| match *step {
            NativeTransactionStep::RunEvent { event_index } => {
                events[event_index].order_key.as_str()
            }
            NativeTransactionStep::PersistDebTriggerActivations {
                boundary: DebTriggerActivationBoundary::BeforePayloadEvents,
                ..
            } => "triggers-before-events",
            NativeTransactionStep::PersistDebTriggerActivations {
                boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
                ..
            } => "triggers-before-payload",
            NativeTransactionStep::PersistDebTriggerActivations {
                boundary: DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
                ..
            } => "triggers-before-finalize",
            NativeTransactionStep::PersistDebTriggerActivations {
                boundary: DebTriggerActivationBoundary::BeforeConfigure,
                ..
            } => "triggers-before-configure",
            NativeTransactionStep::ApplyPayload { .. } => "apply",
            NativeTransactionStep::FinalizeOldPayload { .. } => "finalize",
            NativeTransactionStep::PurgeConfigFiles { .. } => "purge-config",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "triggers-before-events",
            "triggers-before-payload",
            "apply",
            "prerm-remove",
            "triggers-before-finalize",
            "finalize",
            "triggers-before-configure",
            "postrm-remove",
            "purge-config",
            "postrm-purge",
        ]
    );
}
