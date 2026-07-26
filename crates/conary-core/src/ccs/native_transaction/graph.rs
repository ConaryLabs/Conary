// conary-core/src/ccs/native_transaction/graph.rs

//! Typed payload boundaries for native package-manager transaction execution.

use super::{
    NativeEventPlacement, NativeEventStage, NativeTransactionChange, NativeTransactionEvent,
};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

/// One executable node in a native transaction.
///
/// Event nodes index `NativeTransactionPlan::events`. Payload nodes index the
/// original `NativeTransactionChange` slice supplied to the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebTriggerActivationBoundary {
    BeforePayloadEvents,
    BeforePayloadMutation,
    BeforeOldPayloadFinalization,
    BeforeConfigure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTransactionStep {
    RunEvent {
        event_index: usize,
    },
    PersistDebTriggerActivations {
        change_index: usize,
        transaction_index: usize,
        boundary: DebTriggerActivationBoundary,
    },
    ApplyPayload {
        change_index: usize,
    },
    FinalizeOldPayload {
        change_index: usize,
    },
    /// Remove the Debian conffile set after ordinary payload removal and
    /// `postrm remove`, but before `postrm purge`.
    PurgeConfigFiles {
        change_index: usize,
    },
}

/// Package paths visible to one event after replaying preceding graph nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEventPathProjection {
    CurrentRoot,
    Projected {
        introduced_paths: BTreeSet<String>,
        explicitly_removed_paths: BTreeSet<String>,
    },
}

/// Exact transaction execution order, including filesystem visibility changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeTransactionGraph {
    pub steps: Vec<NativeTransactionStep>,
}

impl NativeTransactionGraph {
    /// Replay exact payload boundaries into one path projection per event.
    ///
    /// Paths are returned in normalized archive form without a leading slash.
    /// An ownership transfer only preserves a path when the new owner has
    /// already crossed its `ApplyPayload` boundary.
    pub fn path_projections(
        &self,
        events_len: usize,
        changes: &[NativeTransactionChange],
        final_owned_paths: &BTreeSet<String>,
    ) -> Result<Vec<Option<NativeEventPathProjection>>> {
        let final_owned_paths = normalize_paths(final_owned_paths)?;
        let mut projections = vec![None; events_len];
        let mut introduced_paths = BTreeSet::new();
        let mut explicitly_removed_paths = BTreeSet::new();
        let mut applied_new_owners = BTreeMap::<String, BTreeSet<usize>>::new();
        let mut payload_boundary_crossed = false;
        let mut finalized_changes = 0usize;
        let payload_change_count = changes
            .iter()
            .filter(|change| {
                !matches!(
                    change.operation,
                    super::NativeTransactionOperation::DeconfigureInFavour { .. }
                )
            })
            .count();

        for step in &self.steps {
            match *step {
                NativeTransactionStep::RunEvent { event_index } => {
                    let projection = projections.get_mut(event_index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "native transaction graph refers to missing event {event_index}"
                        )
                    })?;
                    if projection.is_some() {
                        bail!("native transaction graph repeats event {event_index}");
                    }
                    *projection = Some(if payload_boundary_crossed {
                        NativeEventPathProjection::Projected {
                            introduced_paths: introduced_paths.clone(),
                            explicitly_removed_paths: explicitly_removed_paths.clone(),
                        }
                    } else {
                        NativeEventPathProjection::CurrentRoot
                    });
                }
                NativeTransactionStep::PersistDebTriggerActivations {
                    change_index,
                    transaction_index,
                    ..
                } => {
                    let change = changes.get(change_index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "native transaction graph refers to missing trigger-activation change {change_index}"
                        )
                    })?;
                    if change.transaction_index != transaction_index {
                        bail!(
                            "native transaction graph trigger-activation step for change {change_index} carries source index {transaction_index}, expected {}",
                            change.transaction_index
                        );
                    }
                }
                NativeTransactionStep::ApplyPayload { change_index } => {
                    let change = changes.get(change_index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "native transaction graph refers to missing payload change {change_index}"
                        )
                    })?;
                    for path in normalize_paths(&change.new_paths)? {
                        applied_new_owners
                            .entry(path.clone())
                            .or_default()
                            .insert(change_index);
                        explicitly_removed_paths.remove(&path);
                        introduced_paths.insert(path);
                    }
                    payload_boundary_crossed = true;
                }
                NativeTransactionStep::FinalizeOldPayload { change_index } => {
                    let change = changes.get(change_index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "native transaction graph refers to missing old-payload change {change_index}"
                        )
                    })?;
                    let old_paths = normalize_paths(&change.old_paths)?;
                    let new_paths = normalize_paths(&change.new_paths)?;
                    for path in old_paths.difference(&new_paths) {
                        if applied_new_owners
                            .get(path)
                            .is_some_and(|owners| !owners.is_empty())
                        {
                            continue;
                        }
                        introduced_paths.remove(path);
                        explicitly_removed_paths.insert(path.clone());
                    }
                    finalized_changes += 1;
                    if finalized_changes == payload_change_count {
                        introduced_paths.retain(|path| final_owned_paths.contains(path));
                        explicitly_removed_paths.retain(|path| !final_owned_paths.contains(path));
                    }
                    payload_boundary_crossed = true;
                }
                NativeTransactionStep::PurgeConfigFiles { change_index } => {
                    let change = changes.get(change_index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "native transaction graph refers to missing config-purge change {change_index}"
                        )
                    })?;
                    if change.operation != super::NativeTransactionOperation::Purge {
                        bail!(
                            "native transaction graph purges config for non-purge change {change_index}"
                        );
                    }
                    payload_boundary_crossed = true;
                }
            }
        }

        if projections.iter().any(Option::is_none) {
            bail!("native transaction graph does not place every planned event");
        }
        Ok(projections)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ElementBoundary {
    BeforePayload,
    BeforeOldPayloadFinalization,
    AfterOldPayloadFinalization,
    AfterConfigPurge,
}

