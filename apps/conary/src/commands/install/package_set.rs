// apps/conary/src/commands/install/package_set.rs

//! One atomic repository transaction for a declarative package request set.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use conary_core::db::models::InstallReason;
use conary_core::repository::resolution_policy::{
    DependencyMixingPolicy, RequestScope, ResolutionPolicy,
};
use conary_core::repository::selector::{
    PackageSelector, SelectionOptions, candidate_source_identity,
};
use conary_core::repository::versioning::resolve_package_version_scheme;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;

use super::InstallIntent;
use super::batch::BatchInstaller;
use super::dependencies::resolved_repository_deps_from_sat_result;
use super::repository_batch::{RepositoryBatchSelection, prepare_repository_batch};
use super::source_policy::resolve_canonical_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageSetRequest {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) selection_reason: String,
    pub(crate) allow_downgrade: bool,
}

/// Resolve, authenticate, prepare, and execute one complete package request set.
pub(crate) async fn install_package_set(
    db_path: &str,
    sandbox_mode: SandboxMode,
    requests: Vec<PackageSetRequest>,
) -> Result<usize> {
    process_package_set(
        db_path,
        requests,
        PackageSetOperation::Install(sandbox_mode),
    )
    .await
}

pub(crate) async fn validate_package_set(
    db_path: &str,
    requests: Vec<PackageSetRequest>,
) -> Result<usize> {
    process_package_set(db_path, requests, PackageSetOperation::Validate).await
}

enum PackageSetOperation {
    Validate,
    Install(SandboxMode),
}

async fn process_package_set(
    db_path: &str,
    requests: Vec<PackageSetRequest>,
    operation: PackageSetOperation,
) -> Result<usize> {
    if requests.is_empty() {
        return Ok(0);
    }
    crate::commands::hint_default_convergence();

    let conn = super::super::open_db(db_path)?;
    let policy =
        conary_core::repository::load_effective_policy(&conn, RequestScope::Any)?.resolution;
    let mut resolved_requests = BTreeMap::new();
    for request in requests {
        let name = resolve_canonical_name(&conn, &request.name, None, &policy)?
            .unwrap_or_else(|| request.name.clone());
        if resolved_requests.insert(name.clone(), request).is_some() {
            anyhow::bail!("model package set resolves more than one root request to {name}");
        }
    }
    let solver_requests = resolved_requests
        .iter()
        .map(|(name, request)| {
            let constraint = request
                .version
                .as_deref()
                .map(VersionConstraint::parse)
                .transpose()
                .with_context(|| format!("invalid model version for package {name}"))?
                .unwrap_or(VersionConstraint::Any);
            Ok((name.clone(), constraint))
        })
        .collect::<Result<Vec<_>>>()?;
    let policy = bind_package_set_source_identity(&conn, policy, &solver_requests)?;
    let solved = conary_core::resolver::solve_install_with_policy(&conn, &solver_requests, &policy)
        .context("failed to solve the complete model package set")?;
    if let Some(conflict) = solved.conflict_message.as_deref() {
        anyhow::bail!("cannot apply model package set: {conflict}");
    }
    for name in resolved_requests.keys() {
        if !solved
            .install_order
            .iter()
            .any(|package| package.name == *name)
        {
            anyhow::bail!("model root package {name} is absent from the exact SAT selection");
        }
    }
    let selected = resolved_repository_deps_from_sat_result(&solved, "model package set");
    if selected.is_empty() {
        return Ok(0);
    }
    let selections = selected
        .into_iter()
        .map(|selected| {
            if let Some(root) = resolved_requests.get(&selected.package.name) {
                RepositoryBatchSelection {
                    selected,
                    install_reason: InstallReason::Explicit,
                    selection_reason: root.selection_reason.clone(),
                    allow_downgrade: root.allow_downgrade,
                    intent: InstallIntent::PackageChange,
                }
            } else {
                RepositoryBatchSelection {
                    selected,
                    install_reason: InstallReason::Dependency,
                    selection_reason: "Required by model package set".to_string(),
                    allow_downgrade: false,
                    intent: InstallIntent::PackageChange,
                }
            }
        })
        .collect();
    let prepared = prepare_repository_batch(db_path, selections).await?;
    let root_count = resolved_requests.len();
    match operation {
        PackageSetOperation::Validate => {
            println!(
                "Validating model package set ({} explicit roots)...",
                root_count
            );
            prepared.validate(BatchInstaller::new(db_path, SandboxMode::Always))?;
        }
        PackageSetOperation::Install(sandbox_mode) => {
            println!(
                "Installing model package set ({} explicit roots)...",
                root_count
            );
            prepared.install(BatchInstaller::new(db_path, sandbox_mode))?;
        }
    }
    Ok(root_count)
}

