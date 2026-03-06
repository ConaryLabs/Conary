// src/commands/generation/switch.rs
//! Generation switching via composefs mounts
//!
//! Replaces the old renameat2-based directory exchange with composefs
//! mount-based switching. The EROFS image is mounted via composefs,
//! /usr is bind-mounted read-only, and /etc uses an overlayfs.

use super::metadata::{current_link, generation_path, GenerationMetadata};
use anyhow::{Context, Result, anyhow};
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
    if std::path::Path::new(old_mnt).exists()
        && let Err(e) = run_command("umount", &[old_mnt])
    {
        warn!("Failed to unmount old composefs at {old_mnt}: {e}");
    }

    // Step 1: Mount new generation's composefs at staging point
    std::fs::create_dir_all(staging)
        .context("Failed to create composefs staging directory")?;

    // Try with verity_check first, fall back without
    let mount_opts_verity = format!("basedir={cas_dir},verity_check=1");
    let mount_opts_plain = format!("basedir={cas_dir}");

    let mounted = run_mount_composefs(&erofs_img.to_string_lossy(), staging, &mount_opts_verity)
        .or_else(|_| {
            warn!("composefs mount with verity_check failed, retrying without");
            run_mount_composefs(&erofs_img.to_string_lossy(), staging, &mount_opts_plain)
        })
        .context("Failed to mount composefs image")?;

    if !mounted {
        return Err(anyhow!("composefs mount command failed"));
    }

    // Step 2: Bind-mount /usr from composefs tree (read-only)
    let mnt_usr = format!("{staging}/usr");
    if let Err(e) = run_command("mount", &["--bind", &mnt_usr, "/usr"]) {
        // Clean up staging composefs mount before returning error
        let _ = run_command("umount", &[staging]);
        return Err(e).context("Failed to bind-mount /usr from composefs");
    }
    if let Err(e) = run_command("mount", &["-o", "remount,ro", "/usr"]) {
        let _ = run_command("umount", &["/usr"]);
        let _ = run_command("umount", &[staging]);
        return Err(e).context("Failed to remount /usr read-only");
    }

    info!("Bind-mounted /usr from generation {gen_number} (read-only)");

    // Step 3: Rebuild /etc overlay with new lower
    let staging_etc = format!("{staging}/etc");
    // Unmount existing /etc overlay (may fail if busy, that's ok)
    let _ = run_command("umount", &["/etc"]);

    std::fs::create_dir_all("/conary/etc-state/upper")
        .context("Failed to create /etc overlay upper dir")?;
    std::fs::create_dir_all("/conary/etc-state/work")
        .context("Failed to create /etc overlay work dir")?;

    let etc_opts = format!(
        "lowerdir={staging_etc},upperdir=/conary/etc-state/upper,workdir=/conary/etc-state/work"
    );
    match run_command("mount", &["-t", "overlay", "overlay", "/etc", "-o", &etc_opts]) {
        Ok(()) => info!("Mounted /etc overlay with composefs lower"),
        Err(e) => {
            warn!("Failed to mount /etc overlay: {e}; /etc may be stale");
            // Non-fatal — /etc is still readable from the old generation
        }
    }

    // Step 4: Move staging mount to permanent mount point
    std::fs::create_dir_all(old_mnt)
        .context("Failed to create permanent composefs mount dir")?;
    if let Err(e) = run_command("mount", &["--move", staging, old_mnt]) {
        // Clean up mounts before returning error
        let _ = run_command("umount", &["/usr"]);
        let _ = run_command("umount", &[staging]);
        return Err(e).context("Failed to move composefs mount to permanent location");
    }

    // Step 5: Update current symlink
    update_current_symlink(gen_number)
        .context("Failed to update current generation symlink")?;

    info!("Switched to generation {gen_number} (composefs)");
    println!("Switched to generation {gen_number}. Reboot recommended for full consistency.");

    Ok(())
}

/// Mount a composefs image at the given mountpoint.
fn run_mount_composefs(image: &str, mountpoint: &str, opts: &str) -> Result<bool> {
    let status = Command::new("mount")
        .args(["-t", "composefs", image, mountpoint, "-o", opts])
        .status()
        .context("Failed to execute mount command")?;
    Ok(status.success())
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

/// Atomically update the `/conary/current` symlink to point to the given generation.
///
/// Creates a temporary symlink and renames it over the existing one for atomicity.
pub fn update_current_symlink(gen_number: i64) -> Result<()> {
    let link = current_link();
    let target = generation_path(gen_number);
    let tmp_link = link.with_extension("tmp");

    // Remove stale temp link if it exists
    let _ = std::fs::remove_file(&tmp_link);

    std::os::unix::fs::symlink(&target, &tmp_link).with_context(|| {
        format!(
            "Failed to create temp symlink {} -> {}",
            tmp_link.display(),
            target.display()
        )
    })?;

    std::fs::rename(&tmp_link, &link).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            tmp_link.display(),
            link.display()
        )
    })?;

    info!(
        "Updated {} -> {}",
        link.display(),
        target.display()
    );
    Ok(())
}

/// Read the current generation number from the `/conary/current` symlink.
///
/// Returns `None` if the symlink does not exist.
pub fn current_generation() -> Result<Option<i64>> {
    let link = current_link();

    if !link.exists() {
        return Ok(None);
    }

    let target = std::fs::read_link(&link)
        .with_context(|| format!("Failed to read symlink {}", link.display()))?;

    let component = target
        .file_name()
        .ok_or_else(|| anyhow!("Symlink target has no filename: {}", target.display()))?
        .to_string_lossy();

    let gen_number: i64 = component
        .parse()
        .with_context(|| format!("Failed to parse generation number from '{component}'"))?;

    Ok(Some(gen_number))
}
