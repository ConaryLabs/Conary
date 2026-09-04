// apps/conary/src/commands/update/selection.rs

//! Exact-source update candidate selection and security metadata checks.

use super::super::{InstalledPackageSelector, resolve_installed_package};
use anyhow::Result;
use conary_core::db::models::{Repository, RepositoryPackage, SecurityAdvisorySupport, Trove};
use conary_core::repository::{
    PackageArchitectureVariant, PackageSelector, SelectionOptions,
    resolution_policy::ResolutionPolicy,
    versioning::{compare_package_identities, resolve_package_version_scheme},
};
use std::cmp::Ordering;
use tracing::debug;

/// Check whether the repository version is strictly newer than the installed version.
///
/// Returns `true` only when the repository version is strictly newer.
/// Mixed schemes and malformed versions are typed errors.
fn is_repo_version_newer(trove: &Trove, package: &RepositoryPackage) -> Result<bool> {
    let installed_scheme = trove.version_scheme;
    let repository_scheme = resolve_package_version_scheme(package);
    let ordering = compare_package_identities(
        installed_scheme,
        &trove.version,
        trove.package_release.as_deref(),
        repository_scheme,
        &package.version,
        (!package.package_release.is_empty()).then_some(package.package_release.as_str()),
    )?;

    if ordering != Ordering::Less {
        debug!(
            "Skipping {} {} (installed {} is same or newer)",
            trove.name, package.version, trove.version
        );
        return Ok(false);
    }

    Ok(true)
}

#[derive(Debug, Clone)]
pub(super) struct SelectedUpdateCandidate {
    pub(super) package: RepositoryPackage,
    pub(super) repository: Repository,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SecurityMetadataUnavailable {
    package: String,
    repository: String,
    support: SecurityAdvisorySupport,
    candidate_version: String,
}

#[derive(Debug, Clone)]
pub(super) enum UpdateCandidateSelection {
    Selected(Box<SelectedUpdateCandidate>),
    NoEligibleUpdate,
    SecurityMetadataUnavailable(SecurityMetadataUnavailable),
}

impl UpdateCandidateSelection {
    #[cfg(test)]
    fn expect(self, message: &str) -> SelectedUpdateCandidate {
        match self {
            Self::Selected(selected) => *selected,
            Self::NoEligibleUpdate | Self::SecurityMetadataUnavailable(_) => panic!("{message}"),
        }
    }
}

fn persisted_source_profile(trove: &Trove) -> Option<&str> {
    trove.source_profile.as_deref()
}

fn effective_installed_source_identity(
    conn: &rusqlite::Connection,
    trove: &Trove,
) -> Result<Option<String>> {
    if let Some(repository_id) = trove.installed_from_repository_id
        && let Some(repository) = Repository::find_by_id(conn, repository_id)?
    {
        return Ok(repository
            .resolution_source_identity()?
            .map(str::to_string)
            .or_else(|| trove.source_profile.clone()));
    }
    Ok(trove.source_profile.clone())
}

fn candidate_matches_installed_source(
    conn: &rusqlite::Connection,
    trove: &Trove,
    package: &RepositoryPackage,
    repository: &Repository,
) -> Result<bool> {
    let candidate_identity =
        conary_core::repository::selector::candidate_source_identity(package, repository)?;
    if trove
        .installed_from_repository_id
        .zip(repository.id)
        .is_some_and(|(installed_repo_id, candidate_repo_id)| {
            installed_repo_id == candidate_repo_id
        })
    {
        return Ok(true);
    }

    let installed_identity = effective_installed_source_identity(conn, trove)?;
    Ok(matches!(
        (installed_identity.as_deref().or_else(|| persisted_source_profile(trove)), candidate_identity),
        (Some(installed), Some(candidate)) if installed == candidate
    ))
}

/// Select a newer package from the exact installed source.
///
/// Ordinary updates never infer a distro/source migration. Replatforming is a
/// separate explicit operation with its own preview and confirmation.
pub(super) fn select_update_candidate(
    conn: &rusqlite::Connection,
    trove: &Trove,
    security_only: bool,
    policy: &ResolutionPolicy,
) -> Result<UpdateCandidateSelection> {
    let mut transaction_policy = policy.clone();
    transaction_policy
        .set_primary_source_identity(effective_installed_source_identity(conn, trove)?);
    let options = SelectionOptions {
        version: None,
        package_release: None,
        repository: None,
        variant: trove.architecture.as_deref().map(|architecture| {
            PackageArchitectureVariant::from_package(trove.version_scheme, architecture)
        }),
        host_assertion: None,
        architecture_scope: conary_core::repository::selector::ArchitectureScope::Native,
        policy: Some(transaction_policy),
        is_root: false,
    };

    let mut eligible = Vec::new();
    for candidate in PackageSelector::search_packages(conn, &trove.name, &options)? {
        if !candidate_matches_installed_source(
            conn,
            trove,
            &candidate.package,
            &candidate.repository,
        )? {
            continue;
        }
        if is_repo_version_newer(trove, &candidate.package)? {
            if security_only {
                if !candidate
                    .repository
                    .security_advisory_support
                    .authorizes_security_advisories()
                {
                    return Ok(UpdateCandidateSelection::SecurityMetadataUnavailable(
                        SecurityMetadataUnavailable {
                            package: trove.name.clone(),
                            repository: candidate.repository.name,
                            support: candidate.repository.security_advisory_support,
                            candidate_version: candidate.package.version,
                        },
                    ));
                }
                if !candidate.package.is_security_update {
                    continue;
                }
            }
            eligible.push(candidate);
        }
    }

    if eligible.is_empty() {
        return Ok(UpdateCandidateSelection::NoEligibleUpdate);
    }

    let selected = PackageSelector::select_best_with_options(conn, eligible, &options)?;

    Ok(UpdateCandidateSelection::Selected(Box::new(
        SelectedUpdateCandidate {
            package: selected.package,
            repository: selected.repository,
        },
    )))
}

pub(super) fn render_security_update_marker(package: &RepositoryPackage) -> String {
    if !package.is_security_update {
        return String::new();
    }

    let mut parts = Vec::new();
    parts.push(
        package
            .severity
            .as_deref()
            .unwrap_or("security")
            .to_string(),
    );

    if let Some(advisory_id) = package
        .advisory_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(advisory_id.to_string());
    }

