// conary-core/src/resolver/sat/relations.rs

//! Exact post-solve conflict and replacement planning.

use std::collections::BTreeMap;

use resolvo::SolvableId;

use crate::error::{Error, Result};
use crate::repository::package_relation::{
    OwnedPackageRelationCandidate, OwnedPackageRelationProvide, PackageRelationCandidate,
    PackageRelationProvide, expression_matches_candidate_set, minimum_conflict_removal_indices,
    minimum_relation_addition_indices, relation_matches_candidate,
};
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provider::{ConaryProvider, SolverRelation};

use super::{SatPackage, SatRelationRemoval, SatSource};

pub(super) struct RelationPlan {
    pub removals: Vec<SatRelationRemoval>,
    pub conflict: Option<String>,
}

pub(super) fn plan_selected_relations(
    provider: &ConaryProvider<'_>,
    selected: &[SolvableId],
) -> Result<RelationPlan> {
    let selected_new = selected
        .iter()
        .copied()
        .filter(|id| provider.get_solvable(*id).installed_trove_id.is_none())
        .collect::<Vec<_>>();
    let selected_new_names = selected_new
        .iter()
        .map(|id| provider.get_solvable(*id).name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let installed = provider
        .solvable_ids()
        .iter()
        .copied()
        .filter(|id| provider.get_solvable(*id).installed_trove_id.is_some())
        .collect::<Vec<_>>();
    let selected_ids = selected
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mut removals = BTreeMap::new();

    for selected_id in &selected_new {
        let replacement = provider.get_solvable(*selected_id);
        for relation in provider.get_relation_list(*selected_id)? {
            if relation.relation.kind.removes_matching_packages() {
                for candidate_id in &selected_new {
                    if candidate_id == selected_id {
                        continue;
                    }
                    let candidate = provider.get_solvable(*candidate_id);
                    if candidate.name != replacement.name
                        && relation_matches_package(relation, candidate)?
                    {
                        return Ok(RelationPlan {
                            removals: Vec::new(),
                            conflict: Some(conflict_message(replacement, candidate, relation)),
                        });
                    }
                }
                for installed_id in &installed {
                    let existing = provider.get_solvable(*installed_id);
                    if replacement.name == existing.name
                        || selected_new_names.contains(existing.name.as_str())
                        || !relation_matches_package(relation, existing)?
                    {
                        continue;
                    }
                    if selected_ids.contains(installed_id) {
                        return Ok(RelationPlan {
                            removals: Vec::new(),
                            conflict: Some(conflict_message(replacement, existing, relation)),
                        });
                    }
                    insert_removal(&mut removals, existing, &[replacement], relation);
                }
                continue;
            }

            let removable_ids = installed
                .iter()
                .copied()
                .filter(|installed_id| !selected_ids.contains(installed_id))
                .filter(|installed_id| {
                    let name = provider.get_solvable(*installed_id).name.as_str();
                    name != replacement.name && !selected_new_names.contains(name)
                })
                .collect::<Vec<_>>();
            let fixed_ids = selected
                .iter()
                .copied()
                .filter(|candidate_id| *candidate_id != *selected_id)
                .filter(|candidate_id| {
                    provider.get_solvable(*candidate_id).name != replacement.name
                })
                .collect::<Vec<_>>();
            let removable = removable_ids
                .iter()
                .map(|id| owned_candidate(provider.get_solvable(*id)))
                .collect::<Vec<_>>();
            let fixed = fixed_ids
                .iter()
                .map(|id| owned_candidate(provider.get_solvable(*id)))
                .collect::<Vec<_>>();
            let removal_indices = match minimum_conflict_removal_indices(
                &relation.relation,
                relation.scheme,
                &removable,
                &fixed,
            ) {
                Ok(indices) => indices,
                Err(error) => {
                    return Ok(RelationPlan {
                        removals: Vec::new(),
                        conflict: Some(format!(
                            "{} {} relation '{}' is unsatisfiable with co-selected packages: {}",
                            replacement.name,
                            replacement.version,
                            relation
                                .relation
                                .native_text
                                .as_deref()
                                .unwrap_or("<typed relation>"),
                            error
                        )),
                    });
                }
            };
            for index in removal_indices {
                insert_removal(
                    &mut removals,
                    provider.get_solvable(removable_ids[index]),
                    &[replacement],
                    relation,
                );
            }
        }
    }

    // Conflicts and Breaks are mutual co-installation constraints. Installed
    // declarations therefore also constrain newly selected packages.
    for installed_id in &installed {
        let existing = provider.get_solvable(*installed_id);
        for relation in provider.get_relation_list(*installed_id)? {
            if relation.relation.kind.removes_matching_packages() {
                continue;
            }
            let before = installed
                .iter()
                .copied()
                .filter(|candidate_id| *candidate_id != *installed_id)
                .filter(|candidate_id| {
                    !selected_new_names.contains(provider.get_solvable(*candidate_id).name.as_str())
                })
                .map(|candidate_id| owned_candidate(provider.get_solvable(candidate_id)))
                .collect::<Vec<_>>();
            let mut after = before.clone();
            after.extend(
                selected_new
                    .iter()
                    .copied()
                    .filter(|candidate_id| {
                        provider.get_solvable(*candidate_id).name != existing.name
                    })
                    .map(|candidate_id| owned_candidate(provider.get_solvable(candidate_id))),
            );
            let matched_before = expression_matches_candidate_set(
                &relation.relation.expression,
                relation.scheme,
                &before,
            )
            .map_err(Error::ResolutionError)?;
            let matched_after = expression_matches_candidate_set(
                &relation.relation.expression,
                relation.scheme,
                &after,
            )
            .map_err(Error::ResolutionError)?;
            if matched_before || !matched_after {
                continue;
            }
            let addition_ids = selected_new
                .iter()
                .copied()
                .filter(|id| provider.get_solvable(*id).name != existing.name)
                .collect::<Vec<_>>();
            let additions = addition_ids
                .iter()
                .map(|id| owned_candidate(provider.get_solvable(*id)))
                .collect::<Vec<_>>();
            let causal_indices = minimum_relation_addition_indices(
                &relation.relation,
                relation.scheme,
                &before,
                &additions,
            )
            .map_err(Error::ResolutionError)?;
            let incoming = causal_indices
                .into_iter()
                .map(|index| provider.get_solvable(addition_ids[index]))
                .collect::<Vec<_>>();
            if selected_ids.contains(installed_id) {
                return Ok(RelationPlan {
                    removals: Vec::new(),
                    conflict: Some(conflict_message_many(existing, &incoming, relation)),
                });
            }
            insert_removal(&mut removals, existing, &incoming, relation);
        }
    }

    Ok(RelationPlan {
        removals: removals.into_values().collect(),
        conflict: None,
    })
}

fn insert_removal(
    removals: &mut BTreeMap<i64, SatRelationRemoval>,
    installed: &PackageIdentity,
    incoming: &[&PackageIdentity],
    relation: &SolverRelation,
) {
    let trove_id = installed.installed_trove_id.expect("filtered installed");
    let mut authorized_by = incoming
        .iter()
        .map(|package| sat_package(package))
        .collect::<Vec<_>>();
    authorized_by.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.version.cmp(&right.version))
    });
    authorized_by.dedup();
    let removal = SatRelationRemoval {
        trove_id,
        package: sat_package(installed),
        authorized_by,
        kind: relation.relation.kind,
        mode: relation
            .relation
            .kind
            .removal_mode()
            .expect("solver relation is negative"),
        native_text: relation.relation.native_text.clone(),
    };
    removals
        .entry(trove_id)
        .and_modify(|existing| {
            existing
                .authorized_by
                .extend(removal.authorized_by.iter().cloned());
            existing.authorized_by.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.version.cmp(&right.version))
            });
            existing.authorized_by.dedup();
            if removal.kind.removes_matching_packages()
                && !existing.kind.removes_matching_packages()
            {
                existing.kind = removal.kind;
                existing.mode = removal.mode;
                existing.native_text.clone_from(&removal.native_text);
            }
        })
        .or_insert(removal);
}

