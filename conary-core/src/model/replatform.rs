// conary-core/src/model/replatform.rs

//! Shared helpers for source-policy replatform planning.

use rusqlite::Connection;

use crate::db::models::{
    LabelEntry, PackageResolution, ProvideEntry, Repository, RepositoryPackage, RepositoryProvide,
    RepositoryRequirement, SystemAffinity, Trove, TroveType,
};
use crate::error::Result;
use crate::repository::selector::{PackageSelector, SelectionOptions};
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, compare_repo_package_versions, infer_version_scheme,
    parse_repo_constraint, repo_version_satisfies,
};

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
    pub executable: bool,
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
    let repo_packages = RepositoryPackage::find_by_name(conn, &trove.name)?;
    let mut candidates = Vec::new();

    for repo_pkg in repo_packages {
        if repo_pkg.architecture != trove.architecture && repo_pkg.architecture.is_some() {
            continue;
        }

        let Some(repo) = Repository::find_by_id(conn, repo_pkg.repository_id)? else {
            continue;
        };
        if repo.default_strategy_distro.as_deref() == Some(target_distro) {
            candidates.push((repo_pkg, repo));
        }
    }

    candidates.sort_by(|(pkg_a, repo_a), (pkg_b, repo_b)| {
        compare_repo_package_versions(pkg_a, repo_a, pkg_b, repo_b)
            .map(|ord| ord.reverse())
            .unwrap_or_else(|| {
                repo_b
                    .name
                    .cmp(&repo_a.name)
                    .then_with(|| pkg_b.version.cmp(&pkg_a.version))
            })
    });
    Ok(candidates.into_iter().next().map(|(pkg, _)| pkg))
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
        let Some(installed) = state.installed.get(&proposal.package) else {
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
            let blocked_reason = match (
                target_repository,
                target_repository_package_id,
                install_route_kind,
                architecture_mismatch,
                unresolved_dependencies.is_empty(),
            ) {
                (_, _, _, true, _) => Some(ReplatformBlockedReason::ArchitectureMismatch),
                (_, _, _, false, false) => {
                    Some(ReplatformBlockedReason::UnsatisfiedTargetDependencies)
                }
                (Some(_), Some(_), Some(InstallRouteKind::ExactVersion), false, true) => None,
                (Some(_), Some(_), Some(InstallRouteKind::AnyVersionFallback), false, true) => {
                    Some(ReplatformBlockedReason::AnyVersionRouteOnly)
                }
                (Some(_), Some(_), Some(InstallRouteKind::DefaultStrategy), false, true) => {
                    Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
                }
                (None, _, _, false, true) => {
                    Some(ReplatformBlockedReason::MissingRepositoryMetadata)
                }
                (Some(_), None, _, false, true) => {
                    Some(ReplatformBlockedReason::MissingRepositoryPackageId)
                }
                (Some(_), Some(_), None, false, true) => {
                    Some(ReplatformBlockedReason::MissingInstallRoute)
                }
            };
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
                executable: blocked_reason.is_none(),
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
    let Some(target_pkg) =
        RepositoryPackage::find_by_repository(conn, repository.id.unwrap_or_default())?
            .into_iter()
            .find(|pkg| pkg.id == Some(repository_package_id))
    else {
        return Ok(Vec::new());
    };

    let target_scheme = infer_version_scheme(&repository).unwrap_or(VersionScheme::Rpm);
    let requests =
        normalized_requirement_requests(conn, repository_package_id, &target_pkg, target_scheme)?;
    let detected_arch = PackageSelector::detect_architecture();
    let target_arch = architecture.unwrap_or(&detected_arch);
    let mut unresolved = Vec::new();

    for (dep_name, constraint, raw_constraint) in requests {
        let candidates = PackageSelector::search_packages(
            conn,
            &dep_name,
            &SelectionOptions {
                architecture: Some(target_arch.to_string()),
                ..SelectionOptions::default()
            },
        )?;
        let satisfied =
            candidates.into_iter().any(|candidate| {
                let Some(candidate_scheme) = infer_version_scheme(&candidate.repository) else {
                    return matches!(constraint, RepoVersionConstraint::Any);
                };
                if !matches!(constraint, RepoVersionConstraint::Any)
                    && candidate_scheme != target_scheme
                {
                    return false;
                }
                repo_version_satisfies(candidate_scheme, &candidate.package.version, &constraint)
            }) || normalized_repo_provider_satisfies(
                conn,
                &dep_name,
                &constraint,
                target_arch,
                target_scheme,
            )? || repo_metadata_provider_satisfies(
                conn,
                &dep_name,
                &constraint,
                target_arch,
                target_scheme,
            )? || tracked_provider_satisfies(conn, &dep_name, &constraint, target_scheme);

        if !satisfied {
            unresolved.push(match raw_constraint {
                Some(raw) => format!("{dep_name} ({raw})"),
                None => dep_name,
            });
        }
    }

    unresolved.sort();
    unresolved.dedup();
    Ok(unresolved)
}

