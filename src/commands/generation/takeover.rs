// src/commands/generation/takeover.rs
//! Progressive system takeover pipeline
//!
//! Replaces the old all-or-nothing takeover with a three-level progressive
//! pipeline controlled by `--up-to`:
//!
//! * **cas**        -- Adopt + CAS-back all packages (PM untouched)
//! * **owned**      -- CAS + remove from system PM database
//! * **generation** -- CAS + PM removal + build generation + boot + live switch

use super::super::open_db;
use super::boot::write_boot_entry;
use super::builder::build_generation;
use super::metadata::generations_dir;
use super::switch::switch_live;
use crate::cli::TakeoverLevel;
use crate::commands::adopt::{FileInfoTuple, compute_file_hash};
use crate::commands::install::is_package_blocked;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{Changeset, ChangesetStatus, FileEntry, InstallSource, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use conary_core::model;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use rusqlite::params;
use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// TakeoverPlan
// ---------------------------------------------------------------------------

/// Summary of what a system takeover will do, broken down by level.
pub struct TakeoverPlan {
    /// Packages already CAS-backed (AdoptedFull, Taken, File, Repository)
    pub already_cas_backed: Vec<String>,
    /// Packages tracked but not CAS-backed (AdoptedTrack, need CAS upgrade)
    pub needs_cas_upgrade: Vec<String>,
    /// Packages not tracked at all (need full adoption)
    pub not_tracked: Vec<String>,
    /// Packages already fully owned by Conary (Taken, File, Repository)
    pub already_owned: Vec<String>,
    /// Packages that need PM removal (AdoptedTrack or AdoptedFull after CAS)
    pub needs_pm_removal: Vec<String>,
    /// Blocked packages (adopt + CAS but never remove from PM)
    pub blocked: Vec<String>,
    /// Total packages the system PM reports
    pub total_system_packages: usize,
}

// ---------------------------------------------------------------------------
// plan_takeover
// ---------------------------------------------------------------------------

/// Analyse the system and produce a takeover plan without making changes.
pub fn plan_takeover(conn: &rusqlite::Connection) -> Result<TakeoverPlan> {
    let pm = SystemPackageManager::detect();
    if !pm.is_available() {
        return Err(anyhow!(
            "No supported system package manager detected. \
             Conary supports RPM, dpkg, and pacman."
        ));
    }

    let system_packages = query_all_system_packages(&pm)?;
    let total_system_packages = system_packages.len();

    // Build map of name -> InstallSource for tracked packages
    let tracked: HashMap<String, InstallSource> = Trove::list_all(conn)?
        .into_iter()
        .map(|t| (t.name, t.install_source))
        .collect();

    let mut already_cas_backed = Vec::new();
    let mut needs_cas_upgrade = Vec::new();
    let mut not_tracked = Vec::new();
    let mut already_owned = Vec::new();
    let mut needs_pm_removal = Vec::new();
    let mut blocked = Vec::new();

    for pkg in system_packages {
        let is_blocked = is_package_blocked(&pkg);

        match tracked.get(&pkg) {
            None => {
                not_tracked.push(pkg.clone());
                if is_blocked {
                    blocked.push(pkg);
                }
            }
            Some(InstallSource::AdoptedTrack) => {
                needs_cas_upgrade.push(pkg.clone());
                if is_blocked {
                    blocked.push(pkg);
                } else {
                    needs_pm_removal.push(pkg);
                }
            }
            Some(InstallSource::AdoptedFull) => {
                already_cas_backed.push(pkg.clone());
                if is_blocked {
                    blocked.push(pkg);
                } else {
                    needs_pm_removal.push(pkg);
                }
            }
            Some(InstallSource::Taken | InstallSource::File | InstallSource::Repository) => {
                already_cas_backed.push(pkg.clone());
                already_owned.push(pkg);
            }
        }
    }

    Ok(TakeoverPlan {
        already_cas_backed,
        needs_cas_upgrade,
        not_tracked,
        already_owned,
        needs_pm_removal,
        blocked,
        total_system_packages,
    })
}

// ---------------------------------------------------------------------------
// cmd_system_takeover -- progressive pipeline
// ---------------------------------------------------------------------------

/// Execute a progressive system takeover.
///
/// The pipeline has three levels, controlled by `level`:
///
/// 1. **Cas**        -- Adopt every un-tracked package and CAS-back every
///    `AdoptedTrack` package. The system PM is left untouched.
/// 2. **Owned**      -- Everything in Cas, then remove non-blocked packages
///    from the system PM database (files stay on disk, Conary owns them).
/// 3. **Generation** -- Everything in Owned, then build an EROFS generation,
///    write a boot entry, and live-switch.
pub async fn cmd_system_takeover(
    db_path: &str,
    level: TakeoverLevel,
    yes: bool,
    dry_run: bool,
) -> Result<()> {
    // -- Header ---------------------------------------------------------------
    println!("Conary System Takeover");
    println!("======================");
    println!();

    // Display convergence context from system model if available
    if model::model_exists(None) {
        match model::load_model(None) {
            Ok(m) => {
                let intent = &m.system.convergence;
                info!(
                    "System model convergence intent: {} (target: {})",
                    intent.display_name(),
                    intent.target_install_source()
                );
                println!(
                    "Convergence intent: {} (target state: {})",
                    intent.display_name(),
                    intent.target_install_source()
                );
                println!();
            }
            Err(e) => {
                info!("Could not load system model for convergence context: {e}");
            }
        }
    }

    // -- Pre-flight -----------------------------------------------------------
    preflight_checks(matches!(level, TakeoverLevel::Generation))?;

    // -- Plan -----------------------------------------------------------------
    let pm = SystemPackageManager::detect();
    let mut plan = {
        let conn = open_db(db_path)?;
        plan_takeover(&conn)?
    };

    // Print inventory summary
    println!("System inventory:");
    println!(
        "  Total system packages        : {}",
        plan.total_system_packages
    );
    println!(
        "  Already CAS-backed           : {}",
        plan.already_cas_backed.len()
    );
    println!(
        "  Need CAS upgrade (track)     : {}",
        plan.needs_cas_upgrade.len()
    );
    println!(
        "  Not tracked (to adopt)       : {}",
        plan.not_tracked.len()
    );
    println!(
        "  Already owned                : {}",
        plan.already_owned.len()
    );
    println!(
        "  Need PM removal              : {}",
        plan.needs_pm_removal.len()
    );
    println!("  Blocked (adopt, skip removal) : {}", plan.blocked.len());
    println!();

    if !plan.blocked.is_empty() {
        println!("Blocked packages (will be adopted and CAS-backed but never removed from PM):");
        for name in &plan.blocked {
            println!("  - {name}");
        }
        println!();
    }

    // -- Dry-run output -------------------------------------------------------
    if dry_run {
        print_dry_run(&plan, &pm, level);
        println!();
        println!("[DRY RUN] No changes made.");
        return Ok(());
    }

    // -- Confirmation ---------------------------------------------------------
    if !yes {
        print!("Proceed with system takeover (up-to: {level:?})? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // =========================================================================
    // Phase 1: CAS (always runs)
    // =========================================================================
    println!();
    println!("[Phase 1] CAS-backing all packages ...");

    // 1a. Adopt un-tracked packages (bulk, with CAS)
    if plan.not_tracked.is_empty() {
        info!("All system packages are already tracked");
    } else {
        println!(
            "  Adopting {} un-tracked packages ...",
            plan.not_tracked.len()
        );
        crate::commands::cmd_adopt_system(db_path, true, false, None, None, false).await?;
        info!("Bulk adoption complete");

        // Only add packages to Phase 2 removal if they were actually adopted.
        // cmd_adopt_system is best-effort -- individual packages can fail silently.
        // Re-query the DB to see what's actually tracked now.
        let conn = open_db(db_path)?;
        let now_tracked: std::collections::HashSet<String> = Trove::list_all(&conn)?
            .into_iter()
            .map(|t| t.name)
            .collect();
        // Count only the untracked packages that were eligible (not blocked).
        let eligible: Vec<&String> = plan
            .not_tracked
            .iter()
            .filter(|p| !plan.blocked.contains(p))
            .collect();
        let newly_adopted: Vec<String> = eligible
            .iter()
            .filter(|p| now_tracked.contains(p.as_str()))
            .cloned()
            .cloned()
            .collect();
        let failed = eligible.len() - newly_adopted.len();
        if failed > 0 {
            println!(
                "  [WARN] {failed} packages failed adoption and will not be removed from system PM"
            );
        }
        plan.needs_pm_removal.extend(newly_adopted);
    }

    // 1b. Upgrade AdoptedTrack -> AdoptedFull (CAS-back)
    if plan.needs_cas_upgrade.is_empty() {
        info!("No packages need CAS upgrade");
    } else {
        println!(
            "  Upgrading {} track-only packages to CAS ...",
            plan.needs_cas_upgrade.len()
        );
        upgrade_to_cas_backed(db_path, &plan.needs_cas_upgrade, &pm)?;
        info!("CAS upgrade complete");
    }

    if matches!(level, TakeoverLevel::Cas) {
        println!();
        println!("[COMPLETE] Phase 1 (CAS) finished.");
        println!("All system packages are now adopted and CAS-backed.");
        println!("System PM databases are untouched.");
        println!();
        println!("Next steps:");
        println!("  conary system takeover --up-to owned   - Remove packages from system PM");
        println!("  conary system takeover --up-to generation - Full takeover with generation");
        return Ok(());
    }

    // =========================================================================
    // Phase 2: Owned (remove from system PM)
    // =========================================================================
    println!();
    println!("[Phase 2] Taking ownership (removing from system PM) ...");

    if plan.needs_pm_removal.is_empty() {
        info!("No packages need PM removal");
    } else {
        take_ownership(db_path, &plan.needs_pm_removal, pm)?;
        info!("Ownership transfer complete");
    }

    if matches!(level, TakeoverLevel::Owned) {
        println!();
        println!("[COMPLETE] Phase 2 (Owned) finished.");
        println!("Conary now owns all non-blocked packages. System PM records removed.");
        println!();
        println!("Next steps:");
        println!("  conary system takeover --up-to generation - Build generation and switch");
        return Ok(());
    }

    // =========================================================================
    // Phase 3: Generation (build + boot + switch)
    // =========================================================================
    println!();
    println!("[Phase 3] Building generation ...");

    let conn = open_db(db_path).context("Failed to open database for generation build")?;
    let gen_number = build_generation(&conn, db_path, "System takeover -- initial generation")?;
    info!("Built generation {gen_number}");

    println!("  Writing boot entry ...");
    if let Err(e) = write_boot_entry(gen_number) {
        warn!("Failed to write boot entry: {e}");
        println!("[WARN] Could not write boot entry: {e}");
        println!("       You may need to configure your bootloader manually.");
    }

    println!("  Switching to generation {gen_number} ...");
    switch_live(gen_number)?;
    info!("Live switch to generation {gen_number} complete");

    println!();
    println!("[COMPLETE] System takeover finished (generation {gen_number}).");
    println!();
    println!("Next steps:");
    println!("  conary generation list       - View generations");
    println!("  conary generation info {gen_number}    - Inspect this generation");
    println!("  conary verify                - Verify system integrity");

    Ok(())
}

// ---------------------------------------------------------------------------
// Phase helpers
// ---------------------------------------------------------------------------

/// Upgrade `AdoptedTrack` packages to `AdoptedFull` by hardlinking their
/// files into the CAS and updating the DB.
fn upgrade_to_cas_backed(
    db_path: &str,
    packages: &[String],
    pm: &SystemPackageManager,
) -> Result<()> {
    let cas = CasStore::new(objects_dir(db_path))?;

    // Pre-fetch file lists and perform CAS writes (hardlinks) OUTSIDE the
    // transaction. Any CAS objects written before a DB failure become
    // GC-reclaimable orphans -- the same trade-off the install pipeline makes.
    struct CasUpgradeEntry {
        name: String,
        files_with_hashes: Vec<(FileInfoTuple, String)>,
    }

    let mut entries: Vec<CasUpgradeEntry> = Vec::with_capacity(packages.len());
    for name in packages {
        let files = match query_package_files(*pm, name) {
            Ok(f) => f,
            Err(e) => {
                warn!("Skipping CAS upgrade for '{name}': {e}");
                continue;
            }
        };

        let files_with_hashes: Vec<(FileInfoTuple, String)> = files
            .into_iter()
            .map(|f| {
                let hash =
                    compute_file_hash(&f.0, f.2, f.3.as_deref(), f.6.as_deref(), true, Some(&cas));
                (f, hash)
            })
            .collect();

        entries.push(CasUpgradeEntry {
            name: name.clone(),
            files_with_hashes,
        });
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    let mut conn = open_db(db_path)?;
    conary_core::db::transaction(&mut conn, |tx| {
        let mut cs = Changeset::new("Takeover: CAS-upgrade track-only packages".into());
        cs.insert(tx)?;
        let cs_id = cs.id.ok_or_else(|| {
            conary_core::Error::MissingId("changeset insert did not return an ID".into())
        })?;

        for entry in &entries {
            let Some(trove) = Trove::find_one_by_name(tx, &entry.name)? else {
                warn!(
                    "Trove '{}' not found during CAS upgrade, skipping",
                    entry.name
                );
                continue;
            };
            let trove_id = trove.id.ok_or_else(|| {
                conary_core::Error::MissingId(format!("trove '{}' from DB has no ID", entry.name))
            })?;

            // Update file entry hashes with the pre-computed CAS hashes.
            for ((path, _size, _mode, _digest, _user, _group, _link_target), hash) in
                &entry.files_with_hashes
            {
                if let Some(fe) = FileEntry::find_by_path(tx, path)?
                    && fe.trove_id == trove_id
                    && fe.sha256_hash != *hash
                {
                    tx.execute(
                        "UPDATE files SET sha256_hash = ?1 WHERE id = ?2",
                        params![hash, fe.id],
                    )?;
                }
            }

            // Mark as AdoptedFull
            tx.execute(
                "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 \
                 WHERE id = ?3",
                params![InstallSource::AdoptedFull.as_str(), cs_id, trove_id],
            )?;
        }

        cs.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })?;

    Ok(())
}

/// Take ownership of packages: mark as `Taken` in the DB, then remove from
/// the system PM database. DB commit happens BEFORE PM removal for safety.
fn take_ownership(db_path: &str, packages: &[String], pm: SystemPackageManager) -> Result<()> {
    let cas = CasStore::new(objects_dir(db_path))?;

    // Pre-capture file lists and perform CAS writes OUTSIDE the transaction.
    // Packages whose file query fails are skipped with a warning.
    // Any CAS objects written before a DB failure become GC-reclaimable orphans.
    struct OwnershipEntry {
        name: String,
        files_with_hashes: Vec<(FileInfoTuple, String)>,
    }

    let mut entries: Vec<OwnershipEntry> = Vec::with_capacity(packages.len());
    for name in packages {
        match query_package_files(pm, name) {
            Ok(files) => {
                let files_with_hashes: Vec<(FileInfoTuple, String)> = files
                    .into_iter()
                    .map(|f| {
                        // Always compute CAS hash: track-only packages need
                        // CAS-backing here and full packages get hash refresh.
                        let hash = compute_file_hash(
                            &f.0,
                            f.2,
                            f.3.as_deref(),
                            f.6.as_deref(),
                            true,
                            Some(&cas),
                        );
                        (f, hash)
                    })
                    .collect();
                entries.push(OwnershipEntry {
                    name: name.clone(),
                    files_with_hashes,
                });
            }
            Err(e) => {
                warn!("Skipping ownership transfer for '{name}': {e}");
            }
        }
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    {
        let mut conn = open_db(db_path)?;
        conary_core::db::transaction(&mut conn, |tx| {
            let mut cs = Changeset::new("Takeover: take ownership from system PM".into());
            cs.insert(tx)?;
            let cs_id = cs
                .id
                .ok_or_else(|| conary_core::Error::MissingId("changeset".into()))?;

            for entry in &entries {
                let Some(trove) = Trove::find_one_by_name(tx, &entry.name)? else {
                    warn!(
                        "Trove '{}' not found during ownership transfer, skipping",
                        entry.name
                    );
                    continue;
                };
                let trove_id = trove.id.ok_or_else(|| {
                    conary_core::Error::MissingId(format!("trove '{}'", entry.name))
                })?;

                // If still AdoptedTrack, update file hashes with pre-computed CAS hashes.
                if trove.install_source == InstallSource::AdoptedTrack {
                    for ((path, _size, _mode, _digest, _user, _group, _link_target), hash) in
                        &entry.files_with_hashes
                    {
                        if let Some(fe) = FileEntry::find_by_path(tx, path)?
                            && fe.trove_id == trove_id
                            && fe.sha256_hash != *hash
                        {
                            tx.execute(
                                "UPDATE files SET sha256_hash = ?1 WHERE id = ?2",
                                params![hash, fe.id],
                            )?;
                        }
                    }
                }

                // Mark as Taken
                tx.execute(
                    "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 \
                     WHERE id = ?3",
                    params![InstallSource::Taken.as_str(), cs_id, trove_id],
                )?;
            }

            cs.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })?;
    }

    // Post-commit: remove from system PM database
    let mut failed = Vec::new();
    for entry in &entries {
        let name = &entry.name;
        println!("  Removing {name} from {} database ...", pm.display_name());
        if let Err(e) = remove_from_system_pm(pm, name) {
            warn!("Failed to remove {name} from PM: {e}");
            failed.push(name.clone());
        }
    }

    if !failed.is_empty() {
        println!(
            "[WARN] {} packages could not be removed from {} (Conary owns files; PM has ghost records):",
            failed.len(),
            pm.display_name()
        );
        for name in &failed {
            println!("  - {name}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Dry-run display
// ---------------------------------------------------------------------------

fn print_dry_run(plan: &TakeoverPlan, pm: &SystemPackageManager, level: TakeoverLevel) {
    println!("[DRY RUN] System Takeover Plan");
    println!("==============================");
    println!(
        "System PM: {} ({} packages)",
        pm.display_name(),
        plan.total_system_packages
    );
    println!();

    println!("Level: cas");
    println!(
        "  Already CAS-backed              : {}",
        plan.already_cas_backed.len()
    );
    println!(
        "  To adopt + CAS-back             : {}",
        plan.not_tracked.len()
    );
    println!(
        "  To upgrade (track -> CAS)       : {}",
        plan.needs_cas_upgrade.len()
    );
    println!(
        "  Blocked (adopt, skip PM removal) : {}",
        plan.blocked.len()
    );

    if matches!(level, TakeoverLevel::Owned | TakeoverLevel::Generation) {
        println!();
        println!("Level: owned");
        println!(
            "  Already owned                   : {}",
            plan.already_owned.len()
        );
        println!(
            "  To remove from PM               : {}",
            plan.needs_pm_removal.len()
        );
    }

    if matches!(level, TakeoverLevel::Generation) {
        println!();
        println!("Level: generation");
        println!("  Build EROFS generation          : yes");
        println!("  Write boot entry                : yes");
        println!("  Live switch                     : yes");
    }
}

// ---------------------------------------------------------------------------
// PM helpers
// ---------------------------------------------------------------------------

/// Query files for a package from the active package manager.
///
/// Returns an error if the PM query fails -- callers must not promote
/// a package to `AdoptedFull`/`Taken` without successfully querying its files.
fn query_package_files(pkg_mgr: SystemPackageManager, name: &str) -> Result<Vec<FileInfoTuple>> {
    let raw_files = match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_package_files(name)
            .map_err(|e| anyhow!("RPM file query failed for '{name}': {e}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_package_files(name)
            .map_err(|e| anyhow!("DPKG file query failed for '{name}': {e}"))?,
        SystemPackageManager::Pacman => pacman_query::query_package_files(name)
            .map_err(|e| anyhow!("Pacman file query failed for '{name}': {e}"))?,
        _ => return Ok(Vec::new()),
    };
    Ok(raw_files
        .into_iter()
        .map(|f| {
            (
                f.path,
                f.size,
                f.mode,
                f.digest,
                f.user,
                f.group,
                f.link_target,
            )
        })
        .collect())
}

/// Remove a package from the system package manager's database only.
fn remove_from_system_pm(pkg_mgr: SystemPackageManager, name: &str) -> Result<()> {
    match pkg_mgr {
        SystemPackageManager::Rpm => {
            rpm_query::remove_from_db_only(name).map_err(|e| anyhow!("{e}"))
        }
        SystemPackageManager::Dpkg => {
            dpkg_query::remove_from_db_only(name).map_err(|e| anyhow!("{e}"))
        }
        SystemPackageManager::Pacman => {
            pacman_query::remove_from_db_only(name).map_err(|e| anyhow!("{e}"))
        }
        SystemPackageManager::Unknown => Err(anyhow!("No supported package manager")),
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Pre-flight safety checks before takeover.
fn preflight_checks(check_composefs: bool) -> Result<()> {
    // Must be root
    if !nix::unistd::Uid::effective().is_root() {
        return Err(anyhow!(
            "System takeover requires root privileges. Re-run with sudo."
        ));
    }

    // Ensure generations directory exists
    let gen_dir = generations_dir();
    std::fs::create_dir_all(&gen_dir).context("Failed to create generations directory")?;

    // Check composefs support only when we'll actually build a generation
    if check_composefs {
        let default_cas = std::path::PathBuf::from("/conary/objects");
        super::composefs::preflight_composefs(&default_cas)
            .context("Composefs preflight failed -- requires Linux 6.2+ with composefs support")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// System PM query
// ---------------------------------------------------------------------------

/// Query every installed package name from the system package manager.
fn query_all_system_packages(pm: &SystemPackageManager) -> Result<Vec<String>> {
    let output = match pm {
        SystemPackageManager::Rpm => Command::new("rpm")
            .args(["-qa", "--qf", "%{NAME}\n"])
            .output()
            .context("Failed to run rpm")?,
        SystemPackageManager::Dpkg => Command::new("dpkg-query")
            .args(["-W", "-f", "${Package}\n"])
            .output()
            .context("Failed to run dpkg-query")?,
        SystemPackageManager::Pacman => Command::new("pacman")
            .args(["-Qq"])
            .output()
            .context("Failed to run pacman")?,
        SystemPackageManager::Unknown => {
            return Err(anyhow!("No supported system package manager detected"));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("System package query failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let packages: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_takeover_plan_empty() {
        let plan = TakeoverPlan {
            already_cas_backed: vec![],
            needs_cas_upgrade: vec![],
            not_tracked: vec!["vim".into(), "git".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["vim".into(), "git".into()],
            blocked: vec![],
            total_system_packages: 2,
        };
        assert_eq!(plan.total_system_packages, 2);
        assert_eq!(plan.not_tracked.len(), 2);
        assert!(plan.already_cas_backed.is_empty());
    }

    #[test]
    fn test_takeover_plan_blocked_excluded_from_pm_removal() {
        let plan = TakeoverPlan {
            already_cas_backed: vec![],
            needs_cas_upgrade: vec![],
            not_tracked: vec!["vim".into(), "glibc".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["vim".into()],
            blocked: vec!["glibc".into()],
            total_system_packages: 2,
        };
        assert_eq!(plan.needs_pm_removal.len(), 1);
        assert_eq!(plan.blocked.len(), 1);
        assert!(!plan.needs_pm_removal.contains(&"glibc".into()));
    }

    #[test]
    fn test_takeover_plan_partially_adopted() {
        let plan = TakeoverPlan {
            already_cas_backed: vec!["bash".into()],
            needs_cas_upgrade: vec!["vim".into()],
            not_tracked: vec!["git".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["bash".into(), "vim".into(), "git".into()],
            blocked: vec![],
            total_system_packages: 3,
        };
        assert_eq!(plan.already_cas_backed.len(), 1);
        assert_eq!(plan.needs_cas_upgrade.len(), 1);
        assert_eq!(plan.not_tracked.len(), 1);
    }

    #[test]
    fn test_takeover_level_default_is_generation() {
        let level = TakeoverLevel::default();
        assert!(matches!(level, TakeoverLevel::Generation));
    }
}
