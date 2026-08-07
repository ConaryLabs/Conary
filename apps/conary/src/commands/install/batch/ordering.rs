// apps/conary/src/commands/install/batch/ordering.rs

//! Deterministic dependency-first ordering for an exact selected batch.

use super::*;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::resolver::identity::{PackageIdentity, ProvidedCapability};
use std::collections::{BTreeMap, BTreeSet};

/// A promised path the transaction actually leaned on to satisfy a dependency.
///
/// RPM's `%ghost` means the package owns the path *if present*; it never says
/// the package creates it, and paths like `/etc/fstab` or a log file are
/// legitimately absent after a fresh transaction. So the only promises worth
/// verifying are the ones a dependency edge was certified against: the
/// transaction checks its own reasoning and claims nothing beyond it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WitnessedPromise {
    pub(super) provider: String,
    pub(super) provider_version: String,
    pub(super) path: String,
    pub(super) dependent: String,
    pub(super) requirement: String,
}

pub(super) fn order_packages_for_transaction(
    conn: &Connection,
    packages: &mut Vec<PreparedPackage>,
) -> Result<Vec<WitnessedPromise>> {
    let mut witnessed_promises = Vec::new();
    if packages.len() < 2 {
        return Ok(witnessed_promises);
    }

    let native_architecture = conary_core::repository::registry::detect_system_arch();
    let installed = conary_core::resolver::load_installed_package_identities(conn)?
        .into_iter()
        .map(|mut identity| {
            if identity.architecture.as_deref().is_none_or(str::is_empty) {
                identity.architecture = Some(native_architecture.clone());
            }
            identity
        })
        .collect::<Vec<_>>();
    let selected = packages
        .iter()
        .map(|package| transaction_identity(package, &native_architecture))
        .collect::<Vec<_>>();
    let stable_indices = stable_package_indices(packages);
    let mut edges = vec![BTreeSet::new(); packages.len()];
    let mut strong_edges = vec![BTreeSet::new(); packages.len()];

    for (dependent_index, package) in packages.iter().enumerate() {
        let depending_architecture = package
            .architecture
            .as_deref()
            .filter(|architecture| !architecture.is_empty())
            .unwrap_or(&native_architecture);
        for requirement in package.requirements.iter().filter(|requirement| {
            matches!(
                requirement.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            )
        }) {
            let mut witnesses = installed.clone();
            witnesses.push(selected[dependent_index].clone());
            if expression_satisfied(
                requirement,
                package.semantics.version_scheme,
                depending_architecture,
                &native_architecture,
                &witnesses,
            )? {
                continue;
            }

            let mut selected_witnesses = Vec::new();
            for candidate_index in stable_indices
                .iter()
                .copied()
                .filter(|candidate_index| *candidate_index != dependent_index)
            {
                witnesses.push(selected[candidate_index].clone());
                selected_witnesses.push(candidate_index);
                if expression_satisfied(
                    requirement,
                    package.semantics.version_scheme,
                    depending_architecture,
                    &native_architecture,
                    &witnesses,
                )? {
                    break;
                }
            }
            if !expression_satisfied(
                requirement,
                package.semantics.version_scheme,
                depending_architecture,
                &native_architecture,
                &witnesses,
            )? {
                anyhow::bail!(
                    "selected package transaction does not satisfy exact {} requirement for {}: {}",
                    requirement.kind.as_str(),
                    package.name,
                    requirement
                        .native_text
                        .as_deref()
                        .unwrap_or("<typed expression>")
                );
            }

            // Remove every dispensable witness in a deterministic reverse
            // pass. The retained set is exact expression authority for the
            // dependency edge; package names and solver trail order never are.
            for position in (0..selected_witnesses.len()).rev() {
                let mut without_candidate = installed.clone();
                without_candidate.push(selected[dependent_index].clone());
                without_candidate.extend(
                    selected_witnesses
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != position)
                        .map(|(_, index)| selected[*index].clone()),
                );
                if expression_satisfied(
                    requirement,
                    package.semantics.version_scheme,
                    depending_architecture,
                    &native_architecture,
                    &without_candidate,
                )? {
                    selected_witnesses.remove(position);
                }
            }
            // A retained witness may hold the edge up through a promised path.
            // Drop each promise in turn: if the edge falls, the transaction
            // relied on that path existing and must later prove it does.
            for provider_index in &selected_witnesses {
                for promise in packages[*provider_index]
                    .provides
                    .iter()
                    .filter(|provide| provide.is_promised_path())
                {
                    let mut without_promise = installed.clone();
                    without_promise.push(selected[dependent_index].clone());
                    for index in &selected_witnesses {
                        let mut identity = selected[*index].clone();
                        if index == provider_index {
                            identity.provided_capabilities.retain(|capability| {
                                !(capability.is_promised_path() && capability.name == promise.name)
                            });
                        }
                        without_promise.push(identity);
                    }
                    if expression_satisfied(
                        requirement,
                        package.semantics.version_scheme,
                        depending_architecture,
                        &native_architecture,
                        &without_promise,
                    )? {
                        continue;
                    }
                    let provider = &packages[*provider_index];
                    let witnessed = WitnessedPromise {
                        provider: provider.name.clone(),
                        provider_version: provider.version.clone(),
                        path: promise.name.clone(),
                        dependent: package.name.clone(),
                        requirement: requirement
                            .native_text
                            .as_deref()
                            .unwrap_or("<typed expression>")
                            .to_string(),
                    };
                    if !witnessed_promises
                        .iter()
                        .any(|existing: &WitnessedPromise| {
                            existing.provider == witnessed.provider
                                && existing.path == witnessed.path
                        })
                    {
                        witnessed_promises.push(witnessed);
                    }
                }
            }
            for provider_index in selected_witnesses {
                edges[provider_index].insert(dependent_index);
                if requirement.kind == RepositoryRequirementKind::PreDepends {
                    strong_edges[provider_index].insert(dependent_index);
                }
            }
        }
    }

    let order = dependency_first_scc_order(packages, &edges, &strong_edges);
    let mut slots = packages.drain(..).map(Some).collect::<Vec<_>>();
    packages.extend(
        order
            .into_iter()
            .map(|index| slots[index].take().expect("package order repeats an index")),
    );
    Ok(witnessed_promises)
}

