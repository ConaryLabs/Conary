// src/commands/update/selection.rs

//! Exact-source update candidate selection and security metadata checks.

use super::super::{InstalledPackageSelector, resolve_installed_package};
use anyhow::Result;
use conary_core::db::models::{Repository, RepositoryPackage, SecurityAdvisorySupport, Trove};
use conary_core::repository::{
    PackageSelector, SelectionOptions,
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
        architecture: trove.architecture.clone(),
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
                    .is_supported()
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
            Some("trusted") => format!("trusted source: {source}"),
            Some(trust) if !trust.is_empty() => format!("{trust} source: {source}"),
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
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{
        CanonicalPackage, InstallSource, RepologyCacheEntry, Repository, RepositoryPackage,
        SecurityAdvisorySupport, Trove, TroveType,
    };
    use conary_core::repository::resolution_policy::{DependencyMixingPolicy, ResolutionPolicy};
    use conary_core::repository::versioning::VersionScheme;

    fn seed_cross_source_update_fixture(conn: &rusqlite::Connection) -> Trove {
        let mut fedora_repo = Repository::new(
            "fedora-main".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.priority = 50;
        fedora_repo.source_profile = Some("fedora-44".to_string());
        let fedora_repo_id = fedora_repo.insert(conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.priority = 10;
        arch_repo.source_profile = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(conn).unwrap();

        let mut canonical = CanonicalPackage::new("demo".to_string(), "package".to_string());
        let canonical_id = canonical.insert(conn).unwrap();
        let fresh = chrono::Utc::now().to_rfc3339();

        RepologyCacheEntry::insert_or_replace(
            conn,
            &RepologyCacheEntry {
                project_name: "demo".to_string(),
                distro: "fedora-44".to_string(),
                distro_name: "demo".to_string(),
                version: Some("1.1.0-1.fc44".to_string()),
                status: Some("outdated".to_string()),
                fetched_at: fresh.clone(),
            },
        )
        .unwrap();
        RepologyCacheEntry::insert_or_replace(
            conn,
            &RepologyCacheEntry {
                project_name: "demo".to_string(),
                distro: "arch".to_string(),
                distro_name: "demo".to_string(),
                version: Some("1.2.0-1".to_string()),
                status: Some("newest".to_string()),
                fetched_at: fresh,
            },
        )
        .unwrap();

        let mut installed = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0-1.fc44".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_profile = Some("fedora-44".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(conn).unwrap();

        let mut fedora_candidate = RepositoryPackage::new(
            fedora_repo_id,
            "demo".to_string(),
            "1.1.0-1.fc44".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:fedora-demo".to_string(),
            123,
            "https://example.test/fedora/demo-1.1.0-1.fc44.rpm".to_string(),
        );
        fedora_candidate.architecture = Some("x86_64".to_string());
        fedora_candidate.source_profile = Some("fedora-44".to_string());
        fedora_candidate.canonical_id = Some(canonical_id);
        fedora_candidate.insert(conn).unwrap();

        let mut arch_candidate = RepositoryPackage::new(
            arch_repo_id,
            "demo".to_string(),
            "1.2.0-1".to_string(),
            conary_core::repository::versioning::VersionScheme::Arch,
            "sha256:arch-demo".to_string(),
            123,
            "https://example.test/arch/demo-1.2.0-1.pkg.tar.zst".to_string(),
        );
        arch_candidate.architecture = Some("x86_64".to_string());
        arch_candidate.source_profile = Some("arch".to_string());
        arch_candidate.canonical_id = Some(canonical_id);
        arch_candidate.insert(conn).unwrap();

        installed
    }

    fn seed_security_update_fixture(
        conn: &rusqlite::Connection,
        support: SecurityAdvisorySupport,
        candidate_is_security_update: bool,
    ) -> Trove {
        let mut repo = Repository::new(
            "security-repo".to_string(),
            "https://example.test/security".to_string(),
        );
        repo.source_profile = Some("fedora-44".to_string());
        repo.security_advisory_support = support;
        let repo_id = repo.insert(conn).unwrap();

        let mut installed = Trove::new_with_source(
            "openssl".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_profile = Some("fedora-44".to_string());
        installed.installed_from_repository_id = Some(repo_id);
        installed.insert(conn).unwrap();

        let mut candidate = RepositoryPackage::new(
            repo_id,
            "openssl".to_string(),
            "1.0.1".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:openssl".to_string(),
            123,
            "https://example.test/security/openssl-1.0.1.ccs".to_string(),
        );
        candidate.architecture = Some("x86_64".to_string());
        candidate.source_profile = Some("fedora-44".to_string());
        candidate.is_security_update = candidate_is_security_update;
        if candidate_is_security_update {
            candidate.severity = Some("important".to_string());
            candidate.advisory_id = Some("FEDORA-2026-0001".to_string());
        }
        candidate.insert(conn).unwrap();

        installed
    }

    #[test]
    fn test_is_repo_version_newer_uses_debian_scheme() {
        let trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0~beta1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Debian,
        );

        let candidate = RepositoryPackage::new(
            1,
            "demo".to_string(),
            "1.0".to_string(),
            conary_core::repository::versioning::VersionScheme::Debian,
            "sha256:demo".to_string(),
            1,
            "https://deb.example.test/demo_1.0_amd64.deb".to_string(),
        );

        assert!(is_repo_version_newer(&trove, &candidate).unwrap());
    }

    #[test]
    fn test_is_repo_version_newer_uses_arch_scheme() {
        let trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Arch,
        );

        let candidate = RepositoryPackage::new(
            1,
            "demo".to_string(),
            "1.0-2".to_string(),
            conary_core::repository::versioning::VersionScheme::Arch,
            "sha256:demo".to_string(),
            1,
            "https://arch.example.test/demo-1.0-2.pkg.tar.zst".to_string(),
        );

        assert!(is_repo_version_newer(&trove, &candidate).unwrap());
    }

    #[test]
    fn selects_debian_update_from_generic_metadata_driven_repo() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let mut repo = Repository::new(
            "slice-d-local-update".to_string(),
            "http://127.0.0.1:18087".to_string(),
        );
        repo.priority = 500;
        repo.source_profile = Some("ubuntu-26.04".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut package = RepositoryPackage::new(
            repo_id,
            "phase4-runtime-fixture".to_string(),
            "1.0.1".to_string(),
            conary_core::repository::versioning::VersionScheme::Debian,
            "sha256:fixture".to_string(),
            1110,
            "http://127.0.0.1:18087/phase4-runtime-fixture_1.0.1_amd64.deb".to_string(),
        );
        package.architecture = Some("amd64".to_string());
        package.source_profile = Some("ubuntu-26.04".to_string());
        package.insert(&conn).unwrap();

        let mut installed = Trove::new(
            "phase4-runtime-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Debian,
        );
        installed.architecture = Some("amd64".to_string());
        installed.source_profile = Some("ubuntu-26.04".to_string());
        installed.installed_from_repository_id = Some(repo_id);

        let selected = select_update_candidate(
            &conn,
            &installed,
            false,
            &ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict),
        )
        .unwrap()
        .expect("expected generic metadata-driven Debian update");

        assert_eq!(selected.package.version, "1.0.1");
        assert_eq!(selected.repository.name, "slice-d-local-update");
        assert_eq!(selected.package.version_scheme, VersionScheme::Debian);
    }

    #[test]
    fn repology_latest_signal_cannot_switch_update_source() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_cross_source_update_fixture(&conn);
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);

        let selected = select_update_candidate(&conn, &trove, false, &policy)
            .unwrap()
            .expect("expected update candidate");

        assert_eq!(selected.repository.name, "fedora-main");
        assert_eq!(selected.package.version, "1.1.0-1.fc44");
    }

    #[test]
    fn security_update_refuses_unknown_source_metadata_before_mutation() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Unknown, false);
        let policy = ResolutionPolicy::new();

        let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

        assert!(matches!(
            result,
            UpdateCandidateSelection::SecurityMetadataUnavailable(_)
        ));
    }

    #[test]
    fn security_update_refuses_unsupported_source_metadata_before_mutation() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove =
            seed_security_update_fixture(&conn, SecurityAdvisorySupport::Unsupported, false);
        let policy = ResolutionPolicy::new();

        let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

        assert!(matches!(
            result,
            UpdateCandidateSelection::SecurityMetadataUnavailable(_)
        ));
    }

    #[test]
    fn security_update_selects_supported_security_candidate() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Supported, true);
        let policy = ResolutionPolicy::new();

        let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

        assert!(matches!(result, UpdateCandidateSelection::Selected(_)));
    }

    #[test]
    fn security_update_marker_includes_trusted_advisory_details() {
        let mut package = RepositoryPackage::new(
            7,
            "openssl".to_string(),
            "3.2.1-1.fc44".to_string(),
            VersionScheme::Rpm,
            "sha256:openssl-fixed".to_string(),
            4096,
            "https://example.test/openssl-3.2.1-1.fc44.ccs".to_string(),
        );
        package.is_security_update = true;
        package.severity = Some("critical".to_string());
        package.cve_ids = Some("CVE-2026-0001,CVE-2026-0002".to_string());
        package.advisory_id = Some("FEDORA-2026-0001".to_string());
        package.metadata = Some(
            serde_json::json!({
                "security_advisory": {
                    "source": "conary-json",
                    "source_trust": "trusted",
                    "fixed_version": "3.2.1-1.fc44"
                }
            })
            .to_string(),
        );

        let marker = render_security_update_marker(&package);

        assert!(marker.contains("critical"), "{marker}");
        assert!(marker.contains("FEDORA-2026-0001"), "{marker}");
        assert!(marker.contains("CVE-2026-0001,CVE-2026-0002"), "{marker}");
        assert!(marker.contains("fixed: 3.2.1-1.fc44"), "{marker}");
        assert!(marker.contains("trusted source: conary-json"), "{marker}");
    }

    #[test]
    fn security_update_ignores_supported_non_security_candidate() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Supported, false);
        let policy = ResolutionPolicy::new();

        let result = select_update_candidate(&conn, &trove, true, &policy).unwrap();

        assert!(matches!(result, UpdateCandidateSelection::NoEligibleUpdate));
    }

    #[test]
    fn strict_mixing_update_stays_on_current_source() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_cross_source_update_fixture(&conn);
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict);

        let selected = select_update_candidate(&conn, &trove, false, &policy)
            .unwrap()
            .expect("expected strict-mixing update candidate");

        assert_eq!(selected.repository.name, "fedora-main");
        assert_eq!(selected.package.version, "1.1.0-1.fc44");
    }
}
