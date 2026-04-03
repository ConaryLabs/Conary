// conary-core/src/resolver/sat/removal.rs

use resolvo::SolvableId;
use rusqlite::Connection;
use std::collections::HashSet;

use crate::error::Result;
use crate::repository::versioning::{RepoVersionConstraint, repo_version_satisfies};
use crate::version::{RpmVersion, VersionConstraint};

use super::super::provider::{
    ConaryConstraint, ConaryProvider, SolverDep, constraint_matches_package,
};

pub(super) fn build_provider_for_removal<'conn>(
    conn: &'conn Connection,
) -> Result<ConaryProvider<'conn>> {
    let mut provider = ConaryProvider::new(conn);
    provider.load_installed_packages()?;
    provider.intern_all_dependency_version_sets()?;
    provider.load_removal_data()?;
    Ok(provider)
}

pub(super) fn find_breaking_packages(
    provider: &ConaryProvider<'_>,
    to_remove: &[String],
) -> Vec<String> {
    let mut gone_set: HashSet<String> = to_remove.iter().cloned().collect();
    let mut breaking_set: HashSet<String> = HashSet::new();
    let solvable_count = provider.solvable_count();

    loop {
        let mut changed = false;

        for index in 0..solvable_count {
            let sid = SolvableId(index as u32);
            let pkg = provider.get_solvable(sid);

            if pkg.installed_trove_id.is_none()
                || gone_set.contains(&pkg.name)
                || breaking_set.contains(&pkg.name)
            {
                continue;
            }

            let has_broken_dep = provider
                .get_removal_dependency_list(sid)
                .is_some_and(|deps| {
                    deps.iter()
                        .any(|dep| !clause_satisfiable(provider, dep, &gone_set))
                });

            if has_broken_dep {
                breaking_set.insert(pkg.name.clone());
                gone_set.insert(pkg.name.clone());
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    let mut breaking = breaking_set.into_iter().collect::<Vec<_>>();
    breaking.sort();
    breaking
}

fn clause_satisfiable(
    provider: &ConaryProvider<'_>,
    dep: &SolverDep,
    gone: &HashSet<String>,
) -> bool {
    match dep {
        SolverDep::Single(name, constraint) => {
            alternative_satisfiable(provider, name, constraint, gone)
        }
        SolverDep::OrGroup(alternatives) => alternatives
            .iter()
            .any(|(name, constraint)| alternative_satisfiable(provider, name, constraint, gone)),
    }
}

fn alternative_satisfiable(
    provider: &ConaryProvider<'_>,
    dep_name: &str,
    constraint: &ConaryConstraint,
    gone: &HashSet<String>,
) -> bool {
    let providers = provider.find_providers(dep_name);
    if providers.iter().any(|(trove_id, provided_version)| {
        provider
            .trove_name(*trove_id)
            .is_some_and(|name| !gone.contains(name))
            && provider_version_satisfies_constraint(constraint, provided_version.as_deref())
    }) {
        return true;
    }

    let solvable_count = provider.solvable_count();
    let has_matching_installed_name = (0..solvable_count).any(|index| {
        let alt = provider.get_solvable(SolvableId(index as u32));
        alt.installed_trove_id.is_some()
            && alt.name == dep_name
            && !gone.contains(&alt.name)
            && constraint_matches_package(constraint, &alt.version, alt.version_scheme)
    });
    if has_matching_installed_name {
        return true;
    }

    let was_provided_by_removed = providers.iter().any(|(trove_id, _)| {
        provider
            .trove_name(*trove_id)
            .is_some_and(|name| gone.contains(name))
    });
    let removed_name_match = (0..solvable_count).any(|index| {
        let alt = provider.get_solvable(SolvableId(index as u32));
        alt.installed_trove_id.is_some() && alt.name == dep_name && gone.contains(&alt.name)
    });

    !was_provided_by_removed && !removed_name_match
}

fn provider_version_satisfies_constraint(
    constraint: &ConaryConstraint,
    provider_version: Option<&str>,
) -> bool {
    match constraint {
        ConaryConstraint::Legacy(VersionConstraint::Any) => true,
        ConaryConstraint::Legacy(version_constraint) => {
            let Some(version) = provider_version else {
                return false;
            };
            RpmVersion::parse(version)
                .map(|parsed| version_constraint.satisfies(&parsed))
                .unwrap_or(false)
        }
        ConaryConstraint::Repository {
            constraint: RepoVersionConstraint::Any,
            ..
        } => true,
        ConaryConstraint::Repository {
            scheme,
            constraint: repo_constraint,
            ..
        } => {
            let Some(version) = provider_version else {
                return false;
            };
            repo_version_satisfies(*scheme, version, repo_constraint)
        }
    }
}
