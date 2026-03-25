// conary-core/src/resolver/provider/loading.rs

//! Database loading functions for the resolver provider.
//!
//! Contains all functions that query the database to load packages,
//! dependencies, provides, and other data needed by the solver.

use crate::db::models::{
    DependencyEntry, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup,
};
use crate::error::Result;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, infer_version_scheme, parse_repo_constraint,
};
use crate::version::VersionConstraint;

use super::types::{ConaryConstraint, SolverDep};

/// Map a dependency entry to a `SolverDep` using the given version scheme.
///
/// Shared by `load_installed_packages` and `load_removal_data` to avoid
/// duplicating the scheme-aware constraint construction.
pub(super) fn dep_entry_to_solver_dep(dep: &DependencyEntry, scheme: VersionScheme) -> SolverDep {
    let constraint = match (scheme, dep.version_constraint.as_deref()) {
        (VersionScheme::Rpm, Some(s)) => {
            ConaryConstraint::Legacy(match VersionConstraint::parse(s) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        constraint = %s,
                        dep = %dep.depends_on_name,
                        error = %e,
                        "Failed to parse RPM version constraint; treating as unconstrained (may over-satisfy)"
                    );
                    VersionConstraint::Any
                }
            })
        }
        (VersionScheme::Rpm, None) => ConaryConstraint::Legacy(VersionConstraint::Any),
        (native, Some(s)) => ConaryConstraint::Repository {
            scheme: native,
            constraint: match parse_repo_constraint(native, s) {
                Some(c) => c,
                None => {
                    tracing::warn!(
                        constraint = %s,
                        dep = %dep.depends_on_name,
                        scheme = ?native,
                        "Failed to parse repo version constraint; treating as unconstrained (may over-satisfy)"
                    );
                    RepoVersionConstraint::Any
                }
            },
            raw: Some(s.to_string()),
        },
        (native, None) => ConaryConstraint::Repository {
            scheme: native,
            constraint: RepoVersionConstraint::Any,
            raw: None,
        },
    };
    SolverDep::Single(dep.depends_on_name.clone(), constraint)
}

/// Convert a flat requirement row to (name, constraint).
fn row_to_constraint(
    row: RepositoryRequirement,
    repo_scheme: Option<VersionScheme>,
) -> (String, ConaryConstraint) {
    let raw = row.version_constraint.clone();
    let constraint = match (repo_scheme, raw.as_deref()) {
        (Some(scheme), Some(value)) => ConaryConstraint::Repository {
            scheme,
            constraint: match parse_repo_constraint(scheme, value) {
                Some(c) => c,
                None => {
                    tracing::warn!(
                        constraint = %value,
                        capability = %row.capability,
                        scheme = ?scheme,
                        "Failed to parse repo version constraint in requirement row; treating as unconstrained (may over-satisfy)"
                    );
                    RepoVersionConstraint::Any
                }
            },
            raw,
        },
        (Some(scheme), None) => ConaryConstraint::Repository {
            scheme,
            constraint: RepoVersionConstraint::Any,
            raw: None,
        },
        (None, Some(value)) => ConaryConstraint::Legacy(match VersionConstraint::parse(value) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    constraint = %value,
                    capability = %row.capability,
                    error = %e,
                    "Failed to parse legacy version constraint in requirement row; treating as unconstrained (may over-satisfy)"
                );
                VersionConstraint::Any
            }
        }),
        (None, None) => ConaryConstraint::Legacy(VersionConstraint::Any),
    };
    (row.capability, constraint)
}

/// Load dependency requests for a repository package.
pub(super) fn load_repo_dependency_requests(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    repo: &crate::db::models::Repository,
) -> Result<Vec<SolverDep>> {
    let repo_scheme = infer_version_scheme(repo);
    let Some(repository_package_id) = pkg.id else {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| SolverDep::Single(name, ConaryConstraint::Legacy(constraint)))
            .collect());
    };

    // Try group-based loading first for OR and conditional support
    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    if !groups.is_empty() {
        return load_grouped_dependency_requests(conn, &groups, repo_scheme);
    }

    // Fall back to flat requirement rows (legacy data or non-grouped deps)
    let rows = RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| SolverDep::Single(name, ConaryConstraint::Legacy(constraint)))
            .collect());
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let (name, constraint) = row_to_constraint(row, repo_scheme);
            SolverDep::Single(name, constraint)
        })
        .collect())
}

