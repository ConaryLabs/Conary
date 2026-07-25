// conary-core/src/transaction/package_relations/deconfiguration.rs

//! Typed dependency closure for package-relation deconfiguration.

use super::*;

#[derive(Clone)]
struct AvailabilityTransition {
    package: PackageRelationInstalledIdentity,
    triggering_incoming: PackageRelationIncomingIdentity,
    removing_conflict: Option<PackageRelationInstalledIdentity>,
    dependency_depth: usize,
    is_removal: bool,
}

/// Close relation side effects over hard installed dependencies.
///
/// This evaluates persisted typed expressions against exact package/provider
/// candidates. It deliberately does not ask the name-based removal solver to
/// guess a cascade: every deconfigured identity and its root cause are carried
/// into the native transaction.
pub(super) fn plan_dependent_deconfigurations(
    conn: &Connection,
    installed: &[InstalledCandidate],
    incoming: &[IncomingPackageRelations<'_>],
    removals: &[PackageRelationRemoval],
    deconfigurations: &mut BTreeMap<i64, PackageRelationDeconfiguration>,
) -> Result<()> {
    let removed_ids = removals
        .iter()
        .map(|removal| removal.trove_id)
        .collect::<BTreeSet<_>>();
    let installed_by_id = installed
        .iter()
        .map(|candidate| {
            (
                candidate.trove.id.expect("installed trove has id"),
                candidate,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let baseline = installed
        .iter()
        .map(InstalledCandidate::to_owned_relation_candidate)
        .collect::<Vec<_>>();

    loop {
        let transitions = availability_transitions(removals, deconfigurations);
        let final_excluded = transitions
            .iter()
            .map(|transition| transition.package.trove_id)
            .collect::<BTreeSet<_>>();
        let final_candidates = configured_candidate_set(installed, incoming, &final_excluded);
        let mut planned = Vec::new();

        for package in installed {
            let trove_id = package.trove.id.expect("installed trove has id");
            if removed_ids.contains(&trove_id)
                || deconfigurations.contains_key(&trove_id)
                || incoming_replaces_installed(incoming, package)
            {
                continue;
            }
            let requirements = InstalledRequirementGroup::find_by_trove(conn, trove_id)?
                .into_iter()
                .filter(|requirement| {
                    matches!(
                        requirement.kind,
                        RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
                    )
                })
                .collect::<Vec<_>>();
            let mut has_newly_broken_requirement = false;
            for requirement in &requirements {
                if requirement_is_satisfied(requirement, &baseline)?
                    && !requirement_is_satisfied(requirement, &final_candidates)?
                {
                    has_newly_broken_requirement = true;
                    break;
                }
            }
            if !has_newly_broken_requirement {
                continue;
            }

            let mut excluded = BTreeSet::new();
            let mut candidates = configured_candidate_set(installed, incoming, &excluded);
            let mut cause = None;
            for transition in &transitions {
                let before = candidates;
                excluded.insert(transition.package.trove_id);
                candidates = configured_candidate_set(installed, incoming, &excluded);
                let mut broken_requirement = None;
                for requirement in &requirements {
                    if requirement_is_satisfied(requirement, &baseline)?
                        && requirement_is_satisfied(requirement, &before)?
                        && !requirement_is_satisfied(requirement, &candidates)?
                        && !requirement_is_satisfied(requirement, &final_candidates)?
                    {
                        broken_requirement = Some(requirement.clone());
                        break;
                    }
                }
                if let Some(requirement) = broken_requirement {
                    cause = Some((transition.clone(), requirement));
                    break;
                }
            }

            let Some((transition, requirement)) = cause else {
                continue;
            };
            planned.push((
                trove_id,
                transition,
                requirement.kind,
                requirement.requirement.native_text,
            ));
        }

        if planned.is_empty() {
            break;
        }
        for (trove_id, transition, kind, native_text) in planned {
            let package = installed_by_id
                .get(&trove_id)
                .expect("planned deconfiguration target is installed");
            insert_deconfiguration(
                deconfigurations,
                package,
                transition.triggering_incoming,
                PackageRelationDeconfigurationCause::Dependency {
                    kind,
                    native_text,
                    removing_conflict: transition.removing_conflict,
                },
                transition.dependency_depth + 1,
            );
        }
    }
    Ok(())
}

fn availability_transitions(
    removals: &[PackageRelationRemoval],
    deconfigurations: &BTreeMap<i64, PackageRelationDeconfiguration>,
) -> Vec<AvailabilityTransition> {
    let mut transitions = deconfigurations
        .values()
        .map(|deconfiguration| AvailabilityTransition {
            package: deconfiguration.package.clone(),
            triggering_incoming: deconfiguration.triggering_incoming.clone(),
            removing_conflict: match &deconfiguration.cause {
                PackageRelationDeconfigurationCause::Breaks { .. } => None,
                PackageRelationDeconfigurationCause::Dependency {
                    removing_conflict, ..
                } => removing_conflict.clone(),
            },
            dependency_depth: deconfiguration.dependency_depth,
            is_removal: false,
        })
        .chain(removals.iter().map(|removal| {
            let identity = PackageRelationInstalledIdentity {
                trove_id: removal.trove_id,
                package_name: removal.package_name.clone(),
                package_version: removal.package_version.clone(),
                package_architecture: removal.package_architecture.clone(),
            };
            AvailabilityTransition {
                package: identity.clone(),
                triggering_incoming: removal.triggering_incoming.clone(),
                removing_conflict: Some(identity),
                dependency_depth: 0,
                is_removal: true,
            }
        }))
        .collect::<Vec<_>>();
    transitions.sort_by(|left, right| {
        left.triggering_incoming
            .transaction_index
            .cmp(&right.triggering_incoming.transaction_index)
            .then_with(|| left.is_removal.cmp(&right.is_removal))
            .then_with(|| right.dependency_depth.cmp(&left.dependency_depth))
            .then_with(|| left.package.trove_id.cmp(&right.package.trove_id))
    });
    transitions
}

fn configured_candidate_set(
    installed: &[InstalledCandidate],
    incoming: &[IncomingPackageRelations<'_>],
    excluded_trove_ids: &BTreeSet<i64>,
) -> Vec<OwnedPackageRelationCandidate> {
    let mut candidates = installed
        .iter()
        .filter(|candidate| {
            !excluded_trove_ids.contains(
                &candidate
                    .trove
                    .id
                    .expect("configured candidate is installed"),
            ) && !incoming_replaces_installed(incoming, candidate)
        })
        .map(InstalledCandidate::to_owned_relation_candidate)
        .collect::<Vec<_>>();
    candidates.extend(incoming.iter().map(owned_incoming_candidate));
    candidates
}

fn requirement_is_satisfied(
    requirement: &InstalledRequirementGroup,
    candidates: &[OwnedPackageRelationCandidate],
) -> Result<bool> {
    expression_matches_candidate_set(
        &requirement.requirement.expression,
        requirement.version_scheme,
        candidates,
    )
    .map_err(Error::ResolutionError)
}
