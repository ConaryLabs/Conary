// conary-core/src/resolver/provider/loading.rs

//! Database loading functions for the resolver provider.
//!
//! Contains all functions that query the database to load packages,
//! dependencies, provides, and other data needed by the solver.

use crate::db::models::{
    InstalledRequirementGroup, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, parse_repo_constraint};
use crate::resolver::identity::ProvidedCapability;

use super::types::{
    CapabilityExpression, ConaryConstraint, SolverDep, SolverExpression, SolverRelation,
};

/// Load dependency requests for a repository package.
pub(super) fn load_repo_dependency_requests(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    _repo: &crate::db::models::Repository,
) -> Result<Vec<SolverDep>> {
    let package_scheme = pkg.version_scheme;
    let repository_package_id = pkg.id.ok_or_else(|| {
        Error::MissingId(format!(
            "repository package '{}-{}' has no persisted ID while loading dependencies",
            pkg.name, pkg.version
        ))
    })?;

    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    if groups.is_empty() {
        let orphan_clauses =
            RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
        if orphan_clauses.is_empty() {
            return Ok(Vec::new());
        }
        return Err(Error::ConfigError(format!(
            "repository package '{}' has requirement clauses without authoritative expression groups",
            pkg.name
        )));
    }

    load_grouped_dependency_requests(conn, &groups, package_scheme)
}

/// Load dependency requests using the group-based model.
///
/// The persisted expression is authoritative. Behavior labels and clause rows
/// remain diagnostics/search indexes; they never decide solver semantics.
fn load_grouped_dependency_requests(
    _conn: &rusqlite::Connection,
    groups: &[RepositoryRequirementGroup],
    package_scheme: VersionScheme,
) -> Result<Vec<SolverDep>> {
    let mut deps = Vec::new();

    for group in groups {
        // Skip non-hard dependencies (optional, build, etc. for runtime resolution)
        if group.kind != "depends" && group.kind != "pre_depends" {
            continue;
        }

        let expression =
            serde_json::from_str::<RepositoryRequirementExpression>(&group.expression_json)
                .map_err(|error| {
                    Error::ConfigError(format!(
                        "repository requirement group {} has invalid expression JSON: {error}",
                        group
                            .id
                            .map_or_else(|| "<unpersisted>".to_string(), |id| id.to_string())
                    ))
                })?;
        deps.push(SolverDep {
            expression: repository_expression_to_solver(&expression, package_scheme)?,
        });
    }

    Ok(deps)
}

pub(crate) fn repository_expression_to_solver(
    expression: &RepositoryRequirementExpression,
    scheme: VersionScheme,
) -> Result<SolverExpression> {
    use RepositoryRequirementExpression as Source;

    match expression {
        Source::Atom(clause) => {
            let raw = clause.version_constraint.clone();
            let constraint = match raw.as_deref() {
                Some(value) => parse_repo_constraint(scheme, value).map_err(|error| {
                    Error::ConfigError(format!(
                        "repository requirement '{}' has invalid {:?} constraint '{}': {error}",
                        clause.name, scheme, value
                    ))
                })?,
                None => RepoVersionConstraint::Any,
            };
            Ok(SolverExpression::atom(
                clause.name.clone(),
                ConaryConstraint::Repository {
                    scheme,
                    constraint,
                    raw,
                },
            ))
        }
        Source::And(operands) => Ok(SolverExpression::And(
            operands
                .iter()
                .map(|operand| repository_expression_to_solver(operand, scheme))
                .collect::<Result<Vec<_>>>()?,
        )),
        Source::Or(operands) => Ok(SolverExpression::Or(
            operands
                .iter()
                .map(|operand| repository_expression_to_solver(operand, scheme))
                .collect::<Result<Vec<_>>>()?,
        )),
        Source::If {
            requirement,
            condition,
            otherwise,
        } => {
            let requirement = repository_expression_to_solver(requirement, scheme)?;
            let condition = repository_expression_to_solver(condition, scheme)?;
            let first = SolverExpression::Or(vec![
                SolverExpression::Not(Box::new(condition.clone())),
                requirement,
            ]);
            Ok(match otherwise {
                Some(otherwise) => SolverExpression::And(vec![
                    first,
                    SolverExpression::Or(vec![
                        condition,
                        repository_expression_to_solver(otherwise, scheme)?,
                    ]),
                ]),
                None => first,
            })
        }
        Source::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            let requirement = repository_expression_to_solver(requirement, scheme)?;
            let condition = repository_expression_to_solver(condition, scheme)?;
            let first = SolverExpression::Or(vec![condition.clone(), requirement]);
            Ok(match otherwise {
                Some(otherwise) => SolverExpression::And(vec![
                    first,
                    SolverExpression::Or(vec![
                        SolverExpression::Not(Box::new(condition)),
                        repository_expression_to_solver(otherwise, scheme)?,
                    ]),
                ]),
                None => first,
            })
        }
        Source::With { .. } | Source::Without { .. } => {
            let provider_expression = CapabilityExpression::from_repository(expression, scheme)
                .map_err(Error::ConfigError)?;
            Ok(SolverExpression::atom(
                format!("\0conary:same-provider:{provider_expression:?}"),
                ConaryConstraint::ProviderExpression {
                    expression: provider_expression,
                },
            ))
        }
    }
}