fn normalized_requirement_requests(
    conn: &Connection,
    repository_package_id: i64,
    target_pkg: &RepositoryPackage,
    scheme: VersionScheme,
) -> Result<Vec<(String, RepoVersionConstraint, Option<String>)>> {
    let rows = RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(target_pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| {
                let raw = match constraint {
                    crate::version::VersionConstraint::Any => None,
                    _ => Some(constraint.to_string()),
                };
                let repo_constraint = raw
                    .as_deref()
                    .and_then(|value| parse_repo_constraint(scheme, value))
                    .unwrap_or(RepoVersionConstraint::Any);
                (name, repo_constraint, raw)
            })
            .collect());
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let raw = row.version_constraint.clone();
            let constraint = raw
                .as_deref()
                .and_then(|value| parse_repo_constraint(scheme, value))
                .unwrap_or(RepoVersionConstraint::Any);
            (row.capability, constraint, raw)
        })
        .collect())
}

fn normalized_repo_provider_satisfies(
    conn: &Connection,
    dependency_name: &str,
    constraint: &RepoVersionConstraint,
    target_arch: &str,
    requirement_scheme: VersionScheme,
) -> Result<bool> {
    let provides = RepositoryProvide::find_by_capability(conn, dependency_name)?;

    for provide in provides {
        let Some(pkg) = repository_package_by_id(conn, provide.repository_package_id)? else {
            continue;
        };
        if !PackageSelector::is_architecture_compatible(pkg.architecture.as_deref(), target_arch) {
            continue;
        }
        let Some(repo) = Repository::find_by_id(conn, pkg.repository_id)? else {
            continue;
        };
        let Some(provider_scheme) = infer_version_scheme(&repo) else {
            continue;
        };
        if !matches!(constraint, RepoVersionConstraint::Any)
            && provider_scheme != requirement_scheme
        {
            continue;
        }

        let satisfied = match (constraint, provide.version.as_deref()) {
            (RepoVersionConstraint::Any, _) => true,
            (_, Some(version)) => repo_version_satisfies(provider_scheme, version, constraint),
            (_, None) => false,
        };

        if satisfied {
            return Ok(true);
        }
    }

    Ok(false)
}

fn repository_package_by_id(
    conn: &Connection,
    repository_package_id: i64,
) -> Result<Option<RepositoryPackage>> {
    let packages = RepositoryPackage::list_all(conn)?;
    Ok(packages
        .into_iter()
        .find(|pkg| pkg.id == Some(repository_package_id)))
}