pub(super) fn build(
    events: &mut [NativeTransactionEvent],
    changes: &[NativeTransactionChange],
) -> Result<NativeTransactionGraph> {
    let change_indices_by_transaction_index = changes
        .iter()
        .enumerate()
        .map(|(change_index, change)| (change.transaction_index, change_index))
        .collect::<BTreeMap<_, _>>();

    for event in events.iter() {
        validate_placement(event, &change_indices_by_transaction_index)?;
    }
    events.sort_by(|left, right| {
        placement_order(left.placement)
            .cmp(&placement_order(right.placement))
            .then_with(|| left.stage.cmp(&right.stage))
            .then_with(|| left.order_key.cmp(&right.order_key))
            .then_with(|| left.owner_package.cmp(&right.owner_package))
            .then_with(|| left.owner_arch.cmp(&right.owner_arch))
    });

    let mut steps = Vec::with_capacity(events.len() + changes.len() * 6);
    append_transaction_events(&mut steps, events, NativeEventPlacement::TransactionBefore);

    let mut ordered_changes = changes.iter().enumerate().collect::<Vec<_>>();
    ordered_changes.sort_by_key(|(_, change)| change.transaction_index);
    for (change_index, change) in ordered_changes {
        if matches!(
            change.operation,
            super::NativeTransactionOperation::DeconfigureInFavour { .. }
        ) {
            continue;
        }
        steps.push(NativeTransactionStep::PersistDebTriggerActivations {
            change_index,
            transaction_index: change.transaction_index,
            boundary: DebTriggerActivationBoundary::BeforePayloadEvents,
        });
        append_element_events(
            &mut steps,
            events,
            change.transaction_index,
            ElementBoundary::BeforePayload,
        );
        steps.push(NativeTransactionStep::PersistDebTriggerActivations {
            change_index,
            transaction_index: change.transaction_index,
            boundary: DebTriggerActivationBoundary::BeforePayloadMutation,
        });
        steps.push(NativeTransactionStep::ApplyPayload { change_index });
        append_element_events(
            &mut steps,
            events,
            change.transaction_index,
            ElementBoundary::BeforeOldPayloadFinalization,
        );
        steps.push(NativeTransactionStep::PersistDebTriggerActivations {
            change_index,
            transaction_index: change.transaction_index,
            boundary: DebTriggerActivationBoundary::BeforeOldPayloadFinalization,
        });
        steps.push(NativeTransactionStep::FinalizeOldPayload { change_index });
        steps.push(NativeTransactionStep::PersistDebTriggerActivations {
            change_index,
            transaction_index: change.transaction_index,
            boundary: DebTriggerActivationBoundary::BeforeConfigure,
        });
        append_element_events(
            &mut steps,
            events,
            change.transaction_index,
            ElementBoundary::AfterOldPayloadFinalization,
        );
        if change.operation == super::NativeTransactionOperation::Purge {
            steps.push(NativeTransactionStep::PurgeConfigFiles { change_index });
            append_element_events(
                &mut steps,
                events,
                change.transaction_index,
                ElementBoundary::AfterConfigPurge,
            );
        }
    }

    append_transaction_events(&mut steps, events, NativeEventPlacement::TransactionAfter);
    Ok(NativeTransactionGraph { steps })
}

