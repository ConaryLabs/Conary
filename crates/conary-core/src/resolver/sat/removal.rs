// conary-core/src/resolver/sat/removal.rs

use rusqlite::Connection;
use std::collections::HashSet;

use crate::error::{Error, Result};

use super::super::provider::matching::{
    constraint_matches_provide, provider_expression_matches_package,
};
use super::super::provider::{
    ConaryConstraint, ConaryProvider, SolverExpression, constraint_matches_package,
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
) -> Result<Vec<String>> {
    let names = to_remove.iter().map(String::as_str).collect::<HashSet<_>>();
    let trove_ids = provider
        .solvable_ids()
        .iter()
        .filter_map(|id| {
            let package = provider.get_solvable(*id);
            names
                .contains(package.name.as_str())
                .then_some(package.installed_trove_id)
                .flatten()
        })
        .collect::<Vec<_>>();
    find_breaking_packages_for_troves(provider, &trove_ids)
}

pub(super) fn find_breaking_packages_for_troves(
    provider: &ConaryProvider<'_>,
    to_remove: &[i64],
) -> Result<Vec<String>> {
    let mut gone_troves = to_remove.iter().copied().collect::<HashSet<_>>();
    let mut breaking_set: HashSet<String> = HashSet::new();
    let baseline = HashSet::new();
    loop {
        let mut changed = false;

        for sid in provider.solvable_ids() {
            let pkg = provider.get_solvable(*sid);
            let Some(trove_id) = pkg.installed_trove_id else {
                continue;
            };

            if gone_troves.contains(&trove_id) {
                continue;
            }

            let mut has_broken_dep = false;
            if let Some(dependencies) = provider.get_removal_dependency_list(*sid) {
                for dependency in dependencies {
                    if expression_satisfiable(provider, &dependency.expression, &baseline)?
                        && !expression_satisfiable(provider, &dependency.expression, &gone_troves)?
                    {
                        has_broken_dep = true;
                        break;
                    }
                }
            }

            if has_broken_dep {
                breaking_set.insert(pkg.name.clone());
                gone_troves.insert(trove_id);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    let mut breaking = breaking_set.into_iter().collect::<Vec<_>>();
    breaking.sort();
    Ok(breaking)
}

fn expression_satisfiable(
    provider: &ConaryProvider<'_>,
    expression: &SolverExpression,
    gone_troves: &HashSet<i64>,
) -> Result<bool> {
    match expression {
        SolverExpression::Atom(atom) => {
            atom_satisfiable(provider, &atom.name, &atom.constraint, gone_troves)
        }
        SolverExpression::And(operands) => {
            for operand in operands {
                if !expression_satisfiable(provider, operand, gone_troves)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        SolverExpression::Or(operands) => {
            for operand in operands {
                if expression_satisfiable(provider, operand, gone_troves)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        SolverExpression::Not(operand) => {
            Ok(!expression_satisfiable(provider, operand, gone_troves)?)
        }
    }
}

fn atom_satisfiable(
    provider: &ConaryProvider<'_>,
    dep_name: &str,
    constraint: &ConaryConstraint,
    gone_troves: &HashSet<i64>,
) -> Result<bool> {
    if let ConaryConstraint::RpmRuntime(requirement) = constraint {
        requirement
            .ensure_supported()
            .map_err(|error| Error::ResolutionError(error.to_string()))?;
        return Ok(true);
    }
    if let ConaryConstraint::ProviderExpression { expression } = constraint {
        for id in provider.solvable_ids() {
            let package = provider.get_solvable(*id);
            if let Some(trove_id) = package.installed_trove_id
                && !gone_troves.contains(&trove_id)
                && provider_expression_matches_package(expression, package)?
            {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    let providers = provider.find_providers(dep_name);
    for (trove_id, provided_version, provided_relation) in providers {
        if let Some(package) = provider.installed_package_for_trove(trove_id)
            && !gone_troves.contains(&trove_id)
            && constraint_matches_provide(
                constraint,
                provided_version.as_deref(),
                provided_relation,
                package.version_scheme,
            )?
        {
            return Ok(true);
        }
    }

    for id in provider.solvable_ids() {
        let alt = provider.get_solvable(*id);
        if let Some(trove_id) = alt.installed_trove_id
            && !gone_troves.contains(&trove_id)
            && alt.name == dep_name
            && constraint_matches_package(constraint, &alt.version, alt.version_scheme)?
        {
            return Ok(true);
        }
    }

    Ok(false)
}
