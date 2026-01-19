// src/commands/bootstrap/mod.rs

//! Bootstrap command implementations
//!
//! Commands for bootstrapping a complete Conary system from scratch.

use anyhow::{Context, Result};
use conary::bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, ImageBuilder, ImageFormat, ImageSize, ImageTools,
    Prerequisites, Stage0Builder, TargetArch,
};
use std::path::PathBuf;
use std::str::FromStr;

/// Initialize bootstrap environment
pub fn cmd_bootstrap_init(work_dir: &str, target: &str, jobs: Option<usize>) -> Result<()> {
    println!("Initializing bootstrap environment...");
    println!("  Work directory: {}", work_dir);

    let target_arch =
        TargetArch::parse(target).context("Invalid target architecture. Use: x86_64, aarch64, riscv64")?;

    println!("  Target: {} ({})", target_arch, target_arch.triple());

    let mut config = BootstrapConfig::new().with_target(target_arch);

    if let Some(j) = jobs {
        config = config.with_jobs(j);
        println!("  Jobs: {}", j);
    }

    let bootstrap = Bootstrap::with_config(work_dir, config)?;

    println!("\nBootstrap environment initialized at {}", bootstrap.work_dir().display());
    println!("\nNext steps:");
    println!("  1. Run 'conary bootstrap check' to verify prerequisites");
    println!("  2. Run 'conary bootstrap stage0' to build the cross-toolchain");

    Ok(())
}

/// Check prerequisites for bootstrap
pub fn cmd_bootstrap_check(verbose: bool) -> Result<()> {
    println!("Checking bootstrap prerequisites...\n");

    let prereqs = Prerequisites::check()?;

    let status = |present: bool| if present { "[OK]" } else { "[MISSING]" };

    println!(
        "  {} crosstool-ng: {}",
        status(prereqs.crosstool_ng.is_some()),
        prereqs.crosstool_ng.as_deref().unwrap_or("not found")
    );
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
        println!("\nYou can proceed with 'conary bootstrap stage0' to build the toolchain.");
    } else {
        println!("[MISSING] Some prerequisites are not installed:");
        for missing in prereqs.missing() {
            println!("  - {}", missing);
        }
        println!("\nInstall the missing tools before proceeding.");

        if verbose {
            println!("\nInstallation hints:");
            if prereqs.crosstool_ng.is_none() {
                println!("  crosstool-ng:");
                println!("    Fedora: sudo dnf install crosstool-ng");
                println!("    Arch:   sudo pacman -S crosstool-ng");
                println!("    Or:     https://crosstool-ng.github.io/docs/install/");
            }
        }
    }

    Ok(())
}

/// Build Stage 0 cross-compilation toolchain
pub fn cmd_bootstrap_stage0(
    work_dir: &str,
    config: Option<String>,
    jobs: Option<usize>,
    verbose: bool,
    download_only: bool,
    clean: bool,
) -> Result<()> {
    println!("Building Stage 0 toolchain...");

    // Check prerequisites first
    if let Err(e) = Stage0Builder::check_crosstool() {
        println!("[ERROR] {}", e);
        println!("\nInstall crosstool-ng first:");
        println!("  Fedora: sudo dnf install crosstool-ng");
        println!("  Arch:   sudo pacman -S crosstool-ng");
        return Err(e.into());
    }

    let mut bootstrap_config = BootstrapConfig::new().with_verbose(verbose);

    if let Some(j) = jobs {
        bootstrap_config = bootstrap_config.with_jobs(j);
    }

    if let Some(ref cfg_path) = config {
        bootstrap_config = bootstrap_config.with_crosstool_config(cfg_path);
    }

    let mut builder = Stage0Builder::new(work_dir, &bootstrap_config)?;

    if clean {
        println!("Cleaning work directory...");
        builder.clean()?;
    }

    if download_only {
        println!("Downloading source tarballs...");
        builder.download_sources()?;
        println!("\n[OK] Sources downloaded to {}/tarballs/", work_dir);
        return Ok(());
    }

    println!("\nThis will build a complete cross-compilation toolchain.");
    println!("The build typically takes 30-60 minutes.\n");

    let toolchain = builder.build()?;

    println!("\n[OK] Stage 0 toolchain built successfully!");
    println!("  Path: {}", toolchain.path.display());
    println!("  Target: {}", toolchain.target);
    if let Some(ref ver) = toolchain.gcc_version {
        println!("  GCC: {}", ver);
    }
    println!("  Static: {}", if toolchain.is_static { "yes" } else { "no" });

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap stage1' to build the self-hosted toolchain");

    Ok(())
}

