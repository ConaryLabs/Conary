// src/commands/generation/takeover.rs
//! Full system takeover orchestration
//!
//! Adopts all system packages into Conary tracking, builds an initial
//! generation, writes a boot entry, and performs a live switch.

use super::boot::write_boot_entry;
use super::builder::build_generation;
use super::metadata::generations_dir;
use super::switch::switch_live;
use crate::commands::install::is_package_blocked;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::Trove;
use conary_core::model;
use conary_core::packages::SystemPackageManager;
use std::collections::HashSet;
use std::io::Write;
use std::process::Command;
use tracing::{info, warn};

/// Summary of what a system takeover will do
pub struct TakeoverPlan {
    pub already_tracked: Vec<String>,
    pub to_adopt: Vec<String>,
    #[allow(dead_code)] // Reserved for future CCS conversion from Remi
    pub to_convert: Vec<String>,
    pub blocked: Vec<String>,
    pub total_system_packages: usize,
}

/// Analyse the system and produce a takeover plan without making changes.
pub fn plan_takeover(conn: &rusqlite::Connection) -> Result<TakeoverPlan> {
    // Detect system package manager
    let pm = SystemPackageManager::detect();
    if !pm.is_available() {
        return Err(anyhow!(
            "No supported system package manager detected. \
             Conary supports RPM, dpkg, and pacman."
        ));
    }

    // Query every package the system PM knows about
    let system_packages = query_all_system_packages(&pm)?;
    let total_system_packages = system_packages.len();

    // Build a set of names Conary already tracks
    let tracked: HashSet<String> = Trove::list_all(conn)?.into_iter().map(|t| t.name).collect();

    let mut already_tracked = Vec::new();
    let mut to_adopt = Vec::new();
    let mut blocked = Vec::new();

    for pkg in system_packages {
        if tracked.contains(&pkg) {
            already_tracked.push(pkg);
        } else if is_package_blocked(&pkg) {
            to_adopt.push(pkg.clone());
            blocked.push(pkg);
        } else {
            to_adopt.push(pkg);
        }
    }

    Ok(TakeoverPlan {
        already_tracked,
        to_adopt,
        to_convert: Vec::new(),
        blocked,
        total_system_packages,
    })
}

/// Execute a full system takeover.
///
/// # Arguments
///
/// * `db_path`         - Path to the Conary database
/// * `yes`             - Skip interactive confirmation
/// * `dry_run`         - Show what would happen without making changes
/// * `skip_conversion` - Reserved for future use (CCS conversion step)
pub fn cmd_system_takeover(
    db_path: &str,
    yes: bool,
    dry_run: bool,
    _skip_conversion: bool,
) -> Result<()> {
    // Header
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

    // Pre-flight safety checks
    preflight_checks()?;

    // Open database and build the plan, then drop connection so cmd_adopt
    // can operate on a fresh connection. This prevents stale DB state after
    // bulk adoption.
    let plan = {
        let conn = conary_core::db::open(db_path).context("Failed to open package database")?;
        plan_takeover(&conn)?
    };

    // Print inventory summary
    println!("System inventory:");
    println!("  Total system packages : {}", plan.total_system_packages);
    println!("  Already tracked       : {}", plan.already_tracked.len());
    println!("  To adopt              : {}", plan.to_adopt.len());
    println!("  Blocked (critical)    : {}", plan.blocked.len());
    println!();

    if !plan.blocked.is_empty() {
        println!("Blocked packages (will be adopted but never overlaid):");
        for name in &plan.blocked {
            println!("  - {name}");
        }
        println!();
    }

    // Dry-run: report and exit
    if dry_run {
        println!("[DRY RUN] No changes made. The following would happen:");
        println!(
            "  1. Adopt {} packages into Conary tracking",
            plan.to_adopt.len()
        );
        println!("  2. Build initial generation");
        println!("  3. Write boot entry");
        println!("  4. Live-switch to new generation");
        return Ok(());
    }

    // Interactive confirmation
    if !yes {
        print!("Proceed with system takeover? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Step 1: Adopt un-tracked packages
    if plan.to_adopt.is_empty() {
        info!("All system packages are already tracked");
    } else {
        println!("[1/4] Adopting {} packages ...", plan.to_adopt.len());
        crate::commands::cmd_adopt(&plan.to_adopt, db_path, true)?;
        info!("Adoption complete");
    }

    // Step 2: Build generation (reopen DB to see all adopted packages)
    println!("[2/4] Building initial generation ...");
    let conn =
        conary_core::db::open(db_path).context("Failed to open database for generation build")?;
    let gen_number = build_generation(&conn, db_path, "System takeover -- initial generation")?;
    info!("Built generation {gen_number}");

    // Step 3: Write boot entry (warn on failure, do not abort)
    println!("[3/4] Writing boot entry ...");
    if let Err(e) = write_boot_entry(gen_number) {
        warn!("Failed to write boot entry: {e}");
        println!("[WARN] Could not write boot entry: {e}");
        println!("       You may need to configure your bootloader manually.");
    }

    // Step 4: Live switch
    println!("[4/4] Switching to generation {gen_number} ...");
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

/// Pre-flight safety checks before takeover.
fn preflight_checks() -> Result<()> {
    // Must be root
    if !nix::unistd::Uid::effective().is_root() {
        return Err(anyhow!(
            "System takeover requires root privileges. Re-run with sudo."
        ));
    }

    // Ensure generations directory exists
    let gen_dir = generations_dir();
    std::fs::create_dir_all(&gen_dir).context("Failed to create generations directory")?;

    // Check composefs support (uses default CAS path for probe)
    let default_cas = std::path::PathBuf::from("/conary/objects");
    super::composefs::preflight_composefs(&default_cas)
        .context("Composefs preflight failed — requires Linux 6.2+ with composefs support")?;

    Ok(())
}

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
    fn test_takeover_plan_struct() {
        let plan = TakeoverPlan {
            already_tracked: vec!["bash".into()],
            to_adopt: vec!["vim".into()],
            to_convert: vec![],
            blocked: vec!["glibc".into()],
            total_system_packages: 3,
        };
        assert_eq!(plan.total_system_packages, 3);
        assert_eq!(plan.already_tracked.len(), 1);
        assert_eq!(plan.to_adopt.len(), 1);
        assert_eq!(plan.blocked.len(), 1);
    }
}
