// apps/conary/src/commands/generation/takeover.rs
//! Progressive system takeover pipeline
//!
//! Replaces the old all-or-nothing takeover with a three-level progressive
//! pipeline controlled by `--up-to`:
//!
//! * **cas**        -- Internal/debug checkpoint: adopt + CAS-back packages
//! * **owned**      -- Internal/debug checkpoint: CAS + remove from system PM
//! * **generation** -- CAS + PM removal + build generation + boot entry + ready to activate

use self::ownership::{OwnershipTransferReport, take_ownership, upgrade_to_cas_backed};
use super::super::open_db;
use super::boot::{detect_bootloader, write_boot_entry};
use super::builder::build_generation;
use super::metadata::generations_dir;
use super::takeover_state::{
    BootEntryOutcome, TakeoverInventory, TakeoverPhase, TakeoverRecord, TakeoverStatus,
};
use crate::cli::TakeoverLevel;
use crate::commands::adopt::BulkAdoptionOutcome;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{InstallSource, Trove};
use conary_core::model;
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use std::collections::HashMap;
use std::io::Write;
use tracing::{info, warn};

mod ownership;

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
    /// Total packages the system PM reports
    pub total_system_packages: usize,
}

// ---------------------------------------------------------------------------
// plan_takeover
// ---------------------------------------------------------------------------

/// Analyse the system and produce a takeover plan without making changes.
pub fn plan_takeover(
    conn: &rusqlite::Connection,
    pm: SystemPackageManager,
) -> Result<TakeoverPlan> {
    if !pm.is_available() {
        return Err(anyhow!(
            "No supported system package manager detected. \
             Conary supports RPM, dpkg, and pacman."
        ));
    }

    let system_packages = query_all_system_packages(&pm)?;
    // Match exact installed variants; name-only matching collapses multilib and
    // parallel native versions.
    let tracked: HashMap<String, InstallSource> = Trove::list_all(conn)?
        .into_iter()
        .filter_map(|trove| {
            Some((
                trove.native_package_identity?.selector().to_string(),
                trove.install_source,
            ))
        })
        .collect();

    Ok(classify_takeover_inventory(system_packages, &tracked))
}