    if let Some(cves) = package
        .cve_ids
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(cves.to_string());
    }

    if let Some(fixed_version) = security_advisory_metadata_text(package, "fixed_version") {
        parts.push(format!("fixed: {fixed_version}"));
    }

    if let Some(source) = security_advisory_metadata_text(package, "source") {
        let source_label = match security_advisory_metadata_text(package, "source_trust")
            .as_deref()
            .map(str::trim)
        {
            Some(claim) if !claim.is_empty() => {
                format!("source: {source} (feed trust claim: {claim})")
            }
            _ => format!("source: {source}"),
        };
        parts.push(source_label);
    }

    format!(" [{}]", parts.join("; "))
}

fn security_advisory_metadata_text(package: &RepositoryPackage, key: &str) -> Option<String> {
    let metadata = package.metadata.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(metadata).ok()?;
    value
        .get("security_advisory")?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn print_security_metadata_unavailable(unavailable: &[SecurityMetadataUnavailable]) {
    if unavailable.is_empty() {
        return;
    }

    println!("Security metadata unavailable for requested update source(s):");
    for item in unavailable {
        println!(
            "  {} {} from {} ({})",
            item.package,
            item.candidate_version,
            item.repository,
            item.support.as_str()
        );
    }
}

pub(super) fn security_metadata_unavailable_error(count: usize) -> String {
    format!(
        "Cannot run security-only update because {count} source(s) cannot prove security metadata support. Mark the source supported only after its repository metadata publishes advisory data."
    )
}

pub(super) fn installed_troves_for_update(
    conn: &rusqlite::Connection,
    package: Option<String>,
    package_version: Option<String>,
    architecture: Option<String>,
) -> Result<Vec<Trove>> {
    if let Some(pkg_name) = package {
        let selector = InstalledPackageSelector::new(pkg_name, package_version, architecture);
        return Ok(vec![resolve_installed_package(conn, &selector)?.trove]);
    }

    if package_version.is_some() || architecture.is_some() {
        anyhow::bail!("A package name is required with --version or --arch for update");
    }

    Ok(Trove::list_all(conn)?)
}

#[cfg(test)]
#[path = "selection/tests.rs"]
mod tests;
