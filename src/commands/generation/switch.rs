// src/commands/generation/switch.rs
//! Generation switching via composefs mounts
//!
//! CLI wrapper around `conary_core::generation::mount`. Core mount/unmount
//! logic lives in the core crate; this file handles CLI output and the
//! step-by-step orchestration that is specific to live system switching.

use super::metadata::{GenerationMetadata, generation_path};
use anyhow::{Context, Result, anyhow};
use conary_core::generation::mount::{
    MountOptions, is_overlay_mount, mount_generation, unmount_generation, update_current_symlink,
};
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// Switch the live system to the specified generation using composefs mounts.
///
/// 1. Mount the new generation's EROFS image via composefs
/// 2. Bind-mount /usr from the composefs tree (read-only)
/// 3. Rebuild /etc overlay with new composefs lower
/// 4. Update /conary/current symlink
pub fn switch_live(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    if !gen_dir.exists() {
        return Err(anyhow!(
            "Generation {gen_number} does not exist at {}",
            gen_dir.display()
        ));
    }

    let metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let erofs_img = gen_dir.join("root.erofs");
    if !erofs_img.exists() {
        return Err(anyhow!(
            "EROFS image not found at {} (format: {})",
            erofs_img.display(),
            metadata.format
        ));
    }

    let cas_dir = "/conary/objects";
    let old_mnt = "/conary/mnt";
    let staging = "/conary/mnt-new";

    // Step 0: Unmount old composefs mount if present
    if Path::new(old_mnt).exists() {
        if let Err(e) = unmount_generation(Path::new(old_mnt)) {
            warn!("Failed to unmount old composefs at {old_mnt}: {e}");
        }
    }

    // Step 1: Mount new generation's composefs at staging point
    std::fs::create_dir_all(staging).context("Failed to create composefs staging directory")?;

    // Try with verity first, fall back without
    let opts_verity = MountOptions {
        image_path: erofs_img.clone(),
        basedir: cas_dir.into(),
        mount_point: staging.into(),
        verity: true,
        digest: metadata.erofs_verity_digest.clone(),
        upperdir: None,
        workdir: None,
    };
    let opts_plain = MountOptions {
        verity: false,
        digest: None,
        ..opts_verity.clone()
    };

    mount_generation(&opts_verity)
        .or_else(|_| {
            warn!("composefs mount with verity failed, retrying without");
            mount_generation(&opts_plain)
        })
        .map_err(|e| anyhow!("Failed to mount composefs image: {e}"))?;

    // Step 2: Bind-mount /usr from composefs tree (read-only)
    // WARNING: This overwrites the live /usr. Processes with open file descriptors
    // under /usr may crash or fail to load libraries. A reboot is recommended
    // for full consistency (printed at the end of this function).
    let mnt_usr = format!("{staging}/usr");
    if let Err(e) = run_command("mount", &["--bind", &mnt_usr, "/usr"]) {
        let _ = unmount_generation(Path::new(staging));
        return Err(e).context("Failed to bind-mount /usr from composefs");
    }
    if let Err(e) = run_command("mount", &["-o", "remount,ro", "/usr"]) {
        let _ = run_command("umount", &["/usr"]);
        let _ = unmount_generation(Path::new(staging));
        return Err(e).context("Failed to remount /usr read-only");
    }

    info!("Bind-mounted /usr from generation {gen_number} (read-only)");

    // Step 3: Rebuild /etc overlay with new lower
    let staging_etc = format!("{staging}/etc");
    // Unmount existing /etc overlay only if it is currently an overlay mount.
    // If /etc is not an overlay, unmounting it would leave the system with no /etc.
    let etc_is_overlay = is_overlay_mount(Path::new("/etc")).unwrap_or(false);
    if etc_is_overlay {
        let _ = run_command("umount", &["/etc"]);
    }

    std::fs::create_dir_all("/conary/etc-state/upper")
        .context("Failed to create /etc overlay upper dir")?;
    std::fs::create_dir_all("/conary/etc-state/work")
        .context("Failed to create /etc overlay work dir")?;

    let etc_opts = format!(
        "lowerdir={staging_etc},upperdir=/conary/etc-state/upper,workdir=/conary/etc-state/work"
    );
    match run_command(
        "mount",
        &["-t", "overlay", "overlay", "/etc", "-o", &etc_opts],
    ) {
        Ok(()) => info!("Mounted /etc overlay with composefs lower"),
        Err(e) => {
            warn!("Failed to mount /etc overlay: {e}; /etc may be stale");
            // Non-fatal — /etc is still readable from the old generation
        }
    }

    // Step 4: Move staging mount to permanent mount point
    std::fs::create_dir_all(old_mnt).context("Failed to create permanent composefs mount dir")?;
    if let Err(e) = run_command("mount", &["--move", staging, old_mnt]) {
        let _ = run_command("umount", &["/usr"]);
        let _ = unmount_generation(Path::new(staging));
        return Err(e).context("Failed to move composefs mount to permanent location");
    }

    // Step 5: Update current symlink (delegates to conary-core)
    update_current_symlink(Path::new("/conary"), gen_number)
        .map_err(|e| anyhow!("Failed to update current generation symlink: {e}"))?;

    info!("Switched to generation {gen_number} (composefs)");
    println!("Switched to generation {gen_number}. Reboot recommended for full consistency.");

    Ok(())
}

/// Run a simple command, returning Ok on success.
fn run_command(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {cmd}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{cmd} exited with status {status}"))
    }
}