fn append_transaction_events(
    steps: &mut Vec<NativeTransactionStep>,
    events: &[NativeTransactionEvent],
    placement: NativeEventPlacement,
) {
    steps.extend(
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.placement == placement)
            .map(|(event_index, _)| NativeTransactionStep::RunEvent { event_index }),
    );
}

fn append_element_events(
    steps: &mut Vec<NativeTransactionStep>,
    events: &[NativeTransactionEvent],
    transaction_index: usize,
    boundary: ElementBoundary,
) {
    steps.extend(
        events
            .iter()
            .enumerate()
            .filter_map(|(event_index, event)| {
                if event.placement
                    != (NativeEventPlacement::TransactionElement { transaction_index })
                    || element_boundary(event.stage) != Some(boundary)
                {
                    return None;
                }
                Some(NativeTransactionStep::RunEvent { event_index })
            }),
    );
}

fn validate_placement(
    event: &NativeTransactionEvent,
    change_indices_by_transaction_index: &BTreeMap<usize, usize>,
) -> Result<()> {
    match event.placement {
        NativeEventPlacement::TransactionBefore if transaction_before_stage(event.stage) => Ok(()),
        NativeEventPlacement::TransactionAfter if transaction_after_stage(event.stage) => Ok(()),
        NativeEventPlacement::TransactionElement { transaction_index }
            if change_indices_by_transaction_index.contains_key(&transaction_index)
                && element_boundary(event.stage).is_some() =>
        {
            Ok(())
        }
        NativeEventPlacement::TransactionElement { transaction_index }
            if !change_indices_by_transaction_index.contains_key(&transaction_index) =>
        {
            bail!(
                "native event '{}' refers to missing transaction element {}",
                event.order_key,
                transaction_index
            )
        }
        _ => bail!(
            "native event '{}' places stage {:?} at an invalid transaction boundary",
            event.order_key,
            event.stage
        ),
    }
}

fn placement_order(placement: NativeEventPlacement) -> (u8, usize) {
    match placement {
        NativeEventPlacement::TransactionBefore => (0, 0),
        NativeEventPlacement::TransactionElement { transaction_index } => (1, transaction_index),
        NativeEventPlacement::TransactionAfter => (2, 0),
    }
}

fn transaction_before_stage(stage: NativeEventStage) -> bool {
    matches!(
        stage,
        NativeEventStage::ArchPreTransaction
            | NativeEventStage::RpmPreTransaction
            | NativeEventStage::RpmPreUnTransaction
            | NativeEventStage::RpmTransactionFileTriggerUninstall
    )
}

