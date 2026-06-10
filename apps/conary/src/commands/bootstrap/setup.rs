// apps/conary/src/commands/bootstrap/setup.rs

use std::path::PathBuf;

use anyhow::{Context, Result};
use conary_core::bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, TargetArch,
};

use super::image::cmd_bootstrap_image;
use super::phases::{
    cmd_bootstrap_config, cmd_bootstrap_cross_tools, cmd_bootstrap_system,
    cmd_bootstrap_temp_tools, cmd_bootstrap_tier2,
};

/// Initialize bootstrap environment
pub async fn cmd_bootstrap_init(work_dir: &str, target: &str, jobs: Option<usize>) -> Result<()> {
    println!("Initializing bootstrap environment...");
    println!("  Work directory: {}", work_dir);

    let target_arch = TargetArch::parse(target)
        .context("Invalid target architecture. Use: x86_64, aarch64, riscv64")?;

    println!("  Target: {} ({})", target_arch, target_arch.triple());

    let mut config = BootstrapConfig::new().with_target(target_arch);

    if let Some(j) = jobs {
        config = config.with_jobs(j);
        println!("  Jobs: {}", j);
    }

    let bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!(
        "\nBootstrap environment initialized at {}",
        bootstrap.work_dir().display()
    );
    println!("\nNext steps:");
    println!("  1. Run 'conary bootstrap check' to verify prerequisites");
    println!("  2. Run 'conary bootstrap cross-tools' to build the cross-toolchain");

    Ok(())
}
/// Check prerequisites for bootstrap
pub async fn cmd_bootstrap_check(verbose: bool) -> Result<()> {
    println!("Checking bootstrap prerequisites...\n");

    let prereqs = Prerequisites::check()?;

    let status = |present: bool| if present { "[OK]" } else { "[MISSING]" };

    println!(
        "  {} make: {}",
        status(prereqs.make.is_some()),
        prereqs.make.as_deref().unwrap_or("not found")
    );
    println!(
        "  {} gcc: {}",
        status(prereqs.gcc.is_some()),
        prereqs.gcc.as_deref().unwrap_or("not found")
    );
    println!(
        "  {} git: {}",
        status(prereqs.git.is_some()),
        prereqs.git.as_deref().unwrap_or("not found")
    );

    println!();

    if prereqs.all_present() {
        println!("[OK] All prerequisites are satisfied.");
        println!(
            "\nYou can proceed with 'conary bootstrap cross-tools' to build the cross-toolchain."
        );
    } else {
        println!("[MISSING] Some prerequisites are not installed:");
        for missing in prereqs.missing() {
            println!("  - {}", missing);
        }
        println!("\nInstall the missing tools before proceeding.");

        if verbose {
            println!("\nInstallation hints:");
            println!("  Install gcc, make, and git using your distribution's package manager.");
        }
    }

    Ok(())
}
/// Show bootstrap status
pub async fn cmd_bootstrap_status(work_dir: &str, verbose: bool) -> Result<()> {
    let work_path = PathBuf::from(work_dir);

    if !work_path.exists() {
        println!("Bootstrap environment not initialized.");
        println!("Run 'conary bootstrap init' first.");
        return Ok(());
    }

    let bootstrap = Bootstrap::new(work_dir)?;

    println!("Bootstrap Status");
    println!("================\n");
    println!("Work directory: {}\n", bootstrap.work_dir().display());

    for (stage, complete, status) in bootstrap.stages().summary() {
        let marker = if complete { "[COMPLETE]" } else { "[PENDING]" };
        print!("  {} {}", marker, stage);
        if verbose && let Some(ref s) = status {
            print!(" - {}", s);
        }
        println!();
    }

    match bootstrap.stages().current_stage()? {
        Some(current) => println!("\nNext stage: {}", current),
        None => println!("\nAll stages complete."),
    }

    Ok(())
}
/// Resume bootstrap from last checkpoint
pub async fn cmd_bootstrap_resume(work_dir: &str, verbose: bool) -> Result<()> {
    println!("Resuming bootstrap...");

    let mut bootstrap = Bootstrap::new(work_dir)?;

    let Some(current) = bootstrap.resume()? else {
        println!("All bootstrap stages are already complete.");
        return Ok(());
    };

    println!("Resuming from: {}", current);

    match current {
        BootstrapStage::CrossTools => {
            cmd_bootstrap_cross_tools(work_dir, None, verbose, false, None).await
        }
        BootstrapStage::TempTools => {
            cmd_bootstrap_temp_tools(work_dir, None, verbose, false, None).await
        }
        BootstrapStage::FinalSystem => {
            cmd_bootstrap_system(work_dir, None, verbose, false, None).await
        }
        BootstrapStage::SystemConfig => cmd_bootstrap_config(work_dir, verbose, None).await,
        BootstrapStage::BootableImage => {
            cmd_bootstrap_image(work_dir, "conaryos-base.qcow2", "qcow2", "4G").await
        }
        BootstrapStage::Tier2 => cmd_bootstrap_tier2(work_dir, None, verbose, false, None).await,
    }
}
/// Validate the full pipeline without building
pub async fn cmd_bootstrap_dry_run(work_dir: &str, recipe_dir: &str, verbose: bool) -> Result<()> {
    let work_path = PathBuf::from(work_dir);
    let recipe_path = PathBuf::from(recipe_dir);
    let config = BootstrapConfig::new().with_verbose(verbose);
    let bootstrap = Bootstrap::with_config(work_path, config)?;

    println!("Validating bootstrap pipeline...");
    let report = bootstrap
        .dry_run(&recipe_path)
        .map_err(|e| anyhow::anyhow!("Dry run failed: {e}"))?;

    println!("Cross-tools recipes: {}", report.cross_tools_count);
    println!("System recipes:      {}", report.system_count);
    println!("Tier-2 recipes:      {}", report.tier2_count);
    println!("Graph resolved:      {}", report.graph_resolved);

    if report.placeholder_count > 0 {
        println!(
            "[WARNING] Placeholder checksums: {}",
            report.placeholder_count
        );
    }

    for warning in &report.warnings {
        println!("[WARNING] {warning}");
    }

    for error in &report.errors {
        println!("[ERROR] {error}");
    }

    if report.is_ok() {
        println!("[COMPLETE] Pipeline validation passed");
        Ok(())
    } else {
        println!(
            "[FAILED] Pipeline validation failed ({} errors)",
            report.errors.len()
        );
        Err(anyhow::anyhow!(
            "Pipeline validation failed with {} errors",
            report.errors.len()
        ))
    }
}
