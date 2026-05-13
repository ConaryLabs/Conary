// src/commands/generation/switch.rs
//! Generation switching via composefs mounts
//!
//! CLI wrapper around `conary_core::generation::mount`. Core mount/unmount
//! logic lives in the core crate; this file handles CLI output and the
//! step-by-step orchestration that is specific to live system switching.

use crate::commands::generation::builder::requested_generation_verity;
use anyhow::{Context, Result, anyhow};
use conary_core::generation::artifact::load_generation_artifact;
use conary_core::generation::mount::{
    MountOptions, is_overlay_mount, mount_generation, unmount_generation, update_current_symlink,
    verity_downgrade_warning,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// Developer-only live switch helper.
///
/// Release-facing generation activation selects the next boot generation
/// instead of attempting to make a running process tree coherent in place.
///
/// 1. Mount the new generation's EROFS image via composefs
/// 2. Bind-mount /usr from the composefs tree (read-only)
/// 3. Rebuild /etc overlay with new composefs lower
/// 4. Update /conary/current symlink
#[allow(dead_code)]
pub fn switch_live(gen_number: i64) -> Result<()> {
    let runtime_root = ConaryRuntimeRoot::default();
    let gen_dir = runtime_root.generation_path(gen_number);
    if !gen_dir.exists() {
        return Err(anyhow!(
            "Generation {gen_number} does not exist at {}",
            gen_dir.display()
        ));
    }

    let artifact = load_generation_artifact(&gen_dir).with_context(|| {
        format!("Generation {gen_number} is not an activatable composefs artifact")
    })?;
    let metadata = artifact.metadata;

    let erofs_img = artifact.erofs_path;
    let cas_dir = artifact.cas_dir;
    let old_mnt = runtime_root.mount_dir();
    let staging = runtime_root.root().join("mnt-new");
    let old_mnt_display = old_mnt.display().to_string();
    let staging_display = staging.display().to_string();

    // Step 0: Unmount old composefs mount if present
    if old_mnt.exists()
        && let Err(e) = unmount_generation(&old_mnt)
    {
        warn!("Failed to unmount old composefs at {old_mnt_display}: {e}");
    }

    // Step 1: Mount new generation's composefs at staging point
    std::fs::create_dir_all(&staging).context("Failed to create composefs staging directory")?;

    let requested_verity = requested_generation_verity(
        metadata.erofs_verity_digest.as_deref(),
        metadata.fsverity_enabled,
    );

    // Try with verity first when the generation metadata proves the image is ready.
    let opts_verity = MountOptions {
        image_path: erofs_img.clone(),
        basedir: cas_dir,
        mount_point: staging.clone(),
        verity: requested_verity,
        digest: if requested_verity {
            metadata.erofs_verity_digest.clone()
        } else {
            None
        },
        upperdir: None,
        workdir: None,
    };
    let opts_plain = MountOptions {
        verity: false,
        digest: None,
        ..opts_verity.clone()
    };

    let mount_outcome = if requested_verity {
        mount_generation(&opts_verity).map_err(|error| {
            if matches!(&error, conary_core::Error::ChecksumMismatch { .. }) {
                anyhow!("EROFS verity digest mismatch: {error}")
            } else {
                anyhow!(
                    "Failed to mount composefs image with requested fs-verity; no plain composefs downgrade attempted: {error}"
                )
            }
        })?
    } else {
        mount_generation(&opts_plain)
            .map_err(|e| anyhow!("Failed to mount composefs image: {e}"))?
    };
    if let Some(message) = verity_downgrade_warning(requested_verity, mount_outcome, &erofs_img) {
        warn!("{message}");
        eprintln!("Warning: {message}");
    }

    // Step 2: Bind-mount /usr from composefs tree (read-only)
    // WARNING: This overwrites the live /usr. Processes with open file descriptors
    // under /usr may crash or fail to load libraries. A reboot is recommended
    // for full consistency (printed at the end of this function).
    let mnt_usr = staging.join("usr").display().to_string();
    if let Err(e) = run_command("mount", &["--bind", &mnt_usr, "/usr"]) {
        let _ = unmount_generation(&staging);
        return Err(e).context("Failed to bind-mount /usr from composefs");
    }
    if let Err(e) = run_command("mount", &["-o", "remount,ro", "/usr"]) {
        let _ = run_command("umount", &["/usr"]);
        let _ = unmount_generation(&staging);
        return Err(e).context("Failed to remount /usr read-only");
    }

    info!("Bind-mounted /usr from generation {gen_number} (read-only)");

    // Step 3: Rebuild /etc overlay with new lower
    let staging_etc = staging.join("etc").display().to_string();
    // Unmount existing /etc overlay only if it is currently an overlay mount.
    // If /etc is not an overlay, unmounting it would leave the system with no /etc.
    let etc_is_overlay = is_overlay_mount(Path::new("/etc")).unwrap_or(false);
    if etc_is_overlay {
        let _ = run_command("umount", &["/etc"]);
    }

    let etc_upper = runtime_root
        .etc_state_dir()
        .join(gen_number.to_string())
        .display()
        .to_string();
    let etc_work = runtime_root
        .etc_state_dir()
        .join(format!("{gen_number}-work"))
        .display()
        .to_string();
    std::fs::create_dir_all(&etc_upper).context("Failed to create /etc overlay upper dir")?;
    std::fs::create_dir_all(&etc_work).context("Failed to create /etc overlay work dir")?;

    let etc_opts = format!("lowerdir={staging_etc},upperdir={etc_upper},workdir={etc_work}");
    match run_command(
        "mount",
        &["-t", "overlay", "overlay", "/etc", "-o", &etc_opts],
    ) {
        Ok(()) => info!("Mounted /etc overlay with composefs lower"),
        Err(e) => {
            let _ = run_command("umount", &["/usr"]);
            let _ = unmount_generation(&staging);
            return Err(e).context("Failed to mount /etc overlay for live debug switch");
        }
    }

    // Step 4: Move staging mount to permanent mount point
    std::fs::create_dir_all(&old_mnt).context("Failed to create permanent composefs mount dir")?;
    if let Err(e) = run_command("mount", &["--move", &staging_display, &old_mnt_display]) {
        let _ = run_command("umount", &["/usr"]);
        let _ = unmount_generation(&staging);
        return Err(e).context("Failed to move composefs mount to permanent location");
    }

    // Step 5: Update current symlink (delegates to conary-core)
    update_current_symlink(runtime_root.root(), gen_number)
        .map_err(|e| anyhow!("Failed to update current generation symlink: {e}"))?;

    info!("Switched to generation {gen_number} (composefs)");
    println!("Switched to generation {gen_number}. Reboot recommended for full consistency.");
    println!(
        "Generation switches remount filesystems only; they do not run removal scriptlets or undo persistent side effects from removed package versions."
    );

    Ok(())
}

/// Run a simple command, returning Ok on success.
#[allow(dead_code)]
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