fn classify_takeover_inventory(
    system_packages: Vec<InstalledPackageIdentity>,
    tracked: &HashMap<String, InstallSource>,
) -> TakeoverPlan {
    let total_system_packages = system_packages.len();
    let mut already_cas_backed = Vec::new();
    let mut needs_cas_upgrade = Vec::new();
    let mut not_tracked = Vec::new();
    let mut already_owned = Vec::new();
    let mut needs_pm_removal = Vec::new();

    for identity in system_packages {
        let selector = identity.selector().to_string();
        match tracked.get(&selector) {
            None => {
                not_tracked.push(selector);
            }
            Some(InstallSource::AdoptedTrack) => {
                needs_cas_upgrade.push(selector.clone());
                needs_pm_removal.push(selector);
            }
            Some(InstallSource::AdoptedFull) => {
                already_cas_backed.push(selector.clone());
                needs_pm_removal.push(selector);
            }
            Some(
                InstallSource::Taken
                | InstallSource::File
                | InstallSource::Repository
                | InstallSource::CapturedRoot,
            ) => {
                already_cas_backed.push(selector.clone());
                already_owned.push(selector);
            }
        }
    }

    TakeoverPlan {
        already_cas_backed,
        needs_cas_upgrade,
        not_tracked,
        already_owned,
        needs_pm_removal,
        total_system_packages,
    }
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
/// 2. **Owned**      -- Everything in Cas, then remove packages from the native database
///    from the system PM database (files stay on disk, Conary owns them).
/// 3. **Generation** -- Everything in Owned, then build an EROFS generation,
///    write a boot entry, and stop ready to activate.
pub async fn cmd_system_takeover(
    db_path: &str,
    level: TakeoverLevel,
    yes: bool,
    dry_run: bool,
    requested_manager: Option<SystemPackageManager>,
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
    let pm = SystemPackageManager::resolve(requested_manager)?;
    let bootloader = detect_bootloader();
    let mut plan = {
        let conn = open_db(db_path)?;
        plan_takeover(&conn, pm)?
    };
    let mut record = TakeoverRecord::load_latest_incomplete(db_path)?.unwrap_or_else(|| {
        TakeoverRecord::planned(
            db_path,
            takeover_level_name(level),
            takeover_inventory_from_plan(&plan),
            pm.display_name(),
            bootloader_name(&bootloader),
        )
    });
    record.requested_level = takeover_level_name(level).to_string();
    record.inventory = takeover_inventory_from_plan(&plan);
    record.discovered_package_manager = pm.display_name().to_string();
    record.discovered_bootloader = bootloader_name(&bootloader).to_string();
    record.save(db_path)?;

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
    println!();

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
    record.start_phase(TakeoverPhase::Cas);
    record.save(db_path)?;

    // 1a. Adopt un-tracked packages (bulk, with CAS)
    if plan.not_tracked.is_empty() {
        info!("All system packages are already tracked");
    } else {
        println!(
            "  Adopting {} un-tracked packages ...",
            plan.not_tracked.len()
        );
        let adoption_outcome = match crate::commands::cmd_adopt_system(
            db_path,
            true,
            false,
            None,
            None,
            false,
            Some(pm),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                record.mark_failed(format!("CAS adoption phase failed: {error}"));
                record.save(db_path)?;
                return Err(error);
            }
        };
        info!("Bulk adoption complete");

        if let Err(error) = require_complete_adoption_for_pm_removal(&adoption_outcome) {
            let failure_count = adoption_outcome.failures.len();
            record.record_adoption_failures(adoption_outcome.failure_records());
            record.mark_failed(format!(
                "CAS adoption was incomplete for {failure_count} package(s); native package-manager removal did not start"
            ));
            record.save(db_path)?;
            return Err(error);
        }
        plan.needs_pm_removal
            .extend(adoption_outcome.adopted_packages);
    }

    // 1b. Upgrade AdoptedTrack -> AdoptedFull (CAS-back)
    if plan.needs_cas_upgrade.is_empty() {
        info!("No packages need CAS upgrade");
    } else {
        println!(
            "  Upgrading {} track-only packages to CAS ...",
            plan.needs_cas_upgrade.len()
        );
        let cas_upgrade_failures = upgrade_to_cas_backed(db_path, &plan.needs_cas_upgrade, &pm)?;
        if !cas_upgrade_failures.is_empty() {
            let failure_count = cas_upgrade_failures.len();
            record.record_cas_upgrade_failures(cas_upgrade_failures);
            record.mark_failed(format!(
                "CAS upgrade was incomplete for {failure_count} package(s); native package-manager removal did not start"
            ));
            record.save(db_path)?;
            anyhow::bail!(
                "CAS upgrade was incomplete for {failure_count} package(s); inspect the takeover record and rerun after correcting the exact query or capture failure"
            );
        }
        info!("CAS upgrade complete");
    }
    record.finish_phase(TakeoverPhase::Cas);
    record.save(db_path)?;

    if matches!(level, TakeoverLevel::Cas) {
        record.mark_incomplete("Takeover stopped at the requested CAS phase.");
        record.save(db_path)?;
        println!();
        crate::ui::status("Finished", "Phase 1 (CAS).");
        println!("All system packages are now adopted and CAS-backed.");
        println!("System PM databases are untouched.");
        println!();
        println!("CAS is an internal/debug stop-point, not the supported release path.");
        println!("Run generation-level takeover to publish a bootable generation artifact.");
        return Ok(());
    }

    // =========================================================================
    // Phase 2: Owned (remove from system PM)
    // =========================================================================
    println!();
    println!("[Phase 2] Taking ownership (removing from system PM) ...");
    record.start_phase(TakeoverPhase::Owned);
    record.save(db_path)?;

    if plan.needs_pm_removal.is_empty() {
        info!("No packages need PM removal");
    } else {
        let ownership_report = match take_ownership(db_path, &plan.needs_pm_removal, pm) {
            Ok(report) => report,
            Err(error) => {
                record.mark_failed(format!("Ownership transfer failed: {error}"));
                record.save(db_path)?;
                return Err(error);
            }
        };
        if let Err(error) = require_complete_ownership_for_later_phases(&ownership_report) {
            let failure_count =
                ownership_report.query_failures.len() + ownership_report.pm_removal_failures.len();
            if !ownership_report.query_failures.is_empty() {
                record.record_ownership_query_failures(ownership_report.query_failures);
            }
            if !ownership_report.pm_removal_failures.is_empty() {
                record.record_pm_removal_failures(ownership_report.pm_removal_failures);
            }
            record.mark_failed(format!(
                "Ownership transfer was incomplete for {failure_count} package(s); generation construction did not start"
            ));
            record.save(db_path)?;
            return Err(error);
        }
        info!("Ownership transfer complete");
    }
    record.finish_phase(TakeoverPhase::Owned);
    record.save(db_path)?;

    if matches!(level, TakeoverLevel::Owned) {
        record.mark_incomplete("Takeover stopped at the requested ownership phase.");
        record.save(db_path)?;
        println!();
        crate::ui::status("Finished", "Phase 2 (Owned).");
        println!("Conary now owns all adopted packages. System PM records removed.");
        println!();
        println!("Owned is an internal/debug stop-point, not the supported release path.");
        println!("Run generation-level takeover to publish a bootable generation artifact.");
        return Ok(());
    }

    // =========================================================================
    // Phase 3: Generation (build + boot entry + ready to activate)
    // =========================================================================
    println!();
    println!("[Phase 3] Building generation ...");
    record.start_phase(TakeoverPhase::Generation);
    record.save(db_path)?;

    let conn = open_db(db_path).context("Failed to open database for generation build")?;
    let gen_number = match build_generation(&conn, db_path, "System takeover -- initial generation")
    {
        Ok(number) => number,
        Err(error) => {
            record.mark_failed(format!("Generation build failed: {error}"));
            record.save(db_path)?;
            return Err(error);
        }
    };
    info!("Built generation {gen_number}");

    println!("  Writing boot entry ...");
    let boot_entry_outcome = match write_boot_entry(gen_number, &bootloader) {
        Ok(()) => BootEntryOutcome::Written,
        Err(error) => {
            warn!("Failed to write boot entry: {error}");
            crate::ui::warn(&format!("Could not write boot entry: {error}"));
            println!("       You may need to configure your bootloader manually.");
            BootEntryOutcome::Failed(error.to_string())
        }
    };
    record.finish_generation(gen_number, boot_entry_outcome);
    record.save(db_path)?;

    println!();
    match record.status {
        TakeoverStatus::ReadyToActivate => {
            crate::ui::status(
                "Complete",
                &format!("system takeover built generation {gen_number} and is ready to activate."),
            );
            println!("Next step:");
            println!("  conary system generation switch {gen_number}");
        }
        TakeoverStatus::CompletedWithWarnings => {
            crate::ui::warn(&format!(
                "System takeover built generation {gen_number}, but boot integration needs manual follow-up."
            ));
            println!("After bootloader follow-up, activate with:");
            println!("  conary system generation switch {gen_number}");
        }
        TakeoverStatus::Incomplete => {
            crate::ui::warn(&format!(
                "System takeover built generation {gen_number}, but the operation is incomplete."
            ));
            println!(
                "Inspect the recorded failures, then rerun takeover or activate manually if appropriate:"
            );
            println!("  conary system generation switch {gen_number}");
        }
        _ => {
            crate::ui::status(
                "Complete",
                &format!("system takeover finished generation preparation for {gen_number}."),
            );
        }
    }
    println!();
    println!("Next steps:");
    println!("  conary system generation list       - View generations");
    println!("  conary system generation info {gen_number}    - Inspect this generation");
    println!("  conary verify                      - Verify system integrity");

    Ok(())
}

