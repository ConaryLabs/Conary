// conary-core/src/resolver/sat.rs

//! SAT-based dependency resolution using resolvo.
//!
//! Provides policy-explicit install solving and exact removal analysis using
//! the CDCL SAT solver with backtracking support.

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
use crate::resolver::identity::PackageIdentity;
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
    pub package_release: Option<String>,
    pub architecture: Option<String>,
    pub version_scheme: crate::repository::versioning::VersionScheme,
    pub repo_package_id: Option<i64>,
    pub repository_id: Option<i64>,
    pub repository_name: Option<String>,
    pub installed_trove_id: Option<i64>,
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
    policy
        .validate_for_dependency_resolution()
        .map_err(Error::ConfigError)?;

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
    policy
        .validate_for_dependency_resolution()
        .map_err(Error::ConfigError)?;

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

/// Return whether an incoming package already satisfies one positive
/// requirement group from its exact identity and declared provides.
///
/// Conditional and negated forms remain SAT-owned because their truth can
/// depend on other packages selected into the transaction. Positive atoms,
/// conjunctions, and disjunctions are safe to discharge against the incoming
/// package alone.
pub fn positive_requirement_group_satisfied_by_package(
    group: &RepositoryRequirementGroup,
    version_scheme: VersionScheme,
    package: &PackageIdentity,
) -> Result<bool> {
    crate::repository::requirement::validate_requirement_group(group, version_scheme)
        .map_err(Error::ConfigError)?;
    if !matches!(
        group.kind,
        RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
    ) {
        return Ok(false);
    }

    fn matches_positive_expression(
        expression: &crate::repository::dependency_model::RepositoryRequirementExpression,
        version_scheme: VersionScheme,
        package: &PackageIdentity,
        native_architecture: &str,
    ) -> Result<bool> {
        use crate::repository::dependency_model::RepositoryRequirementExpression as Expression;
        match expression {
            Expression::Atom(_) => {
                let solver_expression = crate::resolver::provider::repository_expression_to_solver(
                    expression,
                    version_scheme,
                )?;
                let crate::resolver::provider::SolverExpression::Atom(atom) = solver_expression
                else {
                    return Ok(false);
                };
                crate::resolver::provider::matching::constraint_matches_candidate(
                    &atom.name,
                    &atom.constraint,
                    package,
                    native_architecture,
                    package.name == atom.name,
                )
                .map_err(Error::from)
            }
            Expression::And(operands) => {
                for operand in operands {
                    if !matches_positive_expression(
                        operand,
                        version_scheme,
                        package,
                        native_architecture,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Expression::Or(operands) => {
                for operand in operands {
                    if matches_positive_expression(
                        operand,
                        version_scheme,
                        package,
                        native_architecture,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Expression::If { .. }
            | Expression::Unless { .. }
            | Expression::With { .. }
            | Expression::Without { .. } => Ok(false),
        }
    }

    matches_positive_expression(
        &group.expression,
        version_scheme,
        package,
        &crate::repository::registry::detect_system_arch(),
    )
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
