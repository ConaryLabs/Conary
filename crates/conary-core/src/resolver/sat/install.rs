// conary-core/src/resolver/sat/install.rs

use resolvo::{ConditionalRequirement, SolvableId};
use rusqlite::Connection;
use std::collections::HashSet;
use std::time::Instant;

use crate::error::Result;
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::version::VersionConstraint;

use super::super::provider::{ConaryConstraint, ConaryProvider, SolverExpression};
use super::{SatPackage, SatSource, check_transitive_loading_limits};

pub(super) fn build_provider_for_install<'conn>(
    conn: &'conn Connection,
    requests: &[(String, VersionConstraint)],
    policy: &ResolutionPolicy,
) -> Result<ConaryProvider<'conn>> {
    let mut provider = ConaryProvider::new_with_policy(conn, policy.clone());
    provider.set_root_request_names(requests.iter().map(|(name, _)| name.clone()));
    provider.load_installed_packages()?;
    provider.build_provides_index()?;
    provider.load_canonical_index()?;
    provider.expand_root_request_names_with_canonical_equivalents();
    load_transitive_repo_packages(
        &mut provider,
        requests.iter().map(|(name, _)| name.clone()).collect(),
    )?;
    provider.intern_all_dependency_version_sets()?;
    Ok(provider)
}

pub(super) fn build_provider_for_requirement_expressions<'conn>(
    conn: &'conn Connection,
    expressions: &[SolverExpression],
    policy: &ResolutionPolicy,
) -> Result<ConaryProvider<'conn>> {
    let mut provider = ConaryProvider::new_with_policy(conn, policy.clone());
    provider.set_root_request_names(requirement_names(expressions));
    provider.load_installed_packages()?;
    provider.build_provides_index()?;
    provider.load_canonical_index()?;
    provider.expand_root_request_names_with_canonical_equivalents();
    load_transitive_repo_packages(&mut provider, requirement_names(expressions))?;
    provider.intern_all_dependency_version_sets()?;
    Ok(provider)
}

fn load_transitive_repo_packages(
    provider: &mut ConaryProvider<'_>,
    mut loaded_names: HashSet<String>,
) -> Result<()> {
    let mut to_load: Vec<String> = loaded_names.iter().cloned().collect();
    let load_start = Instant::now();

    while !to_load.is_empty() {
        check_transitive_loading_limits(load_start.elapsed(), loaded_names.len())?;
        provider.load_repo_packages_for_names(&to_load)?;

        let mut new_names = provider
            .new_dependency_names(&loaded_names)
            .into_iter()
            .filter(|name| loaded_names.insert(name.clone()))
            .collect::<Vec<_>>();

        let canonical_equivalents = new_names
            .iter()
            .flat_map(|name| provider.canonical_equivalents(name).iter().cloned())
            .filter(|name| loaded_names.insert(name.clone()))
            .collect::<Vec<_>>();

        new_names.extend(canonical_equivalents);
        check_transitive_loading_limits(load_start.elapsed(), loaded_names.len())?;
        to_load = new_names;
    }

    Ok(())
}

fn requirement_names(expressions: &[SolverExpression]) -> HashSet<String> {
    let known = HashSet::new();
    let mut names = HashSet::new();
    for expression in expressions {
        for atom in expression.atoms() {
            match &atom.constraint {
                ConaryConstraint::ProviderExpression { expression } => {
                    expression.collect_names(&known, &mut names);
                }
                ConaryConstraint::RpmRuntime(_) => {}
                ConaryConstraint::ExactRepositoryPackage(_) => {}
                ConaryConstraint::Requested(_) | ConaryConstraint::Repository { .. } => {
                    names.insert(atom.name.clone());
                }
            }
        }
    }
    names
}

pub(super) fn build_requirements(
    provider: &mut ConaryProvider<'_>,
    requests: &[(String, VersionConstraint)],
) -> Result<Vec<ConditionalRequirement>> {
    let mut requirements = Vec::with_capacity(requests.len());

    for (name, constraint) in requests {
        let name_id = provider.intern_name(name)?;
        let version_set_id = provider.intern_version_set(name_id, constraint.clone())?;
        requirements.push(ConditionalRequirement::from(version_set_id));
    }

    Ok(requirements)
}

pub(super) fn build_expression_requirements(
    provider: &mut ConaryProvider<'_>,
    expressions: &[SolverExpression],
) -> Result<Vec<ConditionalRequirement>> {
    provider.compile_root_requirements(expressions)
}

pub(super) fn collect_install_order(
    provider: &ConaryProvider<'_>,
    solvable_ids: &[SolvableId],
) -> Vec<SatPackage> {
    solvable_ids
        .iter()
        .map(|sid| {
            let pkg = provider.get_solvable(*sid);
            SatPackage {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                package_release: pkg.package_release.clone(),
                architecture: pkg.architecture.clone(),
                version_scheme: pkg.version_scheme,
                repo_package_id: pkg.repo_package_id,
                repository_id: pkg.repository_id,
                repository_name: (!pkg.repository_name.is_empty())
                    .then(|| pkg.repository_name.clone()),
                installed_trove_id: pkg.installed_trove_id,
                source: if pkg.installed_trove_id.is_some() {
                    SatSource::Installed
                } else {
                    SatSource::Repository
                },
            }
        })
        .collect()
}
