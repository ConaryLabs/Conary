// conary-core/src/resolver/sat.rs

//! SAT-based dependency resolution using resolvo.
//!
//! Provides `solve_install` and `solve_removal` functions that use the CDCL SAT
//! solver to find optimal package installation plans with backtracking support.

mod install;
mod relations;
mod removal;

use resolvo::{Problem, Solver, UnsolvableOrCancelled};
use rusqlite::Connection;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::repository::versioning::VersionScheme;
use crate::version::VersionConstraint;

const MAX_LOADED_NAMES: usize = 50_000;
const TRANSITIVE_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

fn check_transitive_loading_limits(elapsed: Duration, loaded_names: usize) -> Result<()> {
    if loaded_names > MAX_LOADED_NAMES {
        return Err(Error::InitError(format!(
            "Dependency resolution discovered too many dependency names ({loaded_names} > {MAX_LOADED_NAMES})"
        )));
    }

    if elapsed > TRANSITIVE_LOAD_TIMEOUT {
        return Err(Error::InitError(format!(
            "Dependency resolution timed out while loading transitive dependencies after {:?}",
            elapsed
        )));
    }

    Ok(())
}

/// Source of a resolved package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SatSource {
    /// Package is already installed on the system.
    Installed,
    /// Package comes from a repository.
    Repository,
}

/// A single package in the SAT resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatPackage {
    pub name: String,
    pub version: String,
    pub source: SatSource,
}

/// Installed package removal authorized by exact selected relation facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatRelationRemoval {
    pub trove_id: i64,
    pub package: SatPackage,
    /// Exact selected packages whose combined facts make the relation apply.
    pub authorized_by: Vec<SatPackage>,
    pub kind: crate::repository::dependency_model::RepositoryRequirementKind,
    pub mode: crate::repository::dependency_model::PackageRelationRemovalMode,
    pub native_text: Option<String>,
}

/// Result of SAT-based dependency resolution.
#[derive(Debug)]
pub struct SatResolution {
    /// Packages to install/upgrade, in dependency order.
    pub install_order: Vec<SatPackage>,
    /// Explicit replacement/obsolescence removals, never inferred from a
    /// positive dependency or provide.
    pub remove_order: Vec<SatRelationRemoval>,
    /// Human-readable conflict explanation if unsolvable.
    pub conflict_message: Option<String>,
}

/// Solve an install request using the SAT solver.
///
/// Takes a list of `(package_name, version_constraint)` pairs and returns a
/// `SatResolution` containing the packages to install in dependency order, or
/// a conflict message if the request is unsolvable.
pub fn solve_install(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
) -> Result<SatResolution> {
    solve_install_with_policy(conn, requests, &ResolutionPolicy::new())
}

/// Solve an install request using the SAT solver with an explicit source-selection policy.
pub fn solve_install_with_policy(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
    policy: &ResolutionPolicy,
) -> Result<SatResolution> {
    if requests.is_empty() {
        return Ok(SatResolution {
            install_order: Vec::new(),
            remove_order: Vec::new(),
            conflict_message: None,
        });
    }

    let mut provider = install::build_provider_for_install(conn, requests, policy)?;
    let requirements = install::build_requirements(&mut provider, requests)?;

    let problem = Problem::new().requirements(requirements);

    // Solve
    let mut solver = Solver::new(provider);
    match solver.solve(problem) {
        Ok(solvable_ids) => {
            let relation_plan =
                relations::plan_selected_relations(solver.provider(), &solvable_ids)?;
            Ok(SatResolution {
                install_order: if relation_plan.conflict.is_some() {
                    Vec::new()
                } else {
                    install::collect_install_order(solver.provider(), &solvable_ids)
                },
                remove_order: relation_plan.removals,
                conflict_message: relation_plan.conflict,
            })
        }
        Err(UnsolvableOrCancelled::Unsolvable(conflict)) => {
            let message = conflict.display_user_friendly(&solver).to_string();
            Ok(SatResolution {
                install_order: Vec::new(),
                remove_order: Vec::new(),
                conflict_message: Some(message),
            })
        }
        Err(UnsolvableOrCancelled::Cancelled(_)) => Err(Error::InitError(
            "Dependency resolution was cancelled".to_string(),
        )),
    }
}

