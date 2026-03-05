// src/commands/generation/switch.rs
//! Atomic generation switching via renameat2(RENAME_EXCHANGE)

use super::metadata::{current_link, generation_path, GenerationMetadata};
use anyhow::{Context, Result, anyhow};
use std::ffi::CString;
use std::path::Path;
use tracing::{info, warn};

/// Top-level directories to swap during a live generation switch
const SWAP_DIRS: &[&str] = &["usr", "etc"];

/// Switch the live system to the specified generation using atomic directory exchanges.
///
/// For each directory in `SWAP_DIRS`, attempts an atomic `renameat2(RENAME_EXCHANGE)`
/// between the generation directory and the live root. Falls back to a non-atomic
/// rename sequence on failure.
pub fn switch_live(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);
    if !gen_dir.exists() {
        return Err(anyhow!(
            "Generation {gen_number} does not exist at {}",
            gen_dir.display()
        ));
    }

    let _metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let mut exchanged = Vec::new();

    for dir in SWAP_DIRS {
        let gen_path = gen_dir.join(dir);
        let live_path = Path::new("/").join(dir);

        if !gen_path.exists() {
            warn!(
                "Generation path {} does not exist, skipping",
                gen_path.display()
            );
            continue;
        }
        if !live_path.exists() {
            warn!(
                "Live path {} does not exist, skipping",
                live_path.display()
            );
            continue;
        }

        let swap_result = match renameat2_exchange(&gen_path, &live_path) {
            Ok(()) => {
                info!("Exchanged {} atomically", dir);
                Ok(())
            }
            Err(e) => {
                warn!(
                    "renameat2 RENAME_EXCHANGE failed for {}: {e}, trying fallback",
                    dir
                );
                fallback_rename(&gen_path, &live_path, dir)
                    .with_context(|| format!("Fallback rename failed for {dir}"))
            }
        };

        match swap_result {
            Ok(()) => exchanged.push(*dir),
            Err(e) => {
                // Roll back already-exchanged directories to restore consistency
                for prev_dir in exchanged.iter().rev() {
                    let prev_gen = gen_dir.join(prev_dir);
                    let prev_live = Path::new("/").join(prev_dir);
                    if let Err(rb_err) = renameat2_exchange(&prev_gen, &prev_live) {
                        warn!(
                            "CRITICAL: Failed to rollback {prev_dir} during switch abort: {rb_err}"
                        );
                    } else {
                        info!("Rolled back {prev_dir} exchange");
                    }
                }
                return Err(e);
            }
        }
    }

    update_current_symlink(gen_number)
        .context("Failed to update current generation symlink")?;

    let dirs_list = exchanged.join(", ");
    info!("Switched to generation {gen_number} (exchanged: {dirs_list})");
    println!("Switched to generation {gen_number} (exchanged: {dirs_list})");
    println!("Reboot recommended for full consistency.");

    Ok(())
}

/// Atomically exchange two paths using the `renameat2` syscall with `RENAME_EXCHANGE`.
fn renameat2_exchange(a: &Path, b: &Path) -> Result<()> {
    let a_cstr = CString::new(a.as_os_str().as_encoded_bytes())
        .context("Path contains null byte")?;
    let b_cstr = CString::new(b.as_os_str().as_encoded_bytes())
        .context("Path contains null byte")?;

    /// renameat2(2) flag: atomically exchange two paths.
    const RENAME_EXCHANGE: u32 = 2;

    #[allow(unsafe_code)]
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a_cstr.as_ptr(),
            libc::AT_FDCWD,
            b_cstr.as_ptr(),
            RENAME_EXCHANGE,
        )
    };

    if ret == 0 {
        Ok(())
    } else {
        Err(anyhow!(
            "renameat2 RENAME_EXCHANGE failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

/// Non-atomic fallback: move live to `.conary-old`, move gen into place,
/// move old into gen dir. Restores backup on step 2 failure.
fn fallback_rename(gen_path: &Path, live_path: &Path, dir_name: &str) -> Result<()> {
    let backup_path = live_path
        .parent()
        .unwrap_or(Path::new("/"))
        .join(format!("{dir_name}.conary-old"));

    // Step 1: move live -> backup
    std::fs::rename(live_path, &backup_path).with_context(|| {
        format!(
            "Failed to move {} to {}",
            live_path.display(),
            backup_path.display()
        )
    })?;

    // Step 2: move gen -> live
    if let Err(e) = std::fs::rename(gen_path, live_path) {
        // Restore backup on failure
        warn!("Restoring backup after failed rename: {e}");
        std::fs::rename(&backup_path, live_path).with_context(|| {
            format!(
                "CRITICAL: Failed to restore backup from {}",
                backup_path.display()
            )
        })?;
        return Err(anyhow!(
            "Failed to move {} to {}: {e}",
            gen_path.display(),
            live_path.display()
        ));
    }

    // Step 3: move backup -> gen dir (complete the exchange)
    std::fs::rename(&backup_path, gen_path).with_context(|| {
        format!(
            "Failed to move backup {} to {}",
            backup_path.display(),
            gen_path.display()
        )
    })?;

    info!("Exchanged {dir_name} via fallback rename");
    Ok(())
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