// ---------------------------------------------------------------------------
// Phase helpers
// ---------------------------------------------------------------------------

fn require_complete_adoption_for_pm_removal(outcome: &BulkAdoptionOutcome) -> Result<()> {
    if outcome.is_complete() {
        return Ok(());
    }
    anyhow::bail!(
        "CAS adoption was incomplete for {} package(s); native package-manager removal did not start; inspect the takeover record and rerun after correcting the exact query, capture, or metadata failure",
        outcome.failures.len()
    )
}

fn require_complete_ownership_for_later_phases(report: &OwnershipTransferReport) -> Result<()> {
    if report.is_complete() {
        return Ok(());
    }
    anyhow::bail!(
        "ownership transfer was incomplete for {} package(s); no later takeover phase may start until every exact query, capture, and native package-manager removal succeeds",
        report.query_failures.len() + report.pm_removal_failures.len()
    )
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

    println!("Level: cas (internal/debug checkpoint)");
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
    if matches!(level, TakeoverLevel::Owned | TakeoverLevel::Generation) {
        println!();
        println!("Level: owned (internal/debug checkpoint)");
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
        println!("  Stop ready to activate          : yes");
    }
}

fn takeover_inventory_from_plan(plan: &TakeoverPlan) -> TakeoverInventory {
    TakeoverInventory {
        already_cas_backed: plan.already_cas_backed.clone(),
        needs_cas_upgrade: plan.needs_cas_upgrade.clone(),
        not_tracked: plan.not_tracked.clone(),
        already_owned: plan.already_owned.clone(),
        needs_pm_removal: plan.needs_pm_removal.clone(),
        total_system_packages: plan.total_system_packages,
    }
}