/// Solve exact typed package requirements using their source-native version
/// algebra and Boolean expression semantics.
pub fn solve_requirement_groups_with_policy(
    conn: &Connection,
    groups: &[RepositoryRequirementGroup],
    version_scheme: VersionScheme,
    policy: &ResolutionPolicy,
) -> Result<SatResolution> {
    let mut expressions = Vec::new();
    for group in groups {
        crate::repository::requirement::validate_requirement_group(group, version_scheme)
            .map_err(Error::ConfigError)?;
        match group.kind {
            RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends => {
                expressions.push(crate::resolver::provider::repository_expression_to_solver(
                    &group.expression,
                    version_scheme,
                )?);
            }
            RepositoryRequirementKind::Optional | RepositoryRequirementKind::Build => {}
            RepositoryRequirementKind::Conflict
            | RepositoryRequirementKind::Breaks
            | RepositoryRequirementKind::Replace
            | RepositoryRequirementKind::Obsolete => {
                return Err(Error::ConfigError(format!(
                    "negative relation '{}' cannot be solved as a positive install requirement",
                    group.kind.as_str()
                )));
            }
        }
    }

    if expressions.is_empty() {
        return Ok(SatResolution {
            install_order: Vec::new(),
            remove_order: Vec::new(),
            conflict_message: None,
        });
    }

    let mut provider =
        install::build_provider_for_requirement_expressions(conn, &expressions, policy)?;
    let requirements = install::build_expression_requirements(&mut provider, &expressions)?;
    let problem = Problem::new().requirements(requirements);
    let mut solver = Solver::new(provider);
    match solver.solve(problem) {
        Ok(solvable_ids) => {
            let relation_plan =
                relations::plan_selected_relations(solver.provider(), &solvable_ids)?;
            Ok(SatResolution {
                install_order: if relation_plan.conflict.is_some() {
                    Vec::new()
                } else {
                    install::collect_install_order(solver.provider(), &solvable_ids)
                },
                remove_order: relation_plan.removals,
                conflict_message: relation_plan.conflict,
            })
        }
        Err(UnsolvableOrCancelled::Unsolvable(conflict)) => Ok(SatResolution {
            install_order: Vec::new(),
            remove_order: Vec::new(),
            conflict_message: Some(conflict.display_user_friendly(&solver).to_string()),
        }),
        Err(UnsolvableOrCancelled::Cancelled(_)) => Err(Error::InitError(
            "Dependency resolution was cancelled".to_string(),
        )),
    }
}

/// Check what packages would break if the given packages are removed.
///
/// Returns the names of packages whose dependencies would be unsatisfied.
///
/// Instead of BFS on package names (which mishandles OR-deps and virtual
/// provides), this evaluates each dependent's full dependency clause set.
/// For OR-deps, a clause is only broken when ALL alternatives are gone.
/// The analysis iterates to a fixed point: breaking one package may cause
/// others to lose a provider, so we re-evaluate until no new breakage is found.
pub fn solve_removal(conn: &Connection, to_remove: &[String]) -> Result<Vec<String>> {
    let provider = removal::build_provider_for_removal(conn)?;
    removal::find_breaking_packages(&provider, to_remove)
}

/// Check dependency breakage for exact installed package identities.
///
/// Unlike [`solve_removal`], this preserves co-installed instances of the same
/// package name and removes only the supplied trove IDs.
pub fn solve_removal_troves(conn: &Connection, to_remove: &[i64]) -> Result<Vec<String>> {
    let provider = removal::build_provider_for_removal(conn)?;
    removal::find_breaking_packages_for_troves(&provider, to_remove)
}

#[cfg(test)]
#[path = "sat/tests.rs"]
mod tests;