fn expression_satisfied(
    requirement: &conary_core::repository::dependency_model::RepositoryRequirementGroup,
    version_scheme: conary_core::repository::versioning::VersionScheme,
    depending_architecture: &str,
    native_architecture: &str,
    packages: &[PackageIdentity],
) -> Result<bool> {
    conary_core::resolver::requirement_expression_satisfied(
        &requirement.expression,
        version_scheme,
        depending_architecture,
        native_architecture,
        packages,
    )
    .map_err(anyhow::Error::from)
}

fn transaction_identity(package: &PreparedPackage, native_architecture: &str) -> PackageIdentity {
    PackageIdentity {
        repo_package_id: None,
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: Some(
            package
                .architecture
                .clone()
                .filter(|architecture| !architecture.is_empty())
                .unwrap_or_else(|| native_architecture.to_string()),
        ),
        debian_multi_arch: package.debian_multi_arch,
        version_scheme: package.semantics.version_scheme,
        repository_id: package
            .repository_provenance
            .as_ref()
            .map(|provenance| provenance.repository_id),
        repository_name: String::new(),
        repository_profile: package
            .repository_provenance
            .as_ref()
            .and_then(|provenance| provenance.source_profile.clone()),
        repository_priority: 0,
        canonical_id: None,
        canonical_name: None,
        installed_trove_id: None,
        installed_pinned: false,
        provided_capabilities: package
            .provides
            .iter()
            .map(|provide| ProvidedCapability {
                kind: provide.kind,
                name: provide.name.clone(),
                version: provide.version.clone(),
                version_relation: provide.version_relation,
                version_scheme: provide.version_scheme,
                architecture_qualifier: provide.architecture_qualifier.clone(),
                provenance: provide.provenance.clone(),
            })
            .collect(),
    }
}

type StablePackageKey = (u8, String, String, String, String, usize);

fn stable_package_key(package: &PreparedPackage, index: usize) -> StablePackageKey {
    (
        match package.install_reason {
            InstallReason::Dependency => 0,
            InstallReason::Explicit => 1,
        },
        package.name.clone(),
        package.version.clone(),
        package.package_release.clone().unwrap_or_default(),
        package.architecture.clone().unwrap_or_default(),
        index,
    )
}

fn stable_package_indices(packages: &[PreparedPackage]) -> Vec<usize> {
    let mut indices = (0..packages.len()).collect::<Vec<_>>();
    indices.sort_by_key(|index| stable_package_key(&packages[*index], *index));
    indices
}

fn stable_component_key(packages: &[PreparedPackage], component: &[usize]) -> StablePackageKey {
    component
        .iter()
        .map(|index| stable_package_key(&packages[*index], *index))
        .min()
        .expect("dependency component must contain a package")
}

