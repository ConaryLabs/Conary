// conary-core/src/transaction/package_relations.rs

//! Installed-state planning for typed conflict and replacement relations.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use crate::db::models::{InstalledRequirementGroup, ProvideEntry, Trove};
use crate::error::{Error, Result};
use crate::packages::PackageFormat;
use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::dependency_model::{
    PackageRelationRemovalMode, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::package_relation::{
    OwnedPackageRelationCandidate, OwnedPackageRelationProvide, PackageRelationCandidate,
    PackageRelationProvide, expression_matches_candidate_set, minimum_conflict_removal_indices,
    minimum_relation_addition_indices, relation_matches_candidate,
};
use crate::repository::versioning::VersionScheme;

#[path = "package_relations/deconfiguration.rs"]
mod deconfiguration;

use deconfiguration::plan_dependent_deconfigurations;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRelationRemoval {
    pub trove_id: i64,
    pub package_name: String,
    pub package_version: String,
    pub package_architecture: Option<String>,
    /// Exact incoming transaction element before which this removal runs.
    pub triggering_incoming: PackageRelationIncomingIdentity,
    /// Exact incoming packages that jointly authorize this removal.
    pub incoming_packages: Vec<String>,
    /// Incoming Replace/Obsolete declarers that may claim overlapping payload
    /// ownership from this target.
    pub ownership_transfer_packages: Vec<String>,
    pub kind: RepositoryRequirementKind,
    pub mode: PackageRelationRemovalMode,
    pub native_text: Option<String>,
}

/// Exact incoming transaction identity carried by relation side effects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageRelationIncomingIdentity {
    pub transaction_index: usize,
    pub package_name: String,
    pub package_version: String,
    pub package_architecture: Option<String>,
}

/// Exact installed package identity carried by relation side effects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageRelationInstalledIdentity {
    pub trove_id: i64,
    pub package_name: String,
    pub package_version: String,
    pub package_architecture: Option<String>,
}

/// Why one installed package must leave Debian's configured state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageRelationDeconfigurationCause {
    /// An exact incoming `Breaks` expression matched the package.
    Breaks { native_text: Option<String> },
    /// Removing or deconfiguring another package invalidates a previously
    /// satisfied hard requirement.
    Dependency {
        kind: RepositoryRequirementKind,
        native_text: Option<String>,
        /// Debian's optional `removing <package> <version>` ABI suffix. This
        /// is present only when one exact relation removal is the root cause.
        removing_conflict: Option<PackageRelationInstalledIdentity>,
    },
}

/// One exact installed package that must be deconfigured before an incoming
/// transaction element can proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRelationDeconfiguration {
    pub package: PackageRelationInstalledIdentity,
    pub triggering_incoming: PackageRelationIncomingIdentity,
    pub cause: PackageRelationDeconfigurationCause,
    /// Dependency distance from the direct Breaks/removal cause. The planner
    /// orders larger depths first so dependents deconfigure before providers.
    pub dependency_depth: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageRelationPlan {
    pub removals: Vec<PackageRelationRemoval>,
    pub deconfigurations: Vec<PackageRelationDeconfiguration>,
}

/// Exact package facts needed to plan negative relations.
///
/// Batch installation keeps these facts after the source archive has been
/// parsed, so transaction planning does not need to reopen or reinterpret the
/// artifact.
#[derive(Clone, Copy)]
pub struct IncomingPackageRelations<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub architecture: Option<&'a str>,
    pub version_scheme: VersionScheme,
    pub provides: &'a [ProvidedCapability],
    pub relations: &'a [RepositoryRequirementGroup],
}

/// Plan native conflict and replacement effects against installed state.
///
/// This is read-only. The caller owns transaction ordering, lifecycle events,
/// payload ownership transfer, and deletion of `removals`.
pub fn plan_package_relations(
    conn: &Connection,
    incoming: &dyn PackageFormat,
    incoming_scheme: VersionScheme,
) -> Result<PackageRelationPlan> {
    let provides = incoming.resolution_capabilities()?;
    plan_package_relation_facts(
        conn,
        IncomingPackageRelations {
            name: incoming.name(),
            version: incoming.version(),
            architecture: incoming.architecture(),
            version_scheme: incoming_scheme,
            provides: &provides,
            relations: incoming.relations(),
        },
    )
}

