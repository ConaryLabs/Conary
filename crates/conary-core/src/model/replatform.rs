// conary-core/src/model/replatform.rs

//! Shared helpers for source-policy replatform planning.

use rusqlite::Connection;

use crate::db::models::{
    LabelEntry, PackageResolution, Repository, RepositoryPackage, RepositoryRequirementGroup,
    SystemAffinity, Trove,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::load_effective_policy;
use crate::repository::resolution_policy::RequestScope;
use crate::repository::selector::{PackageSelector, SelectionOptions};
use crate::resolver::requirement_expression_satisfied;
use crate::resolver::requirements::load_requirement_candidate_identities;

use super::diff::{DiffAction, ReplatformEstimate};
use super::state::SystemState;

/// Visible package-level realignment candidates for a target distro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRealignmentCandidates {
    pub target_distro: String,
    pub candidate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePolicyReplatformSnapshot {
    pub target_distro: String,
    pub estimate: Option<ReplatformEstimate>,
    pub visible_realignment_candidates: usize,
    pub visible_realignment_proposals: Vec<VisibleRealignmentProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRealignmentProposal {
    pub package: String,
    pub current_distro: Option<String>,
    pub target_distro: String,
    pub target_version: String,
    pub architecture: Option<String>,
    pub target_repository: Option<String>,
    pub target_repository_package_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplatformExecutionLeg {
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplatformExecutionTransaction {
    pub package: String,
    pub current_distro: Option<String>,
    pub target_distro: String,
    pub current_version: String,
    pub current_architecture: Option<String>,
    pub target_version: String,
    pub architecture: Option<String>,
    pub install_repository: Option<String>,
    pub install_repository_package_id: Option<i64>,
    pub install_route: Option<String>,
    pub unresolved_dependencies: Vec<String>,
    pub remove_leg: ReplatformExecutionLeg,
    pub install_leg: ReplatformExecutionLeg,
    pub metadata_leg: ReplatformExecutionLeg,
    pub executable: bool,
    pub blocked_reasons: Vec<ReplatformBlockedReason>,
    pub blocked_reason: Option<ReplatformBlockedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplatformExecutionPlan {
    pub transactions: Vec<ReplatformExecutionTransaction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplatformBlockedReason {
    MissingRepositoryMetadata,
    MissingRepositoryPackageId,
    AnyVersionRouteOnly,
    MissingInstallRoute,
    MissingVersionedInstallRoute,
    UnsatisfiedTargetDependencies,
    ArchitectureMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallRouteKind {
    ExactVersion,
    AnyVersionFallback,
    DefaultStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedInstallRoute {
    route: String,
    kind: InstallRouteKind,
}

fn candidate_target_package(
    conn: &Connection,
    trove: &Trove,
    target_distro: &str,
) -> Result<Option<RepositoryPackage>> {
    // Replatform planning is an explicit lookup in the desired profile. The
    // current system pin describes the source state and must not remain the
    // transaction authority while selecting the target replacement.
    let mut effective_policy =
        load_effective_policy(conn, RequestScope::DistroProfile(target_distro.to_string()))?;
    effective_policy.resolution.allowed_distros = vec![target_distro.to_string()];

    let options = SelectionOptions {
        architecture: trove.architecture.clone(),
        policy: Some(effective_policy.resolution),
        is_root: false,
        ..SelectionOptions::default()
    };

    let candidates = PackageSelector::search_packages(conn, &trove.name, &options)?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = PackageSelector::select_best_with_options(conn, candidates, &options)?;
    Ok(Some(selected.package))
}

fn install_route_for_target(
    conn: &Connection,
    repository_name: Option<&str>,
    repository_package_id: Option<i64>,
    package: &str,
    version: &str,
) -> Result<Option<PlannedInstallRoute>> {
    let Some(repository_name) = repository_name else {
        return Ok(None);
    };
    let Some(repo) = Repository::find_by_name(conn, repository_name)? else {
        return Ok(None);
    };
    let Some(repo_id) = repo.id else {
        return Ok(None);
    };

    if let Some(resolution) = PackageResolution::find(conn, repo_id, package, Some(version))? {
        let kind = if resolution.version.as_deref() == Some(version) {
            InstallRouteKind::ExactVersion
        } else {
            InstallRouteKind::AnyVersionFallback
        };
        return Ok(Some(PlannedInstallRoute {
            route: format!("resolution:{}", resolution.primary_strategy.as_str()),
            kind,
        }));
    }

    if let Some(default_strategy) = repo.default_strategy {
        return Ok(Some(PlannedInstallRoute {
            route: format!("default:{}", default_strategy),
            kind: InstallRouteKind::DefaultStrategy,
        }));
    }

    if repository_package_id.is_some() {
        return Ok(None);
    }

    Ok(None)
}

pub fn replatform_estimate_from_affinities(
    affinities: &[SystemAffinity],
    target_distro: &str,
) -> Option<ReplatformEstimate> {
    if affinities.is_empty() {
        return None;
    }

    let total_packages: i64 = affinities
        .iter()
        .map(|affinity| affinity.package_count)
        .sum();
    if total_packages == 0 {
        return None;
    }

    let aligned_packages = affinities
        .iter()
        .find(|affinity| affinity.distro == target_distro)
        .map(|affinity| affinity.package_count)
        .unwrap_or(0);

    Some(ReplatformEstimate {
        target_distro: target_distro.to_string(),
        aligned_packages,
        packages_to_realign: total_packages.saturating_sub(aligned_packages),
        total_packages,
    })
}

pub fn source_policy_replatform_snapshot(
    conn: &Connection,
    target_distro: &str,
) -> Result<SourcePolicyReplatformSnapshot> {
    let affinities = SystemAffinity::list(conn)?;
    let visible_realignment_proposals = visible_realignment_proposals(conn, target_distro)?;

    Ok(SourcePolicyReplatformSnapshot {
        target_distro: target_distro.to_string(),
        estimate: replatform_estimate_from_affinities(&affinities, target_distro),
        visible_realignment_candidates: visible_realignment_proposals.len(),
        visible_realignment_proposals,
    })
}

pub fn planned_replatform_actions(
    snapshot: &SourcePolicyReplatformSnapshot,
    state: &SystemState,
) -> Vec<DiffAction> {
    let mut actions = Vec::new();

    for proposal in &snapshot.visible_realignment_proposals {
        // When the proposal targets a specific architecture, look up that
        // instance so multilib systems get the correct current_version/arch
        // instead of always using the first (typically native-arch) instance.
        let installed = if let Some(ref target_arch) = proposal.architecture {
            state
                .get_all_instances(&proposal.package)
                .iter()
                .find(|p| p.architecture.as_deref() == Some(target_arch))
                .or_else(|| state.get_package(&proposal.package))
        } else {
            state.get_package(&proposal.package)
        };

        let Some(installed) = installed else {
            continue;
        };

        actions.push(DiffAction::ReplatformReplace {
            package: proposal.package.clone(),
            current_distro: proposal.current_distro.clone(),
            target_distro: proposal.target_distro.clone(),
            current_version: installed.version.clone(),
            current_architecture: installed.architecture.clone(),
            target_version: proposal.target_version.clone(),
            architecture: proposal
                .architecture
                .clone()
                .or_else(|| installed.architecture.clone()),
            target_repository: proposal.target_repository.clone(),
            target_repository_package_id: proposal.target_repository_package_id,
        });
    }

    actions
}

pub fn replatform_execution_plan(
    conn: &Connection,
    actions: &[DiffAction],
) -> Result<Option<ReplatformExecutionPlan>> {
    let mut transactions = Vec::new();

    for action in actions {
        if let DiffAction::ReplatformReplace {
            package,
            current_distro,
            target_distro,
            current_version,
            current_architecture,
            target_version,
            architecture,
            target_repository,
            target_repository_package_id,
        } = action
        {
            let install_route = install_route_for_target(
                conn,
                target_repository.as_deref(),
                *target_repository_package_id,
                package,
                target_version,
            )?;
            let unresolved_dependencies = unresolved_target_dependencies(
                conn,
                target_repository.as_deref(),
                *target_repository_package_id,
                architecture.as_deref(),
            )?;
            let install_route_kind = install_route.as_ref().map(|route| route.kind);
            let architecture_mismatch = match (current_architecture.as_ref(), architecture.as_ref())
            {
                (Some(current_arch), Some(target_arch)) => current_arch != target_arch,
                _ => false,
            };
            let mut blocked_reasons = Vec::new();
            if architecture_mismatch {
                blocked_reasons.push(ReplatformBlockedReason::ArchitectureMismatch);
            }
            if !unresolved_dependencies.is_empty() {
                blocked_reasons.push(ReplatformBlockedReason::UnsatisfiedTargetDependencies);
            }
            if target_repository.is_none() {
                blocked_reasons.push(ReplatformBlockedReason::MissingRepositoryMetadata);
            }
            if target_repository.is_some() && target_repository_package_id.is_none() {
                blocked_reasons.push(ReplatformBlockedReason::MissingRepositoryPackageId);
            }
            if target_repository.is_some()
                && target_repository_package_id.is_some()
                && install_route_kind.is_none()
            {
                blocked_reasons.push(ReplatformBlockedReason::MissingInstallRoute);
            }
            if matches!(
                install_route_kind,
                Some(InstallRouteKind::AnyVersionFallback)
            ) {
                blocked_reasons.push(ReplatformBlockedReason::AnyVersionRouteOnly);
            }
            if matches!(install_route_kind, Some(InstallRouteKind::DefaultStrategy)) {
                blocked_reasons.push(ReplatformBlockedReason::MissingVersionedInstallRoute);
            }

            let remove_leg = ReplatformExecutionLeg { ready: true };
            let install_leg = ReplatformExecutionLeg {
                ready: target_repository.is_some()
                    && target_repository_package_id.is_some()
                    && matches!(install_route_kind, Some(InstallRouteKind::ExactVersion))
                    && !architecture_mismatch
                    && unresolved_dependencies.is_empty(),
            };
            let metadata_leg = ReplatformExecutionLeg {
                ready: target_repository.is_some() && target_repository_package_id.is_some(),
            };
            let blocked_reason = blocked_reasons.first().cloned();
            transactions.push(ReplatformExecutionTransaction {
                package: package.clone(),
                current_distro: current_distro.clone(),
                target_distro: target_distro.clone(),
                current_version: current_version.clone(),
                current_architecture: current_architecture.clone(),
                target_version: target_version.clone(),
                architecture: architecture.clone(),
                install_repository: target_repository.clone(),
                install_repository_package_id: *target_repository_package_id,
                install_route: install_route.map(|route| route.route),
                unresolved_dependencies,
                remove_leg,
                install_leg,
                metadata_leg,
                executable: blocked_reasons.is_empty(),
                blocked_reasons,
                blocked_reason,
            });
        }
    }

    if transactions.is_empty() {
        return Ok(None);
    }

    transactions.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(Some(ReplatformExecutionPlan { transactions }))
}

fn unresolved_target_dependencies(
    conn: &Connection,
    repository_name: Option<&str>,
    repository_package_id: Option<i64>,
    architecture: Option<&str>,
) -> Result<Vec<String>> {
    let Some(repository_name) = repository_name else {
        return Ok(Vec::new());
    };
    let Some(repository_package_id) = repository_package_id else {
        return Ok(Vec::new());
    };

    let Some(repository) = Repository::find_by_name(conn, repository_name)? else {
        return Ok(Vec::new());
    };
    let repository_id = repository
        .id
        .ok_or_else(|| Error::InitError(format!("repository {repository_name} has no ID")))?;
    let Some(target_pkg) = RepositoryPackage::find_by_repository(conn, repository_id)?
        .into_iter()
        .find(|pkg| pkg.id == Some(repository_package_id))
    else {
        return Ok(Vec::new());
    };

    let target_scheme = target_pkg.version_scheme;
    let depending_architecture = target_pkg
        .architecture
        .as_deref()
        .filter(|architecture| !architecture.is_empty())
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "repository package '{}' has no architecture authority",
                target_pkg.name
            ))
        })?;
    let native_architecture = architecture
        .filter(|architecture| !architecture.is_empty())
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "replatform target '{}' has no native architecture authority",
                target_pkg.name
            ))
        })?;
    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    let mut unresolved = Vec::new();

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
        if !matches!(
            kind,
            RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
        ) {
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
        let candidates = load_requirement_candidate_identities(conn, &expression, target_scheme)?;
        if !requirement_expression_satisfied(
            &expression,
            target_scheme,
            depending_architecture,
            native_architecture,
            &candidates,
        )? {
            unresolved.push(requirement_group_label(&group, &expression));
        }
    }

    unresolved.sort();
    unresolved.dedup();
    Ok(unresolved)
}

