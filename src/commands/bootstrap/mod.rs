// src/commands/bootstrap/mod.rs

//! Bootstrap command implementations
//!
//! Commands for bootstrapping a complete Conary system from scratch.

use anyhow::{Context, Result};
use conary_core::bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, ImageBuilder, ImageFormat, ImageSize, ImageTools,
    Prerequisites, TargetArch,
};
use std::path::PathBuf;
use std::str::FromStr;

/// Initialize bootstrap environment
pub fn cmd_bootstrap_init(work_dir: &str, target: &str, jobs: Option<usize>) -> Result<()> {
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
pub fn cmd_bootstrap_check(verbose: bool) -> Result<()> {
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

/// Generate bootable image
pub fn cmd_bootstrap_image(work_dir: &str, output: &str, format: &str, size: &str) -> Result<()> {
    println!("Generating bootable image...");
    println!("  Work directory: {}", work_dir);
    println!("  Output: {}", output);
    println!("  Format: {}", format);
    println!("  Size: {}", size);

    // Parse format
    let image_format =
        ImageFormat::from_str(format).context("Invalid image format. Use: raw, qcow2, iso")?;

    // Parse size
    let image_size = ImageSize::from_str(size).context("Invalid size. Use: 4G, 8G, 512M, etc.")?;

    // Check prerequisites
    println!("\nChecking required tools...");
    let tools = ImageTools::check()?;

    if let Err(e) = tools.check_for_format(image_format) {
        println!("[ERROR] {}", e);
        println!("\nRequired tools for {} format:", image_format);
        match image_format {
            ImageFormat::Raw | ImageFormat::Qcow2 => {
                println!("  - sfdisk or parted (partitioning)");
                println!("  - mkfs.fat (ESP filesystem)");
                println!("  - mkfs.ext4 (root filesystem)");
                println!("  - losetup (loop device setup)");
                if image_format == ImageFormat::Qcow2 {
                    println!("  - qemu-img (format conversion)");
                }
            }
            ImageFormat::Iso => {
                println!("  - xorriso (ISO creation)");
                println!("  - mksquashfs (squashfs creation)");
            }
        }
        return Err(e.into());
    }
    println!("[OK] All required tools found.");

    // Check if base system exists
    let bootstrap = Bootstrap::new(work_dir)?;
    let Some(sysroot) = bootstrap.get_sysroot() else {
        println!("[ERROR] Base system not found.");
        println!("Run 'conary bootstrap system' first to build the base system.");
        return Err(anyhow::anyhow!("Base system not complete"));
    };
    println!("  Base system: {}", sysroot.display());

    // Build the image
    println!("\nThis will create a bootable {} image.", image_format);
    println!("Image size: {}", image_size);
    println!();

    let config = BootstrapConfig::new();
    let mut builder = ImageBuilder::new(
        work_dir,
        &config,
        &sysroot,
        output,
        image_format,
        image_size,
    )?;

    let result = builder.build()?;

    println!("\n[OK] Image generated successfully!");
    println!("  Path: {}", result.path.display());
    println!("  Format: {}", result.format);
    println!(
        "  Size: {} bytes ({:.1} GB)",
        result.size,
        result.size as f64 / 1_073_741_824.0
    );
    println!(
        "  EFI bootable: {}",
        if result.efi_bootable { "yes" } else { "no" }
    );
    println!(
        "  BIOS bootable: {}",
        if result.bios_bootable { "yes" } else { "no" }
    );

    println!("\nUsage:");
    match image_format {
        ImageFormat::Raw => {
            println!(
                "  QEMU: qemu-system-x86_64 -drive file={},format=raw -m 2G -enable-kvm",
                output
            );
            println!(
                "  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress",
                output
            );
        }
        ImageFormat::Qcow2 => {
            println!(
                "  QEMU: qemu-system-x86_64 -drive file={},format=qcow2 -m 2G -enable-kvm",
                output
            );
        }
        ImageFormat::Iso => {
            println!(
                "  QEMU: qemu-system-x86_64 -cdrom {} -m 2G -enable-kvm",
                output
            );
            println!(
                "  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress",
                output
            );
        }
    }

    Ok(())
}

/// Show bootstrap status
pub fn cmd_bootstrap_status(work_dir: &str, verbose: bool) -> Result<()> {
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
pub fn cmd_bootstrap_resume(work_dir: &str, verbose: bool) -> Result<()> {
    println!("Resuming bootstrap...");

    let mut bootstrap = Bootstrap::new(work_dir)?;

    let Some(current) = bootstrap.resume()? else {
        println!("All bootstrap stages are already complete.");
        return Ok(());
    };

    println!("Resuming from: {}", current);

    match current {
        BootstrapStage::CrossTools => {
            cmd_bootstrap_cross_tools(work_dir, None, verbose, false, None)
        }
        BootstrapStage::TempTools => cmd_bootstrap_temp_tools(work_dir, None, verbose, false, None),
        BootstrapStage::FinalSystem => cmd_bootstrap_system(work_dir, None, verbose, false, None),
        BootstrapStage::SystemConfig => cmd_bootstrap_config(work_dir, verbose, None),
        BootstrapStage::BootableImage => {
            cmd_bootstrap_image(work_dir, "conaryos-base.qcow2", "qcow2", "4G")
        }
        BootstrapStage::Tier2 => cmd_bootstrap_tier2(work_dir, None, verbose, false, None),
    }
}

/// Validate the full pipeline without building
pub fn cmd_bootstrap_dry_run(work_dir: &str, recipe_dir: &str, verbose: bool) -> Result<()> {
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

/// Build Phase 1: Cross-toolchain (LFS Chapter 5)
pub fn cmd_bootstrap_cross_tools(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 1: Cross-Toolchain (LFS Ch5)...");
    println!("  Work directory: {}", work_dir);

    let mut config = BootstrapConfig::new()
        .with_verbose(verbose)
        .with_skip_verify(skip_verify);
    if let Some(j) = jobs {
        config = config.with_jobs(j);
    }
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let mut bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nThis will build the cross-toolchain using the host compiler.");
    println!("Build order: binutils-pass1 -> gcc-pass1 -> linux-headers -> glibc -> libstdc++\n");

    let toolchain = bootstrap.build_cross_tools()?;

    println!("\n[OK] Phase 1 cross-toolchain built successfully!");
    println!("  Path: {}", toolchain.path.display());
    println!("  Target: {}", toolchain.target);

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap temp-tools' to build Phase 2 temporary tools");

    Ok(())
}

/// Build Phase 2: Temporary tools (LFS Chapters 6-7)
pub fn cmd_bootstrap_temp_tools(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 2: Temporary Tools (LFS Ch6-7)...");
    println!("  Work directory: {}", work_dir);

    let mut config = BootstrapConfig::new()
        .with_verbose(verbose)
        .with_skip_verify(skip_verify);
    if let Some(j) = jobs {
        config = config.with_jobs(j);
    }
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let mut bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nThis will cross-compile 17 packages and build 6 in the chroot.\n");

    bootstrap.build_temp_tools()?;

    println!("\n[OK] Phase 2 temporary tools built successfully!");

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap system' to build Phase 3 final system");

    Ok(())
}

/// Build Phase 3: Final system (LFS Chapter 8)
pub fn cmd_bootstrap_system(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 3: Final System (LFS Ch8)...");
    println!("  Work directory: {}", work_dir);

    let mut config = BootstrapConfig::new()
        .with_verbose(verbose)
        .with_skip_verify(skip_verify);
    if let Some(j) = jobs {
        config = config.with_jobs(j);
    }
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let mut bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nThis will build all 77 packages of the final LFS system.\n");

    bootstrap.build_final_system()?;

    println!("\n[OK] Phase 3 final system built successfully!");

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap config' to configure the system for booting");

    Ok(())
}

/// Run Phase 4: System configuration (LFS Chapter 9)
pub fn cmd_bootstrap_config(work_dir: &str, verbose: bool, lfs_root: Option<&str>) -> Result<()> {
    println!("Running Phase 4: System Configuration (LFS Ch9)...");
    println!("  Work directory: {}", work_dir);

    let mut config = BootstrapConfig::new().with_verbose(verbose);
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let mut bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nConfiguring network, fstab, kernel, and bootloader...\n");

    bootstrap.configure_system()?;

    println!("\n[OK] Phase 4 system configuration complete!");

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap image' to generate a bootable image");

    Ok(())
}

/// Build Phase 6: Tier-2 packages (BLFS + Conary self-hosting)
pub fn cmd_bootstrap_tier2(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 6: Tier-2 Packages (BLFS + Conary)...");
    println!("  Work directory: {}", work_dir);

    let mut config = BootstrapConfig::new()
        .with_verbose(verbose)
        .with_skip_verify(skip_verify);
    if let Some(j) = jobs {
        config = config.with_jobs(j);
    }
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let mut bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nThis will build 8 additional packages: PAM, OpenSSH, make-ca,");
    println!("curl, sudo, nano, Rust, and Conary.\n");

    bootstrap.build_tier2()?;

    println!("\n[OK] Phase 6 Tier-2 packages built successfully!");
    println!("  The system is now self-hosting.");

    Ok(())
}

/// Clean bootstrap work directory
pub fn cmd_bootstrap_clean(work_dir: &str, stage: Option<String>, sources: bool) -> Result<()> {
    println!("Cleaning bootstrap work directory...");
    println!("  Work directory: {}", work_dir);

    let work_path = PathBuf::from(work_dir);

    if !work_path.exists() {
        println!("Work directory does not exist.");
        return Ok(());
    }

    if let Some(ref stage_name) = stage {
        // Clean specific stage
        let stage_dir = work_path.join(stage_name);
        if stage_dir.exists() {
            println!("  Removing: {}", stage_dir.display());
            std::fs::remove_dir_all(&stage_dir)?;
        } else {
            println!("  Stage directory not found: {}", stage_dir.display());
        }
    } else {
        // Clean everything except tarballs (unless --sources)
        for entry in std::fs::read_dir(&work_path)? {
            let entry = entry?;
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if name == "tarballs" && !sources {
                println!("  Keeping: {}", path.display());
                continue;
            }

            if path.is_dir() {
                println!("  Removing: {}", path.display());
                std::fs::remove_dir_all(&path)?;
            } else {
                println!("  Removing: {}", path.display());
                std::fs::remove_file(&path)?;
            }
        }
    }

    println!("\n[OK] Clean complete.");

    Ok(())
}