fn transaction_after_stage(stage: NativeEventStage) -> bool {
    matches!(
        stage,
        NativeEventStage::RpmPostTransaction
            | NativeEventStage::RpmPostUnTransaction
            | NativeEventStage::RpmDatabaseTransactionFileTriggerInstall
            | NativeEventStage::RpmTransactionFileTriggerPostUninstall
            | NativeEventStage::RpmTransactionFileTriggerInstall
            | NativeEventStage::DebAwaitedTriggerProcessing
            | NativeEventStage::DebTriggerProcessing
            | NativeEventStage::ArchPostTransaction
    )
}

fn element_boundary(stage: NativeEventStage) -> Option<ElementBoundary> {
    match stage {
        NativeEventStage::DebPreRemoveUpgrade
        | NativeEventStage::DebPreDeconfigure
        | NativeEventStage::DebPreRemoveInFavour
        | NativeEventStage::RpmSysusers
        | NativeEventStage::RpmTriggerPreInstall
        | NativeEventStage::DebPreConfigure
        | NativeEventStage::PackagePreInstall => Some(ElementBoundary::BeforePayload),
        NativeEventStage::RpmFileTriggerInstallHigh
        | NativeEventStage::DebPostRemoveUpgrade
        | NativeEventStage::PackagePostInstall
        | NativeEventStage::RpmTriggerInstall
        | NativeEventStage::RpmFileTriggerInstallLow
        | NativeEventStage::RpmFileTriggerUninstallHigh
        | NativeEventStage::RpmTriggerUninstall
        | NativeEventStage::PackagePreRemove
        | NativeEventStage::RpmFileTriggerUninstallLow => {
            Some(ElementBoundary::BeforeOldPayloadFinalization)
        }
        NativeEventStage::RpmFileTriggerPostUninstallHigh
        | NativeEventStage::DebPostInstall
        | NativeEventStage::PackagePostRemove
        | NativeEventStage::RpmTriggerPostUninstall
        | NativeEventStage::RpmFileTriggerPostUninstallLow => {
            Some(ElementBoundary::AfterOldPayloadFinalization)
        }
        NativeEventStage::DebPostRemovePurge => Some(ElementBoundary::AfterConfigPurge),
        NativeEventStage::DebPostRemoveDisappear => {
            Some(ElementBoundary::BeforeOldPayloadFinalization)
        }
        NativeEventStage::ArchPreTransaction
        | NativeEventStage::RpmPreTransaction
        | NativeEventStage::RpmPreUnTransaction
        | NativeEventStage::RpmTransactionFileTriggerUninstall
        | NativeEventStage::DebAwaitedTriggerProcessing
        | NativeEventStage::DebErrorRecovery
        | NativeEventStage::RpmPostTransaction
        | NativeEventStage::RpmPostUnTransaction
        | NativeEventStage::RpmDatabaseTransactionFileTriggerInstall
        | NativeEventStage::RpmTransactionFileTriggerPostUninstall
        | NativeEventStage::RpmTransactionFileTriggerInstall
        | NativeEventStage::DebTriggerProcessing
        | NativeEventStage::ArchPostTransaction => None,
    }
}

fn normalize_paths(paths: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    paths
        .iter()
        .map(|path| normalize_path(path))
        .collect::<Result<_>>()
}

fn normalize_path(path: &str) -> Result<String> {
    if path.contains('\0') {
        bail!("native transaction path contains NUL");
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => bail!("native transaction path '{path}' escapes its archive root"),
            component => components.push(component),
        }
    }
    if components.is_empty() {
        bail!("native transaction path '{path}' does not name a file");
    }
    Ok(components.join("/"))
}

#[cfg(test)]
mod tests {
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

        let projections = graph
            .path_projections(3, &changes, &BTreeSet::from(["usr/bin/tool".to_string()]))
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

        let projections = graph
            .path_projections(1, &changes, &BTreeSet::from(["usr/bin/tool".to_string()]))
            .unwrap();
        let NativeEventPathProjection::Projected {
            introduced_paths,
            explicitly_removed_paths,
        } = projections[0].as_ref().unwrap()
        else {
            panic!("event after transfer must use a projected root");
        };
        assert!(introduced_paths.contains("usr/bin/tool"));
        assert!(!explicitly_removed_paths.contains("usr/bin/tool"));
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
}