/// Build Stage 1 self-hosted toolchain
pub fn cmd_bootstrap_stage1(
    work_dir: &str,
    recipe_dir: Option<&str>,
    jobs: Option<usize>,
    verbose: bool,
) -> Result<()> {
    println!("Building Stage 1 toolchain...");
    println!("  Work directory: {}", work_dir);

    // Check if Stage 0 is complete
    let mut bootstrap = Bootstrap::new(work_dir)?;

    // Set config options (used when we reconfigure the bootstrap)
    let _config = {
        let mut c = BootstrapConfig::new().with_verbose(verbose);
        if let Some(j) = jobs {
            c = c.with_jobs(j);
        }
        c
    };

    let stage0_toolchain = bootstrap.get_stage0_toolchain();

    if stage0_toolchain.is_none() {
        println!("[ERROR] Stage 0 toolchain not found.");
        println!("Run 'conary bootstrap stage0' first.");
        return Err(anyhow::anyhow!("Stage 0 not complete"));
    }

    let toolchain = stage0_toolchain.unwrap();
    println!("  Using Stage 0 toolchain: {}", toolchain.path.display());

    // Determine recipe directory
    let recipe_path = recipe_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("recipes/core"));

    if !recipe_path.exists() {
        println!("[ERROR] Recipe directory not found: {}", recipe_path.display());
        println!("Specify --recipe-dir or ensure recipes/core exists.");
        return Err(anyhow::anyhow!("Recipe directory not found"));
    }

    println!("  Recipe directory: {}", recipe_path.display());

    // Build Stage 1
    println!("\nThis will build the self-hosted toolchain using Stage 0.");
    println!("Build order: linux-headers -> binutils -> gcc-pass1 -> glibc -> gcc-pass2\n");

    let stage1_toolchain = bootstrap.build_stage1(&recipe_path)?;

    println!("\n[OK] Stage 1 toolchain built successfully!");
    println!("  Path: {}", stage1_toolchain.path.display());
    println!("  Target: {}", stage1_toolchain.target);
    if let Some(ref ver) = stage1_toolchain.gcc_version {
        println!("  GCC: {}", ver);
    }

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap base' to build the base system packages");

    Ok(())
}

/// Build base system packages
pub fn cmd_bootstrap_base(
    work_dir: &str,
    root: &str,
    recipe_dir: Option<&str>,
    _verbose: bool,
) -> Result<()> {
    println!("Building base system...");
    println!("  Work directory: {}", work_dir);
    println!("  Target root: {}", root);

    // Check if Stage 1 is complete
    let mut bootstrap = Bootstrap::new(work_dir)?;

    let stage1_toolchain = bootstrap.get_stage1_toolchain();

    if stage1_toolchain.is_none() {
        println!("[ERROR] Stage 1 toolchain not found.");
        println!("Run 'conary bootstrap stage1' first.");
        return Err(anyhow::anyhow!("Stage 1 not complete"));
    }

    let toolchain = stage1_toolchain.unwrap();
    println!("  Using Stage 1 toolchain: {}", toolchain.path.display());

    // Determine recipe directory
    let recipe_path = recipe_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("recipes/core"));

    if !recipe_path.exists() {
        println!("[ERROR] Recipe directory not found: {}", recipe_path.display());
        println!("Specify --recipe-dir or ensure recipes/core exists.");
        return Err(anyhow::anyhow!("Recipe directory not found"));
    }

    println!("  Recipe directory: {}", recipe_path.display());

    // Build base system
    println!("\nThis will build the complete base system (~52 packages).");
    println!("Phases: Libraries -> Dev Tools -> Core System -> Userland -> Boot\n");

    let summary = bootstrap.build_base(&recipe_path, root)?;

    println!("\n[OK] Base system build complete!");
    println!("  Target root: {}", root);
    println!("  {}", summary);

    if summary.failed > 0 {
        println!("\n[WARN] {} packages failed (see logs in {}/base/logs/)",
                 summary.failed, work_dir);
    }

    println!("\nNext steps:");
    println!("  Run 'conary bootstrap image' to generate a bootable image");

    Ok(())
}

