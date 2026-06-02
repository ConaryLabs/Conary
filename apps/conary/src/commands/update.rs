// src/commands/update.rs
//! Update, pinning, and delta statistics commands

use super::install::{
    DepMode, repository_install_provenance_from_package, resolve_default_dep_mode_from_model,
};
use super::open_db;
use super::progress::{UpdatePhase, UpdateProgress};
use super::{InstalledPackageSelector, SandboxMode, cmd_install, resolve_installed_package};
use anyhow::{Context, Result};
use chrono::Utc;
use conary_core::db::models::{
    DeltaStats, DistroPin, PackageDelta, RepologyCacheEntry, Repository, RepositoryPackage,
    SecurityAdvisorySupport, SystemAffinity, Trove, TroveType,
};
use conary_core::db::paths::objects_dir;
use conary_core::delta::DeltaApplier;
use conary_core::model::{
    DiffAction, capture_current_state, planned_replatform_actions, replatform_execution_plan,
    source_policy_replatform_snapshot,
};
use conary_core::packages::SystemPackageManager;
use conary_core::repository::{
    self, DownloadOptions, LatestSignal, PackageSelector, PackageSource, ResolutionOptions,
    SelectionOptions,
    dependency_model::RepositoryDependencyFlavor,
    resolution_policy::{ResolutionPolicy, SelectionMode},
    resolve_package,
    versioning::{VersionScheme, compare_mixed_repo_versions, resolve_package_version_scheme},
};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

fn read_delta_result_from_cas(
    cas: &conary_core::filesystem::CasStore,
    hash: &str,
) -> Result<Vec<u8>> {
    cas.retrieve(hash)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("failed to retrieve verified delta result from CAS: {hash}"))
}

fn resolution_options_for_selected_update(
    repo_pkg: &RepositoryPackage,
    repo: &Repository,
    temp_dir: &Path,
    keyring_dir: &Path,
    policy: &ResolutionPolicy,
    primary_flavor: Option<RepositoryDependencyFlavor>,
) -> ResolutionOptions {
    ResolutionOptions {
        version: Some(repo_pkg.version.clone()),
        repository: Some(repo.name.clone()),
        architecture: repo_pkg.architecture.clone(),
        output_dir: Some(PathBuf::from(temp_dir)),
        gpg_options: if repo.gpg_check {
            Some(DownloadOptions {
                gpg_check: true,
                gpg_strict: repo.gpg_strict,
                keyring_dir: keyring_dir.to_path_buf(),
                repository_name: repo.name.clone(),
            })
        } else {
            None
        },
        // Update has already selected a repository package. Do not let the
        // generic resolver short-circuit on an installed same-version trove,
        // because source-switch updates can intentionally reinstall the same
        // version from a different authority.
        skip_cas: true,
        policy: Some(policy.clone()),
        is_root: false,
        primary_flavor,
    }
}

fn mark_pending_changeset_rolled_back(
    conn: &mut rusqlite::Connection,
    changeset_id: i64,
) -> Result<bool> {
    use conary_core::db::models::{Changeset, ChangesetStatus};

    Ok(conary_core::db::transaction(conn, |tx| {
        let Some(mut changeset) = Changeset::find_by_id(tx, changeset_id)? else {
            return Ok(false);
        };

        if changeset.status != ChangesetStatus::Pending {
            return Ok(false);
        }

        changeset.update_status(tx, ChangesetStatus::RolledBack)?;
        Ok(true)
    })?)
}