fn takeover_level_name(level: TakeoverLevel) -> &'static str {
    match level {
        TakeoverLevel::Cas => "cas",
        TakeoverLevel::Owned => "owned",
        TakeoverLevel::Generation => "generation",
    }
}

fn bootloader_name(bootloader: &super::boot::BootLoader) -> &'static str {
    match bootloader {
        super::boot::BootLoader::Bls => "bls",
        super::boot::BootLoader::Grub(_) => "grub",
        super::boot::BootLoader::None => "none",
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

/// Query every exact installed package-manager database identity.
fn query_all_system_packages(pm: &SystemPackageManager) -> Result<Vec<InstalledPackageIdentity>> {
    match pm {
        SystemPackageManager::Rpm => Ok(rpm_query::query_all_packages()?
            .into_iter()
            .map(|record| record.identity)
            .collect()),
        SystemPackageManager::Dpkg => Ok(dpkg_query::query_all_packages()?
            .into_iter()
            .map(|record| record.identity)
            .collect()),
        SystemPackageManager::Pacman => Ok(pacman_query::query_all_packages()?
            .into_iter()
            .map(|record| record.identity)
            .collect()),
        SystemPackageManager::Unknown => {
            Err(anyhow!("No supported system package manager detected"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::adopt::{BulkAdoptionFailure, BulkAdoptionFailureStage};

    #[test]
    fn test_takeover_plan_empty() {
        let plan = TakeoverPlan {
            already_cas_backed: vec![],
            needs_cas_upgrade: vec![],
            not_tracked: vec!["vim".into(), "git".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["vim".into(), "git".into()],
            total_system_packages: 2,
        };
        assert_eq!(plan.total_system_packages, 2);
        assert_eq!(plan.not_tracked.len(), 2);
        assert!(plan.already_cas_backed.is_empty());
    }

    #[test]
    fn test_takeover_plan_has_no_package_name_exceptions() {
        let plan = TakeoverPlan {
            already_cas_backed: vec![],
            needs_cas_upgrade: vec![],
            not_tracked: vec!["vim".into(), "glibc".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["vim".into(), "glibc".into()],
            total_system_packages: 2,
        };
        assert_eq!(plan.needs_pm_removal.len(), 2);
        assert!(plan.needs_pm_removal.contains(&"glibc".into()));
    }

    #[test]
    fn test_takeover_plan_partially_adopted() {
        let plan = TakeoverPlan {
            already_cas_backed: vec!["bash".into()],
            needs_cas_upgrade: vec!["vim".into()],
            not_tracked: vec!["git".into()],
            already_owned: vec![],
            needs_pm_removal: vec!["bash".into(), "vim".into(), "git".into()],
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

    #[test]
    fn incomplete_bulk_adoption_cannot_cross_pm_removal_boundary() {
        let outcome = BulkAdoptionOutcome {
            failures: vec![BulkAdoptionFailure::new(
                "broken",
                BulkAdoptionFailureStage::PayloadCapture,
                "exact capture failed",
            )],
            ..Default::default()
        };

        let error = require_complete_adoption_for_pm_removal(&outcome).unwrap_err();
        assert!(error.to_string().contains("native package-manager"));
    }

    #[test]
    fn incomplete_ownership_cannot_cross_generation_boundary() {
        let report = OwnershipTransferReport {
            query_failures: vec!["broken: exact query failed".into()],
            pm_removal_failures: Vec::new(),
        };

        let error = require_complete_ownership_for_later_phases(&report).unwrap_err();
        assert!(error.to_string().contains("no later takeover phase"));
    }

    #[test]
    fn takeover_inventory_preserves_both_multiarch_variants() {
        let amd64 =
            InstalledPackageIdentity::dpkg("libc6:amd64", "libc6", "2.42-1", "amd64").unwrap();
        let i386 = InstalledPackageIdentity::dpkg("libc6:i386", "libc6", "2.42-1", "i386").unwrap();
        let tracked = HashMap::from([
            (amd64.selector().to_string(), InstallSource::AdoptedFull),
            (i386.selector().to_string(), InstallSource::AdoptedTrack),
        ]);

        let plan = classify_takeover_inventory(vec![amd64, i386], &tracked);

        assert_eq!(plan.total_system_packages, 2);
        assert_eq!(plan.already_cas_backed, vec!["libc6:amd64"]);
        assert_eq!(plan.needs_cas_upgrade, vec!["libc6:i386"]);
        assert_eq!(plan.needs_pm_removal, vec!["libc6:amd64", "libc6:i386"]);
    }
}