/// Generate bootable image
pub fn cmd_bootstrap_image(
    work_dir: &str,
    output: &str,
    format: &str,
    size: &str,
) -> Result<()> {
    println!("Generating bootable image...");
    println!("  Work directory: {}", work_dir);
    println!("  Output: {}", output);
    println!("  Format: {}", format);
    println!("  Size: {}", size);

    // Parse format
    let image_format = ImageFormat::from_str(format)
        .context("Invalid image format. Use: raw, qcow2, iso")?;

    // Parse size
    let image_size = ImageSize::from_str(size)
        .context("Invalid size. Use: 4G, 8G, 512M, etc.")?;

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
    let sysroot = bootstrap.get_sysroot();

    if sysroot.is_none() {
        println!("[ERROR] Base system not found.");
        println!("Run 'conary bootstrap base' first to build the base system.");
        return Err(anyhow::anyhow!("Base system not complete"));
    }

    let sysroot = sysroot.unwrap();
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
    println!("  Size: {} bytes ({:.1} GB)", result.size, result.size as f64 / 1_073_741_824.0);
    println!("  EFI bootable: {}", if result.efi_bootable { "yes" } else { "no" });
    println!("  BIOS bootable: {}", if result.bios_bootable { "yes" } else { "no" });

    println!("\nUsage:");
    match image_format {
        ImageFormat::Raw => {
            println!("  QEMU: qemu-system-x86_64 -drive file={},format=raw -m 2G -enable-kvm", output);
            println!("  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress", output);
        }
        ImageFormat::Qcow2 => {
            println!("  QEMU: qemu-system-x86_64 -drive file={},format=qcow2 -m 2G -enable-kvm", output);
        }
        ImageFormat::Iso => {
            println!("  QEMU: qemu-system-x86_64 -cdrom {} -m 2G -enable-kvm", output);
            println!("  USB:  sudo dd if={} of=/dev/sdX bs=4M status=progress", output);
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
        if verbose
            && let Some(ref s) = status
        {
            print!(" - {}", s);
        }
        println!();
    }

    let current = bootstrap.stages().current_stage()?;
    println!("\nNext stage: {}", current);

    Ok(())
}

/// Resume bootstrap from last checkpoint
pub fn cmd_bootstrap_resume(work_dir: &str, verbose: bool) -> Result<()> {
    println!("Resuming bootstrap...");

    let mut bootstrap = Bootstrap::new(work_dir)?;
    let current = bootstrap.resume()?;

    println!("Resuming from: {}", current);

    match current {
        BootstrapStage::Stage0 => {
            cmd_bootstrap_stage0(work_dir, None, None, verbose, false, false)
        }
        BootstrapStage::Stage1 => cmd_bootstrap_stage1(work_dir, None, None, verbose),
        BootstrapStage::BaseSystem => cmd_bootstrap_base(work_dir, "/conary/sysroot", None, verbose),
        BootstrapStage::Image => cmd_bootstrap_image(work_dir, "conary.img", "raw", "4G"),
        stage => {
            // Handle other stages (Stage2, Boot, Networking, Conary) - not yet implemented
            println!("[NOT IMPLEMENTED] Resume for stage {} is not yet implemented.", stage);
            Ok(())
        }
    }
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