/// Load dependency requests using the group-based model.
///
/// Groups with `behavior = "hard"` become solver requirements.
/// Multi-clause groups produce `SolverDep::OrGroup` (Debian OR dependencies).
/// Conditional and unsupported-rich behaviors are logged and skipped.
fn load_grouped_dependency_requests(
    conn: &rusqlite::Connection,
    groups: &[RepositoryRequirementGroup],
    repo_scheme: Option<VersionScheme>,
) -> Result<Vec<SolverDep>> {
    let mut deps = Vec::new();

    for group in groups {
        // Skip non-hard dependencies (optional, build, etc. for runtime resolution)
        if group.kind != "depends" && group.kind != "pre_depends" {
            continue;
        }

        match group.behavior.as_str() {
            "conditional" | "unsupported_rich" => {
                tracing::debug!(
                    "Skipping {} dependency (behavior={}): {:?}",
                    group.kind,
                    group.behavior,
                    group.native_text,
                );
                continue;
            }
            _ => {} // "hard" -- process normally
        }

        let Some(group_id) = group.id else {
            continue;
        };
        let clauses = RepositoryRequirement::find_by_group(conn, group_id)?;

        if clauses.is_empty() {
            continue;
        }

        if clauses.len() == 1 {
            let (name, constraint) =
                row_to_constraint(clauses.into_iter().next().unwrap(), repo_scheme);
            deps.push(SolverDep::Single(name, constraint));
        } else {
            // Multi-clause: OR-group
            let alternatives: Vec<(String, ConaryConstraint)> = clauses
                .into_iter()
                .map(|clause| row_to_constraint(clause, repo_scheme))
                .collect();
            deps.push(SolverDep::OrGroup(alternatives));
        }
    }

    Ok(deps)
}

/// Load provided capabilities for a repository package as simple (name, version) pairs.
pub(super) fn load_repo_provided_capabilities(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    repo: &crate::db::models::Repository,
) -> Result<Vec<(String, Option<String>)>> {
    let _repo_scheme = infer_version_scheme(repo);
    let Some(repository_package_id) = pkg.id else {
        return Ok(parse_repo_provides(pkg));
    };

    let rows = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(parse_repo_provides(pkg));
    }

    Ok(rows
        .into_iter()
        .map(|row| (row.capability, row.version))
        .collect())
}

/// Find a repository package by its ID.
pub(super) fn find_repo_package_by_id(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
) -> Result<Option<RepositoryPackage>> {
    RepositoryPackage::find_by_id(conn, repository_package_id)
}

/// Escape special characters for SQL LIKE patterns.
///
/// SQLite LIKE treats `%` and `_` as wildcards. When searching for literal
/// text we must escape them (along with the escape character itself).
pub(super) fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Parse a stored version scheme string into its enum variant.
pub(super) fn parse_stored_version_scheme(raw: Option<&str>) -> Option<VersionScheme> {
    match raw? {
        "rpm" => Some(VersionScheme::Rpm),
        "debian" => Some(VersionScheme::Debian),
        "arch" => Some(VersionScheme::Arch),
        _ => None,
    }
}

fn parse_repo_provides(pkg: &RepositoryPackage) -> Vec<(String, Option<String>)> {
    let Some(metadata_json) = pkg.metadata.as_deref() else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return Vec::new();
    };
    let Some(provides) = metadata
        .get("rpm_provides")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };

    provides
        .iter()
        .filter_map(|value| value.as_str())
        .map(parse_provide_entry)
        .collect()
}

fn parse_provide_entry(entry: &str) -> (String, Option<String>) {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = entry.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            return (name.to_string(), Some(version.to_string()));
        }
    }

    (entry.trim().to_string(), None)
}

/// Check whether a repository package provides a given capability.
pub(super) fn repo_package_provides_capability(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    capability: &str,
) -> Result<bool> {
    let Some(repo) = crate::db::models::Repository::find_by_id(conn, pkg.repository_id)? else {
        return Ok(false);
    };
    Ok(load_repo_provided_capabilities(conn, pkg, &repo)?
        .iter()
        .any(|(provided, _version)| provided == capability))
}