/// Establish the strict transaction source identity shared by every typed root.
///
/// A single-package install selects its repository-backed root before solving
/// dependencies, and that exact provenance binds the transaction source. A
/// package set has no privileged root, so the only implicit authority is the
/// one exact source identity common to every root's version-compatible candidates.
/// Ambiguous or disjoint identity sets require an explicit source scope instead
/// of deriving authority from model order. SAT remains the sole owner of the
/// exact package-set selection.
fn bind_package_set_source_identity(
    conn: &rusqlite::Connection,
    mut policy: ResolutionPolicy,
    requests: &[(String, VersionConstraint)],
) -> Result<ResolutionPolicy> {
    if policy.mixing != DependencyMixingPolicy::Strict
        || policy.primary_source_identity().is_some()
        || !matches!(policy.request_scope, RequestScope::Any)
    {
        return Ok(policy);
    }

    let mut common_identities: Option<BTreeSet<String>> = None;
    for (name, constraint) in requests {
        let options = SelectionOptions {
            policy: Some(policy.clone()),
            is_root: true,
            ..SelectionOptions::default()
        };
        let candidates = PackageSelector::search_packages(conn, name, &options)?;
        let mut candidate_identities = BTreeSet::new();
        for candidate in candidates {
            let scheme = resolve_package_version_scheme(&candidate.package);
            if super::dep_resolution::version_satisfies_constraint(
                scheme,
                &candidate.package.version,
                constraint,
            )? && let Some(source_identity) =
                candidate_source_identity(&candidate.package, &candidate.repository)?
            {
                candidate_identities.insert(source_identity.to_string());
            }
        }
        if candidate_identities.is_empty() {
            continue;
        }

        common_identities = Some(match common_identities.take() {
            Some(current) => current
                .intersection(&candidate_identities)
                .cloned()
                .collect(),
            None => candidate_identities,
        });
        if common_identities.as_ref().is_some_and(BTreeSet::is_empty) {
            anyhow::bail!(
                "model package set has no exact source identity common to every repository-backed root; select one source identity explicitly"
            );
        }
    }

    let Some(common_identities) = common_identities else {
        return Ok(policy);
    };
    if common_identities.len() != 1 {
        anyhow::bail!(
            "model package set has ambiguous source identities [{}]; select one source identity explicitly",
            common_identities.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    policy.set_primary_source_identity(common_identities.into_iter().next());
    policy
        .validate_source_identities()
        .map_err(anyhow::Error::msg)?;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Repository, RepositoryPackage};
    use conary_core::db::schema;
    use conary_core::repository::versioning::VersionScheme;

    fn test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        conn
    }

    fn add_candidate(
        conn: &rusqlite::Connection,
        repository_name: &str,
        profile: &str,
        priority: i32,
        package_name: &str,
        version: &str,
    ) {
        let mut repository = Repository::new(
            repository_name.to_string(),
            format!("https://{repository_name}.example.invalid"),
        );
        repository.priority = priority;
        repository.source_profile = Some(profile.to_string());
        let repository_id = Repository::find_by_name(conn, repository_name)
            .unwrap()
            .map_or_else(
                || repository.insert(conn).unwrap(),
                |repository| repository.id.unwrap(),
            );

        let mut package = RepositoryPackage::new(
            repository_id,
            package_name.to_string(),
            version.to_string(),
            VersionScheme::Rpm,
            format!("sha256:{repository_name}-{version}"),
            1,
            format!("https://{repository_name}.example.invalid/{package_name}-{version}.rpm"),
        );
        package.architecture = Some("noarch".to_string());
        package.source_profile = Some(profile.to_string());
        package.insert(conn).unwrap();
    }

    #[test]
    fn package_set_binds_strict_policy_from_matching_root_candidate() {
        let conn = test_db();
        add_candidate(&conn, "ubuntu", "ubuntu-26.04", 100, "demo", "2");
        add_candidate(&conn, "fedora", "fedora-44", 10, "demo", "1");

        let policy = bind_package_set_source_identity(
            &conn,
            ResolutionPolicy::new(),
            &[("demo".to_string(), VersionConstraint::parse("= 1").unwrap())],
        )
        .unwrap();

        assert_eq!(policy.primary_source_identity(), Some("fedora-44"));
    }

    #[test]
    fn package_set_binds_only_source_identity_common_to_every_root() {
        let conn = test_db();
        add_candidate(&conn, "fedora", "fedora-44", 10, "alpha", "1");
        add_candidate(&conn, "ubuntu", "ubuntu-26.04", 10, "alpha", "1");
        add_candidate(&conn, "fedora", "fedora-44", 10, "beta", "1");

        let policy = bind_package_set_source_identity(
            &conn,
            ResolutionPolicy::new(),
            &[
                ("alpha".to_string(), VersionConstraint::Any),
                ("beta".to_string(), VersionConstraint::Any),
            ],
        )
        .unwrap();

        assert_eq!(policy.primary_source_identity(), Some("fedora-44"));
    }

    #[test]
    fn package_set_rejects_disjoint_root_source_identities() {
        let conn = test_db();
        add_candidate(&conn, "fedora", "fedora-44", 10, "alpha", "1");
        add_candidate(&conn, "ubuntu", "ubuntu-26.04", 10, "beta", "1");

        let error = bind_package_set_source_identity(
            &conn,
            ResolutionPolicy::new(),
            &[
                ("alpha".to_string(), VersionConstraint::Any),
                ("beta".to_string(), VersionConstraint::Any),
            ],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("no exact source identity common")
        );
    }

    #[test]
    fn package_set_rejects_ambiguous_common_source_identities() {
        let conn = test_db();
        for package in ["alpha", "beta"] {
            add_candidate(&conn, "fedora", "fedora-44", 10, package, "1");
            add_candidate(&conn, "ubuntu", "ubuntu-26.04", 10, package, "1");
        }

        let error = bind_package_set_source_identity(
            &conn,
            ResolutionPolicy::new(),
            &[
                ("alpha".to_string(), VersionConstraint::Any),
                ("beta".to_string(), VersionConstraint::Any),
            ],
        )
        .unwrap_err();

        assert!(error.to_string().contains("ambiguous source identities"));
    }

    #[test]
    fn package_set_preserves_an_existing_transaction_source_identity() {
        let conn = test_db();
        let policy = bind_package_set_source_identity(
            &conn,
            ResolutionPolicy::new().with_primary_source_identity("fedora-44"),
            &[("not-in-any-repository".to_string(), VersionConstraint::Any)],
        )
        .unwrap();

        assert_eq!(policy.primary_source_identity(), Some("fedora-44"));
    }
}
