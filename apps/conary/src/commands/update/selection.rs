// src/commands/update/selection.rs

//! Update candidate selection, source-switch previewing, and security metadata checks.

use anyhow::Result;
use chrono::Utc;
use conary_core::db::models::{
    RepologyCacheEntry, Repository, RepositoryPackage, SecurityAdvisorySupport, Trove,
};
use conary_core::repository::{
    LatestSignal, PackageSelector, SelectionOptions,
    dependency_model::RepositoryDependencyFlavor,
    resolution_policy::{ResolutionPolicy, SelectionMode},
    versioning::{VersionScheme, compare_mixed_repo_versions, resolve_package_version_scheme},
};
use std::cmp::Ordering;
use tracing::{debug, warn};

/// Check whether the repository version is strictly newer than the installed version.
///
/// Returns `true` if `repo_version` parses and compares greater than `installed_version`.
/// Returns `false` (and logs a warning) when either version fails to parse or when the
/// repository version is the same or older.
fn is_repo_version_newer(trove: &Trove, repo: &Repository, package: &RepositoryPackage) -> bool {
    let Some(ordering) = compare_mixed_repo_versions(
        trove_version_scheme(trove),
        &trove.version,
        resolve_package_version_scheme(package, repo).unwrap_or(VersionScheme::Rpm),
        &package.version,
    ) else {
        warn!(
            "Could not compare versions for {}: {} vs {}, skipping",
            trove.name, package.version, trove.version
        );
        return false;
    };

    if ordering != Ordering::Less {
        debug!(
            "Skipping {} {} (installed {} is same or newer)",
            trove.name, package.version, trove.version
        );
        return false;
    }

    true
}