fn source_policy_update_context(
    pin: Option<&DistroPin>,
    affinities: &[SystemAffinity],
    realignment_candidates: Option<usize>,
) -> Option<String> {
    let pin = pin?;
    let strength = pin.mixing_policy.as_str();

    if affinities.is_empty() {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no source affinity data yet.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let total_packages: i64 = affinities
        .iter()
        .map(|affinity| affinity.package_count)
        .sum();
    if total_packages == 0 {
        return Some(format!(
            "Active source policy pin: {} ({}). Replatform estimate unavailable: no installed packages are represented in current affinity data.{}",
            pin.distro,
            strength,
            match realignment_candidates {
                Some(count) => format!(
                    " Package-level realignment candidates currently visible: {}.",
                    count
                ),
                None => String::new(),
            }
        ));
    }

    let aligned_packages = affinities
        .iter()
        .find(|affinity| affinity.distro == pin.distro)
        .map(|affinity| affinity.package_count)
        .unwrap_or(0);
    let packages_to_realign = total_packages.saturating_sub(aligned_packages);

    Some(format!(
        "Active source policy pin: {} ({}). About {} installed package(s) already align, and about {} may need source realignment during future convergence.{}",
        pin.distro,
        strength,
        aligned_packages,
        packages_to_realign,
        match realignment_candidates {
            Some(count) => format!(
                " Package-level realignment candidates currently visible: {}.",
                count
            ),
            None => String::new(),
        }
    ))
}

fn render_replatform_action_preview(actions: &[DiffAction]) -> Option<String> {
    let replatforms: Vec<_> = actions
        .iter()
        .filter_map(|action| match action {
            DiffAction::ReplatformReplace { .. } => Some(action.description()),
            _ => None,
        })
        .collect();

    if replatforms.is_empty() {
        return None;
    }

    let preview: Vec<String> = replatforms.iter().take(3).cloned().collect();

    let mut line = format!("Planned replatform replacements: {}", preview.join(", "));
    if replatforms.len() > preview.len() {
        line.push_str(&format!(", +{} more", replatforms.len() - preview.len()));
    }
    Some(line)
}

use super::replatform_rendering::render_replatform_execution_plan;

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
struct SelectedUpdateCandidate {
    package: RepositoryPackage,
    repository: Repository,
    source_switch: Option<UpdateSourceSwitch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecurityMetadataUnavailable {
    package: String,
    repository: String,
    support: SecurityAdvisorySupport,
    candidate_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatePackageFailure {
    package: String,
    version: String,
    reason: String,
}

#[derive(Debug, Clone)]
enum UpdateCandidateSelection {
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
fn select_update_candidate(
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

fn requires_source_switch_confirmation(updates: &[SelectedUpdateCandidate], yes: bool) -> bool {
    !yes && updates.iter().any(|update| update.source_switch.is_some())
}

fn render_source_switch_preview_lines(updates: &[(Trove, SelectedUpdateCandidate)]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|(_, selection)| render_source_switch_preview_line(selection))
        .collect()
}

fn print_source_switch_preview(updates: &[(Trove, SelectedUpdateCandidate)]) {
    let preview_lines = render_source_switch_preview_lines(updates);
    if preview_lines.is_empty() {
        return;
    }

    println!("\nSource switches proposed:");
    for line in preview_lines {
        println!("  {}", line);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdoptedUpdateDecision {
    SkipNativeAuthority,
    QueueTakeover,
    BlockCritical,
}

fn adopted_update_decision(
    trove: &Trove,
    dep_mode: DepMode,
    requested_dep_mode: Option<DepMode>,
) -> AdoptedUpdateDecision {
    let explicit_takeover = matches!(requested_dep_mode, Some(DepMode::Takeover));
    if dep_mode == DepMode::Takeover && explicit_takeover {
        if super::install::is_package_blocked(&trove.name) {
            AdoptedUpdateDecision::BlockCritical
        } else {
            AdoptedUpdateDecision::QueueTakeover
        }
    } else {
        AdoptedUpdateDecision::SkipNativeAuthority
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdoptedUpdateSkipReason {
    NativeAuthority,
    CriticalBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdoptedUpdateSkip {
    package: String,
    manager: SystemPackageManager,
    reason: AdoptedUpdateSkipReason,
}

fn native_manager_for_trove(
    trove: &Trove,
    fallback_manager: SystemPackageManager,
) -> SystemPackageManager {
    SystemPackageManager::from_version_scheme(trove.version_scheme.as_deref())
        .unwrap_or(fallback_manager)
}

fn render_adopted_skip_sample(skips: &[&AdoptedUpdateSkip]) -> String {
    let mut sample: Vec<String> = skips
        .iter()
        .take(5)
        .map(|skip| {
            format!(
                "{} ({})",
                skip.package,
                skip.manager.update_command(&skip.package)
            )
        })
        .collect();
    if skips.len() > 5 {
        sample.push(format!("... and {} more", skips.len() - 5));
    }
    sample.join(", ")
}

fn no_update_message(security_only: bool, adopted_updates_skipped: bool) -> &'static str {
    match (security_only, adopted_updates_skipped) {
        (true, true) => {
            "No Conary-managed security updates available; adopted package updates remain under native package-manager authority"
        }
        (false, true) => {
            "No Conary-managed updates available; adopted package updates remain under native package-manager authority"
        }
        (true, false) => "No security updates available",
        (false, false) => "All packages are up to date",
    }
}

fn render_security_update_marker(package: &RepositoryPackage) -> String {
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

fn print_security_metadata_unavailable(unavailable: &[SecurityMetadataUnavailable]) {
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

fn security_metadata_unavailable_error(count: usize) -> String {
    format!(
        "Cannot run security-only update because {count} source(s) cannot prove security metadata support. Mark the source supported only after its repository metadata publishes advisory data."
    )
}

fn update_required_failure_message(
    failures: &[UpdatePackageFailure],
    total_requested: usize,
) -> Option<String> {
    if failures.is_empty() {
        return None;
    }

    let sample = failures
        .iter()
        .map(|failure| {
            format!(
                "{} {} ({})",
                failure.package, failure.version, failure.reason
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    Some(format!(
        "{} of {} requested package update(s) failed: {}",
        failures.len(),
        total_requested,
        sample
    ))
}

/// Pin a package to prevent updates and removal
pub async fn cmd_pin(selector: InstalledPackageSelector, db_path: &str) -> Result<()> {
    info!("Pinning package: {}", selector.name);
    let conn = open_db(db_path)?;
    let resolved = resolve_installed_package(&conn, &selector)?;
    let trove = resolved.trove;
    let trove_id = resolved.trove_id;

    if trove.pinned {
        println!("Package '{}' is already pinned", trove.name);
        return Ok(());
    }

    Trove::pin(&conn, trove_id)?;
    println!(
        "Pinned package '{}' at version {}",
        trove.name, trove.version
    );
    println!("This package will be skipped during updates and cannot be removed until unpinned.");

    Ok(())
}

/// Unpin a package to allow updates and removal
pub async fn cmd_unpin(selector: InstalledPackageSelector, db_path: &str) -> Result<()> {
    info!("Unpinning package: {}", selector.name);
    let conn = open_db(db_path)?;
    let resolved = resolve_installed_package(&conn, &selector)?;
    let trove = resolved.trove;
    let trove_id = resolved.trove_id;

    if !trove.pinned {
        println!("Package '{}' is not pinned", trove.name);
        return Ok(());
    }

    Trove::unpin(&conn, trove_id)?;
    println!(
        "Unpinned package '{}' (version {})",
        trove.name, trove.version
    );
    println!("This package can now be updated or removed.");

    Ok(())
}

/// List all pinned packages
pub async fn cmd_list_pinned(db_path: &str) -> Result<()> {
    info!("Listing pinned packages");

    let conn = open_db(db_path)?;
    let pinned = Trove::find_pinned(&conn)?;

    if pinned.is_empty() {
        println!("No packages are pinned.");
        return Ok(());
    }

    println!("Pinned packages:");
    for trove in &pinned {
        print!("  {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} pinned package(s)", pinned.len());

    Ok(())
}

/// Check for and apply package updates
///
/// If `security_only` is true, only applies updates from sources with trusted
/// advisory metadata that mark the candidate as a security update.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_update(
    package: Option<String>,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: Option<DepMode>,
    yes: bool,
    package_version: Option<String>,
    architecture: Option<String>,
    legacy_replay: super::LegacyReplayOptions,
) -> Result<()> {
    if security_only {
        info!("Checking for security updates only");
    } else {
        info!("Checking for package updates");
    }

    let requested_dep_mode = dep_mode;
    let dep_mode = requested_dep_mode.unwrap_or_else(resolve_default_dep_mode_from_model);

    let mut conn = open_db(db_path)?;
    let effective_source_policy = conary_core::repository::load_effective_policy(
        &conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
    let policy = effective_source_policy.resolution.clone();
    let primary_flavor = effective_source_policy.primary_flavor;

    if package.is_none() {
        let current_pin = DistroPin::get_current(&conn)?;
        let affinities = SystemAffinity::list(&conn)?;
        let realignment_snapshot = current_pin
            .as_ref()
            .map(|pin| source_policy_replatform_snapshot(&conn, &pin.distro))
            .transpose()?;
        let realignment_candidates = realignment_snapshot
            .as_ref()
            .map(|snapshot| snapshot.visible_realignment_candidates);
        if let Some(context) =
            source_policy_update_context(current_pin.as_ref(), &affinities, realignment_candidates)
        {
            println!("{}", context);
        }
        if let Some(snapshot) = realignment_snapshot.as_ref() {
            let state = capture_current_state(&conn)?;
            let actions = planned_replatform_actions(snapshot, &state);
            if let Some(plan) = replatform_execution_plan(&conn, &actions)? {
                println!("{}", render_replatform_execution_plan(&plan));
            } else if let Some(preview) = render_replatform_action_preview(&actions) {
                println!("{}", preview);
            }
        }
    }

    let objects_dir = objects_dir(db_path);
    let temp_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("tmp");
    std::fs::create_dir_all(&temp_dir)?;

    let keyring_dir = conary_core::db::paths::keyring_dir(db_path);

    let installed_troves =
        installed_troves_for_update(&conn, package, package_version, architecture)?;

    if installed_troves.is_empty() {
        println!("No packages to update");
        return Ok(());
    }

    // Collect updates with their repository info (needed for GPG verification)
    let mut updates_available: Vec<(Trove, SelectedUpdateCandidate)> = Vec::new();
    let mut pinned_skipped: Vec<String> = Vec::new();

    let detected_pkg_mgr = SystemPackageManager::detect();
    let mut adopted_skipped: Vec<AdoptedUpdateSkip> = Vec::new();
    let mut security_metadata_unavailable: Vec<SecurityMetadataUnavailable> = Vec::new();

    for trove in &installed_troves {
        // Skip pinned packages
        if trove.pinned {
            pinned_skipped.push(trove.name.clone());
            continue;
        }

        let adopted_decision = if trove.install_source.is_adopted() {
            Some(adopted_update_decision(trove, dep_mode, requested_dep_mode))
        } else {
            None
        };
        let enforce_security_metadata = security_only
            && !matches!(
                adopted_decision,
                Some(
                    AdoptedUpdateDecision::SkipNativeAuthority
                        | AdoptedUpdateDecision::BlockCritical
                )
            );

        let selected = match select_update_candidate(
            &conn,
            trove,
            enforce_security_metadata,
            &policy,
            primary_flavor,
        )? {
            UpdateCandidateSelection::Selected(selected) => *selected,
            UpdateCandidateSelection::NoEligibleUpdate => continue,
            UpdateCandidateSelection::SecurityMetadataUnavailable(unavailable) => {
                security_metadata_unavailable.push(unavailable);
                continue;
            }
        };

        // For adopted packages, native package-manager authority is preserved
        // unless the user explicitly asks Conary to take ownership.
        if trove.install_source.is_adopted() {
            let native_manager = native_manager_for_trove(trove, detected_pkg_mgr);
            match adopted_decision.expect("adopted trove must have an update decision") {
                AdoptedUpdateDecision::SkipNativeAuthority => {
                    println!(
                        "  {} {} -> {} (adopted as {}, native authority: use '{}')",
                        trove.name,
                        trove.version,
                        selected.package.version,
                        trove.install_source.as_str(),
                        native_manager.update_command(&trove.name),
                    );
                    adopted_skipped.push(AdoptedUpdateSkip {
                        package: trove.name.clone(),
                        manager: native_manager,
                        reason: AdoptedUpdateSkipReason::NativeAuthority,
                    });
                    continue;
                }
                AdoptedUpdateDecision::BlockCritical => {
                    println!(
                        "  {} {} (blocked - critical adopted package remains under native authority: use '{}')",
                        trove.name,
                        trove.version,
                        native_manager.update_command(&trove.name),
                    );
                    adopted_skipped.push(AdoptedUpdateSkip {
                        package: trove.name.clone(),
                        manager: native_manager,
                        reason: AdoptedUpdateSkipReason::CriticalBlocked,
                    });
                    continue;
                }
                AdoptedUpdateDecision::QueueTakeover => {
                    println!(
                        "  {} {} -> {} (taking over from system PM)",
                        trove.name, trove.version, selected.package.version
                    );
                }
            }
        }

        let security_marker = render_security_update_marker(&selected.package);
        info!(
            "Update available: {} {} -> {}{}",
            trove.name, trove.version, selected.package.version, security_marker
        );
        updates_available.push((trove.clone(), selected));
    }

    if !security_metadata_unavailable.is_empty() {
        print_security_metadata_unavailable(&security_metadata_unavailable);
        anyhow::bail!(security_metadata_unavailable_error(
            security_metadata_unavailable.len()
        ));
    }

    // Report pinned packages that were skipped
    if !pinned_skipped.is_empty() {
        println!(
            "Skipping {} pinned package(s): {}",
            pinned_skipped.len(),
            pinned_skipped.join(", ")
        );
    }

    // Report adopted packages that were skipped because native authority still owns them.
    if !adopted_skipped.is_empty() {
        let native_authority: Vec<&AdoptedUpdateSkip> = adopted_skipped
            .iter()
            .filter(|skip| skip.reason == AdoptedUpdateSkipReason::NativeAuthority)
            .collect();
        if !native_authority.is_empty() {
            println!(
                "Skipping {} adopted package(s); native package-manager authority owns updates: {}",
                native_authority.len(),
                render_adopted_skip_sample(&native_authority)
            );
            println!(
                "Run 'conary --allow-live-system-mutation system adopt --refresh' after native package-manager changes before retrying Conary workflows."
            );
            if !matches!(requested_dep_mode, Some(DepMode::Takeover)) {
                println!(
                    "Use --dep-mode takeover to request Conary takeover for non-critical adopted packages."
                );
            }
        }

        let critical_blocked: Vec<&AdoptedUpdateSkip> = adopted_skipped
            .iter()
            .filter(|skip| skip.reason == AdoptedUpdateSkipReason::CriticalBlocked)
            .collect();
        if !critical_blocked.is_empty() {
            println!(
                "Blocked {} critical adopted package(s) from takeover; native package-manager authority remains required: {}",
                critical_blocked.len(),
                render_adopted_skip_sample(&critical_blocked)
            );
        }
    }

    if updates_available.is_empty() {
        println!(
            "{}",
            no_update_message(security_only, !adopted_skipped.is_empty())
        );
        return Ok(());
    }

    let security_count = updates_available
        .iter()
        .filter(|(_, selected)| selected.package.is_security_update)
        .count();
    if security_only {
        println!(
            "Found {} security update(s) available:",
            updates_available.len()
        );
    } else {
        println!(
            "Found {} package(s) with updates available{}:",
            updates_available.len(),
            if security_count > 0 {
                format!(" ({} security)", security_count)
            } else {
                String::new()
            }
        );
    }
    for (trove, selected) in &updates_available {
        let security_marker = render_security_update_marker(&selected.package);
        println!(
            "  {} {} -> {}{}",
            trove.name, trove.version, selected.package.version, security_marker
        );
    }

    print_source_switch_preview(&updates_available);

    let selected_updates: Vec<_> = updates_available
        .iter()
        .map(|(_, selected)| selected.clone())
        .collect();
    if requires_source_switch_confirmation(&selected_updates, yes) {
        anyhow::bail!(
            "One or more updates would switch package sources. Review the preview above and rerun with --yes to confirm, or use --dry-run first."
        );
    }

    if dry_run {
        println!("\nDry run: no updates were applied.");
        return Ok(());
    }

    // Phase 1: Check for deltas and categorize updates
    let mut delta_updates: Vec<(Trove, RepositoryPackage, Repository, PackageDelta)> = Vec::new();
    let mut full_updates: Vec<(Trove, RepositoryPackage, Repository)> = Vec::new();

    for (trove, selected) in updates_available {
        let repo_pkg = selected.package;
        let repo = selected.repository;
        if let Ok(Some(delta_info)) =
            PackageDelta::find_delta(&conn, &trove.name, &trove.version, &repo_pkg.version)
        {
            println!(
                "  {} has delta: {} bytes ({:.1}% of full)",
                trove.name,
                delta_info.delta_size,
                delta_info.compression_ratio * 100.0
            );
            delta_updates.push((trove, repo_pkg, repo, delta_info));
        } else {
            full_updates.push((trove, repo_pkg, repo));
        }
    }

    let mut total_bytes_saved = 0i64;
    let mut deltas_applied = 0i32;
    let mut full_downloads = 0i32;
    let mut delta_failures = 0i32;
    let mut required_failures: Vec<UpdatePackageFailure> = Vec::new();

    // Save counts before consuming the vectors
    let delta_count = delta_updates.len();
    let initial_full_count = full_updates.len();
    let total_requested = delta_count + initial_full_count;

    // Only create a changeset when there is actual work to do
    if total_requested == 0 {
        println!("No updates to apply.");
        return Ok(());
    }

    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new(format!(
            "Update {} package(s)",
            total_requested
        ));
        changeset.insert(tx)
    })?;

    let update_result: Result<()> = async {
        // Phase 2: Download and apply deltas (sequential - requires CAS access)
        for (trove, repo_pkg, repo, delta_info) in delta_updates {
            println!("\nUpdating {} (delta)...", trove.name);

            match repository::download_delta(
                &repository::DeltaInfo {
                    from_version: delta_info.from_version.clone(),
                    from_hash: delta_info.from_hash.clone(),
                    delta_url: delta_info.delta_url.clone(),
                    delta_size: delta_info.delta_size,
                    delta_checksum: delta_info.delta_checksum.clone(),
                    compression_ratio: delta_info.compression_ratio,
                },
                &trove.name,
                &repo_pkg.version,
                &temp_dir,
            )
            .await
            {
                Ok(actual_delta_path) => {
                    let applier = DeltaApplier::new(&objects_dir)?;
                    match applier.apply_delta(
                        &delta_info.from_hash,
                        &actual_delta_path,
                        &delta_info.to_hash,
                    ) {
                        Ok(new_hash) => {
                            println!("  [OK] Delta applied to CAS");
                            let delta_saved = (repo_pkg.size - delta_info.delta_size).max(0);
                            // Delta reconstructed the new package in CAS. Retrieve
                            // it and feed through the normal install pipeline so all
                            // DB metadata (files, deps, provides, history) and the
                            // live generation transition correctly -- without a
                            // redundant network download.
                            let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;
                            let mut delta_installed = false;
                            match read_delta_result_from_cas(&cas, &new_hash) {
                                Ok(content) => {
                                    let pkg_file = temp_dir
                                        .join(format!("{}-{}.ccs", trove.name, repo_pkg.version));
                                    if let Err(e) = std::fs::write(&pkg_file, &content) {
                                        warn!(
                                            "  Failed to write delta result for {}: {}",
                                            trove.name, e
                                        );
                                    } else {
                                        let path_str = pkg_file.to_string_lossy().to_string();
                                        match cmd_install(
                                            &path_str,
                                            super::InstallOptions {
                                                db_path,
                                                root,
                                                no_scripts,
                                                sandbox_mode,
                                                dep_mode: Some(dep_mode),
                                                yes,
                                                legacy_replay,
                                                repository_provenance: Some(
                                                    repository_install_provenance_from_package(
                                                        &repo_pkg, &repo,
                                                    )?,
                                                ),
                                                ..Default::default()
                                            },
                                        )
                                        .await
                                        {
                                            Ok(()) => {
                                                delta_installed = true;
                                                println!(
                                                    "  [OK] {} {} -> {}",
                                                    trove.name, trove.version, repo_pkg.version
                                                );
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "  Delta install failed for {}: {}",
                                                    trove.name, e
                                                );
                                            }
                                        }
                                        let _ = std::fs::remove_file(&pkg_file);
                                    }
                                }
                                Err(e) => {
                                    warn!("  Failed to retrieve delta result from CAS: {}", e);
                                }
                            }
                            if delta_installed {
                                // Only count success after the full install pipeline
                                // completes -- not just after apply_delta().
                                deltas_applied += 1;
                                total_bytes_saved += delta_saved;
                            } else {
                                // Fall back to full download
                                delta_failures += 1;
                                if let Ok(Some(repo)) =
                                    Repository::find_by_id(&conn, repo_pkg.repository_id)
                                {
                                    full_updates.push((trove, repo_pkg, repo));
                                } else {
                                    required_failures.push(UpdatePackageFailure {
                                        package: trove.name,
                                        version: repo_pkg.version,
                                        reason: "delta failed and fallback repository was not found"
                                            .to_string(),
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "  Delta application failed: {}, will download full package",
                                e
                            );
                            delta_failures += 1;
                            // Get repository for fallback download
                            if let Ok(Some(repo)) =
                                Repository::find_by_id(&conn, repo_pkg.repository_id)
                            {
                                full_updates.push((trove, repo_pkg, repo));
                            } else {
                                required_failures.push(UpdatePackageFailure {
                                    package: trove.name,
                                    version: repo_pkg.version,
                                    reason: "delta application failed and fallback repository was not found"
                                        .to_string(),
                                });
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&actual_delta_path);
                }
                Err(e) => {
                    warn!("  Delta download failed: {}, will download full package", e);
                    delta_failures += 1;
                    // Get repository for fallback download
                    if let Ok(Some(repo)) = Repository::find_by_id(&conn, repo_pkg.repository_id) {
                        full_updates.push((trove, repo_pkg, repo));
                    } else {
                        required_failures.push(UpdatePackageFailure {
                            package: trove.name,
                            version: repo_pkg.version,
                            reason: "delta download failed and fallback repository was not found"
                                .to_string(),
                        });
                    }
                }
            }
        }

        // Phase 3 & 4: Resolve and install full packages using unified resolution
        // This respects per-repo routing strategies (remi, binary, etc.)
        if !full_updates.is_empty() {
            let total_to_install = full_updates.len() as u64;
            let mut progress = UpdateProgress::new(total_to_install);

            progress.set_status("Resolving and downloading packages...");

            // Process packages sequentially (resolution requires DB access)
            for (trove, repo_pkg, repo) in full_updates {
                info!("Resolving {} from {}", trove.name, repo.name);
                progress.set_phase(&trove.name, UpdatePhase::DownloadingFull);

                let options = resolution_options_for_selected_update(
                    &repo_pkg,
                    &repo,
                    &temp_dir,
                    &keyring_dir,
                    &policy,
                    primary_flavor,
                );

                // Use unified resolver - respects remi/binary/recipe strategies
                let source = match resolve_package(&conn, &trove.name, &options).await {
                    Ok(source) => source,
                    Err(e) => {
                        progress.fail_package(&trove.name, &e.to_string());
                        warn!("Failed to resolve {}: {}", trove.name, e);
                        required_failures.push(UpdatePackageFailure {
                            package: trove.name.clone(),
                            version: repo_pkg.version.clone(),
                            reason: e.to_string(),
                        });
                        continue;
                    }
                };

                // Get path from source
                let pkg_path = match &source {
                    PackageSource::Binary { path, .. } => path.clone(),
                    PackageSource::Ccs { path, .. } => path.clone(),
                    PackageSource::Delta { delta_path, .. } => delta_path.clone(),
                    PackageSource::LocalCas { hash } => {
                        // Check if this is an "already installed" marker
                        if hash.starts_with("installed:") {
                            info!("{} is already at the latest version (skipping)", trove.name);
                            progress.complete_package(&trove.name);
                            continue;
                        }
                        // Future: handle actual CAS content hashes
                        progress.fail_package(&trove.name, "LocalCas not yet supported");
                        warn!(
                            "LocalCas resolution not yet implemented for {}: {}",
                            trove.name, hash
                        );
                        required_failures.push(UpdatePackageFailure {
                            package: trove.name.clone(),
                            version: repo_pkg.version.clone(),
                            reason: format!("LocalCas not yet supported: {hash}"),
                        });
                        continue;
                    }
                };

                progress.set_phase(&trove.name, UpdatePhase::Installing);

                let path_str = pkg_path.to_string_lossy().to_string();

                if let Err(e) = cmd_install(
                    &path_str,
                    super::InstallOptions {
                        db_path,
                        root,
                        no_scripts,
                        sandbox_mode,
                        dep_mode: Some(dep_mode),
                        yes,
                        legacy_replay,
                        repository_provenance: Some(repository_install_provenance_from_package(
                            &repo_pkg, &repo,
                        )?),
                        ..Default::default()
                    },
                )
                .await
                {
                    progress.fail_package(&trove.name, &e.to_string());
                    warn!("  Package installation failed: {}", e);
                    required_failures.push(UpdatePackageFailure {
                        package: trove.name.clone(),
                        version: repo_pkg.version.clone(),
                        reason: e.to_string(),
                    });
                    let _ = std::fs::remove_file(&pkg_path);
                    continue;
                }

                full_downloads += 1;
                progress.complete_package(&trove.name);
                let _ = std::fs::remove_file(&pkg_path);
            }

            progress.finish(&format!(
                "Updated {} package(s)",
                deltas_applied + full_downloads
            ));
        }

        conary_core::db::transaction(&mut conn, |tx| {
            let mut stats = DeltaStats::new(changeset_id);
            stats.total_bytes_saved = total_bytes_saved;
            stats.deltas_applied = deltas_applied;
            stats.full_downloads = full_downloads;
            stats.delta_failures = delta_failures;
            stats.insert(tx)?;

            let mut changeset = conary_core::db::models::Changeset::find_by_id(tx, changeset_id)?
                .ok_or_else(|| {
                conary_core::Error::NotFound("Changeset not found".to_string())
            })?;
            if deltas_applied > 0 || full_downloads > 0 {
                changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            } else if !required_failures.is_empty() {
                changeset
                    .update_status(tx, conary_core::db::models::ChangesetStatus::RolledBack)?;
            } else {
                changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            }

            Ok(())
        })?;

        println!("\n=== Update Summary ===");
        println!("Delta updates: {}", deltas_applied);
        println!("Full downloads: {}", full_downloads);
        println!("Delta failures: {}", delta_failures);
        if let Some(message) = update_required_failure_message(&required_failures, total_requested)
        {
            println!("Required failures: {}", required_failures.len());
            for failure in &required_failures {
                println!(
                    "  {} {}: {}",
                    failure.package, failure.version, failure.reason
                );
            }
            return Err(anyhow::anyhow!(message));
        }
        if total_bytes_saved > 0 {
            let saved_mb = total_bytes_saved as f64 / 1_048_576.0;
            println!("Bandwidth saved: {:.2} MB", saved_mb);
        }

        Ok(())
    }
    .await;

    match update_result {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Err(cleanup_err) = mark_pending_changeset_rolled_back(&mut conn, changeset_id) {
                warn!(
                    "Failed to mark abandoned update changeset {} as rolled back: {}",
                    changeset_id, cleanup_err
                );
            }
            Err(err)
        }
    }
}

/// Show delta update statistics
pub async fn cmd_delta_stats(db_path: &str) -> Result<()> {
    info!("Showing delta update statistics");

    let conn = open_db(db_path)?;
    let total_stats = DeltaStats::get_total_stats(&conn)?;

    let all_stats = {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DeltaStats {
                id: Some(row.get(0)?),
                changeset_id: row.get(1)?,
                total_bytes_saved: row.get(2)?,
                deltas_applied: row.get(3)?,
                full_downloads: row.get(4)?,
                delta_failures: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if all_stats.is_empty() {
        println!("No delta statistics available");
        println!("Run 'conary update' to start tracking delta usage");
        return Ok(());
    }

    println!("=== Delta Update Statistics ===\n");
    println!("Total Statistics:");
    println!("  Delta updates applied: {}", total_stats.deltas_applied);
    println!("  Full downloads: {}", total_stats.full_downloads);
    println!("  Delta failures: {}", total_stats.delta_failures);

    let total_mb = total_stats.total_bytes_saved as f64 / 1_048_576.0;
    println!("  Total bandwidth saved: {:.2} MB", total_mb);

    let total_updates = total_stats.deltas_applied + total_stats.full_downloads;
    if total_updates > 0 {
        let success_rate = (total_stats.deltas_applied as f64 / total_updates as f64) * 100.0;
        println!("  Delta success rate: {:.1}%", success_rate);
    }

    println!("\nRecent Operations:");
    for (idx, stats) in all_stats.iter().take(10).enumerate() {
        if idx > 0 {
            println!();
        }

        let timestamp = stats.created_at.as_deref().unwrap_or("unknown");
        println!("  [Changeset {}] {}", stats.changeset_id, timestamp);
        println!("    Deltas applied: {}", stats.deltas_applied);
        println!("    Full downloads: {}", stats.full_downloads);

        if stats.delta_failures > 0 {
            println!("    Delta failures: {}", stats.delta_failures);
        }

        if stats.total_bytes_saved > 0 {
            let saved_mb = stats.total_bytes_saved as f64 / 1_048_576.0;
            println!("    Bandwidth saved: {:.2} MB", saved_mb);
        }
    }

    if all_stats.len() > 10 {
        println!("\n... and {} more operations", all_stats.len() - 10);
    }

    Ok(())
}

fn installed_troves_for_update(
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionUpdateTarget {
    name: String,
    version: String,
    architecture: Option<String>,
}

impl CollectionUpdateTarget {
    fn from_trove(trove: &Trove) -> Self {
        Self {
            name: trove.name.clone(),
            version: trove.version.clone(),
            architecture: trove.architecture.clone(),
        }
    }

    fn display(&self) -> String {
        match self.architecture.as_deref() {
            Some(arch) => format!("{} {} [{}]", self.name, self.version, arch),
            None => format!("{} {}", self.name, self.version),
        }
    }
}

/// Update all members of a collection/group (best-effort, per-package)
///
/// This updates all installed packages that are members of the specified collection.
/// Updates are applied one package at a time; earlier members remain updated even if
/// a later one fails.  Returns an error if any member fails to update.
/// If `security_only` is true, only applies security updates.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_update_group(
    name: &str,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: Option<DepMode>,
    yes: bool,
    legacy_replay: super::LegacyReplayOptions,
) -> Result<()> {
    info!("Updating collection: {}", name);
    let requested_dep_mode = dep_mode;
    let effective_dep_mode = requested_dep_mode.unwrap_or_else(resolve_default_dep_mode_from_model);
    let conn = open_db(db_path)?;
    let effective_source_policy = conary_core::repository::load_effective_policy(
        &conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
    let policy = effective_source_policy.resolution;
    let primary_flavor = effective_source_policy.primary_flavor;

    // Find the collection
    let troves = conary_core::db::models::Trove::find_by_name(&conn, name)?;
    let collection = troves
        .iter()
        .find(|t| t.trove_type == conary_core::db::models::TroveType::Collection)
        .ok_or_else(|| anyhow::anyhow!("Collection '{}' not found", name))?;

    let collection_id = collection
        .id
        .ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;
    let members =
        conary_core::db::models::CollectionMember::find_by_collection(&conn, collection_id)?;

    if members.is_empty() {
        println!("Collection '{}' has no members.", name);
        return Ok(());
    }

    // Find installed members that need updates
    let mut updates_to_apply: Vec<CollectionUpdateTarget> = Vec::new();
    let mut not_installed: Vec<String> = Vec::new();
    let mut adopted_updates_skipped = false;
    let mut security_metadata_unavailable: Vec<SecurityMetadataUnavailable> = Vec::new();
    let detected_pkg_mgr = SystemPackageManager::detect();

    for member in &members {
        let installed = Trove::find_by_name(&conn, &member.member_name)?
            .into_iter()
            .filter(|trove| trove.trove_type == TroveType::Package)
            .collect::<Vec<_>>();
        if installed.is_empty() {
            not_installed.push(member.member_name.clone());
            continue;
        }

        for trove in &installed {
            if trove.pinned {
                println!(
                    "  {} is pinned, skipping",
                    CollectionUpdateTarget::from_trove(trove).display()
                );
                continue;
            }

            let adopted_decision = if trove.install_source.is_adopted() {
                Some(adopted_update_decision(
                    trove,
                    effective_dep_mode,
                    requested_dep_mode,
                ))
            } else {
                None
            };

            if trove.install_source.is_adopted() {
                let native_manager = native_manager_for_trove(trove, detected_pkg_mgr);
                match adopted_decision.expect("adopted trove must have an update decision") {
                    AdoptedUpdateDecision::QueueTakeover => {}
                    AdoptedUpdateDecision::SkipNativeAuthority => {
                        println!(
                            "  {} is adopted; native authority owns updates: use '{}'",
                            CollectionUpdateTarget::from_trove(trove).display(),
                            native_manager.update_command(&trove.name)
                        );
                        adopted_updates_skipped = true;
                        continue;
                    }
                    AdoptedUpdateDecision::BlockCritical => {
                        println!(
                            "  {} is a critical adopted package; native authority remains required: use '{}'",
                            CollectionUpdateTarget::from_trove(trove).display(),
                            native_manager.update_command(&trove.name)
                        );
                        adopted_updates_skipped = true;
                        continue;
                    }
                }
            }

            let enforce_security_metadata = security_only
                && !matches!(
                    adopted_decision,
                    Some(
                        AdoptedUpdateDecision::SkipNativeAuthority
                            | AdoptedUpdateDecision::BlockCritical
                    )
                );
            match select_update_candidate(
                &conn,
                trove,
                enforce_security_metadata,
                &policy,
                primary_flavor,
            )? {
                UpdateCandidateSelection::Selected(_) => {
                    updates_to_apply.push(CollectionUpdateTarget::from_trove(trove));
                }
                UpdateCandidateSelection::NoEligibleUpdate => {}
                UpdateCandidateSelection::SecurityMetadataUnavailable(unavailable) => {
                    security_metadata_unavailable.push(unavailable);
                }
            }
        }
    }

    drop(conn);

    if !security_metadata_unavailable.is_empty() {
        print_security_metadata_unavailable(&security_metadata_unavailable);
        anyhow::bail!(security_metadata_unavailable_error(
            security_metadata_unavailable.len()
        ));
    }

    if !not_installed.is_empty() {
        println!(
            "Note: {} member(s) not installed: {}",
            not_installed.len(),
            not_installed.join(", ")
        );
    }

    if updates_to_apply.is_empty() {
        if adopted_updates_skipped {
            println!(
                "No Conary-managed updates available for collection '{}'; adopted package updates remain under native package-manager authority",
                name
            );
            println!(
                "Run 'conary --allow-live-system-mutation system adopt --refresh' after native package-manager changes before retrying Conary workflows."
            );
        } else if security_only {
            println!("No security updates available for collection '{}'", name);
        } else {
            println!("All members of collection '{}' are up to date", name);
        }
        return Ok(());
    }

    println!(
        "Updating {} package(s) from collection '{}':",
        updates_to_apply.len(),
        name
    );
    for target in &updates_to_apply {
        println!("  {}", target.display());
    }

    // Update each package
    let mut updated_count = 0;
    let mut failed_count = 0;

    for target in &updates_to_apply {
        println!("\nUpdating {}...", target.display());
        match cmd_update(
            Some(target.name.clone()),
            db_path,
            root,
            security_only,
            dry_run,
            no_scripts,
            sandbox_mode,
            requested_dep_mode,
            yes,
            Some(target.version.clone()),
            target.architecture.clone(),
            legacy_replay,
        )
        .await
        {
            Ok(()) => updated_count += 1,
            Err(e) => {
                eprintln!("  Failed to update {}: {}", target.display(), e);
                failed_count += 1;
            }
        }
    }

    println!("\nCollection update complete:");
    println!("  Updated: {} package(s)", updated_count);
    if failed_count > 0 {
        println!("  Failed: {} package(s)", failed_count);
        return Err(anyhow::anyhow!(
            "{} of {} package(s) in collection '{}' failed to update",
            failed_count,
            updates_to_apply.len(),
            name
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{create_test_db, seed_mixed_replatform_fixture};
    use super::*;
    use conary_core::db::models::{
        CanonicalPackage, CollectionMember, DistroPin, InstallSource, RepologyCacheEntry,
        Repository, Trove, TroveType,
    };
    use conary_core::filesystem::{CasStore, object_path};
    use conary_core::model::ReplatformBlockedReason;
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
    fn package_specific_update_requires_selector_for_ambiguous_variants() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        for arch in ["x86_64", "aarch64"] {
            let mut trove = Trove::new("demo".to_string(), "1.0.0".to_string(), TroveType::Package);
            trove.architecture = Some(arch.to_string());
            trove.insert(&conn).unwrap();
        }

        let err = installed_troves_for_update(&conn, Some("demo".to_string()), None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Multiple installed variants"), "{err}");
        assert!(err.contains("--arch"), "{err}");

        let selected = installed_troves_for_update(
            &conn,
            Some("demo".to_string()),
            Some("1.0.0".to_string()),
            Some("aarch64".to_string()),
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].architecture.as_deref(), Some("aarch64"));
    }

    #[test]
    fn update_selector_without_package_refuses() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();

        let err = installed_troves_for_update(&conn, None, None, Some("x86_64".to_string()))
            .unwrap_err()
            .to_string();

        assert!(err.contains("A package name is required"), "{err}");
    }

    #[tokio::test]
    async fn collection_update_preserves_member_variant_selector() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        let mut repo = Repository::new(
            "variant-repo".to_string(),
            "https://example.test/variant".to_string(),
        );
        repo.gpg_check = false;
        repo.gpg_strict = false;
        repo.default_strategy_distro = Some("fedora-44".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        let mut collection = Trove::new(
            "base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
        );
        let collection_id = collection.insert(&conn).unwrap();
        CollectionMember::new(collection_id, "demo".to_string())
            .insert(&conn)
            .unwrap();

        for arch in ["x86_64", "aarch64"] {
            let mut installed = Trove::new_with_source(
                "demo".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            installed.architecture = Some(arch.to_string());
            installed.source_distro = Some("fedora-44".to_string());
            installed.version_scheme = Some("rpm".to_string());
            installed.installed_from_repository_id = Some(repo_id);
            installed.insert(&conn).unwrap();

            let mut candidate = RepositoryPackage::new(
                repo_id,
                "demo".to_string(),
                "1.0.1".to_string(),
                format!("sha256:demo-{arch}"),
                123,
                format!("https://example.test/variant/demo-1.0.1-{arch}.ccs"),
            );
            candidate.architecture = Some(arch.to_string());
            candidate.distro = Some("fedora-44".to_string());
            candidate.version_scheme = Some("rpm".to_string());
            candidate.insert(&conn).unwrap();
        }
        drop(conn);

        let result = cmd_update_group(
            "base",
            &db_path,
            "/",
            false,
            true,
            false,
            SandboxMode::None,
            None,
            true,
            crate::commands::LegacyReplayOptions::default(),
        )
        .await;

        assert!(
            result.is_ok(),
            "collection update should preserve member variant selectors: {:?}",
            result
        );
    }

    #[test]
    fn test_source_policy_update_context_with_affinity() {
        let pin = DistroPin {
            id: Some(1),
            distro: "arch".to_string(),
            mixing_policy: "strict".to_string(),
            created_at: "2026-03-12".to_string(),
        };
        let affinities = vec![
            SystemAffinity {
                distro: "arch".to_string(),
                package_count: 10,
                percentage: 25.0,
            },
            SystemAffinity {
                distro: "fedora-44".to_string(),
                package_count: 30,
                percentage: 75.0,
            },
        ];

        let context = source_policy_update_context(Some(&pin), &affinities, Some(0)).unwrap();

        assert!(context.contains("Package-level realignment candidates"));
        assert!(context.contains("0."));
        assert!(context.contains("Active source policy pin: arch (strict)"));
        assert!(context.contains("10 installed package(s) already align"));
        assert!(context.contains("30 may need source realignment"));
    }

    #[test]
    fn test_source_policy_update_context_without_affinity_data() {
        let pin = DistroPin {
            id: Some(1),
            distro: "arch".to_string(),
            mixing_policy: "strict".to_string(),
            created_at: "2026-03-12".to_string(),
        };

        let context = source_policy_update_context(Some(&pin), &[], None).unwrap();

        assert!(context.contains("Replatform estimate unavailable"));
        assert!(context.contains("no source affinity data yet"));
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
    fn update_repository_install_provenance_uses_selected_package_metadata() {
        let mut repo = Repository::new(
            "slice-d-local-update".to_string(),
            "https://example.test/slice-d".to_string(),
        );
        repo.default_strategy_distro = Some("fedora".to_string());
        repo.id = Some(42);

        let mut package = RepositoryPackage::new(
            42,
            "phase4-runtime-fixture".to_string(),
            "1.0.1-1".to_string(),
            "sha256:fixture".to_string(),
            123,
            "https://example.test/phase4-runtime-fixture-1.0.1.rpm".to_string(),
        );
        package.architecture = Some("x86_64".to_string());
        package.distro = Some("fedora".to_string());
        package.version_scheme = Some("rpm".to_string());

        let provenance = repository_install_provenance_from_package(&package, &repo).unwrap();

        assert_eq!(provenance.repository_id, 42);
        assert_eq!(provenance.source_distro.as_deref(), Some("fedora"));
        assert_eq!(provenance.version_scheme.as_deref(), Some("rpm"));
    }

    #[test]
    fn selected_update_resolution_bypasses_local_cas_shortcut() {
        let temp = tempfile::tempdir().unwrap();
        let keyring_dir = temp.path().join("keyrings");
        let repo = Repository::new(
            "slice-d-source-switch".to_string(),
            "https://example.test/slice-d".to_string(),
        );
        let mut package = RepositoryPackage::new(
            42,
            "phase4-runtime-fixture".to_string(),
            "1.0.1-1".to_string(),
            "sha256:fixture".to_string(),
            123,
            "https://example.test/phase4-runtime-fixture-1.0.1.rpm".to_string(),
        );
        package.architecture = Some("x86_64".to_string());

        let options = resolution_options_for_selected_update(
            &package,
            &repo,
            temp.path(),
            &keyring_dir,
            &ResolutionPolicy::new(),
            Some(RepositoryDependencyFlavor::Rpm),
        );

        assert!(options.skip_cas);
        assert_eq!(options.version.as_deref(), Some("1.0.1-1"));
        assert_eq!(options.repository.as_deref(), Some("slice-d-source-switch"));
        assert_eq!(options.architecture.as_deref(), Some("x86_64"));
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
    fn partial_update_failure_message_is_not_clean_success() {
        let failures = vec![UpdatePackageFailure {
            package: "broken".to_string(),
            version: "2.0.0".to_string(),
            reason: "resolver failed".to_string(),
        }];

        let message = update_required_failure_message(&failures, 2).unwrap();

        assert!(message.contains("1 of 2"));
        assert!(message.contains("broken"));
        assert!(!message.contains("All packages are up to date"));
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

    #[test]
    fn test_update_replatform_planning_surfaces_mixed_execution_states() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_mixed_replatform_fixture(&conn);
        DistroPin::set(&conn, "arch", "strict").unwrap();

        let pin = DistroPin::get_current(&conn)
            .unwrap()
            .expect("expected source pin");
        let snapshot = source_policy_replatform_snapshot(&conn, &pin.distro).unwrap();
        let state = capture_current_state(&conn).unwrap();
        let actions = planned_replatform_actions(&snapshot, &state);
        let plan = replatform_execution_plan(&conn, &actions)
            .unwrap()
            .expect("expected replatform execution plan");

        assert_eq!(plan.transactions.len(), 3);

        let bash = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "bash")
            .expect("expected bash transaction");
        assert!(!bash.executable);
        assert_eq!(bash.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(
            bash.blocked_reason,
            Some(ReplatformBlockedReason::AnyVersionRouteOnly)
        );

        let vim = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "vim")
            .expect("expected vim transaction");
        assert!(vim.executable);
        assert_eq!(vim.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(vim.blocked_reason, None);

        let zsh = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "zsh")
            .expect("expected zsh transaction");
        assert!(!zsh.executable);
        assert_eq!(zsh.install_route.as_deref(), Some("default:legacy"));
        assert_eq!(
            zsh.blocked_reason,
            Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
        );
    }

    #[test]
    fn test_render_replatform_action_preview_lists_examples() {
        let actions = vec![
            DiffAction::ReplatformReplace {
                package: "bash".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "5.1.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "5.2.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(11),
            },
            DiffAction::ReplatformReplace {
                package: "vim".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(22),
            },
            DiffAction::ReplatformReplace {
                package: "zsh".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "5.8.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "5.9.1".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(33),
            },
            DiffAction::ReplatformReplace {
                package: "curl".to_string(),
                current_distro: Some("fedora-44".to_string()),
                target_distro: "arch".to_string(),
                current_version: "8.7.0".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "8.8.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(44),
            },
        ];

        let rendered = render_replatform_action_preview(&actions).unwrap();

        assert!(rendered.contains("Replatform bash"));
        assert!(rendered.contains("Replatform vim"));
        assert!(rendered.contains("Replatform zsh"));
        assert!(rendered.contains("+1 more"));
    }

    #[test]
    fn delta_result_uses_verified_cas_retrieval() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas = CasStore::new(temp_dir.path()).unwrap();
        let expected_hash = conary_core::hash::sha256(b"expected-bytes");
        let corrupted_path = object_path(temp_dir.path(), &expected_hash).unwrap();
        std::fs::create_dir_all(corrupted_path.parent().unwrap()).unwrap();
        std::fs::write(&corrupted_path, b"corrupted-bytes").unwrap();

        assert!(read_delta_result_from_cas(&cas, &expected_hash).is_err());
    }

    #[test]
    fn mark_pending_changeset_rolled_back_updates_pending_rows() {
        let (_temp, db_path) = create_test_db();
        let mut conn = rusqlite::Connection::open(&db_path).unwrap();
        let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = conary_core::db::models::Changeset::new("test update".to_string());
            changeset.insert(tx)
        })
        .unwrap();

        assert!(mark_pending_changeset_rolled_back(&mut conn, changeset_id).unwrap());

        let changeset = conary_core::db::models::Changeset::find_by_id(&conn, changeset_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            changeset.status,
            conary_core::db::models::ChangesetStatus::RolledBack
        );
    }

    #[test]
    fn mark_pending_changeset_rolled_back_leaves_applied_rows_alone() {
        let (_temp, db_path) = create_test_db();
        let mut conn = rusqlite::Connection::open(&db_path).unwrap();
        let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset =
                conary_core::db::models::Changeset::new("applied update".to_string());
            let id = changeset.insert(tx)?;
            changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(id)
        })
        .unwrap();

        assert!(!mark_pending_changeset_rolled_back(&mut conn, changeset_id).unwrap());

        let changeset = conary_core::db::models::Changeset::find_by_id(&conn, changeset_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            changeset.status,
            conary_core::db::models::ChangesetStatus::Applied
        );
    }

    mod adopted_update_tests {
        use super::*;

        fn adopted_trove(name: &str) -> Trove {
            let mut trove = Trove::new_with_source(
                name.to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::AdoptedFull,
            );
            trove.version_scheme = Some("debian".to_string());
            trove
        }

        #[test]
        fn adopted_updates_do_not_take_over_without_explicit_takeover_mode() {
            let trove = adopted_trove("curl");

            assert_eq!(
                adopted_update_decision(&trove, DepMode::Takeover, None),
                AdoptedUpdateDecision::SkipNativeAuthority
            );
        }

        #[test]
        fn adopted_updates_take_over_only_under_explicit_takeover_mode() {
            let trove = adopted_trove("curl");

            assert_eq!(
                adopted_update_decision(&trove, DepMode::Takeover, Some(DepMode::Takeover)),
                AdoptedUpdateDecision::QueueTakeover
            );
            assert_eq!(
                adopted_update_decision(&trove, DepMode::Takeover, None),
                AdoptedUpdateDecision::SkipNativeAuthority
            );
        }

        #[test]
        fn critical_adopted_packages_are_blocked_even_under_takeover_mode() {
            let trove = adopted_trove("glibc");

            assert_eq!(
                adopted_update_decision(&trove, DepMode::Takeover, Some(DepMode::Takeover)),
                AdoptedUpdateDecision::BlockCritical
            );
        }

        #[test]
        fn adopted_updates_are_not_queued_under_satisfy_or_adopt() {
            let trove = adopted_trove("curl");

            for dep_mode in [DepMode::Satisfy, DepMode::Adopt] {
                assert_eq!(
                    adopted_update_decision(&trove, dep_mode, Some(dep_mode)),
                    AdoptedUpdateDecision::SkipNativeAuthority
                );
            }
        }

        #[test]
        fn adopted_update_guidance_uses_recorded_version_scheme_before_live_detection() {
            let mut trove = adopted_trove("curl");
            trove.version_scheme = Some("arch".to_string());

            assert_eq!(
                native_manager_for_trove(&trove, conary_core::packages::SystemPackageManager::Rpm),
                conary_core::packages::SystemPackageManager::Pacman
            );
        }

        #[test]
        fn adopted_update_skip_message_is_not_generic_up_to_date_text() {
            let message = no_update_message(false, true);

            assert!(!message.contains("All packages are up to date"));
            assert!(message.contains("native package-manager authority"));
        }
    }
}