/// Plan negative relations from already-parsed package facts.
pub fn plan_package_relation_facts(
    conn: &Connection,
    incoming: IncomingPackageRelations<'_>,
) -> Result<PackageRelationPlan> {
    plan_package_relation_batch_facts(conn, &[incoming])
}

/// Plan negative relations for one atomic set of already-parsed packages.
///
/// Incoming packages are fixed transaction selections. A Conflict/Breaks
/// expression may remove installed candidates, but cannot silently remove a
/// co-selected incoming package. Existing installed declarations are evaluated
/// against the complete incoming set so rich expressions spanning packages
/// cannot escape per-package planning.
pub fn plan_package_relation_batch_facts(
    conn: &Connection,
    incoming: &[IncomingPackageRelations<'_>],
) -> Result<PackageRelationPlan> {
    let incoming_owned = incoming
        .iter()
        .map(owned_incoming_candidate)
        .collect::<Vec<_>>();
    let installed = load_installed_candidates(conn)?;
    let mut removals = BTreeMap::new();
    let mut deconfigurations = BTreeMap::new();

    for (incoming_index, package) in incoming.iter().enumerate() {
        for relation in package.relations {
            match relation.kind {
                RepositoryRequirementKind::Conflict => {
                    let fixed = incoming_owned
                        .iter()
                        .enumerate()
                        .filter(|(candidate_index, _)| *candidate_index != incoming_index)
                        .map(|(_, candidate)| candidate.clone())
                        .collect::<Vec<_>>();
                    for installed_package in conflict_removal_set(
                        relation,
                        package.version_scheme,
                        &installed,
                        incoming,
                        &fixed,
                    )? {
                        insert_removal(
                            &mut removals,
                            installed_package,
                            incoming_identity(package, incoming_index),
                            vec![package.name.to_string()],
                            relation.kind,
                            relation.native_text.clone(),
                        );
                    }
                }
                RepositoryRequirementKind::Breaks => {
                    let fixed = incoming_owned
                        .iter()
                        .enumerate()
                        .filter(|(candidate_index, _)| *candidate_index != incoming_index)
                        .map(|(_, candidate)| candidate.clone())
                        .collect::<Vec<_>>();
                    for installed_package in conflict_removal_set(
                        relation,
                        package.version_scheme,
                        &installed,
                        incoming,
                        &fixed,
                    )? {
                        insert_deconfiguration(
                            &mut deconfigurations,
                            installed_package,
                            incoming_identity(package, incoming_index),
                            PackageRelationDeconfigurationCause::Breaks {
                                native_text: relation.native_text.clone(),
                            },
                            0,
                        );
                    }
                }
                RepositoryRequirementKind::Replace | RepositoryRequirementKind::Obsolete => {
                    for (candidate_index, candidate) in incoming_owned.iter().enumerate() {
                        if candidate_index == incoming_index {
                            continue;
                        }
                        let provides = candidate
                            .provides
                            .iter()
                            .map(|provide| PackageRelationProvide {
                                name: &provide.name,
                                version: provide.version.as_deref(),
                                version_scheme: provide.version_scheme,
                            })
                            .collect::<Vec<_>>();
                        if relation_matches_candidate(
                            relation,
                            package.version_scheme,
                            &PackageRelationCandidate {
                                name: &candidate.name,
                                version: &candidate.version,
                                version_scheme: candidate.version_scheme,
                                provides: &provides,
                            },
                        )
                        .map_err(Error::ResolutionError)?
                        {
                            return Err(Error::ResolutionError(format!(
                                "atomic install co-selects {} {} even though {} {} declares {} '{}'",
                                candidate.name,
                                candidate.version,
                                package.name,
                                package.version,
                                relation.kind.as_str(),
                                relation
                                    .native_text
                                    .as_deref()
                                    .unwrap_or("<typed relation>")
                            )));
                        }
                    }
                    for installed_package in &installed {
                        if incoming_replaces_installed(incoming, installed_package)
                            || !relation_matches_installed(
                                relation,
                                package.version_scheme,
                                installed_package,
                            )
                            .map_err(Error::ResolutionError)?
                        {
                            continue;
                        }
                        insert_removal(
                            &mut removals,
                            installed_package,
                            incoming_identity(package, incoming_index),
                            vec![package.name.to_string()],
                            relation.kind,
                            relation.native_text.clone(),
                        );
                    }
                }
                RepositoryRequirementKind::Depends
                | RepositoryRequirementKind::PreDepends
                | RepositoryRequirementKind::Optional
                | RepositoryRequirementKind::Recommends
                | RepositoryRequirementKind::Suggests
                | RepositoryRequirementKind::Supplements
                | RepositoryRequirementKind::Enhances
                | RepositoryRequirementKind::Build => {
                    return Err(Error::ResolutionError(format!(
                        "non-negative relation kind '{}' reached transaction planning",
                        relation.kind.as_str()
                    )));
                }
            }
        }
    }

    // Existing Conflicts are mutual removals. Existing Breaks deconfigure the
    // declaring installed package. Existing Replaces/Obsoletes are directional
    // history and positive dependencies never authorize relation side effects.
    for package in &installed {
        if incoming_replaces_installed(incoming, package) {
            continue;
        }
        for relation in InstalledRequirementGroup::find_by_trove(
            conn,
            package.trove.id.expect("installed trove has id"),
        )? {
            if !matches!(
                relation.kind,
                RepositoryRequirementKind::Conflict | RepositoryRequirementKind::Breaks
            ) {
                continue;
            }
            let before = installed
                .iter()
                .filter(|candidate| {
                    candidate.trove.id != package.trove.id
                        && !incoming_replaces_installed(incoming, candidate)
                })
                .map(InstalledCandidate::to_owned_relation_candidate)
                .collect::<Vec<_>>();
            if expression_matches_candidate_set(
                &relation.requirement.expression,
                relation.version_scheme,
                &before,
            )
            .map_err(Error::ResolutionError)?
            {
                continue;
            }
            let additions_with_indices = incoming_owned
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    !same_package_instance(
                        &incoming[*index],
                        package.trove.name.as_str(),
                        package.trove.architecture.as_deref(),
                    )
                })
                .map(|(index, candidate)| (index, candidate.clone()))
                .collect::<Vec<_>>();
            let additions = additions_with_indices
                .iter()
                .map(|(_, candidate)| candidate.clone())
                .collect::<Vec<_>>();
            let mut after = before.clone();
            after.extend_from_slice(&additions);
            if !expression_matches_candidate_set(
                &relation.requirement.expression,
                relation.version_scheme,
                &after,
            )
            .map_err(Error::ResolutionError)?
            {
                continue;
            }
            let causal_indices = minimum_relation_addition_indices(
                &relation.requirement,
                relation.version_scheme,
                &before,
                &additions,
            )
            .map_err(Error::ResolutionError)?;
            let triggering_index = causal_indices
                .iter()
                .map(|index| additions_with_indices[*index].0)
                .max()
                .ok_or_else(|| {
                    Error::ResolutionError(
                        "installed negative relation has no incoming cause".to_string(),
                    )
                })?;
            let incoming_packages = causal_indices
                .into_iter()
                .map(|index| additions_with_indices[index].1.name.clone())
                .collect::<Vec<_>>();
            match relation.kind {
                RepositoryRequirementKind::Conflict => insert_removal(
                    &mut removals,
                    package,
                    incoming_identity(&incoming[triggering_index], triggering_index),
                    incoming_packages,
                    relation.kind,
                    relation.requirement.native_text,
                ),
                RepositoryRequirementKind::Breaks => insert_deconfiguration(
                    &mut deconfigurations,
                    package,
                    incoming_identity(&incoming[triggering_index], triggering_index),
                    PackageRelationDeconfigurationCause::Breaks {
                        native_text: relation.requirement.native_text,
                    },
                    0,
                ),
                _ => unreachable!("installed relation was filtered to Conflict or Breaks"),
            }
        }
    }

    for trove_id in removals.keys() {
        deconfigurations.remove(trove_id);
    }
    let removals = removals.into_values().collect::<Vec<_>>();
    plan_dependent_deconfigurations(conn, &installed, incoming, &removals, &mut deconfigurations)?;
    let mut deconfigurations = deconfigurations.into_values().collect::<Vec<_>>();
    deconfigurations.sort_by(|left, right| {
        left.triggering_incoming
            .transaction_index
            .cmp(&right.triggering_incoming.transaction_index)
            .then_with(|| right.dependency_depth.cmp(&left.dependency_depth))
            .then_with(|| left.package.trove_id.cmp(&right.package.trove_id))
    });

    Ok(PackageRelationPlan {
        removals,
        deconfigurations,
    })
}

