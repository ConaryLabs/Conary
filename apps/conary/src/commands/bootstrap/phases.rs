// apps/conary/src/commands/bootstrap/phases.rs

use std::path::Path;

use anyhow::Result;
use conary_core::bootstrap::{Bootstrap, BootstrapConfig};

fn skip_verify_warning_message() -> &'static str {
    "WARNING: UNSAFE bootstrap mode enabled via --skip-verify. placeholder source checksums will be accepted, so only use this during an authenticated bootstrap flow where you independently trust the source tarballs."
}
fn print_skip_verify_warning(skip_verify: bool) {
    if skip_verify {
        eprintln!("{}", skip_verify_warning_message());
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

    println!("\nThis will build all 82 packages of the final LFS system.\n");

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
/// Apply the self-host guest profile to the built sysroot.
pub async fn cmd_bootstrap_guest_profile(
    work_dir: &str,
    public_key: &str,
    verbose: bool,
    lfs_root: Option<&str>,
) -> Result<()> {
    println!("Applying self-host guest profile...");
    println!("  Work directory: {}", work_dir);
    println!("  Public key: {}", public_key);

    let mut config = BootstrapConfig::new().with_verbose(verbose);
    if let Some(root) = lfs_root {
        config = config.with_lfs_root(root);
    }

    println!("  LFS root: {}", config.lfs_root.display());

    let bootstrap = Bootstrap::with_config(work_dir, config)?;
    bootstrap.apply_guest_profile(Path::new(public_key))?;

    println!("\n[OK] Self-host guest profile applied successfully!");
    println!("  The sysroot now has SSH-ready test posture for VM validation.");

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

#[cfg(test)]
mod tests {
    use super::skip_verify_warning_message;

    #[test]
    fn skip_verify_warning_message_is_prominent() {
        let warning = skip_verify_warning_message();
        assert!(warning.contains("UNSAFE"));
        assert!(warning.contains("--skip-verify"));
        assert!(warning.contains("placeholder"));
    }
}