pub(super) fn relation_to_solver_dep(relation: &SolverRelation) -> Result<SolverDep> {
    Ok(SolverDep {
        expression: SolverExpression::Not(Box::new(repository_expression_to_solver(
            &relation.relation.expression,
            relation.scheme,
        )?)),
    })
}

pub(super) fn load_repo_relations(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
) -> Result<Vec<SolverRelation>> {
    let package_scheme = pkg.version_scheme;
    let repository_package_id = pkg.id.ok_or_else(|| {
        Error::MissingId(format!(
            "repository package '{}-{}' has no persisted ID while loading relations",
            pkg.name, pkg.version
        ))
    })?;
    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    let mut relations = Vec::new();
    for group in groups {
        let Some(kind) = RepositoryRequirementKind::from_str_exact(&group.kind) else {
            return Err(Error::ConfigError(format!(
                "repository requirement group {} has unknown kind '{}'",
                group
                    .id
                    .map_or_else(|| "<unpersisted>".to_string(), |id| id.to_string()),
                group.kind
            )));
        };
        if !kind.is_negative_relation() {
            continue;
        }
        let expression =
            serde_json::from_str::<RepositoryRequirementExpression>(&group.expression_json)
                .map_err(|error| {
                    Error::ConfigError(format!(
                        "repository relation group {} has invalid expression JSON: {error}",
                        group
                            .id
                            .map_or_else(|| "<unpersisted>".to_string(), |id| id.to_string())
                    ))
                })?;
        // Conversion is also strict constraint validation.
        repository_expression_to_solver(&expression, package_scheme)?;
        let alternatives = expression.atoms().into_iter().cloned().collect::<Vec<_>>();
        let first = alternatives
            .first()
            .cloned()
            .ok_or_else(|| Error::ConfigError("repository relation has no atoms".to_string()))?;
        let mut relation =
            crate::repository::dependency_model::RepositoryRequirementGroup::simple(kind, first)
                .with_behavior(if expression.is_conditional() {
                    ConditionalRequirementBehavior::Conditional
                } else {
                    ConditionalRequirementBehavior::Hard
                })
                .with_expression(expression);
        relation.alternatives = alternatives;
        relation.native_text = group.native_text;
        crate::repository::package_relation::validate_native_relation(&relation, package_scheme)
            .map_err(Error::ConfigError)?;
        relations.push(SolverRelation {
            scheme: package_scheme,
            relation,
        });
    }
    Ok(relations)
}

pub(super) fn load_installed_relations(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Vec<SolverRelation>> {
    InstalledRequirementGroup::find_by_trove(conn, trove_id)?
        .into_iter()
        .filter(|record| record.kind.is_negative_relation())
        .map(|record| {
            let relation = SolverRelation {
                scheme: record.version_scheme,
                relation: record.requirement,
            };
            repository_expression_to_solver(&relation.relation.expression, relation.scheme)?;
            Ok(relation)
        })
        .collect()
}

pub(super) fn load_installed_dependency_requests(
    conn: &rusqlite::Connection,
    trove_id: i64,
) -> Result<Vec<SolverDep>> {
    InstalledRequirementGroup::find_by_trove(conn, trove_id)?
        .into_iter()
        .filter(|record| {
            matches!(
                record.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            )
        })
        .map(|record| {
            Ok(SolverDep {
                expression: repository_expression_to_solver(
                    &record.requirement.expression,
                    record.version_scheme,
                )?,
            })
        })
        .collect()
}

/// Load provided capabilities for a repository package as simple (name, version) pairs.
pub(super) fn load_repo_provided_capabilities(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    _repo: &crate::db::models::Repository,
) -> Result<Vec<ProvidedCapability>> {
    let repository_package_id = pkg.id.ok_or_else(|| {
        Error::MissingId(format!(
            "repository package '{}-{}' has no persisted ID while loading provides",
            pkg.name, pkg.version
        ))
    })?;

    let rows = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut capabilities = Vec::new();
    for row in rows {
        let typed = if row.kind == "package" || row.kind.is_empty() {
            row.capability.clone()
        } else {
            format!("{}({})", row.kind, row.capability)
        };
        capabilities.push(ProvidedCapability {
            name: row.capability.clone(),
            version: row.version.clone(),
            version_scheme: row.version_scheme,
        });
        if typed != row.capability {
            capabilities.push(ProvidedCapability {
                name: typed,
                version: row.version,
                version_scheme: row.version_scheme,
            });
        }
    }

    Ok(capabilities)
}

/// Find a repository package by its ID.
pub(super) fn find_repo_package_by_id(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
) -> Result<Option<RepositoryPackage>> {
    RepositoryPackage::find_by_id(conn, repository_package_id)
}
