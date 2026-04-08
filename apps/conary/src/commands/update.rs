// src/commands/update.rs
//! Update, pinning, and delta statistics commands

use super::install::DepMode;
use super::open_db;
use super::progress::{UpdatePhase, UpdateProgress};
use super::{SandboxMode, cmd_install};
use anyhow::{Context, Result};
use chrono::Utc;
use conary_core::db::models::{
    DeltaStats, DistroPin, PackageDelta, RepologyCacheEntry, Repository, RepositoryPackage,
    SystemAffinity, Trove,
};
use conary_core::db::paths::objects_dir;
use conary_core::delta::DeltaApplier;
use conary_core::model::{
    DiffAction, capture_current_state, planned_replatform_actions, replatform_execution_plan,
    source_policy_replatform_snapshot,
};
use conary_core::repository::{
    self, DownloadOptions, LatestSignal, PackageSelector, PackageSource, ResolutionOptions,
    SelectionOptions,
    dependency_model::RepositoryDependencyFlavor,
    resolution_policy::{ResolutionPolicy, SelectionMode},
    resolve_package,
    versioning::{VersionScheme, compare_mixed_repo_versions, infer_version_scheme},
};
use std::cmp::Ordering;
use std::path::Path;
use tracing::{debug, info, warn};

fn read_delta_result_from_cas(
    cas: &conary_core::filesystem::CasStore,
    hash: &str,
) -> Result<Vec<u8>> {
    cas.retrieve(hash)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("failed to retrieve verified delta result from CAS: {hash}"))
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
fn is_repo_version_newer(trove: &Trove, repo: &Repository, repo_version: &str) -> bool {
    let Some(ordering) = compare_mixed_repo_versions(
        trove_version_scheme(trove),
        &trove.version,
        infer_version_scheme(repo).unwrap_or(VersionScheme::Rpm),
        repo_version,
    ) else {
        warn!(
            "Could not compare versions for {}: {} vs {}, skipping",
            trove.name, repo_version, trove.version
        );
        return false;
    };

    if ordering != Ordering::Less {
        debug!(
            "Skipping {} {} (installed {} is same or newer)",
            trove.name, repo_version, trove.version
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
) -> Result<Option<SelectedUpdateCandidate>> {
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
        if security_only && !candidate.package.is_security_update {
            continue;
        }

        let same_source =
            candidate_matches_installed_source(trove, &candidate.package, &candidate.repository);
        let newer_in_scheme =
            is_repo_version_newer(trove, &candidate.repository, &candidate.package.version);
        let allow_cross_source_latest = policy.selection_mode == SelectionMode::Latest
            && !same_source
            && candidate_has_positive_latest_signal(
                conn,
                &candidate.package,
                &candidate.repository,
            )?;

        if newer_in_scheme || allow_cross_source_latest {
            eligible.push(candidate);
        }
    }

    if eligible.is_empty() {
        return Ok(None);
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

    Ok(Some(SelectedUpdateCandidate {
        package: selected.package,
        repository: selected.repository,
        source_switch,
    }))
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

fn find_installed_trove(conn: &rusqlite::Connection, package_name: &str) -> Result<(Trove, i64)> {
    let trove = Trove::find_one_by_name(conn, package_name)?
        .ok_or_else(|| anyhow::anyhow!("Package '{}' is not installed", package_name))?;
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Package '{}' has no database ID", package_name))?;
    Ok((trove, trove_id))
}

/// Pin a package to prevent updates and removal
pub async fn cmd_pin(package_name: &str, db_path: &str) -> Result<()> {
    info!("Pinning package: {}", package_name);
    let conn = open_db(db_path)?;
    let (trove, trove_id) = find_installed_trove(&conn, package_name)?;

    if trove.pinned {
        println!("Package '{}' is already pinned", package_name);
        return Ok(());
    }

    Trove::pin(&conn, trove_id)?;
    println!(
        "Pinned package '{}' at version {}",
        package_name, trove.version
    );
    println!("This package will be skipped during updates and cannot be removed until unpinned.");

    Ok(())
}

/// Unpin a package to allow updates and removal
pub async fn cmd_unpin(package_name: &str, db_path: &str) -> Result<()> {
    info!("Unpinning package: {}", package_name);
    let conn = open_db(db_path)?;
    let (trove, trove_id) = find_installed_trove(&conn, package_name)?;

    if !trove.pinned {
        println!("Package '{}' is not pinned", package_name);
        return Ok(());
    }

    Trove::unpin(&conn, trove_id)?;
    println!(
        "Unpinned package '{}' (version {})",
        package_name, trove.version
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
/// If `security_only` is true, only applies security updates (critical/important severity).
pub async fn cmd_update(
    package: Option<String>,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    sandbox_mode: SandboxMode,
    dep_mode: DepMode,
    yes: bool,
) -> Result<()> {
    if security_only {
        info!("Checking for security updates only");
    } else {
        info!("Checking for package updates");
    }

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

    let installed_troves = if let Some(pkg_name) = package {
        Trove::find_by_name(&conn, &pkg_name)?
    } else {
        Trove::list_all(&conn)?
    };

    if installed_troves.is_empty() {
        println!("No packages to update");
        return Ok(());
    }

    // Collect updates with their repository info (needed for GPG verification)
    let mut updates_available: Vec<(Trove, SelectedUpdateCandidate)> = Vec::new();
    let mut pinned_skipped: Vec<String> = Vec::new();

    let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
    let mut adopted_skipped: Vec<String> = Vec::new();

    for trove in &installed_troves {
        // Skip pinned packages
        if trove.pinned {
            pinned_skipped.push(trove.name.clone());
            continue;
        }

        let Some(selected) =
            select_update_candidate(&conn, trove, security_only, &policy, primary_flavor)?
        else {
            continue;
        };

        // For adopted packages, behavior depends on dep-mode.
        // The ownership ladder: AdoptedTrack -> AdoptedFull -> Taken/Repository.
        // In satisfy mode, adopted packages are left to the system PM.
        // In adopt mode, we track the new version metadata.
        // In takeover mode, we download the CCS from Remi and take full ownership.
        if trove.install_source.is_adopted() {
            match dep_mode {
                DepMode::Satisfy => {
                    println!(
                        "  {} {} -> {} (adopted as {}, use --dep-mode takeover to update via Conary)",
                        trove.name,
                        trove.version,
                        selected.package.version,
                        trove.install_source.as_str(),
                    );
                    adopted_skipped.push(trove.name.clone());
                    continue;
                }
                DepMode::Adopt => {
                    println!(
                        "  {} {} -> {} (adopted, tracking update)",
                        trove.name, trove.version, selected.package.version
                    );
                    adopted_skipped.push(trove.name.clone());
                    continue;
                }
                DepMode::Takeover => {
                    if super::install::is_package_blocked(&trove.name) {
                        println!(
                            "  {} {} (blocked - critical system package, skipping)",
                            trove.name, trove.version
                        );
                        adopted_skipped.push(trove.name.clone());
                        continue;
                    }
                    println!(
                        "  {} {} -> {} (taking over from system PM)",
                        trove.name, trove.version, selected.package.version
                    );
                }
            }
        }

        let security_marker = if selected.package.is_security_update {
            format!(
                " [{}]",
                selected.package.severity.as_deref().unwrap_or("security")
            )
        } else {
            String::new()
        };
        info!(
            "Update available: {} {} -> {}{}",
            trove.name, trove.version, selected.package.version, security_marker
        );
        updates_available.push((trove.clone(), selected));
    }

    // Report pinned packages that were skipped
    if !pinned_skipped.is_empty() {
        println!(
            "Skipping {} pinned package(s): {}",
            pinned_skipped.len(),
            pinned_skipped.join(", ")
        );
    }

    // Report adopted packages that were skipped (only in satisfy/adopt modes)
    if !adopted_skipped.is_empty() {
        let sample: Vec<&str> = adopted_skipped.iter().take(5).map(|s| s.as_str()).collect();
        let suffix = if adopted_skipped.len() > 5 {
            format!(", ... and {} more", adopted_skipped.len() - 5)
        } else {
            String::new()
        };
        match dep_mode {
            DepMode::Satisfy => {
                println!(
                    "Skipping {} adopted package(s) (use '{}' or --dep-mode takeover): {}{}",
                    adopted_skipped.len(),
                    pkg_mgr.update_command("<package>"),
                    sample.join(", "),
                    suffix
                );
            }
            DepMode::Adopt => {
                println!(
                    "Tracking {} adopted package(s) (metadata only): {}{}",
                    adopted_skipped.len(),
                    sample.join(", "),
                    suffix
                );
            }
            DepMode::Takeover => {
                // In takeover mode, adopted_skipped only contains blocked packages
                println!(
                    "Blocked {} critical package(s) from takeover: {}{}",
                    adopted_skipped.len(),
                    sample.join(", "),
                    suffix
                );
            }
        }
    }

    if updates_available.is_empty() {
        if security_only {
            println!("No security updates available");
        } else {
            println!("All packages are up to date");
        }
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
        let security_marker = if selected.package.is_security_update {
            format!(
                " [{}]",
                selected.package.severity.as_deref().unwrap_or("security")
            )
        } else {
            String::new()
        };
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
    let mut delta_updates: Vec<(Trove, RepositoryPackage, PackageDelta)> = Vec::new();
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
            delta_updates.push((trove, repo_pkg, delta_info));
        } else {
            full_updates.push((trove, repo_pkg, repo));
        }
    }

    let mut total_bytes_saved = 0i64;
    let mut deltas_applied = 0i32;
    let mut full_downloads = 0i32;
    let mut delta_failures = 0i32;
    let mut had_failures = false;

    // Save counts before consuming the vectors
    let delta_count = delta_updates.len();
    let initial_full_count = full_updates.len();

    // Only create a changeset when there is actual work to do
    if delta_count + initial_full_count == 0 {
        println!("No updates to apply.");
        return Ok(());
    }

    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new(format!(
            "Update {} package(s)",
            delta_count + initial_full_count
        ));
        changeset.insert(tx)
    })?;

    let update_result: Result<()> = async {
        // Phase 2: Download and apply deltas (sequential - requires CAS access)
        for (trove, repo_pkg, delta_info) in delta_updates {
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
                                                sandbox_mode,
                                                dep_mode: Some(dep_mode),
                                                yes,
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
                                had_failures = true;
                                if let Ok(Some(repo)) =
                                    Repository::find_by_id(&conn, repo_pkg.repository_id)
                                {
                                    full_updates.push((trove, repo_pkg, repo));
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "  Delta application failed: {}, will download full package",
                                e
                            );
                            delta_failures += 1;
                            had_failures = true;
                            // Get repository for fallback download
                            if let Ok(Some(repo)) =
                                Repository::find_by_id(&conn, repo_pkg.repository_id)
                            {
                                full_updates.push((trove, repo_pkg, repo));
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&actual_delta_path);
                }
                Err(e) => {
                    warn!("  Delta download failed: {}, will download full package", e);
                    delta_failures += 1;
                    had_failures = true;
                    // Get repository for fallback download
                    if let Ok(Some(repo)) = Repository::find_by_id(&conn, repo_pkg.repository_id) {
                        full_updates.push((trove, repo_pkg, repo));
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

                // Build resolution options
                let options = ResolutionOptions {
                    version: Some(repo_pkg.version.clone()),
                    repository: Some(repo.name.clone()),
                    architecture: repo_pkg.architecture.clone(),
                    output_dir: Some(temp_dir.clone()),
                    gpg_options: if repo.gpg_check {
                        Some(DownloadOptions {
                            gpg_check: true,
                            gpg_strict: repo.gpg_strict,
                            keyring_dir: keyring_dir.clone(),
                            repository_name: repo.name.clone(),
                        })
                    } else {
                        None
                    },
                    skip_cas: false,
                    policy: Some(policy.clone()),
                    is_root: false,
                    primary_flavor,
                };

                // Use unified resolver - respects remi/binary/recipe strategies
                let source = match resolve_package(&conn, &trove.name, &options).await {
                    Ok(source) => source,
                    Err(e) => {
                        progress.fail_package(&trove.name, &e.to_string());
                        warn!("Failed to resolve {}: {}", trove.name, e);
                        had_failures = true;
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
                        had_failures = true;
                        warn!(
                            "LocalCas resolution not yet implemented for {}: {}",
                            trove.name, hash
                        );
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
                        sandbox_mode,
                        dep_mode: Some(dep_mode),
                        yes,
                        ..Default::default()
                    },
                )
                .await
                {
                    progress.fail_package(&trove.name, &e.to_string());
                    warn!("  Package installation failed: {}", e);
                    had_failures = true;
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
            } else if had_failures {
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

/// Update all members of a collection/group (best-effort, per-package)
///
/// This updates all installed packages that are members of the specified collection.
/// Updates are applied one package at a time; earlier members remain updated even if
/// a later one fails.  Returns an error if any member fails to update.
/// If `security_only` is true, only applies security updates.
pub async fn cmd_update_group(
    name: &str,
    db_path: &str,
    root: &str,
    security_only: bool,
    dry_run: bool,
    sandbox_mode: SandboxMode,
    dep_mode: DepMode,
    yes: bool,
) -> Result<()> {
    info!("Updating collection: {}", name);
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
    let mut updates_to_apply: Vec<String> = Vec::new();
    let mut not_installed: Vec<String> = Vec::new();

    for member in &members {
        let installed = Trove::find_by_name(&conn, &member.member_name)?;
        if installed.is_empty() {
            not_installed.push(member.member_name.clone());
            continue;
        }

        let trove = &installed[0];
        if trove.pinned {
            println!("  {} is pinned, skipping", member.member_name);
            continue;
        }

        if select_update_candidate(&conn, trove, security_only, &policy, primary_flavor)?.is_some()
        {
            updates_to_apply.push(member.member_name.clone());
        }
    }

    drop(conn);

    if !not_installed.is_empty() {
        println!(
            "Note: {} member(s) not installed: {}",
            not_installed.len(),
            not_installed.join(", ")
        );
    }

    if updates_to_apply.is_empty() {
        if security_only {
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
    for pkg in &updates_to_apply {
        println!("  {}", pkg);
    }

    // Update each package
    let mut updated_count = 0;
    let mut failed_count = 0;

    for pkg_name in &updates_to_apply {
        println!("\nUpdating {}...", pkg_name);
        match cmd_update(
            Some(pkg_name.clone()),
            db_path,
            root,
            security_only,
            dry_run,
            sandbox_mode,
            dep_mode,
            yes,
        )
        .await
        {
            Ok(()) => updated_count += 1,
            Err(e) => {
                eprintln!("  Failed to update {}: {}", pkg_name, e);
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
        CanonicalPackage, DistroPin, InstallSource, RepologyCacheEntry, Repository, Trove,
        TroveType,
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
        fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
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

        RepologyCacheEntry::insert_or_replace(
            conn,
            &RepologyCacheEntry {
                project_name: "demo".to_string(),
                distro: "fedora-43".to_string(),
                distro_name: "demo".to_string(),
                version: Some("1.1.0-1.fc43".to_string()),
                status: Some("outdated".to_string()),
                fetched_at: "2026-04-07T00:00:00Z".to_string(),
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
                fetched_at: "2026-04-07T00:00:00Z".to_string(),
            },
        )
        .unwrap();

        let mut installed = Trove::new_with_source(
            "demo".to_string(),
            "1.0.0-1.fc43".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-43".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(conn).unwrap();

        let mut fedora_candidate = RepositoryPackage::new(
            fedora_repo_id,
            "demo".to_string(),
            "1.1.0-1.fc43".to_string(),
            "sha256:fedora-demo".to_string(),
            123,
            "https://example.test/fedora/demo-1.1.0-1.fc43.rpm".to_string(),
        );
        fedora_candidate.architecture = Some("x86_64".to_string());
        fedora_candidate.distro = Some("fedora-43".to_string());
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
                distro: "fedora-43".to_string(),
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

        assert!(is_repo_version_newer(&trove, &repo, "1.0"));
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

        assert!(is_repo_version_newer(&trove, &repo, "1.0-2"));
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
        assert_eq!(source_switch.from_distro, "fedora-43");
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
        assert!(preview.contains("fedora-43"));
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
        assert_eq!(selected.package.version, "1.1.0-1.fc43");
        assert!(selected.source_switch.is_none());
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
        assert_eq!(selected.package.version, "1.1.0-1.fc43");
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
                current_distro: Some("fedora-43".to_string()),
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
                package: "zsh".to_string(),
                current_distro: Some("fedora-43".to_string()),
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
                current_distro: Some("fedora-43".to_string()),
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
}