fn repo_metadata_provider_satisfies(
    conn: &Connection,
    dependency_name: &str,
    constraint: &RepoVersionConstraint,
    target_arch: &str,
    requirement_scheme: VersionScheme,
) -> Result<bool> {
    let packages = RepositoryPackage::list_all(conn)?;

    for pkg in packages {
        if !PackageSelector::is_architecture_compatible(pkg.architecture.as_deref(), target_arch) {
            continue;
        }
        let Some(repo) = Repository::find_by_id(conn, pkg.repository_id)? else {
            continue;
        };
        let Some(provider_scheme) = infer_version_scheme(&repo) else {
            continue;
        };
        if !matches!(constraint, RepoVersionConstraint::Any)
            && provider_scheme != requirement_scheme
        {
            continue;
        }

        for (provided_name, provided_version) in parse_repo_metadata_provides(&pkg) {
            if provided_name != dependency_name {
                continue;
            }

            let satisfied = match (constraint, provided_version) {
                (RepoVersionConstraint::Any, _) => true,
                (_, Some(version)) => repo_version_satisfies(provider_scheme, &version, constraint),
                (_, None) => false,
            };

            if satisfied {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn parse_repo_metadata_provides(pkg: &RepositoryPackage) -> Vec<(String, Option<String>)> {
    let Some(metadata_json) = pkg.metadata.as_deref() else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return Vec::new();
    };
    let mut parsed = Vec::new();

    for key in ["rpm_provides", "deb_provides", "arch_provides"] {
        let Some(provides) = metadata.get(key).and_then(|value| value.as_array()) else {
            continue;
        };
        parsed.extend(
            provides
                .iter()
                .filter_map(|value| value.as_str())
                .map(parse_repo_metadata_provide_entry),
        );
    }

    parsed
}

fn parse_repo_metadata_provide_entry(entry: &str) -> (String, Option<String>) {
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

fn tracked_provider_satisfies(
    conn: &Connection,
    dependency_name: &str,
    constraint: &RepoVersionConstraint,
    requirement_scheme: VersionScheme,
) -> bool {
    let Ok(provider) = ProvideEntry::find_satisfying_provider_fuzzy(conn, dependency_name) else {
        return false;
    };
    let Some((_provider_name, provider_version)) = provider else {
        return false;
    };

    match constraint {
        RepoVersionConstraint::Any => true,
        _ if requirement_scheme != VersionScheme::Rpm => false,
        _ => repo_version_satisfies(VersionScheme::Rpm, &provider_version, constraint),
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
    Ok(Repository::find_by_id(conn, repo_id)?.and_then(|repo| repo.default_strategy_distro))
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
    let troves = Trove::list_all(conn)?;
    let mut proposals = Vec::new();

    for trove in troves
        .into_iter()
        .filter(|t| t.trove_type == TroveType::Package)
    {
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
mod tests {
    use super::*;
    use crate::db::models::{
        InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, SystemAffinity, Trove, TroveType,
    };
    use crate::db::testing::create_test_db;
    use crate::model::state::{InstalledPackage, SystemState};

    #[test]
    fn test_visible_realignment_candidates_counts_same_name_target_impls() {
        let (_temp, conn) = create_test_db();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo =
            Repository::new("arch".to_string(), "https://example.test/arch".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.insert(&conn).unwrap();
        fedora_label
            .set_repository(&conn, Some(fedora_repo_id))
            .unwrap();

        let mut trove = Trove::new_with_source(
            "vim".to_string(),
            "1.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.label_id = fedora_label.id;
        trove.insert(&conn).unwrap();

        let mut arch_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "2.0".to_string(),
            "sha256:test".to_string(),
            123,
            "https://example.test/arch/vim.pkg.tar.zst".to_string(),
        );
        arch_pkg.architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        let summary = visible_realignment_candidates(&conn, "arch").unwrap();
        assert_eq!(summary.target_distro, "arch");
        assert_eq!(summary.candidate_count, 1);
    }

    #[test]
    fn test_replatform_estimate_from_affinities_uses_target_counts() {
        let affinities = vec![
            SystemAffinity {
                distro: "fedora-43".to_string(),
                package_count: 9,
                percentage: 75.0,
            },
            SystemAffinity {
                distro: "arch".to_string(),
                package_count: 3,
                percentage: 25.0,
            },
        ];

        let estimate = replatform_estimate_from_affinities(&affinities, "arch")
            .expect("expected affinity-based estimate");

        assert_eq!(estimate.target_distro, "arch");
        assert_eq!(estimate.aligned_packages, 3);
        assert_eq!(estimate.packages_to_realign, 9);
        assert_eq!(estimate.total_packages, 12);
    }

    #[test]
    fn test_source_policy_replatform_snapshot_combines_estimate_and_candidates() {
        let (_temp, conn) = create_test_db();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo =
            Repository::new("arch".to_string(), "https://example.test/arch".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.insert(&conn).unwrap();
        fedora_label
            .set_repository(&conn, Some(fedora_repo_id))
            .unwrap();

        let mut trove = Trove::new_with_source(
            "vim".to_string(),
            "1.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.label_id = fedora_label.id;
        trove.insert(&conn).unwrap();

        let mut arch_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "2.0".to_string(),
            "sha256:test".to_string(),
            123,
            "https://example.test/arch/vim.pkg.tar.zst".to_string(),
        );
        arch_pkg.architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            ("fedora-43", 1_i64, 100.0_f64),
        )
        .unwrap();

        let snapshot = source_policy_replatform_snapshot(&conn, "arch").unwrap();

        assert_eq!(snapshot.target_distro, "arch");
        assert_eq!(snapshot.visible_realignment_candidates, 1);
        assert_eq!(snapshot.visible_realignment_proposals.len(), 1);
        assert_eq!(snapshot.visible_realignment_proposals[0].package, "vim");
        assert_eq!(
            snapshot.visible_realignment_proposals[0]
                .current_distro
                .as_deref(),
            Some("fedora-43")
        );
        assert_eq!(
            snapshot.visible_realignment_proposals[0].target_distro,
            "arch"
        );
        assert_eq!(
            snapshot.visible_realignment_proposals[0].target_version,
            "2.0"
        );
        assert_eq!(
            snapshot.visible_realignment_proposals[0]
                .architecture
                .as_deref(),
            Some("x86_64")
        );
        let estimate = snapshot.estimate.expect("expected estimate");
        assert_eq!(estimate.aligned_packages, 0);
        assert_eq!(estimate.packages_to_realign, 1);
        assert_eq!(estimate.total_packages, 1);
    }

    #[test]
    fn test_source_policy_replatform_snapshot_uses_native_repo_version_ordering() {
        let (_temp, conn) = create_test_db();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.repository_id = Some(fedora_repo_id);
        let fedora_label_id = fedora_label.insert(&conn).unwrap();

        let mut installed = Trove::new_with_source(
            "demo".to_string(),
            "0.9".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.label_id = Some(fedora_label_id);
        installed.architecture = Some("amd64".to_string());
        installed.insert(&conn).unwrap();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.default_strategy_distro = Some("ubuntu-24.04".to_string());
        let ubuntu_repo_id = ubuntu_repo.insert(&conn).unwrap();

        let mut prerelease = RepositoryPackage::new(
            ubuntu_repo_id,
            "demo".to_string(),
            "1.0~beta1".to_string(),
            "sha256:beta".to_string(),
            123,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0~beta1_amd64.deb".to_string(),
        );
        prerelease.architecture = Some("amd64".to_string());
        prerelease.insert(&conn).unwrap();

        let mut stable = RepositoryPackage::new(
            ubuntu_repo_id,
            "demo".to_string(),
            "1.0".to_string(),
            "sha256:stable".to_string(),
            123,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0_amd64.deb".to_string(),
        );
        stable.architecture = Some("amd64".to_string());
        stable.insert(&conn).unwrap();

        let snapshot = source_policy_replatform_snapshot(&conn, "ubuntu-24.04").unwrap();

        assert_eq!(snapshot.visible_realignment_candidates, 1);
        assert_eq!(snapshot.visible_realignment_proposals[0].package, "demo");
        assert_eq!(
            snapshot.visible_realignment_proposals[0].target_version,
            "1.0"
        );
    }

    #[test]
    fn test_planned_replatform_actions_use_installed_versions() {
        let snapshot = SourcePolicyReplatformSnapshot {
            target_distro: "arch".to_string(),
            estimate: Some(ReplatformEstimate {
                target_distro: "arch".to_string(),
                aligned_packages: 1,
                packages_to_realign: 1,
                total_packages: 2,
            }),
            visible_realignment_candidates: 1,
            visible_realignment_proposals: vec![VisibleRealignmentProposal {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(22),
            }],
        };
        let mut state = SystemState::new();
        state.installed.insert(
            "vim".to_string(),
            InstalledPackage {
                name: "vim".to_string(),
                version: "9.0.1".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                label: Some("fedora@f43:stable".to_string()),
            },
        );

        let actions = planned_replatform_actions(&snapshot, &state);

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            crate::model::DiffAction::ReplatformReplace {
                package,
                current_distro,
                target_distro,
                current_version,
                current_architecture,
                target_version,
                architecture,
                target_repository,
                target_repository_package_id,
            } if package == "vim"
                && current_distro.as_deref() == Some("fedora-43")
                && target_distro == "arch"
                && current_version == "9.0.1"
                && current_architecture.as_deref() == Some("x86_64")
                && target_version == "9.1.0"
                && architecture.as_deref() == Some("x86_64")
                && target_repository.as_deref() == Some("arch-core")
                && *target_repository_package_id == Some(22)
        ));
    }

    #[test]
    fn test_replatform_execution_plan_collects_replace_actions() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        arch_repo.insert(&conn).unwrap();

        let actions = vec![
            DiffAction::SetSourcePin {
                distro: "arch".to_string(),
                strength: Some("strict".to_string()),
            },
            DiffAction::ReplatformReplace {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(22),
            },
            DiffAction::ReplatformReplace {
                package: "bash".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "5.1.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "5.2.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(11),
            },
        ];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert_eq!(plan.transactions.len(), 2);
        assert_eq!(plan.transactions[0].package, "bash");
        assert_eq!(plan.transactions[1].package, "vim");
        assert_eq!(plan.transactions[0].current_version, "5.1.0");
        assert_eq!(
            plan.transactions[0].current_architecture.as_deref(),
            Some("x86_64")
        );
        assert_eq!(plan.transactions[0].target_version, "5.2.0");
        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].install_repository.as_deref(),
            Some("arch-core")
        );
        assert_eq!(plan.transactions[0].install_repository_package_id, Some(11));
        assert_eq!(
            plan.transactions[0].install_route.as_deref(),
            Some("default:legacy")
        );
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
        );
    }

    #[test]
    fn test_replatform_execution_plan_reports_block_reason_when_repo_metadata_missing() {
        let (_temp, conn) = create_test_db();
        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: None,
            target_repository_package_id: None,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::MissingRepositoryMetadata)
        );
    }

    #[test]
    fn test_replatform_execution_plan_reports_missing_versioned_install_route() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy_distro = Some("arch".to_string());
        arch_repo.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::MissingInstallRoute)
        );
    }

    #[test]
    fn test_replatform_execution_plan_reports_any_version_route_only() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/vim-latest.ccs".to_string(),
                checksum: "sha256:any-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].install_route.as_deref(),
            Some("resolution:binary")
        );
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::AnyVersionRouteOnly)
        );
    }

    #[test]
    fn test_replatform_execution_plan_marks_exact_version_resolution_executable() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("9.1.0".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].install_route.as_deref(),
            Some("resolution:binary")
        );
        assert_eq!(plan.transactions[0].blocked_reason, None);
    }

    #[test]
    fn test_replatform_execution_plan_blocks_when_target_dependencies_are_missing() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            "sha256:vim".to_string(),
            123,
            "https://example.test/arch/vim.pkg.tar.zst".to_string(),
        );
        target_pkg.architecture = Some("x86_64".to_string());
        target_pkg.dependencies =
            Some(serde_json::to_string(&vec!["libmagic >= 1.0".to_string()]).unwrap());
        target_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("9.1.0".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::UnsatisfiedTargetDependencies)
        );
        assert_eq!(
            plan.transactions[0].unresolved_dependencies,
            vec!["libmagic (>= 1.0)".to_string()]
        );
    }

    #[test]
    fn test_replatform_execution_plan_accepts_tracked_capability_provider_for_target_dependency() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            "sha256:vim".to_string(),
            123,
            "https://example.test/arch/vim.pkg.tar.zst".to_string(),
        );
        target_pkg.architecture = Some("x86_64".to_string());
        target_pkg.dependencies =
            Some(serde_json::to_string(&vec!["libmagic.so.1".to_string()]).unwrap());
        target_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("9.1.0".to_string());
        resolution.insert(&conn).unwrap();

        let mut provider_trove = Trove::new_with_source(
            "file-libs".to_string(),
            "5.45".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        provider_trove.architecture = Some("x86_64".to_string());
        let provider_trove_id = provider_trove.insert(&conn).unwrap();

        let mut provide = ProvideEntry::new(
            provider_trove_id,
            "libmagic.so.1()(64bit)".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(plan.transactions[0].blocked_reason, None);
        assert!(plan.transactions[0].unresolved_dependencies.is_empty());
    }

    #[test]
    fn test_replatform_execution_plan_accepts_repo_metadata_provider_for_target_dependency() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            arch_repo_id,
            "kernel".to_string(),
            "6.19.6-1".to_string(),
            "sha256:kernel".to_string(),
            123,
            "https://example.test/arch/kernel.pkg.tar.zst".to_string(),
        );
        target_pkg.architecture = Some("x86_64".to_string());
        target_pkg.dependencies = Some(
            serde_json::to_string(&vec![
                "kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string(),
            ])
            .unwrap(),
        );
        target_pkg.insert(&conn).unwrap();

        let mut provider_pkg = RepositoryPackage::new(
            arch_repo_id,
            "kernel-core".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:kernel-core".to_string(),
            123,
            "https://example.test/arch/kernel-core.pkg.tar.zst".to_string(),
        );
        provider_pkg.architecture = Some("x86_64".to_string());
        provider_pkg.metadata = Some(
            serde_json::json!({
                "rpm_provides": ["kernel-core-uname-r = 6.19.6-200.fc43.x86_64"]
            })
            .to_string(),
        );
        provider_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "kernel".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/kernel-6.19.6-1.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("6.19.6-1".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "kernel".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "6.19.5-1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "6.19.6-1".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(plan.transactions[0].blocked_reason, None);
        assert!(plan.transactions[0].unresolved_dependencies.is_empty());
    }

    #[test]
    fn test_replatform_execution_plan_accepts_debian_repo_metadata_provider_for_target_dependency()
    {
        let (_temp, conn) = create_test_db();
        let mut deb_repo = Repository::new(
            "ubuntu-main".to_string(),
            "https://example.test/ubuntu".to_string(),
        );
        deb_repo.default_strategy = Some("legacy".to_string());
        deb_repo.default_strategy_distro = Some("ubuntu-24.04".to_string());
        let deb_repo_id = deb_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            deb_repo_id,
            "mailer".to_string(),
            "1.0-1".to_string(),
            "sha256:mailer".to_string(),
            123,
            "https://example.test/ubuntu/mailer.deb".to_string(),
        );
        target_pkg.architecture = Some("amd64".to_string());
        target_pkg.dependencies =
            Some(serde_json::to_string(&vec!["mail-transport-agent".to_string()]).unwrap());
        target_pkg.insert(&conn).unwrap();

        let mut provider_pkg = RepositoryPackage::new(
            deb_repo_id,
            "postfix".to_string(),
            "3.8.0-1".to_string(),
            "sha256:postfix".to_string(),
            123,
            "https://example.test/ubuntu/postfix.deb".to_string(),
        );
        provider_pkg.architecture = Some("amd64".to_string());
        provider_pkg.metadata = Some(
            serde_json::json!({
                "deb_provides": ["mail-transport-agent"]
            })
            .to_string(),
        );
        provider_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            deb_repo_id,
            "mailer".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/ubuntu/mailer-1.0-1.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("1.0-1".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "mailer".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "ubuntu-24.04".to_string(),
            current_version: "0.9-1".to_string(),
            current_architecture: Some("amd64".to_string()),
            target_version: "1.0-1".to_string(),
            architecture: Some("amd64".to_string()),
            target_repository: Some("ubuntu-main".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(plan.transactions[0].blocked_reason, None);
        assert!(plan.transactions[0].unresolved_dependencies.is_empty());
    }

    #[test]
    fn test_replatform_execution_plan_accepts_debian_normalized_provider_with_version_constraint() {
        let (_temp, conn) = create_test_db();
        let mut deb_repo = Repository::new(
            "ubuntu-main".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        deb_repo.default_strategy = Some("legacy".to_string());
        deb_repo.default_strategy_distro = Some("ubuntu-24.04".to_string());
        let deb_repo_id = deb_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            deb_repo_id,
            "mailer".to_string(),
            "1.0-1".to_string(),
            "sha256:mailer".to_string(),
            123,
            "https://archive.ubuntu.com/ubuntu/pool/mailer_1.0-1_amd64.deb".to_string(),
        );
        target_pkg.architecture = Some("amd64".to_string());
        target_pkg.insert(&conn).unwrap();

        let mut requirement = RepositoryRequirement::new(
            target_pkg.id.unwrap(),
            "mail-transport-agent".to_string(),
            Some(">= 1.0~beta1".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            Some("mail-transport-agent (>= 1.0~beta1)".to_string()),
        );
        requirement.insert(&conn).unwrap();

        let mut provider_pkg = RepositoryPackage::new(
            deb_repo_id,
            "postfix".to_string(),
            "1.0-1".to_string(),
            "sha256:postfix".to_string(),
            123,
            "https://archive.ubuntu.com/ubuntu/pool/postfix_1.0-1_amd64.deb".to_string(),
        );
        provider_pkg.architecture = Some("amd64".to_string());
        provider_pkg.insert(&conn).unwrap();

        let mut provide = RepositoryProvide::new(
            provider_pkg.id.unwrap(),
            "mail-transport-agent".to_string(),
            Some("1.0".to_string()),
            "package".to_string(),
            Some("mail-transport-agent (= 1.0)".to_string()),
        );
        provide.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            deb_repo_id,
            "mailer".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://archive.ubuntu.com/ubuntu/pool/mailer_1.0-1.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("1.0-1".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "mailer".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "ubuntu-24.04".to_string(),
            current_version: "0.9-1".to_string(),
            current_architecture: Some("amd64".to_string()),
            target_version: "1.0-1".to_string(),
            architecture: Some("amd64".to_string()),
            target_repository: Some("ubuntu-main".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(plan.transactions[0].blocked_reason, None);
        assert!(plan.transactions[0].unresolved_dependencies.is_empty());
    }

    #[test]
    fn test_replatform_execution_plan_accepts_arch_repo_metadata_provider_for_target_dependency() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut target_pkg = RepositoryPackage::new(
            arch_repo_id,
            "mailer".to_string(),
            "1.0-1".to_string(),
            "sha256:mailer".to_string(),
            123,
            "https://example.test/arch/mailer.pkg.tar.zst".to_string(),
        );
        target_pkg.architecture = Some("x86_64".to_string());
        target_pkg.dependencies =
            Some(serde_json::to_string(&vec!["mail-transport-agent".to_string()]).unwrap());
        target_pkg.insert(&conn).unwrap();

        let mut provider_pkg = RepositoryPackage::new(
            arch_repo_id,
            "postfix".to_string(),
            "3.8.0-1".to_string(),
            "sha256:postfix".to_string(),
            123,
            "https://example.test/arch/postfix.pkg.tar.zst".to_string(),
        );
        provider_pkg.architecture = Some("x86_64".to_string());
        provider_pkg.metadata = Some(
            serde_json::json!({
                "arch_provides": ["mail-transport-agent"]
            })
            .to_string(),
        );
        provider_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            arch_repo_id,
            "mailer".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/mailer-1.0-1.ccs".to_string(),
                checksum: "sha256:exact-version".to_string(),
                delta_base: None,
            }],
        );
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.version = Some("1.0-1".to_string());
        resolution.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "mailer".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "0.9-1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "1.0-1".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: target_pkg.id,
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(plan.transactions[0].executable);
        assert_eq!(plan.transactions[0].blocked_reason, None);
        assert!(plan.transactions[0].unresolved_dependencies.is_empty());
    }

    #[test]
    fn test_replatform_execution_plan_reports_architecture_mismatch() {
        let (_temp, conn) = create_test_db();
        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        arch_repo.insert(&conn).unwrap();

        let actions = vec![DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("aarch64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(22),
        }];

        let plan = replatform_execution_plan(&conn, &actions)
            .expect("plan query should succeed")
            .expect("expected plan");

        assert!(!plan.transactions[0].executable);
        assert_eq!(
            plan.transactions[0].blocked_reason,
            Some(ReplatformBlockedReason::ArchitectureMismatch)
        );
    }

    #[test]
    fn test_convergence_plans_ownership_state_transition_adopted_track_to_taken() {
        use crate::model::parser::ConvergenceIntent;

        // Given: packages at various adoption states and FullOwnership convergence intent
        let convergence = ConvergenceIntent::FullOwnership;
        let target_source = convergence.target_install_source();

        // Verify the convergence target maps to "taken"
        assert_eq!(target_source, "taken");

        // Given: a package currently at AdoptedTrack
        let adopted_track = InstallSource::AdoptedTrack;
        let adopted_full = InstallSource::AdoptedFull;
        let taken = InstallSource::Taken;

        // AdoptedTrack is not at the convergence target
        assert_ne!(adopted_track.as_str(), target_source);
        // AdoptedFull is not at the convergence target either
        assert_ne!(adopted_full.as_str(), target_source);
        // Taken IS the convergence target
        assert_eq!(taken.as_str(), target_source);

        // Verify the state ordering: AdoptedTrack < AdoptedFull < Taken
        // Each convergence level maps to a progressively deeper ownership state
        assert_eq!(
            ConvergenceIntent::TrackOnly.target_install_source(),
            adopted_track.as_str()
        );
        assert_eq!(
            ConvergenceIntent::CasBacked.target_install_source(),
            adopted_full.as_str()
        );
        assert_eq!(
            ConvergenceIntent::FullOwnership.target_install_source(),
            taken.as_str()
        );

        // AdoptedTrack is adopted (not yet converged)
        assert!(adopted_track.is_adopted());
        // AdoptedFull is adopted (not yet converged for FullOwnership)
        assert!(adopted_full.is_adopted());
        // Taken is Conary-owned (fully converged)
        assert!(!taken.is_adopted());
        assert!(taken.is_conary_owned());
    }
}