fn dependency_first_scc_order(
    packages: &[PreparedPackage],
    edges: &[BTreeSet<usize>],
    strong_edges: &[BTreeSet<usize>],
) -> Vec<usize> {
    let finish_order = graph_finish_order(edges);
    let reverse = reverse_edges(edges);
    let mut component_by_node = vec![usize::MAX; packages.len()];
    let mut components = Vec::<Vec<usize>>::new();
    for start in finish_order.into_iter().rev() {
        if component_by_node[start] != usize::MAX {
            continue;
        }
        let component_index = components.len();
        let mut component = Vec::new();
        let mut stack = vec![start];
        component_by_node[start] = component_index;
        while let Some(node) = stack.pop() {
            component.push(node);
            for predecessor in reverse[node].iter().rev().copied() {
                if component_by_node[predecessor] == usize::MAX {
                    component_by_node[predecessor] = component_index;
                    stack.push(predecessor);
                }
            }
        }
        component = strong_first_component_order(packages, &component, strong_edges);
        components.push(component);
    }

    let mut component_edges = vec![BTreeSet::new(); components.len()];
    let mut indegree = vec![0_usize; components.len()];
    for (source, targets) in edges.iter().enumerate() {
        for target in targets {
            let source_component = component_by_node[source];
            let target_component = component_by_node[*target];
            if source_component != target_component
                && component_edges[source_component].insert(target_component)
            {
                indegree[target_component] += 1;
            }
        }
    }
    let mut ready = BTreeSet::new();
    for (component_index, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            ready.insert((
                stable_component_key(packages, &components[component_index]),
                component_index,
            ));
        }
    }
    let mut ordered = Vec::with_capacity(packages.len());
    while let Some((_key, component_index)) = ready.pop_first() {
        ordered.extend(components[component_index].iter().copied());
        for target in component_edges[component_index].iter().copied() {
            indegree[target] -= 1;
            if indegree[target] == 0 {
                ready.insert((stable_component_key(packages, &components[target]), target));
            }
        }
    }
    debug_assert_eq!(ordered.len(), packages.len());
    ordered
}

/// Order one dependency SCC by the source-defined strong edges it contains.
///
/// Ordinary dependency edges made the component cyclic, so they cannot all be
/// honored. An acyclic strong subset remains authoritative and is exhausted
/// before the stable fallback. If the strong subset is itself cyclic, no
/// linear order can satisfy it; selecting the least stable key breaks exactly
/// that irreducible cycle and keeps the result deterministic.
fn strong_first_component_order(
    packages: &[PreparedPackage],
    component: &[usize],
    strong_edges: &[BTreeSet<usize>],
) -> Vec<usize> {
    if component.len() < 2 {
        return component.to_vec();
    }

    let in_component = component.iter().copied().collect::<BTreeSet<_>>();
    let mut indegree = component
        .iter()
        .copied()
        .map(|member| (member, 0_usize))
        .collect::<BTreeMap<_, _>>();
    for source in component {
        for target in strong_edges[*source]
            .iter()
            .filter(|target| in_component.contains(target))
        {
            *indegree
                .get_mut(target)
                .expect("strong edge target belongs to its dependency component") += 1;
        }
    }

    let mut ready = BTreeSet::new();
    let mut remaining = in_component;
    for member in component {
        if indegree[member] == 0 {
            ready.insert((stable_package_key(&packages[*member], *member), *member));
        }
    }

    let mut ordered = Vec::with_capacity(component.len());
    while ordered.len() < component.len() {
        let member = if let Some((_key, member)) = ready.pop_first() {
            member
        } else {
            component
                .iter()
                .copied()
                .filter(|member| remaining.contains(member))
                .min_by_key(|member| stable_package_key(&packages[*member], *member))
                .expect("strong dependency cycle has no remaining member")
        };
        if !remaining.remove(&member) {
            continue;
        }
        ordered.push(member);
        for target in &strong_edges[member] {
            if remaining.contains(target) {
                let target_indegree = indegree
                    .get_mut(target)
                    .expect("remaining strong edge target belongs to its dependency component");
                *target_indegree -= 1;
                if *target_indegree == 0 {
                    ready.insert((stable_package_key(&packages[*target], *target), *target));
                }
            }
        }
    }
    ordered
}

fn graph_finish_order(edges: &[BTreeSet<usize>]) -> Vec<usize> {
    let mut visited = vec![false; edges.len()];
    let mut finished = Vec::with_capacity(edges.len());
    for start in 0..edges.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, edges[start].iter())];
        while let Some((node, successors)) = stack.last_mut() {
            if let Some(successor) = successors.next().copied() {
                if !visited[successor] {
                    visited[successor] = true;
                    stack.push((successor, edges[successor].iter()));
                }
            } else {
                finished.push(*node);
                stack.pop();
            }
        }
    }
    finished
}

fn reverse_edges(edges: &[BTreeSet<usize>]) -> Vec<BTreeSet<usize>> {
    let mut reverse = vec![BTreeSet::new(); edges.len()];
    for (source, targets) in edges.iter().enumerate() {
        for target in targets {
            reverse[*target].insert(source);
        }
    }
    reverse
}
