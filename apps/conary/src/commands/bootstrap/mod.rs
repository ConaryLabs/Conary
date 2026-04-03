// src/commands/bootstrap/mod.rs

//! Bootstrap command implementations
//!
//! Commands for bootstrapping a complete Conary system from scratch.

pub mod state;

use anyhow::{Context, Result};
use conary_core::bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, ImageBuilder, ImageFormat, ImageSize, ImageTools,
    Prerequisites, TargetArch,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::info;

use self::state::{BootstrapLatestPointer, BootstrapRunRecord};
use crate::commands::operation_records::new_operation_id;

fn skip_verify_warning_message() -> &'static str {
    "WARNING: UNSAFE bootstrap mode enabled via --skip-verify. placeholder source checksums will be accepted, so only use this during an authenticated bootstrap flow where you independently trust the source tarballs."
}

fn print_skip_verify_warning(skip_verify: bool) {
    if skip_verify {
        eprintln!("{}", skip_verify_warning_message());
    }
}

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

/// Generate bootable image
pub async fn cmd_bootstrap_image(
    work_dir: &str,
    output: &str,
    format: &str,
    size: &str,
    from_generation: Option<&str>,
) -> Result<()> {
    println!("Generating bootable image...");
    println!("  Work directory: {}", work_dir);
    println!("  Output: {}", output);
    println!("  Format: {}", format);
    println!("  Size: {}", size);

    // If --from-generation is provided, use the EROFS generation path
    if let Some(gen_dir) = from_generation {
        println!("Generating image from EROFS generation...");
        println!("  Generation: {}", gen_dir);
        println!("  Output: {}", output);
        println!("  Format: {}", format);

        let image_format = ImageFormat::from_str(format)
            .context("Invalid image format. Use: raw, qcow2, iso, erofs")?;
        let image_size =
            ImageSize::from_str(size).context("Invalid size. Use: 4G, 8G, 512M, etc.")?;

        let result = ImageBuilder::build_from_generation(
            Path::new(gen_dir),
            Path::new(output),
            image_format,
            image_size,
        )?;

        println!("\n[OK] Image generated successfully!");
        println!("  Path: {}", result.path.display());
        println!("  Format: {}", result.format);
        println!(
            "  Size: {} bytes ({:.1} GB)",
            result.size,
            result.size as f64 / 1_073_741_824.0
        );
        println!("  Method: {}", result.method);
        println!("\nUsage:");
        println!(
            "  qemu-system-x86_64 -drive file={},format={} -m 2G -enable-kvm -nographic",
            output,
            if image_format == ImageFormat::Qcow2 {
                "qcow2"
            } else {
                "raw"
            }
        );

        return Ok(());
    }

    // Parse format
    let image_format = ImageFormat::from_str(format)
        .context("Invalid image format. Use: raw, qcow2, iso, erofs")?;

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
            ImageFormat::Erofs => {
                println!("  (no external tools required -- composefs-rs builds in userspace)");
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
    match image_format {
        ImageFormat::Erofs => {
            println!("\nThis will create composefs-native output (EROFS + CAS + DB).");
            println!("Output directory: {}", output);
        }
        _ => {
            println!("\nThis will create a bootable {} image.", image_format);
            println!("Image size: {}", image_size);
        }
    }
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
    println!("  Method: {}", result.method);

    match image_format {
        ImageFormat::Erofs => {
            println!("\nOutput layout:");
            for desc in &result.partitions {
                println!("  - {desc}");
            }
            println!("\nThis is generation 1 -- the same artifact type as runtime generations.");
            println!("To boot, wrap in a qcow2 image or deploy to a disk with:");
            println!("  conary bootstrap image -f qcow2 -o conaryos.qcow2");
        }
        _ => {
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
                ImageFormat::Erofs => unreachable!(),
            }
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
            cmd_bootstrap_image(work_dir, "conaryos-base.qcow2", "qcow2", "4G", None).await
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

/// Build Phase 1: Cross-toolchain (LFS Chapter 5)
pub async fn cmd_bootstrap_cross_tools(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 1: Cross-Toolchain (LFS Ch5)...");
    println!("  Work directory: {}", work_dir);
    print_skip_verify_warning(skip_verify);

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
pub async fn cmd_bootstrap_temp_tools(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 2: Temporary Tools (LFS Ch6-7)...");
    println!("  Work directory: {}", work_dir);
    print_skip_verify_warning(skip_verify);

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
pub async fn cmd_bootstrap_system(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 3: Final System (LFS Ch8)...");
    println!("  Work directory: {}", work_dir);
    print_skip_verify_warning(skip_verify);

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
pub async fn cmd_bootstrap_config(
    work_dir: &str,
    verbose: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
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
pub async fn cmd_bootstrap_tier2(
    work_dir: &str,
    jobs: Option<usize>,
    verbose: bool,
    skip_verify: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Building Phase 6: Tier-2 Packages (BLFS + Conary)...");
    println!("  Work directory: {}", work_dir);
    print_skip_verify_warning(skip_verify);

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

/// Package cross-tools output as a derivation seed
pub async fn cmd_bootstrap_seed(from: &str, output: &str, target: &str) -> Result<()> {
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use conary_core::filesystem::CasStore;
    use conary_core::generation::builder::{FileEntryRef, SymlinkEntryRef, build_erofs_image};
    use std::os::unix::fs::MetadataExt;
    use walkdir::WalkDir;

    let from_path = PathBuf::from(from);
    let output_path = PathBuf::from(output);

    // Validate input
    if !from_path.exists() {
        return Err(anyhow::anyhow!(
            "Cross-tools directory not found: {}",
            from_path.display()
        ));
    }
    if !from_path.join("bin").exists() && !from_path.join("lib").exists() {
        return Err(anyhow::anyhow!(
            "Directory does not look like a cross-toolchain (no bin/ or lib/): {}",
            from_path.display()
        ));
    }

    println!("Creating seed from cross-tools output...");
    println!("  Source: {}", from_path.display());
    println!("  Output: {}", output_path.display());
    println!("  Target: {target}");

    // Create output structure
    std::fs::create_dir_all(&output_path)?;
    let cas_dir = output_path.join("cas");
    let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

    // Walk source tree, store files in CAS, collect entries
    let mut file_entries = Vec::new();
    let mut symlink_entries = Vec::new();
    let mut file_count: u64 = 0;

    for entry in WalkDir::new(&from_path).follow_links(false) {
        let entry = entry.context("Failed to walk directory")?;
        let rel_path = entry
            .path()
            .strip_prefix(&from_path)
            .context("Failed to compute relative path")?;

        // Skip the root directory itself
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let abs_path = format!("/tools/{}", rel_path.display());
        let metadata = entry.path().symlink_metadata()?;

        if metadata.is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            symlink_entries.push(SymlinkEntryRef {
                path: abs_path,
                target: link_target.to_string_lossy().to_string(),
            });
        } else if metadata.is_file() {
            let content = std::fs::read(entry.path())?;
            let hash = cas.store(&content).context("CAS store failed")?;
            file_entries.push(FileEntryRef {
                path: abs_path,
                sha256_hash: hash,
                size: metadata.len(),
                permissions: metadata.mode() & 0o7777,
                owner: None,
                group_name: None,
            });
            file_count += 1;
        }
        // Directories are implicit in EROFS
    }

    println!(
        "  Stored {} files, {} symlinks in CAS",
        file_count,
        symlink_entries.len()
    );

    // Build EROFS image
    let gen_dir = output_path.join("gen");
    std::fs::create_dir_all(&gen_dir)?;
    let build_result = build_erofs_image(&file_entries, &symlink_entries, &gen_dir)
        .context("Failed to build EROFS image")?;

    // Move EROFS image to seed.erofs
    let seed_erofs = output_path.join("seed.erofs");
    std::fs::rename(&build_result.image_path, &seed_erofs)?;
    // Clean up temp gen dir
    let _ = std::fs::remove_dir_all(&gen_dir);

    // Compute image hash
    let seed_id = erofs_image_hash(&seed_erofs).context("Failed to hash seed EROFS image")?;

    // Write seed.toml
    let seed_metadata = SeedMetadata {
        seed_id: seed_id.clone(),
        source: SeedSource::SelfBuilt,
        origin_url: None,
        builder: Some("conary-bootstrap".to_string()),
        packages: vec![
            "binutils-pass1".to_string(),
            "gcc-pass1".to_string(),
            "linux-headers".to_string(),
            "glibc".to_string(),
            "libstdcxx".to_string(),
        ],
        target_triple: target.to_string(),
        verified_by: vec![],
        origin_distro: None,
        origin_version: None,
    };

    let toml_str =
        toml::to_string_pretty(&seed_metadata).context("Failed to serialize seed metadata")?;
    std::fs::write(output_path.join("seed.toml"), &toml_str)?;

    println!("\n[OK] Seed created successfully!");
    println!(
        "  EROFS image: {} ({} bytes)",
        seed_erofs.display(),
        build_result.image_size
    );
    println!("  CAS objects: {file_count}");
    println!("  Seed ID: {}", &seed_id[..16]);

    Ok(())
}

/// Load all recipes from subdirectories of `recipe_dir`, returning a `HashMap`
/// keyed by package name. Walks `cross-tools/`, `temp-tools/`, `system/`, `tier2/`.
fn load_recipes(
    recipe_dir: &std::path::Path,
) -> Result<std::collections::HashMap<String, conary_core::recipe::Recipe>> {
    use conary_core::recipe::parser::parse_recipe_file;

    let mut recipes = std::collections::HashMap::new();
    let subdirs = ["cross-tools", "temp-tools", "system", "tier2"];

    for subdir in &subdirs {
        let dir = recipe_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match parse_recipe_file(&path) {
                    Ok(recipe) => {
                        recipes.insert(recipe.package.name.clone(), recipe);
                    }
                    Err(e) => {
                        tracing::warn!("Skipping {}: {e}", path.display());
                    }
                }
            }
        }
    }

    Ok(recipes)
}

fn start_bootstrap_run_record(
    opts: &BootstrapRunOptions<'_>,
    manifest_path: &Path,
    recipe_dir: &Path,
    seed_id: &str,
) -> Result<BootstrapRunRecord> {
    let work_dir = PathBuf::from(opts.work_dir);
    std::fs::create_dir_all(&work_dir)?;

    let mut record = BootstrapRunRecord::started(
        new_operation_id("bootstrap-run"),
        work_dir,
        manifest_path.to_path_buf(),
        recipe_dir.to_path_buf(),
        seed_id.to_string(),
    );
    record.up_to = opts.up_to.map(str::to_owned);
    record.only = opts.only.map(|only| only.to_vec()).unwrap_or_default();
    record.cascade = opts.cascade;

    std::fs::create_dir_all(record.operation_dir())?;
    record.save()?;

    Ok(record)
}

fn link_bootstrap_run_outputs(record: &BootstrapRunRecord) -> Result<()> {
    std::fs::create_dir_all(&record.output_dir)?;
    let run_current_link = record.output_dir.join("current");
    let _ = std::fs::remove_file(&run_current_link);
    std::os::unix::fs::symlink("generations/1", &run_current_link)?;

    let top_output_dir = record.work_dir.join("output");
    std::fs::create_dir_all(&top_output_dir)?;
    let top_current_link = top_output_dir.join("current");
    let _ = std::fs::remove_file(&top_current_link);
    let relative_target = PathBuf::from("..")
        .join("operations")
        .join(&record.id)
        .join("output")
        .join("current");
    std::os::unix::fs::symlink(relative_target, &top_current_link)?;
    Ok(())
}

fn finish_bootstrap_run_success(
    record: &mut BootstrapRunRecord,
    generation_dir: &Path,
    profile_hash: &str,
) -> Result<()> {
    record.generation_dir = Some(generation_dir.to_path_buf());
    record.profile_hash = Some(profile_hash.to_string());
    record.completed_successfully = true;
    record.failure_reason = None;
    record.save()?;
    BootstrapLatestPointer::new(record.id.clone(), record.path())
        .save(&BootstrapLatestPointer::path_for(&record.work_dir))?;
    link_bootstrap_run_outputs(record)?;
    Ok(())
}

fn finish_bootstrap_run_failure(
    record: &mut BootstrapRunRecord,
    error: &anyhow::Error,
) -> Result<()> {
    record.completed_successfully = false;
    record.failure_reason = Some(error.to_string());
    record.save()
}

fn load_completed_bootstrap_run_record(work_dir: &Path) -> Result<BootstrapRunRecord> {
    let pointer_path = BootstrapLatestPointer::path_for(work_dir);
    let latest = BootstrapLatestPointer::load(&pointer_path).with_context(|| {
        format!(
            "Failed to load bootstrap latest pointer from {}",
            pointer_path.display()
        )
    })?;
    let record = BootstrapRunRecord::load(&latest.record_path).with_context(|| {
        format!(
            "Failed to load bootstrap run record from {}",
            latest.record_path.display()
        )
    })?;
    if record.id != latest.operation_id {
        anyhow::bail!(
            "Bootstrap latest pointer {} does not match record id {}",
            latest.operation_id,
            record.id
        );
    }
    if !record.completed_successfully {
        anyhow::bail!(
            "Bootstrap run {} in {} did not complete successfully",
            record.id,
            work_dir.display()
        );
    }
    Ok(record)
}

/// Options for the `bootstrap run` command.
pub struct BootstrapRunOptions<'a> {
    /// Path to system manifest TOML.
    pub manifest: &'a str,
    /// Working directory for build artifacts.
    pub work_dir: &'a str,
    /// Path to seed directory.
    pub seed: &'a str,
    /// Recipe directory.
    pub recipe_dir: &'a str,
    /// Stop after completing this stage.
    pub up_to: Option<&'a str>,
    /// Only build these packages.
    pub only: Option<&'a [String]>,
    /// Also rebuild reverse dependents of `only` targets.
    pub cascade: bool,
    /// Preserve build logs for successful builds.
    pub keep_logs: bool,
    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
    /// Show verbose build output.
    pub verbose: bool,
    /// Skip remote substituters.
    pub no_substituters: bool,
    /// Auto-publish successful builds.
    pub publish: bool,
}

/// Run the derivation pipeline from a system manifest.
///
/// Loads the manifest, seed, and recipes, assigns stages, then executes the
/// full derivation pipeline. Writes generation output (EROFS image, metadata,
/// profile) and creates a `current` symlink.
pub async fn cmd_bootstrap_run(opts: BootstrapRunOptions<'_>) -> Result<()> {
    use conary_core::db::schema::migrate;
    use conary_core::derivation::build_order::Stage;
    use conary_core::derivation::build_order::compute_build_order;
    use conary_core::derivation::executor::{DerivationExecutor, ExecutorConfig};
    use conary_core::derivation::manifest::SystemManifest;
    use conary_core::derivation::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
    use conary_core::derivation::seed::Seed;
    use conary_core::filesystem::CasStore;
    use rusqlite::Connection;
    use std::collections::HashSet;

    info!(
        "bootstrap run: manifest={}, work_dir={}, seed={}",
        opts.manifest, opts.work_dir, opts.seed
    );

    if opts.verbose {
        println!("  manifest: {}", opts.manifest);
        println!("  work_dir: {}", opts.work_dir);
        println!("  seed: {}", opts.seed);
        println!("  recipe_dir: {}", opts.recipe_dir);
        if let Some(s) = opts.up_to {
            println!("  up_to: {s}");
        }
        if opts.no_substituters {
            println!("  substituters: disabled");
        }
        if opts.publish {
            println!("  publish: enabled");
        }
    }

    // 1. Load manifest
    let manifest_path = PathBuf::from(opts.manifest);
    let manifest =
        SystemManifest::load(&manifest_path).context("Failed to load system manifest")?;
    println!(
        "System: {} ({})",
        manifest.system.name, manifest.system.target
    );
    println!("Packages: {} included", manifest.packages.include.len());

    // 2. Load seed
    let seed_path = PathBuf::from(opts.seed);
    let seed =
        Seed::load_local(&seed_path).map_err(|e| anyhow::anyhow!("Failed to load seed: {e}"))?;
    println!(
        "Seed: {} ({})",
        &seed.build_env_hash()[..16],
        seed_path.display()
    );

    // 3. Load recipes and filter to manifest includes + transitive deps
    let recipe_dir = PathBuf::from(opts.recipe_dir);
    let all_recipes = load_recipes(&recipe_dir)?;
    println!("Recipes loaded: {}", all_recipes.len());

    let included: HashSet<String> = manifest.packages.include.iter().cloned().collect();
    let mut needed: HashSet<String> = included.clone();
    let mut frontier: Vec<String> = included.into_iter().collect();
    while let Some(pkg) = frontier.pop() {
        if let Some(recipe) = all_recipes.get(&pkg) {
            for dep in recipe
                .build
                .requires
                .iter()
                .chain(recipe.build.makedepends.iter())
            {
                if needed.insert(dep.clone()) {
                    frontier.push(dep.clone());
                }
            }
        }
    }

    let recipes: std::collections::HashMap<String, conary_core::recipe::Recipe> = all_recipes
        .into_iter()
        .filter(|(name, _)| needed.contains(name))
        .collect();
    println!("Recipes after dep resolution: {}", recipes.len());

    // 4. Compute build order
    let custom_packages: HashSet<String> = HashSet::new();
    let mut build_steps = compute_build_order(&recipes, &custom_packages)
        .map_err(|e| anyhow::anyhow!("Build order computation failed: {e}"))?;
    println!("Build order: {} packages", build_steps.len());

    // Apply --up-to filter: drop packages in stages beyond the cutoff.
    if let Some(ref up_to) = opts.up_to {
        let cutoff = Stage::from_str_name(up_to)
            .ok_or_else(|| anyhow::anyhow!("invalid --up-to stage: {up_to}"))?;
        build_steps.retain(|step| step.stage <= cutoff);
        println!("After --up-to {up_to}: {} packages", build_steps.len());
    }

    let mut record =
        start_bootstrap_run_record(&opts, &manifest_path, &recipe_dir, seed.build_env_hash())?;
    let op_dir = record.operation_dir();
    let output_dir = record.output_dir.clone();

    let run_result: Result<(PathBuf, String)> = async {
        // 5. Open DB
        let conn = Connection::open(&record.derivation_db_path)
            .context("Failed to open derivation database")?;
        migrate(&conn).context("Failed to run database migrations")?;

        // 6. Create CAS and executor
        let cas_dir = output_dir.join("objects");
        std::fs::create_dir_all(&cas_dir)?;
        let cas = CasStore::new(&cas_dir).context("Failed to create CAS store")?;

        let executor_config = ExecutorConfig {
            log_dir: Some(op_dir.join("logs")),
            keep_logs: opts.keep_logs,
            shell_on_failure: opts.shell_on_failure,
        };
        let executor = DerivationExecutor::new(cas, cas_dir.clone(), executor_config);

        // 7. Create pipeline
        let pipeline_config = PipelineConfig {
            cas_dir: cas_dir.clone(),
            work_dir: op_dir.join("pipeline"),
            target_triple: manifest.system.target.clone(),
            jobs: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            log_dir: Some(op_dir.join("logs")),
            keep_logs: opts.keep_logs,
            shell_on_failure: opts.shell_on_failure,
            only_packages: opts.only.map(|s| s.to_vec()),
            cascade: opts.cascade,
            substituter_sources: if opts.no_substituters {
                vec![]
            } else {
                manifest
                    .substituters
                    .as_ref()
                    .map(|s| s.sources.clone())
                    .unwrap_or_default()
            },
            publish_endpoint: if opts.publish {
                Some("https://remi.conary.io".to_string())
            } else {
                None
            },
            publish_token: None,
        };

        std::fs::create_dir_all(&pipeline_config.work_dir)?;
        let pipeline = Pipeline::new(pipeline_config, executor);

        // 8. Execute pipeline
        println!("\nStarting derivation pipeline...\n");
        let profile = pipeline
            .execute(&seed, &recipes, &build_steps, &conn, |event| match event {
                PipelineEvent::StageStarted {
                    name,
                    package_count,
                } => {
                    println!("[{name}] Stage started ({package_count} packages)");
                }
                PipelineEvent::PackageBuilding { name, stage } => {
                    println!("[{stage}] Building {name}...");
                }
                PipelineEvent::PackageCached { name } => {
                    println!("  [cached] {name}");
                }
                PipelineEvent::PackageBuilt {
                    name,
                    duration_secs,
                } => {
                    println!("  [built] {name} in {duration_secs}s");
                }
                PipelineEvent::PackageFailed { name, error } => {
                    println!("  [FAILED] {name}: {error}");
                }
                PipelineEvent::SubstituterHit {
                    name,
                    peer,
                    objects_fetched,
                } => {
                    println!("  [substituted] {name} from {peer} ({objects_fetched} objects)");
                }
                PipelineEvent::BuildLogWritten { package, path } => {
                    println!("  [log] {package}: {}", path.display());
                }
                PipelineEvent::StageCompleted { name } => {
                    println!("[{name}] Stage completed\n");
                }
                PipelineEvent::PipelineCompleted {
                    total_packages,
                    cached,
                    built,
                } => {
                    println!(
                        "[COMPLETE] {total_packages} packages processed ({built} built, {cached} cached)"
                    );
                }
            })
            .await?;

        // 9. Write generation output
        let gen_dir = output_dir.join("generations").join("1");
        std::fs::create_dir_all(&gen_dir)?;

        let compose_erofs = op_dir.join("pipeline").join("compose").join("root.erofs");
        let stage_erofs = profile.stages.last().map(|stage| {
            op_dir
                .join("pipeline")
                .join(format!("stage-{}", stage.name))
                .join("root.erofs")
        });
        let erofs_source = if compose_erofs.exists() {
            Some(compose_erofs)
        } else {
            stage_erofs.filter(|p| p.exists())
        };
        if let Some(src) = erofs_source {
            let dest = gen_dir.join("root.erofs");
            std::fs::copy(&src, &dest)?;
            println!("Generation 1 EROFS: {}", dest.display());
        } else {
            tracing::warn!(
                "No EROFS image found in pipeline output -- generation may be incomplete"
            );
        }

        let gen_meta = serde_json::json!({
            "generation": 1,
            "system_name": manifest.system.name,
            "target": manifest.system.target,
            "packages": profile.stages.iter()
                .flat_map(|s| s.derivations.iter())
                .map(|d| format!("{}-{}", d.package, d.version))
                .collect::<Vec<_>>(),
            "profile_hash": profile.profile.profile_hash,
        });
        std::fs::write(
            gen_dir.join(".conary-gen.json"),
            serde_json::to_string_pretty(&gen_meta)?,
        )?;

        let profile_hash = profile.profile.profile_hash.clone();
        let profile_toml = toml::to_string_pretty(&profile)?;
        std::fs::write(gen_dir.join("profile.toml"), &profile_toml)?;

        Ok((gen_dir, profile_hash))
    }
    .await;

    match run_result {
        Ok((gen_dir, profile_hash)) => {
            finish_bootstrap_run_success(&mut record, &gen_dir, &profile_hash)?;
            println!("\nOutput: {}", output_dir.display());
            println!("Profile hash: {profile_hash}");
            Ok(())
        }
        Err(error) => {
            finish_bootstrap_run_failure(&mut record, &error)?;
            Err(error)
        }
    }
}

/// Create a seed from the currently adopted system filesystem
pub async fn cmd_bootstrap_seed_adopted(
    output: &str,
    distro: Option<&str>,
    distro_version: Option<&str>,
) -> Result<()> {
    use conary_core::bootstrap::adopt_seed;

    let distro_name = distro.unwrap_or("unknown");
    let version = distro_version.unwrap_or("unknown");

    println!("Building adopted seed from system filesystem...");
    println!("  Distro: {distro_name} {version}");
    println!("  Output: {output}");

    let meta = adopt_seed::build_adopted_seed(std::path::Path::new(output), distro_name, version)?;

    println!("[COMPLETE] Seed built: {}", meta.seed_id);
    Ok(())
}

/// Verify convergence between builds from two different seeds
pub async fn cmd_bootstrap_verify_convergence(
    run_a: &str,
    run_b: &str,
    seed_a: Option<&str>,
    seed_b: Option<&str>,
    diff: bool,
) -> Result<()> {
    use conary_core::derivation::{Seed, compare_build_sets, load_build_set};
    use rusqlite::Connection;

    let record_a = load_completed_bootstrap_run_record(Path::new(run_a))?;
    let record_b = load_completed_bootstrap_run_record(Path::new(run_b))?;

    if let Some(seed_path) = seed_a {
        let seed = Seed::load_local(Path::new(seed_path))
            .map_err(|error| anyhow::anyhow!("Failed to load seed A: {error}"))?;
        if seed.build_env_hash() != record_a.seed_id {
            anyhow::bail!(
                "Seed A hash {} does not match run A seed {}",
                seed.build_env_hash(),
                record_a.seed_id
            );
        }
    }
    if let Some(seed_path) = seed_b {
        let seed = Seed::load_local(Path::new(seed_path))
            .map_err(|error| anyhow::anyhow!("Failed to load seed B: {error}"))?;
        if seed.build_env_hash() != record_b.seed_id {
            anyhow::bail!(
                "Seed B hash {} does not match run B seed {}",
                seed.build_env_hash(),
                record_b.seed_id
            );
        }
    }

    let conn_a = Connection::open(&record_a.derivation_db_path)
        .with_context(|| format!("Failed to open {}", record_a.derivation_db_path.display()))?;
    let conn_b = Connection::open(&record_b.derivation_db_path)
        .with_context(|| format!("Failed to open {}", record_b.derivation_db_path.display()))?;

    let builds_a = load_build_set(&conn_a, &record_a.seed_id)
        .map_err(|error| anyhow::anyhow!("Failed to load build set A: {error}"))?;
    let builds_b = load_build_set(&conn_b, &record_b.seed_id)
        .map_err(|error| anyhow::anyhow!("Failed to load build set B: {error}"))?;
    let report = compare_build_sets(builds_a, builds_b);

    if report.total() == 0 {
        anyhow::bail!(
            "No comparable packages found between {} and {}",
            run_a,
            run_b
        );
    }

    println!("Compared {} packages", report.total());
    println!("Matched: {}", report.matched());
    println!("Mismatched: {}", report.mismatched());
    println!("Skipped: {}", report.skipped_total());
    println!("Only in run A: {}", report.only_in_a().len());
    println!("Only in run B: {}", report.only_in_b().len());

    if diff {
        if !report.mismatches().is_empty() {
            println!("\nMismatched packages:");
            for mismatch in report.mismatches() {
                println!(
                    "  {}: {} != {}",
                    mismatch.package, mismatch.hash_a, mismatch.hash_b
                );
            }
        }
        if !report.only_in_a().is_empty() {
            println!("\nOnly in run A:");
            for package in report.only_in_a() {
                println!("  {package}");
            }
        }
        if !report.only_in_b().is_empty() {
            println!("\nOnly in run B:");
            for package in report.only_in_b() {
                println!("  {package}");
            }
        }
    }

    if report.mismatched() > 0 {
        anyhow::bail!(
            "Convergence verification failed with {} mismatched packages",
            report.mismatched()
        );
    }

    println!("[COMPLETE] All compared packages converged.");
    Ok(())
}

/// Diff two seed EROFS images
pub async fn cmd_bootstrap_diff_seeds(path_a: &str, path_b: &str) -> Result<()> {
    let report = conary_core::derivation::diff_seed_dirs(Path::new(path_a), Path::new(path_b))
        .map_err(|error| anyhow::anyhow!("Failed to diff seeds: {error}"))?;

    println!("Seed A: {path_a}");
    println!("Seed B: {path_b}");

    match (&report.erofs_hash_a, &report.erofs_hash_b) {
        (Some(hash_a), Some(hash_b)) => {
            println!("EROFS hash A: {hash_a}");
            println!("EROFS hash B: {hash_b}");
        }
        (Some(hash_a), None) => {
            println!("EROFS hash A: {hash_a}");
            println!("EROFS hash B: <missing>");
        }
        (None, Some(hash_b)) => {
            println!("EROFS hash A: <missing>");
            println!("EROFS hash B: {hash_b}");
        }
        (None, None) => {
            println!("EROFS hash A: <missing>");
            println!("EROFS hash B: <missing>");
        }
    }

    if report.metadata_differences.is_empty()
        && report.artifact_differences.is_empty()
        && report.erofs_hash_a == report.erofs_hash_b
    {
        println!("[COMPLETE] No metadata, artifact, or hash differences found.");
        return Ok(());
    }

    if !report.metadata_differences.is_empty() {
        println!("\nMetadata differences:");
        for line in &report.metadata_differences {
            println!("  {line}");
        }
    }

    if !report.artifact_differences.is_empty() {
        println!("\nArtifact differences:");
        for line in &report.artifact_differences {
            println!("  {line}");
        }
    }

    Ok(())
}

/// Clean bootstrap work directory
pub async fn cmd_bootstrap_clean(
    work_dir: &str,
    stage: Option<String>,
    sources: bool,
) -> Result<()> {
    println!("Cleaning bootstrap work directory...");
    println!("  Work directory: {}", work_dir);

    let work_path = PathBuf::from(work_dir);

    if !work_path.exists() {
        println!("Work directory does not exist.");
        return Ok(());
    }

    if let Some(ref stage_name) = stage {
        // Validate stage name: only allow known stage directory names to
        // prevent path traversal (absolute paths, ".." segments, etc.).
        const ALLOWED_STAGES: &[&str] =
            &["cross-tools", "temp-tools", "system", "image", "sources"];
        if !ALLOWED_STAGES.contains(&stage_name.as_str()) {
            anyhow::bail!(
                "Invalid stage '{}'. Allowed stages: {}",
                stage_name,
                ALLOWED_STAGES.join(", ")
            );
        }

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        BootstrapLatestPointer, BootstrapRunOptions, BootstrapRunRecord,
        finish_bootstrap_run_success, skip_verify_warning_message, start_bootstrap_run_record,
    };

    #[test]
    fn skip_verify_warning_message_is_prominent() {
        let warning = skip_verify_warning_message();
        assert!(warning.contains("UNSAFE"));
        assert!(warning.contains("--skip-verify"));
        assert!(warning.contains("placeholder"));
    }

    #[test]
    fn test_bootstrap_run_writes_success_record_with_output_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("system.toml");
        let recipe_dir = temp.path().join("recipes");
        std::fs::create_dir_all(&recipe_dir).expect("recipe dir");
        std::fs::write(
            &manifest_path,
            "[system]\nname = 'test'\ntarget = 'x86_64-conary-linux-gnu'\n",
        )
        .expect("manifest");

        let only = vec!["bash".to_string(), "coreutils".to_string()];
        let opts = BootstrapRunOptions {
            manifest: manifest_path.to_str().expect("manifest path"),
            work_dir: temp.path().to_str().expect("work dir"),
            seed: "/tmp/seed",
            recipe_dir: recipe_dir.to_str().expect("recipe dir"),
            up_to: Some("system"),
            only: Some(&only),
            cascade: true,
            keep_logs: false,
            shell_on_failure: false,
            verbose: false,
            no_substituters: false,
            publish: false,
        };

        let mut record = start_bootstrap_run_record(&opts, &manifest_path, &recipe_dir, "seed-abc")
            .expect("start record");
        let generation_dir = record.output_dir.join("generations").join("1");
        std::fs::create_dir_all(&generation_dir).expect("generation dir");

        finish_bootstrap_run_success(&mut record, &generation_dir, "profile-xyz")
            .expect("finish record");

        let loaded = BootstrapRunRecord::load(&record.path()).expect("load record");
        let latest = BootstrapLatestPointer::load(&BootstrapLatestPointer::path_for(temp.path()))
            .expect("load latest");

        assert_eq!(loaded.manifest_path, manifest_path);
        assert_eq!(loaded.recipe_dir, recipe_dir);
        assert_eq!(loaded.seed_id, "seed-abc");
        assert_eq!(loaded.up_to.as_deref(), Some("system"));
        assert_eq!(loaded.only, only);
        assert!(loaded.cascade);
        assert_eq!(
            loaded.derivation_db_path,
            loaded.operation_dir().join("derivations.db")
        );
        assert_eq!(loaded.output_dir, loaded.operation_dir().join("output"));
        assert_eq!(loaded.generation_dir, Some(generation_dir.clone()));
        assert_eq!(loaded.profile_hash.as_deref(), Some("profile-xyz"));
        assert!(loaded.completed_successfully);
        assert_eq!(latest.operation_id, loaded.id);
        assert_eq!(latest.record_path, loaded.path());
        assert_eq!(
            std::fs::read_link(loaded.output_dir.join("current")).expect("run current link"),
            PathBuf::from("generations").join("1")
        );
        assert_eq!(
            std::fs::read_link(temp.path().join("output/current")).expect("top current link"),
            PathBuf::from("..")
                .join("operations")
                .join(&loaded.id)
                .join("output")
                .join("current")
        );
    }
}
