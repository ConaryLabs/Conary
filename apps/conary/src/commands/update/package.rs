// src/commands/update/package.rs

//! Single-package update command execution.

use super::super::install::{
    CcsTransactionInstallOptions, ComponentSelection, DepMode,
    repository_install_provenance_from_package, resolve_default_dep_mode_from_model,
    verify_static_repository_ccs_package_if_needed,
};
use super::super::progress::{UpdatePhase, UpdateProgress};
use super::super::{InstallOptions, LegacyReplayOptions, SandboxMode, cmd_install, open_db};
use super::adopted_authority::{
    AdoptedUpdateDecision, AdoptedUpdateSkip, AdoptedUpdateSkipReason, adopted_update_decision,
    native_manager_for_trove, no_update_message, render_adopted_skip_sample,
};
use super::selection::{
    SecurityMetadataUnavailable, SelectedUpdateCandidate, UpdateCandidateSelection,
    installed_troves_for_update, print_security_metadata_unavailable, print_source_switch_preview,
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

    let repository_provenance = repository_install_provenance_from_package(repo_pkg, repo)?;
    verify_static_repository_ccs_package_if_needed(
        db_path,
        pkg_path,
        Some(&repository_provenance),
    )?;
    let pkg = CcsPackage::parse(&pkg_path.to_string_lossy())
        .with_context(|| format!("failed to parse selected update CCS {}", pkg_path.display()))?;
    let ccs_opts = CcsTransactionInstallOptions {
        db_path,
        root,
        dry_run: false,
        defer_generation: false,
        quiet: false,
        no_scripts,
        sandbox_mode,
        allow_downgrade: false,
        reinstall: false,
        selection_reason: Some("Updated by conary update"),
        component_selection: ComponentSelection::Defaults,
        selected_manifest_components: None,
        repository_provenance: Some(repository_provenance),
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
                            crate::ui::row(crate::ui::Status::Ok, &["Delta applied to CAS"]);
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
                                                let row = format!(
                                                    "{} {} -> {}",
                                                    trove.name, trove.version, repo_pkg.version
                                                );
                                                crate::ui::row(crate::ui::Status::Ok, &[&row]);
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

#[cfg(test)]
mod tests;