fn relation_matches_package(relation: &SolverRelation, package: &PackageIdentity) -> Result<bool> {
    let provides = package
        .provided_capabilities
        .iter()
        .map(|provide| PackageRelationProvide {
            name: &provide.name,
            version: provide.version.as_deref(),
            version_scheme: provide.version_scheme,
        })
        .collect::<Vec<_>>();
    relation_matches_candidate(
        &relation.relation,
        relation.scheme,
        &PackageRelationCandidate {
            name: &package.name,
            version: &package.version,
            version_scheme: package.version_scheme,
            provides: &provides,
        },
    )
    .map_err(Error::ResolutionError)
}

fn owned_candidate(package: &PackageIdentity) -> OwnedPackageRelationCandidate {
    OwnedPackageRelationCandidate {
        name: package.name.clone(),
        version: package.version.clone(),
        version_scheme: package.version_scheme,
        provides: package
            .provided_capabilities
            .iter()
            .map(|provide| OwnedPackageRelationProvide {
                name: provide.name.clone(),
                version: provide.version.clone(),
                version_scheme: provide.version_scheme,
            })
            .collect(),
    }
}

fn conflict_message(
    declaring: &PackageIdentity,
    matched: &PackageIdentity,
    relation: &SolverRelation,
) -> String {
    let relation_text = relation
        .relation
        .native_text
        .as_deref()
        .unwrap_or("<typed relation>");
    format!(
        "{} {} declares {} '{}' matching installed/selected {} {}",
        declaring.name,
        declaring.version,
        relation.relation.kind.as_str(),
        relation_text,
        matched.name,
        matched.version
    )
}

fn conflict_message_many(
    declaring: &PackageIdentity,
    matched: &[&PackageIdentity],
    relation: &SolverRelation,
) -> String {
    let relation_text = relation
        .relation
        .native_text
        .as_deref()
        .unwrap_or("<typed relation>");
    let matched = matched
        .iter()
        .map(|package| format!("{} {}", package.name, package.version))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} {} declares {} '{}' matching selected {}",
        declaring.name,
        declaring.version,
        relation.relation.kind.as_str(),
        relation_text,
        matched
    )
}

fn sat_package(package: &PackageIdentity) -> SatPackage {
    SatPackage {
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        version_scheme: package.version_scheme,
        repo_package_id: package.repo_package_id,
        repository_id: package.repository_id,
        repository_name: (!package.repository_name.is_empty())
            .then(|| package.repository_name.clone()),
        installed_trove_id: package.installed_trove_id,
        source: if package.installed_trove_id.is_some() {
            SatSource::Installed
        } else {
            SatSource::Repository
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn relation_kind_contract_keeps_removal_direction_explicit() {
        use crate::repository::dependency_model::RepositoryRequirementKind;

        assert!(RepositoryRequirementKind::Replace.removes_matching_packages());
        assert!(RepositoryRequirementKind::Obsolete.removes_matching_packages());
        assert!(!RepositoryRequirementKind::Conflict.removes_matching_packages());
        assert!(!RepositoryRequirementKind::Breaks.removes_matching_packages());
    }
}