fn owned_incoming_candidate(
    incoming: &IncomingPackageRelations<'_>,
) -> OwnedPackageRelationCandidate {
    OwnedPackageRelationCandidate {
        name: incoming.name.to_string(),
        version: incoming.version.to_string(),
        version_scheme: incoming.version_scheme,
        provides: incoming
            .provides
            .iter()
            .map(|provide| OwnedPackageRelationProvide {
                name: provide.name.clone(),
                version: provide.version.clone(),
                version_scheme: provide.version_scheme,
            })
            .collect(),
    }
}

fn incoming_identity(
    incoming: &IncomingPackageRelations<'_>,
    transaction_index: usize,
) -> PackageRelationIncomingIdentity {
    PackageRelationIncomingIdentity {
        transaction_index,
        package_name: incoming.name.to_string(),
        package_version: incoming.version.to_string(),
        package_architecture: incoming.architecture.map(str::to_string),
    }
}

fn installed_identity(installed: &InstalledCandidate) -> PackageRelationInstalledIdentity {
    PackageRelationInstalledIdentity {
        trove_id: installed.trove.id.expect("installed trove has id"),
        package_name: installed.trove.name.clone(),
        package_version: installed.trove.version.clone(),
        package_architecture: installed.trove.architecture.clone(),
    }
}