fn requirement_group_label(
    group: &RepositoryRequirementGroup,
    expression: &RepositoryRequirementExpression,
) -> String {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => match &clause.version_constraint {
            Some(constraint) => format!("{} ({constraint})", clause.name),
            None => clause.name.clone(),
        },
        _ => group
            .native_text
            .clone()
            .unwrap_or_else(|| group.expression_json.clone()),
    }
}

fn current_package_distro(conn: &Connection, trove: &Trove) -> Result<Option<String>> {
    let Some(label_id) = trove.label_id else {
        return Ok(None);
    };
    let Some(label) = LabelEntry::find_by_id(conn, label_id)? else {
        return Ok(None);
    };
    let Some(repo_id) = label.repository_id else {
        return Ok(None);
    };
    Ok(Repository::find_by_id(conn, repo_id)?.and_then(|repo| repo.source_profile))
}

pub fn visible_realignment_candidates(
    conn: &Connection,
    target_distro: &str,
) -> Result<VisibleRealignmentCandidates> {
    let proposals = visible_realignment_proposals(conn, target_distro)?;
    Ok(VisibleRealignmentCandidates {
        target_distro: target_distro.to_string(),
        candidate_count: proposals.len(),
    })
}

pub fn visible_realignment_proposals(
    conn: &Connection,
    target_distro: &str,
) -> Result<Vec<VisibleRealignmentProposal>> {
    let troves = Trove::list_packages(conn)?;
    let mut proposals = Vec::new();

    for trove in troves.into_iter() {
        let current_distro = current_package_distro(conn, &trove)?;
        if current_distro.as_deref() == Some(target_distro) {
            continue;
        }

        if let Some(target_pkg) = candidate_target_package(conn, &trove, target_distro)? {
            proposals.push(VisibleRealignmentProposal {
                package: trove.name.clone(),
                current_distro,
                target_distro: target_distro.to_string(),
                target_version: target_pkg.version,
                architecture: target_pkg
                    .architecture
                    .or_else(|| trove.architecture.clone()),
                target_repository: Repository::find_by_id(conn, target_pkg.repository_id)?
                    .map(|repo| repo.name),
                target_repository_package_id: target_pkg.id,
            });
        }
    }

    proposals.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(proposals)
}

#[cfg(test)]
#[path = "replatform/tests.rs"]
mod tests;