fn trove_version_scheme(trove: &Trove) -> VersionScheme {
    match trove.version_scheme.as_deref() {
        Some("debian") => VersionScheme::Debian,
        Some("arch") => VersionScheme::Arch,
        Some("rpm") | None => VersionScheme::Rpm,
        Some(other) => {
            warn!(
                "Unknown installed version scheme '{}' for {}, falling back to RPM",
                other, trove.name
            );
            VersionScheme::Rpm
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateSourceSwitch {
    from_distro: String,
    to_distro: String,
    reason: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct SelectedUpdateCandidate {
    pub(super) package: RepositoryPackage,
    pub(super) repository: Repository,
    source_switch: Option<UpdateSourceSwitch>,
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
fn installed_source_distro(trove: &Trove) -> Option<&str> {
    trove.source_distro.as_deref()
}

#[allow(dead_code)]
fn candidate_source_distro<'a>(
    package: &'a RepositoryPackage,
    repository: &'a Repository,
) -> Option<&'a str> {
    package
        .distro
        .as_deref()
        .or(repository.default_strategy_distro.as_deref())
}

fn candidate_matches_installed_source(
    trove: &Trove,
    package: &RepositoryPackage,
    repository: &Repository,
) -> bool {
    if trove
        .installed_from_repository_id
        .zip(repository.id)
        .is_some_and(|(installed_repo_id, candidate_repo_id)| {
            installed_repo_id == candidate_repo_id
        })
    {
        return true;
    }

    matches!(
        (installed_source_distro(trove), candidate_source_distro(package, repository)),
        (Some(installed), Some(candidate)) if installed == candidate
    )
}

fn candidate_has_positive_latest_signal(
    conn: &rusqlite::Connection,
    package: &RepositoryPackage,
    repository: &Repository,
) -> Result<bool> {
    let Some(canonical_id) = package.canonical_id else {
        return Ok(false);
    };
    let Some(distro) = candidate_source_distro(package, repository).map(ToOwned::to_owned) else {
        return Ok(false);
    };

    let rows = RepologyCacheEntry::find_for_canonical_and_distros(conn, canonical_id, &[distro])?;
    let now = Utc::now();
    for row in rows {
        let signal = LatestSignal::from_repology(
            row.status.as_deref().unwrap_or_default(),
            row.version.as_deref(),
            &row.fetched_at,
            now,
        )?;
        if signal.is_positive() {
            return Ok(true);
        }
    }

    Ok(false)
}

fn source_switch_reason() -> String {
    "selection_mode=latest prefers the allowed source with a positive newest Repology signal"
        .to_string()
}

// In latest mode, updates re-evaluate allowed sources rather than staying
// pinned to the currently installed repository when a newer allowed source exists.
// Source switches must be previewed and confirmed unless --yes is supplied.
pub(super) fn select_update_candidate(
    conn: &rusqlite::Connection,
    trove: &Trove,
    security_only: bool,
    policy: &ResolutionPolicy,
    primary_flavor: Option<RepositoryDependencyFlavor>,
) -> Result<UpdateCandidateSelection> {
    let options = SelectionOptions {
        version: None,
        repository: None,
        architecture: trove.architecture.clone(),
        policy: Some(policy.clone()),
        is_root: false,
        primary_flavor,
    };

    let mut eligible = Vec::new();
    for candidate in PackageSelector::search_packages(conn, &trove.name, &options)? {
        let same_source =
            candidate_matches_installed_source(trove, &candidate.package, &candidate.repository);
        let newer_in_scheme =
            is_repo_version_newer(trove, &candidate.repository, &candidate.package);
        let allow_cross_source_latest = policy.selection_mode == SelectionMode::Latest
            && !same_source
            && candidate_has_positive_latest_signal(
                conn,
                &candidate.package,
                &candidate.repository,
            )?;

        if newer_in_scheme || allow_cross_source_latest {
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

    let selected = if policy.selection_mode == SelectionMode::Latest {
        PackageSelector::select_best_with_options(conn, eligible, &options)?
    } else {
        let (same_source, other_sources): (Vec<_>, Vec<_>) =
            eligible.into_iter().partition(|candidate| {
                candidate_matches_installed_source(trove, &candidate.package, &candidate.repository)
            });

        if !same_source.is_empty() {
            PackageSelector::select_best_with_options(conn, same_source, &options)?
        } else {
            PackageSelector::select_best_with_options(conn, other_sources, &options)?
        }
    };

    let source_switch =
        if candidate_matches_installed_source(trove, &selected.package, &selected.repository) {
            None
        } else {
            Some(UpdateSourceSwitch {
                from_distro: installed_source_distro(trove)
                    .unwrap_or("current-source")
                    .to_string(),
                to_distro: candidate_source_distro(&selected.package, &selected.repository)
                    .unwrap_or(selected.repository.name.as_str())
                    .to_string(),
                reason: source_switch_reason(),
            })
        };

    Ok(UpdateCandidateSelection::Selected(Box::new(
        SelectedUpdateCandidate {
            package: selected.package,
            repository: selected.repository,
            source_switch,
        },
    )))
}

fn render_source_switch_preview_line(selection: &SelectedUpdateCandidate) -> Option<String> {
    selection.source_switch.as_ref().map(|source_switch| {
        format!(
            "{}: {} -> {} ({})",
            selection.package.name,
            source_switch.from_distro,
            source_switch.to_distro,
            source_switch.reason
        )
    })
}

pub(super) fn requires_source_switch_confirmation(
    updates: &[SelectedUpdateCandidate],
    yes: bool,
) -> bool {
    !yes && updates.iter().any(|update| update.source_switch.is_some())
}

fn render_source_switch_preview_lines(updates: &[(Trove, SelectedUpdateCandidate)]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|(_, selection)| render_source_switch_preview_line(selection))
        .collect()
}

pub(super) fn print_source_switch_preview(updates: &[(Trove, SelectedUpdateCandidate)]) {
    let preview_lines = render_source_switch_preview_lines(updates);
    if preview_lines.is_empty() {
        return;
    }

    println!("\nSource switches proposed:");
    for line in preview_lines {
        println!("  {}", line);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{
        CanonicalPackage, InstallSource, RepologyCacheEntry, Repository, RepositoryPackage,
        SecurityAdvisorySupport, Trove, TroveType,
    };
    use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
    use conary_core::repository::resolution_policy::{
        DependencyMixingPolicy, ResolutionPolicy, SelectionMode,
    };

    fn seed_latest_mode_update_fixture(conn: &rusqlite::Connection) -> Trove {
        let mut fedora_repo = Repository::new(
            "fedora-main".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.priority = 50;
        fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
        let fedora_repo_id = fedora_repo.insert(conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.priority = 10;
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(conn).unwrap();

        let mut canonical = CanonicalPackage::new("demo".to_string(), "package".to_string());
        let canonical_id = canonical.insert(conn).unwrap();
        let fresh = Utc::now().to_rfc3339();

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
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(conn).unwrap();

        let mut fedora_candidate = RepositoryPackage::new(
            fedora_repo_id,
            "demo".to_string(),
            "1.1.0-1.fc44".to_string(),
            "sha256:fedora-demo".to_string(),
            123,
            "https://example.test/fedora/demo-1.1.0-1.fc44.rpm".to_string(),
        );
        fedora_candidate.architecture = Some("x86_64".to_string());
        fedora_candidate.distro = Some("fedora-44".to_string());
        fedora_candidate.version_scheme = Some("rpm".to_string());
        fedora_candidate.canonical_id = Some(canonical_id);
        fedora_candidate.insert(conn).unwrap();

        let mut arch_candidate = RepositoryPackage::new(
            arch_repo_id,
            "demo".to_string(),
            "1.2.0-1".to_string(),
            "sha256:arch-demo".to_string(),
            123,
            "https://example.test/arch/demo-1.2.0-1.pkg.tar.zst".to_string(),
        );
        arch_candidate.architecture = Some("x86_64".to_string());
        arch_candidate.distro = Some("arch".to_string());
        arch_candidate.version_scheme = Some("arch".to_string());
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
        repo.default_strategy_distro = Some("fedora-44".to_string());
        repo.security_advisory_support = support;
        let repo_id = repo.insert(conn).unwrap();

        let mut installed = Trove::new_with_source(
            "openssl".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(repo_id);
        installed.insert(conn).unwrap();

        let mut candidate = RepositoryPackage::new(
            repo_id,
            "openssl".to_string(),
            "1.0.1".to_string(),
            "sha256:openssl".to_string(),
            123,
            "https://example.test/security/openssl-1.0.1.ccs".to_string(),
        );
        candidate.architecture = Some("x86_64".to_string());
        candidate.distro = Some("fedora-44".to_string());
        candidate.version_scheme = Some("rpm".to_string());
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
        let mut repo = Repository::new(
            "debian-main".to_string(),
            "https://deb.example.test".to_string(),
        );
        repo.default_strategy_distro = Some("ubuntu-24.04".to_string());

        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0~beta1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.version_scheme = Some("debian".to_string());

        let mut candidate = RepositoryPackage::new(
            1,
            "demo".to_string(),
            "1.0".to_string(),
            "sha256:demo".to_string(),
            1,
            "https://deb.example.test/demo_1.0_amd64.deb".to_string(),
        );
        candidate.version_scheme = Some("debian".to_string());

        assert!(is_repo_version_newer(&trove, &repo, &candidate));
    }

    #[test]
    fn test_is_repo_version_newer_uses_arch_scheme() {
        let mut repo = Repository::new(
            "arch-core".to_string(),
            "https://arch.example.test".to_string(),
        );
        repo.default_strategy_distro = Some("arch".to_string());

        let mut trove = Trove::new_with_source(
            "demo".to_string(),
            "1.0-1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        trove.version_scheme = Some("arch".to_string());

        let mut candidate = RepositoryPackage::new(
            1,
            "demo".to_string(),
            "1.0-2".to_string(),
            "sha256:demo".to_string(),
            1,
            "https://arch.example.test/demo-1.0-2.pkg.tar.zst".to_string(),
        );
        candidate.version_scheme = Some("arch".to_string());

        assert!(is_repo_version_newer(&trove, &repo, &candidate));
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
        repo.default_strategy_distro = Some("ubuntu".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut package = RepositoryPackage::new(
            repo_id,
            "phase4-runtime-fixture".to_string(),
            "1.0.1".to_string(),
            "sha256:fixture".to_string(),
            1110,
            "http://127.0.0.1:18087/phase4-runtime-fixture_1.0.1_amd64.deb".to_string(),
        );
        package.architecture = Some("amd64".to_string());
        package.distro = Some("ubuntu".to_string());
        package.version_scheme = Some("debian".to_string());
        package.insert(&conn).unwrap();

        let mut installed = Trove::new(
            "phase4-runtime-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        installed.architecture = Some("amd64".to_string());
        installed.version_scheme = Some("debian".to_string());

        let selected = select_update_candidate(
            &conn,
            &installed,
            false,
            &ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict),
            Some(RepositoryDependencyFlavor::Deb),
        )
        .unwrap()
        .expect("expected generic metadata-driven Debian update");

        assert_eq!(selected.package.version, "1.0.1");
        assert_eq!(selected.repository.name, "slice-d-local-update");
        assert_eq!(selected.package.version_scheme.as_deref(), Some("debian"));
    }

    #[test]
    fn latest_mode_update_can_switch_sources_when_newest_allowed_candidate_differs() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_latest_mode_update_fixture(&conn);
        let policy = ResolutionPolicy::new()
            .with_selection_mode(SelectionMode::Latest)
            .with_mixing(DependencyMixingPolicy::Permissive);

        let selected = select_update_candidate(
            &conn,
            &trove,
            false,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap()
        .expect("expected update candidate");

        assert_eq!(selected.repository.name, "arch-core");
        assert_eq!(selected.package.version, "1.2.0-1");
        let source_switch = selected
            .source_switch
            .expect("expected source-switch metadata for latest-mode update");
        assert_eq!(source_switch.from_distro, "fedora-44");
        assert_eq!(source_switch.to_distro, "arch");
        assert!(source_switch.reason.contains("latest"));
    }

    #[test]
    fn latest_mode_update_previews_source_switches_in_dry_run() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_latest_mode_update_fixture(&conn);
        let policy = ResolutionPolicy::new()
            .with_selection_mode(SelectionMode::Latest)
            .with_mixing(DependencyMixingPolicy::Permissive);

        let selected = select_update_candidate(
            &conn,
            &trove,
            false,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap()
        .expect("expected update candidate");

        let preview = render_source_switch_preview_line(&selected)
            .expect("expected latest-mode update preview for source switch");
        assert!(preview.contains("demo"));
        assert!(preview.contains("fedora-44"));
        assert!(preview.contains("arch"));
        assert!(preview.contains("latest"));
    }

    #[test]
    fn latest_mode_update_requires_confirmation_for_source_switch_without_yes() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_latest_mode_update_fixture(&conn);
        let policy = ResolutionPolicy::new()
            .with_selection_mode(SelectionMode::Latest)
            .with_mixing(DependencyMixingPolicy::Permissive);

        let selected = select_update_candidate(
            &conn,
            &trove,
            false,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap()
        .expect("expected update candidate");

        assert!(requires_source_switch_confirmation(
            std::slice::from_ref(&selected),
            false
        ));
        assert!(!requires_source_switch_confirmation(&[selected], true));
    }

    #[test]
    fn policy_mode_update_prefers_current_source_candidate() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_latest_mode_update_fixture(&conn);
        let policy = ResolutionPolicy::new().with_selection_mode(SelectionMode::Policy);

        let selected = select_update_candidate(
            &conn,
            &trove,
            false,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap()
        .expect("expected same-source update candidate");

        assert_eq!(selected.repository.name, "fedora-main");
        assert_eq!(selected.package.version, "1.1.0-1.fc44");
        assert!(selected.source_switch.is_none());
    }

    #[test]
    fn security_update_refuses_unknown_source_metadata_before_mutation() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_security_update_fixture(&conn, SecurityAdvisorySupport::Unknown, false);
        let policy = ResolutionPolicy::new();

        let result = select_update_candidate(
            &conn,
            &trove,
            true,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap();

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

        let result = select_update_candidate(
            &conn,
            &trove,
            true,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap();

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

        let result = select_update_candidate(
            &conn,
            &trove,
            true,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap();

        assert!(matches!(result, UpdateCandidateSelection::Selected(_)));
    }

    #[test]
    fn security_update_marker_includes_trusted_advisory_details() {
        let mut package = RepositoryPackage::new(
            7,
            "openssl".to_string(),
            "3.2.1-1.fc44".to_string(),
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

        let result = select_update_candidate(
            &conn,
            &trove,
            true,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap();

        assert!(matches!(result, UpdateCandidateSelection::NoEligibleUpdate));
    }

    #[test]
    fn latest_mode_update_respects_strict_mixing_and_stays_on_current_source() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let trove = seed_latest_mode_update_fixture(&conn);
        let policy = ResolutionPolicy::new()
            .with_selection_mode(SelectionMode::Latest)
            .with_mixing(DependencyMixingPolicy::Strict);

        let selected = select_update_candidate(
            &conn,
            &trove,
            false,
            &policy,
            Some(RepositoryDependencyFlavor::Rpm),
        )
        .unwrap()
        .expect("expected strict-mixing update candidate");

        assert_eq!(selected.repository.name, "fedora-main");
        assert_eq!(selected.package.version, "1.1.0-1.fc44");
        assert!(selected.source_switch.is_none());
    }
}
