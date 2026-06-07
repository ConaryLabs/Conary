// src/commands/update/package.rs

//! Single-package update command execution.

use super::super::install::{
    CcsTransactionInstallOptions, ComponentSelection, DepMode,
    repository_install_provenance_from_package, resolve_default_dep_mode_from_model,
};
use super::super::progress::{UpdatePhase, UpdateProgress};
use super::super::{
    InstallOptions, InstalledPackageSelector, LegacyReplayOptions, SandboxMode, cmd_install,
    open_db, resolve_installed_package,
};
use super::adopted_authority::{
    AdoptedUpdateDecision, AdoptedUpdateSkip, AdoptedUpdateSkipReason, adopted_update_decision,
    native_manager_for_trove, no_update_message, render_adopted_skip_sample,
};
use super::selection::{
    SecurityMetadataUnavailable, SelectedUpdateCandidate, UpdateCandidateSelection,
    print_security_metadata_unavailable, print_source_switch_preview,
    render_security_update_marker, requires_source_switch_confirmation,
    security_metadata_unavailable_error, select_update_candidate,
};
use super::source_policy::print_source_policy_update_preview;
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::db::models::{DeltaStats, PackageDelta, Repository, RepositoryPackage, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::delta::DeltaApplier;
use conary_core::packages::{PackageFormat, SystemPackageManager};
use conary_core::repository::{
    self, DownloadOptions, PackageSource, ResolutionOptions,
    dependency_model::RepositoryDependencyFlavor, resolution_policy::ResolutionPolicy,
    resolve_package,
};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdatePackageFailure {
    package: String,
    version: String,
    reason: String,
}

struct PreparedFullUpdate {
    trove: Trove,
    repo_pkg: RepositoryPackage,
    repo: Repository,
    pkg_path: PathBuf,
    _source: PackageSource,
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

#[allow(clippy::too_many_arguments)]
async fn prepare_full_updates_before_changeset(
    conn: &rusqlite::Connection,
    full_updates: Vec<(Trove, RepositoryPackage, Repository)>,
    db_path: &str,
    root: &str,
    temp_dir: &Path,
    keyring_dir: &Path,
    policy: &ResolutionPolicy,
    primary_flavor: Option<RepositoryDependencyFlavor>,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    legacy_replay: LegacyReplayOptions,
) -> Result<Vec<PreparedFullUpdate>> {
    let mut prepared = Vec::with_capacity(full_updates.len());

    for (trove, repo_pkg, repo) in full_updates {
        let options = resolution_options_for_selected_update(
            &repo_pkg,
            &repo,
            temp_dir,
            keyring_dir,
            policy,
            primary_flavor,
        );

        let source = resolve_package(conn, &trove.name, &options)
            .await
            .with_context(|| {
                format!(
                    "failed to resolve selected update package {} {}",
                    trove.name, repo_pkg.version
                )
            })?;
        let pkg_path = source
            .path()
            .ok_or_else(|| anyhow::anyhow!("LocalCas not yet supported for {}", trove.name))?
            .to_path_buf();

        preflight_prepared_full_update_legacy_replay(
            conn,
            &trove,
            &repo_pkg,
            &repo,
            &pkg_path,
            db_path,
            root,
            no_scripts,
            sandbox_mode,
            legacy_replay,
        )?;

        prepared.push(PreparedFullUpdate {
            trove,
            repo_pkg,
            repo,
            pkg_path,
            _source: source,
        });
    }

    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
fn preflight_prepared_full_update_legacy_replay(
    conn: &rusqlite::Connection,
    trove: &Trove,
    repo_pkg: &RepositoryPackage,
    repo: &Repository,
    pkg_path: &Path,
    db_path: &str,
    root: &str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    legacy_replay: LegacyReplayOptions,
) -> Result<()> {
    if pkg_path.extension().and_then(|ext| ext.to_str()) != Some("ccs") {
        return Ok(());
    }

    let pkg = CcsPackage::parse(&pkg_path.to_string_lossy())
        .with_context(|| format!("failed to parse selected update CCS {}", pkg_path.display()))?;
    let ccs_opts = CcsTransactionInstallOptions {
        db_path,
        root,
        dry_run: false,
        defer_generation: false,
        no_scripts,
        sandbox_mode,
        allow_downgrade: false,
        reinstall: false,
        selection_reason: Some("Updated by conary update"),
        component_selection: ComponentSelection::Defaults,
        selected_manifest_components: None,
        repository_provenance: Some(repository_install_provenance_from_package(repo_pkg, repo)?),
        legacy_replay,
    };

    let mut state = super::super::install::plan_ccs_fresh_install_legacy_replay(
        conn,
        pkg.manifest().legacy_scriptlets.as_ref(),
        &ccs_opts,
        true,
    )?;
    let old_state = super::super::install::plan_ccs_old_installed_upgrade_legacy_replay(
        conn,
        Some(trove),
        &ccs_opts,
    )?;
    super::super::install::merge_old_upgrade_legacy_replay_state(&mut state, old_state);

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn install_options_for_update<'a>(
    db_path: &'a str,
    root: &'a str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    dep_mode: DepMode,
    yes: bool,
    legacy_replay: LegacyReplayOptions,
    repo_pkg: &RepositoryPackage,
    repo: &Repository,
) -> Result<InstallOptions<'a>> {
    Ok(InstallOptions {
        db_path,
        root,
        no_scripts,
        sandbox_mode,
        dep_mode: Some(dep_mode),
        yes,
        legacy_replay,
        repository_provenance: Some(repository_install_provenance_from_package(repo_pkg, repo)?),
        ..Default::default()
    })
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
    legacy_replay: LegacyReplayOptions,
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
        print_source_policy_update_preview(&conn)?;
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
                "Run 'conary system adopt --refresh' after native package-manager changes before retrying Conary workflows."
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

    let delta_admission_updates = delta_updates
        .iter()
        .map(|(trove, repo_pkg, repo, _)| (trove.clone(), repo_pkg.clone(), repo.clone()))
        .collect();
    let prepared_delta_admissions = prepare_full_updates_before_changeset(
        &conn,
        delta_admission_updates,
        db_path,
        root,
        &temp_dir,
        &keyring_dir,
        &policy,
        primary_flavor,
        no_scripts,
        sandbox_mode,
        legacy_replay,
    )
    .await?;
    for prepared in prepared_delta_admissions {
        let _ = std::fs::remove_file(&prepared.pkg_path);
    }

    let prepared_full_updates = prepare_full_updates_before_changeset(
        &conn,
        full_updates,
        db_path,
        root,
        &temp_dir,
        &keyring_dir,
        &policy,
        primary_flavor,
        no_scripts,
        sandbox_mode,
        legacy_replay,
    )
    .await?;
    let mut full_updates: Vec<(Trove, RepositoryPackage, Repository)> = Vec::new();

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
                                            InstallOptions {
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
        if !prepared_full_updates.is_empty() || !full_updates.is_empty() {
            let total_to_install = (prepared_full_updates.len() + full_updates.len()) as u64;
            let mut progress = UpdateProgress::new(total_to_install);

            progress.set_status("Installing packages...");

            for PreparedFullUpdate {
                trove,
                repo_pkg,
                repo,
                pkg_path,
                _source,
            } in prepared_full_updates
            {
                info!("Installing prepared update {} from {}", trove.name, repo.name);
                progress.set_phase(&trove.name, UpdatePhase::Installing);

                let path_str = pkg_path.to_string_lossy().to_string();

                if let Err(e) = cmd_install(
                    &path_str,
                    install_options_for_update(
                        db_path,
                        root,
                        no_scripts,
                        sandbox_mode,
                        dep_mode,
                        yes,
                        legacy_replay,
                        &repo_pkg,
                        &repo,
                    )?,
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
                    install_options_for_update(
                        db_path,
                        root,
                        no_scripts,
                        sandbox_mode,
                        dep_mode,
                        yes,
                        legacy_replay,
                        &repo_pkg,
                        &repo,
                    )?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::ccs::manifest::{CcsManifest, Platform};
    use conary_core::db::models::{
        Changeset, ChangesetStatus, DistroPin, InstallSource, PackageDelta, PackageResolution,
        PrimaryStrategy, Repository, ResolutionStrategy, Trove, TroveType,
    };
    use conary_core::filesystem::{CasStore, object_path};
    use conary_core::repository::resolution_policy::ResolutionPolicy;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};

    fn build_test_ccs_package_with_bundle(
        dir: &Path,
        name: &str,
        version: &str,
        legacy_scriptlets: Option<LegacyScriptletBundle>,
    ) -> PathBuf {
        let source_dir = dir.join("src");
        std::fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
        std::fs::write(
            source_dir.join("usr/bin").join(name),
            format!("#!/bin/sh\necho {name} {version}\n"),
        )
        .unwrap();

        let mut manifest = CcsManifest::new_minimal(name, version);
        manifest.package.platform = Some(Platform {
            os: "linux".to_string(),
            arch: Some("x86_64".to_string()),
            libc: "gnu".to_string(),
            abi: None,
        });
        manifest.legacy_scriptlets = legacy_scriptlets;

        let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let package_path = dir.join(format!("{name}-{version}.ccs"));
        write_ccs_package(&result, &package_path).unwrap();
        package_path
    }

    fn legacy_upgrade_bundle(package: &str, version: &str) -> LegacyScriptletBundle {
        let entry = legacy_upgrade_entry();
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: package.to_string(),
            source_version: version.to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-update-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{package}-{version}-legacy-upgrade").as_bytes(),
            )),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::LocalOnly,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts: DecisionCounts {
                replaced: 0,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![entry],
            extra: BTreeMap::new(),
        }
    }

    fn legacy_upgrade_entry() -> LegacyScriptletEntry {
        let body = "echo replay-upgrade-new-pre\n";
        LegacyScriptletEntry {
            id: "rpm:%pre".to_string(),
            native_slot: "%pre".to_string(),
            phase: LifecyclePath::PreUpgrade,
            lifecycle_paths: vec!["upgrade:new-pre".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "before-payload".to_string(),
                before: vec!["payload".to_string()],
                after: Vec::new(),
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Legacy,
            reason_code: "legacy-replay-required".to_string(),
            human_reason: Some("fixture legacy pre-upgrade".to_string()),
            evidence_digest: None,
            source_evidence_refs: Vec::new(),
            effects: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
        let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&file_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                stream.write_all(headers.as_bytes()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
        });
        (format!("http://{addr}/{filename}"), handle)
    }

    fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
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
    async fn update_refuses_legacy_replay_before_creating_changeset() {
        let (_temp, db_path) = create_test_db();
        let root = tempfile::tempdir().unwrap();
        let package_dir = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let package_path = build_test_ccs_package_with_bundle(
            package_dir.path(),
            "vim",
            "2.0.0",
            Some(legacy_upgrade_bundle("vim", "2.0.0")),
        );
        let package_bytes = std::fs::read(&package_path).unwrap();
        let package_checksum = conary_core::hash::sha256(&package_bytes);
        let package_size = i64::try_from(package_bytes.len()).unwrap();
        let (package_url, _server_handle) = serve_test_file(package_path);

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        DistroPin::set(&conn, "fedora-44", "strict").unwrap();
        let mut repo = Repository::new("fedora-test".to_string(), package_url.clone());
        repo.gpg_check = false;
        repo.gpg_strict = false;
        repo.default_strategy_distro = Some("fedora-44".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("Install vim-1.0.0".to_string());
            let changeset_id = changeset.insert(tx)?;
            let mut installed = Trove::new_with_source(
                "vim".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            installed.architecture = Some("x86_64".to_string());
            installed.source_distro = Some("fedora-44".to_string());
            installed.version_scheme = Some("rpm".to_string());
            installed.installed_from_repository_id = Some(repo_id);
            installed.installed_by_changeset_id = Some(changeset_id);
            installed.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "vim".to_string(),
            "2.0.0".to_string(),
            package_checksum.clone(),
            package_size,
            package_url.clone(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        repo_pkg.distro = Some("fedora-44".to_string());
        repo_pkg.version_scheme = Some("rpm".to_string());
        repo_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        resolution.version = Some("2.0.0".to_string());
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.insert(&conn).unwrap();

        let before_changesets = table_count(&conn, "changesets");
        drop(conn);

        let err = cmd_update(
            Some("vim".to_string()),
            &db_path,
            root.path().to_str().unwrap(),
            false,
            false,
            false,
            SandboxMode::None,
            None,
            true,
            None,
            Some("x86_64".to_string()),
            crate::commands::LegacyReplayOptions::default(),
        )
        .await
        .expect_err("update should fail closed before admitting a raw legacy replay package");
        let message = err.to_string();
        assert!(message.contains("LegacyReplayFeatureDisabled"), "{message}");

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert_eq!(
            table_count(&conn, "changesets"),
            before_changesets,
            "legacy replay refusal must happen before update changeset insertion"
        );
        let installed_versions = Trove::find_by_name(&conn, "vim")
            .unwrap()
            .into_iter()
            .filter(|trove| trove.trove_type == TroveType::Package)
            .map(|trove| trove.version)
            .collect::<Vec<_>>();
        assert_eq!(installed_versions, vec!["1.0.0".to_string()]);
    }

    #[tokio::test]
    async fn update_delta_candidate_refuses_legacy_replay_before_creating_changeset() {
        let (_temp, db_path) = create_test_db();
        let root = tempfile::tempdir().unwrap();
        let package_dir = tempfile::tempdir().unwrap();
        let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

        let package_path = build_test_ccs_package_with_bundle(
            package_dir.path(),
            "vim",
            "2.0.0",
            Some(legacy_upgrade_bundle("vim", "2.0.0")),
        );
        let package_bytes = std::fs::read(&package_path).unwrap();
        let package_checksum = conary_core::hash::sha256(&package_bytes);
        let package_size = i64::try_from(package_bytes.len()).unwrap();
        let (package_url, _server_handle) = serve_test_file(package_path);

        let mut conn = crate::commands::open_db(&db_path).unwrap();
        DistroPin::set(&conn, "fedora-44", "strict").unwrap();
        let mut repo = Repository::new("fedora-test".to_string(), package_url.clone());
        repo.gpg_check = false;
        repo.gpg_strict = false;
        repo.default_strategy_distro = Some("fedora-44".to_string());
        let repo_id = repo.insert(&conn).unwrap();

        conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("Install vim-1.0.0".to_string());
            let changeset_id = changeset.insert(tx)?;
            let mut installed = Trove::new_with_source(
                "vim".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            installed.architecture = Some("x86_64".to_string());
            installed.source_distro = Some("fedora-44".to_string());
            installed.version_scheme = Some("rpm".to_string());
            installed.installed_from_repository_id = Some(repo_id);
            installed.installed_by_changeset_id = Some(changeset_id);
            installed.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok::<_, conary_core::Error>(())
        })
        .unwrap();

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "vim".to_string(),
            "2.0.0".to_string(),
            package_checksum.clone(),
            package_size,
            package_url.clone(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());
        repo_pkg.distro = Some("fedora-44".to_string());
        repo_pkg.version_scheme = Some("rpm".to_string());
        repo_pkg.insert(&conn).unwrap();

        let mut resolution = PackageResolution::new(
            repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        resolution.version = Some("2.0.0".to_string());
        resolution.primary_strategy = PrimaryStrategy::Binary;
        resolution.insert(&conn).unwrap();

        let from_hash = conary_core::hash::sha256(b"old-package-placeholder");
        let to_hash = conary_core::hash::sha256(b"new-package-placeholder");
        for hash in [&from_hash, &to_hash] {
            conn.execute(
                "INSERT INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, 0)",
                rusqlite::params![hash, format!("objects/{hash}")],
            )
            .unwrap();
        }

        let mut delta = PackageDelta::new(
            "vim".to_string(),
            "1.0.0".to_string(),
            "2.0.0".to_string(),
            from_hash,
            to_hash,
            "http://127.0.0.1:9/vim.delta".to_string(),
            1,
            conary_core::hash::sha256(b"unused-delta"),
            package_size,
        );
        delta.insert(&conn).unwrap();

        let before_changesets = table_count(&conn, "changesets");
        drop(conn);

        let err = cmd_update(
            Some("vim".to_string()),
            &db_path,
            root.path().to_str().unwrap(),
            false,
            false,
            false,
            SandboxMode::None,
            None,
            true,
            None,
            Some("x86_64".to_string()),
            crate::commands::LegacyReplayOptions::default(),
        )
        .await
        .expect_err("delta update should fail closed during admission preflight");
        let message = err.to_string();
        assert!(message.contains("LegacyReplayFeatureDisabled"), "{message}");

        let conn = crate::commands::open_db(&db_path).unwrap();
        assert_eq!(
            table_count(&conn, "changesets"),
            before_changesets,
            "delta legacy replay refusal must happen before update changeset insertion"
        );
        let installed_versions = Trove::find_by_name(&conn, "vim")
            .unwrap()
            .into_iter()
            .filter(|trove| trove.trove_type == TroveType::Package)
            .map(|trove| trove.version)
            .collect::<Vec<_>>();
        assert_eq!(installed_versions, vec!["1.0.0".to_string()]);
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