fn same_package_instance(
    incoming: &IncomingPackageRelations<'_>,
    installed_name: &str,
    installed_architecture: Option<&str>,
) -> bool {
    incoming.name == installed_name && incoming.architecture == installed_architecture
}

fn incoming_replaces_installed(
    incoming: &[IncomingPackageRelations<'_>],
    installed: &InstalledCandidate,
) -> bool {
    incoming.iter().any(|candidate| {
        same_package_instance(
            candidate,
            &installed.trove.name,
            installed.trove.architecture.as_deref(),
        )
    })
}

/// Revalidate every exact relation transition immediately before lifecycle or
/// payload mutation.
pub fn validate_package_relation_plan(conn: &Connection, plan: &PackageRelationPlan) -> Result<()> {
    validate_package_relation_transitions(conn, &plan.removals, &plan.deconfigurations)
}

/// Revalidate an already-carried relation transition slice.
pub fn validate_package_relation_transitions(
    conn: &Connection,
    removals: &[PackageRelationRemoval],
    deconfigurations: &[PackageRelationDeconfiguration],
) -> Result<()> {
    validate_package_relation_removals(conn, removals)?;
    let removal_by_id = removals
        .iter()
        .map(|removal| (removal.trove_id, removal))
        .collect::<BTreeMap<_, _>>();
    let mut deconfigured_ids = BTreeSet::new();

    for deconfiguration in deconfigurations {
        let target = &deconfiguration.package;
        if removal_by_id.contains_key(&target.trove_id) {
            return Err(Error::ResolutionError(format!(
                "relation plan both removes and deconfigures {} {}",
                target.package_name, target.package_version
            )));
        }
        if !deconfigured_ids.insert(target.trove_id) {
            return Err(Error::ResolutionError(format!(
                "relation plan repeats deconfiguration target {} {}",
                target.package_name, target.package_version
            )));
        }
        validate_incoming_identity(&deconfiguration.triggering_incoming)?;
        let trove = validate_installed_identity(conn, target, "deconfiguration")?;
        if trove.pinned {
            return Err(Error::ResolutionError(format!(
                "{} cannot deconfigure pinned package {} {}",
                deconfiguration.triggering_incoming.package_name,
                target.package_name,
                target.package_version
            )));
        }
        if !trove.install_source.is_conary_owned() {
            return Err(Error::ResolutionError(format!(
                "{} requires deconfiguring externally owned package {} {}; take package ownership before applying this relation",
                deconfiguration.triggering_incoming.package_name,
                target.package_name,
                target.package_version
            )));
        }
        match &deconfiguration.cause {
            PackageRelationDeconfigurationCause::Breaks { .. } => {}
            PackageRelationDeconfigurationCause::Dependency {
                kind,
                removing_conflict,
                ..
            } => {
                if !matches!(
                    kind,
                    RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
                ) {
                    return Err(Error::ResolutionError(format!(
                        "deconfiguration of {} {} carries non-hard dependency kind '{}'",
                        target.package_name,
                        target.package_version,
                        kind.as_str()
                    )));
                }
                if let Some(conflict) = removing_conflict {
                    let removal = removal_by_id.get(&conflict.trove_id).ok_or_else(|| {
                        Error::ResolutionError(format!(
                            "deconfiguration of {} {} refers to unplanned conflict removal {} {}",
                            target.package_name,
                            target.package_version,
                            conflict.package_name,
                            conflict.package_version
                        ))
                    })?;
                    if removal.package_name != conflict.package_name
                        || removal.package_version != conflict.package_version
                        || removal.package_architecture != conflict.package_architecture
                        || removal.triggering_incoming != deconfiguration.triggering_incoming
                    {
                        return Err(Error::ResolutionError(format!(
                            "deconfiguration of {} {} carries inconsistent conflict-removal identity",
                            target.package_name, target.package_version
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Revalidate an already-carried relation removal slice.
///
/// Dependency closure is represented by
/// [`PackageRelationDeconfiguration`], not reconstructed by a name-based
/// removal solver.
pub fn validate_package_relation_removals(
    conn: &Connection,
    removals: &[PackageRelationRemoval],
) -> Result<()> {
    if removals.is_empty() {
        return Ok(());
    }

    for removal in removals {
        validate_incoming_identity(&removal.triggering_incoming)?;
        let mut canonical_incoming = removal.incoming_packages.clone();
        canonical_incoming.sort();
        canonical_incoming.dedup();
        if canonical_incoming.is_empty()
            || canonical_incoming != removal.incoming_packages
            || canonical_incoming
                .binary_search(&removal.triggering_incoming.package_name)
                .is_err()
        {
            return Err(Error::ResolutionError(format!(
                "relation removal for {} {} has no canonical incoming package authority",
                removal.package_name, removal.package_version
            )));
        }
        let expected_mode = removal.kind.removal_mode().ok_or_else(|| {
            Error::ResolutionError(format!(
                "non-negative relation kind '{}' cannot authorize package removal",
                removal.kind.as_str()
            ))
        })?;
        if removal.mode != expected_mode {
            return Err(Error::ResolutionError(format!(
                "{} relation kind '{}' requires {:?} removal, not {:?}",
                incoming_package_label(removal),
                removal.kind.as_str(),
                expected_mode,
                removal.mode
            )));
        }
        let mut canonical_transfers = removal.ownership_transfer_packages.clone();
        canonical_transfers.sort();
        canonical_transfers.dedup();
        let transfer_contract_valid = canonical_transfers == removal.ownership_transfer_packages
            && canonical_transfers
                .iter()
                .all(|package| canonical_incoming.binary_search(package).is_ok())
            && match removal.mode {
                PackageRelationRemovalMode::Constraint => canonical_transfers.is_empty(),
                PackageRelationRemovalMode::OwnershipTransfer => canonical_transfers
                    .binary_search(&removal.triggering_incoming.package_name)
                    .is_ok(),
            };
        if !transfer_contract_valid {
            return Err(Error::ResolutionError(format!(
                "relation removal for {} {} has invalid ownership-transfer authority",
                removal.package_name, removal.package_version
            )));
        }
        let identity = PackageRelationInstalledIdentity {
            trove_id: removal.trove_id,
            package_name: removal.package_name.clone(),
            package_version: removal.package_version.clone(),
            package_architecture: removal.package_architecture.clone(),
        };
        let trove = validate_installed_identity(conn, &identity, "removal")?;
        if trove.pinned {
            return Err(Error::ResolutionError(format!(
                "{} {} cannot replace or conflict-remove pinned package {} {}",
                incoming_package_label(removal),
                relation_verb(removal.kind),
                removal.package_name,
                removal.package_version
            )));
        }
        if !trove.install_source.is_conary_owned() {
            return Err(Error::ResolutionError(format!(
                "{} {} matches externally owned package {} {}; take package ownership before applying this relation",
                incoming_package_label(removal),
                relation_verb(removal.kind),
                removal.package_name,
                removal.package_version
            )));
        }
    }
    Ok(())
}

fn validate_incoming_identity(incoming: &PackageRelationIncomingIdentity) -> Result<()> {
    if incoming.package_name.is_empty()
        || incoming.package_version.is_empty()
        || incoming
            .package_architecture
            .as_deref()
            .is_some_and(str::is_empty)
    {
        return Err(Error::ResolutionError(
            "relation transition has an incomplete incoming package identity".to_string(),
        ));
    }
    Ok(())
}

fn validate_installed_identity(
    conn: &Connection,
    identity: &PackageRelationInstalledIdentity,
    action: &str,
) -> Result<Trove> {
    let trove = Trove::find_by_id(conn, identity.trove_id)?.ok_or_else(|| {
        Error::ResolutionError(format!(
            "relation {action} target {} {} disappeared before transaction",
            identity.package_name, identity.package_version
        ))
    })?;
    if trove.name != identity.package_name
        || trove.version != identity.package_version
        || trove.architecture != identity.package_architecture
    {
        return Err(Error::ResolutionError(format!(
            "relation {action} target {} {} changed identity before transaction",
            identity.package_name, identity.package_version
        )));
    }
    Ok(trove)
}

fn incoming_package_label(removal: &PackageRelationRemoval) -> String {
    removal.incoming_packages.join(", ")
}

fn relation_verb(kind: RepositoryRequirementKind) -> &'static str {
    match kind {
        RepositoryRequirementKind::Conflict | RepositoryRequirementKind::Breaks => "conflict",
        RepositoryRequirementKind::Replace | RepositoryRequirementKind::Obsolete => "replacement",
        RepositoryRequirementKind::Depends
        | RepositoryRequirementKind::PreDepends
        | RepositoryRequirementKind::Optional
        | RepositoryRequirementKind::Recommends
        | RepositoryRequirementKind::Suggests
        | RepositoryRequirementKind::Supplements
        | RepositoryRequirementKind::Enhances
        | RepositoryRequirementKind::Build => "invalid negative relation",
    }
}

fn insert_removal(
    removals: &mut BTreeMap<i64, PackageRelationRemoval>,
    installed: &InstalledCandidate,
    triggering_incoming: PackageRelationIncomingIdentity,
    mut incoming_packages: Vec<String>,
    kind: RepositoryRequirementKind,
    native_text: Option<String>,
) {
    incoming_packages.sort();
    incoming_packages.dedup();
    let ownership_transfer_packages =
        if kind.removal_mode() == Some(PackageRelationRemovalMode::OwnershipTransfer) {
            incoming_packages.clone()
        } else {
            Vec::new()
        };
    let trove_id = installed.trove.id.expect("installed trove has id");
    let removal = PackageRelationRemoval {
        trove_id,
        package_name: installed.trove.name.clone(),
        package_version: installed.trove.version.clone(),
        package_architecture: installed.trove.architecture.clone(),
        triggering_incoming,
        incoming_packages,
        ownership_transfer_packages,
        kind,
        mode: kind.removal_mode().expect("relation kind is negative"),
        native_text,
    };
    removals
        .entry(trove_id)
        .and_modify(|existing| {
            let replacement_is_earlier = removal.triggering_incoming.transaction_index
                < existing.triggering_incoming.transaction_index;
            let replacement_is_same_boundary_transfer =
                removal.triggering_incoming.transaction_index
                    == existing.triggering_incoming.transaction_index
                    && removal.mode == PackageRelationRemovalMode::OwnershipTransfer
                    && existing.mode == PackageRelationRemovalMode::Constraint;
            existing
                .incoming_packages
                .extend(removal.incoming_packages.iter().cloned());
            existing.incoming_packages.sort();
            existing.incoming_packages.dedup();
            if replacement_is_earlier || replacement_is_same_boundary_transfer {
                existing.triggering_incoming = removal.triggering_incoming.clone();
                existing.kind = removal.kind;
                existing.mode = removal.mode;
                existing.native_text.clone_from(&removal.native_text);
                existing
                    .ownership_transfer_packages
                    .clone_from(&removal.ownership_transfer_packages);
            } else if removal.triggering_incoming.transaction_index
                == existing.triggering_incoming.transaction_index
                && removal.mode == PackageRelationRemovalMode::OwnershipTransfer
                && existing.mode == PackageRelationRemovalMode::OwnershipTransfer
            {
                existing
                    .ownership_transfer_packages
                    .extend(removal.ownership_transfer_packages.iter().cloned());
                existing.ownership_transfer_packages.sort();
                existing.ownership_transfer_packages.dedup();
            }
        })
        .or_insert(removal);
}

fn insert_deconfiguration(
    deconfigurations: &mut BTreeMap<i64, PackageRelationDeconfiguration>,
    installed: &InstalledCandidate,
    triggering_incoming: PackageRelationIncomingIdentity,
    cause: PackageRelationDeconfigurationCause,
    dependency_depth: usize,
) {
    let package = installed_identity(installed);
    let deconfiguration = PackageRelationDeconfiguration {
        package: package.clone(),
        triggering_incoming,
        cause,
        dependency_depth,
    };
    deconfigurations
        .entry(package.trove_id)
        .and_modify(|existing| {
            if deconfiguration.triggering_incoming.transaction_index
                < existing.triggering_incoming.transaction_index
            {
                existing.clone_from(&deconfiguration);
            }
        })
        .or_insert(deconfiguration);
}

struct InstalledCandidate {
    trove: Trove,
    provides: Vec<OwnedProvide>,
    version_scheme: VersionScheme,
}

struct OwnedProvide {
    name: String,
    version: Option<String>,
}

impl InstalledCandidate {
    fn to_owned_relation_candidate(&self) -> OwnedPackageRelationCandidate {
        OwnedPackageRelationCandidate {
            name: self.trove.name.clone(),
            version: self.trove.version.clone(),
            version_scheme: self.version_scheme,
            provides: self
                .provides
                .iter()
                .map(|provide| OwnedPackageRelationProvide {
                    name: provide.name.clone(),
                    version: provide.version.clone(),
                    version_scheme: self.version_scheme,
                })
                .collect(),
        }
    }
}

fn relation_matches_installed(
    relation: &crate::repository::dependency_model::RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    package: &InstalledCandidate,
) -> std::result::Result<bool, String> {
    let provides = package
        .provides
        .iter()
        .map(|provide| PackageRelationProvide {
            name: &provide.name,
            version: provide.version.as_deref(),
            version_scheme: package.version_scheme,
        })
        .collect::<Vec<_>>();
    relation_matches_candidate(
        relation,
        relation_scheme,
        &PackageRelationCandidate {
            name: &package.trove.name,
            version: &package.trove.version,
            version_scheme: package.version_scheme,
            provides: &provides,
        },
    )
}

fn conflict_removal_set<'a>(
    relation: &RepositoryRequirementGroup,
    relation_scheme: VersionScheme,
    installed: &'a [InstalledCandidate],
    incoming: &[IncomingPackageRelations<'_>],
    fixed: &[OwnedPackageRelationCandidate],
) -> Result<Vec<&'a InstalledCandidate>> {
    let candidates = installed
        .iter()
        .filter(|package| !incoming_replaces_installed(incoming, package))
        .collect::<Vec<_>>();
    let owned = candidates
        .iter()
        .map(|package| package.to_owned_relation_candidate())
        .collect::<Vec<_>>();
    let removal_indices =
        minimum_conflict_removal_indices(relation, relation_scheme, &owned, fixed)
            .map_err(Error::ResolutionError)?;
    Ok(removal_indices
        .into_iter()
        .map(|index| candidates[index])
        .collect())
}

fn load_installed_candidates(conn: &Connection) -> Result<Vec<InstalledCandidate>> {
    let mut candidates = Trove::list_all(conn)?
        .into_iter()
        .map(|trove| {
            let version_scheme = trove.version_scheme;
            let provides = match trove.id {
                Some(trove_id) => ProvideEntry::find_by_trove(conn, trove_id)?
                    .into_iter()
                    .flat_map(|provide| {
                        let typed = provide.to_typed_string();
                        let raw = OwnedProvide {
                            name: provide.capability.clone(),
                            version: provide.version.clone(),
                        };
                        if typed == provide.capability {
                            vec![raw]
                        } else {
                            vec![
                                raw,
                                OwnedProvide {
                                    name: typed,
                                    version: provide.version,
                                },
                            ]
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            Ok(InstalledCandidate {
                trove,
                provides,
                version_scheme,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    candidates.sort_by_key(|candidate| candidate.trove.id);
    Ok(candidates)
}

#[cfg(test)]
#[path = "package_relations/tests.rs"]
mod tests;
