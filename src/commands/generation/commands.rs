// src/commands/generation/commands.rs
//! CLI implementations for generation list, info, gc, build, switch, rollback,
//! and recover commands

use super::metadata::{GenerationMetadata, gc_roots_dir, generation_path, generations_dir};
use crate::commands::format_bytes;
use anyhow::{Result, anyhow};
use conary_core::generation::mount::current_generation;
use std::path::Path;
use tracing::info;

/// List all generations with a summary table.
///
/// Prints each generation's number, creation date, package count, kernel version,
/// and whether it is the currently active generation.
pub async fn cmd_generation_list() -> Result<()> {
    let dir = generations_dir();

    if !dir.exists() {
        println!("No generations found. Run 'conary system takeover' to create the first.");
        return Ok(());
    }

    let current = current_generation(Path::new("/conary"))?;

    let mut generations: Vec<(i64, GenerationMetadata)> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Ok(number) = name_str.parse::<i64>() {
            let gen_dir = entry.path();
            match GenerationMetadata::read_from(&gen_dir) {
                Ok(meta) => generations.push((number, meta)),
                Err(e) => {
                    eprintln!("Warning: skipping generation {number}: {e}");
                }
            }
        }
    }

    generations.sort_by_key(|(number, _)| *number);

    if generations.is_empty() {
        println!("No valid generations found.");
        return Ok(());
    }

    for (number, meta) in &generations {
        let kernel = meta.kernel_version.as_deref().unwrap_or("none");
        let active = if current == Some(*number) {
            " [active]"
        } else {
            ""
        };
        println!(
            "{number}  {date}  {count} packages  kernel {kernel}{active}",
            date = meta.created_at,
            count = meta.package_count,
        );
    }

    Ok(())
}

/// Print detailed information about a specific generation.
pub async fn cmd_generation_info(gen_number: i64) -> Result<()> {
    let gen_dir = generation_path(gen_number);

    if !gen_dir.exists() {
        return Err(anyhow!("Generation {gen_number} does not exist"));
    }

    let meta = GenerationMetadata::read_from(&gen_dir)?;
    let current = current_generation(Path::new("/conary"))?;
    let is_active = current == Some(gen_number);

    let status = if is_active { "active" } else { "inactive" };
    let kernel = meta.kernel_version.as_deref().unwrap_or("none");

    println!("Generation {gen_number}");
    println!("  Status:   {status}");
    println!(
        "  Format:   {}",
        if meta.format.is_empty() {
            "reflink"
        } else {
            &meta.format
        }
    );
    println!("  Created:  {}", meta.created_at);
    println!("  Packages: {}", meta.package_count);
    println!("  Kernel:   {kernel}");
    println!("  Summary:  {}", meta.summary);

    // Show EROFS-specific info if available
    if let Some(erofs_size) = meta.erofs_size {
        println!(
            "  Image:    {} (root.erofs)",
            format_bytes(erofs_size as u64)
        );
    } else {
        let size = dir_size_bytes(&gen_dir);
        println!("  Size:     {}", format_bytes(size));
    }
    if let Some(cas_refs) = meta.cas_objects_referenced {
        println!("  CAS refs: {cas_refs}");
    }
    if meta.fsverity_enabled {
        println!("  Verity:   enabled");
    }

    Ok(())
}

/// Garbage-collect old generations, keeping the current generation, GC roots,
/// and the most recent `keep` generations.
///
/// After removing old generation directories and their BLS entries, performs
/// CAS garbage collection: queries the database for hashes referenced by
/// surviving generations and removes unreferenced objects from the CAS store.
pub async fn cmd_generation_gc(keep: usize, db_path: &str) -> Result<()> {
    let current = current_generation(Path::new("/conary"))?;
    let gc_roots = load_gc_roots();
    let dir = generations_dir();

    if !dir.exists() {
        println!("No generations directory found. Nothing to collect.");
        return Ok(());
    }

    let mut all_numbers: Vec<i64> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(number) = name_str.parse::<i64>() {
            all_numbers.push(number);
        }
    }

    all_numbers.sort();

    // Build the keep set: current + booted + gc_roots + last N generations
    let mut keep_set = std::collections::HashSet::new();

    if let Some(cur) = current {
        keep_set.insert(cur);
    }

    // Protect the currently-booted generation (may differ from current)
    if let Some(booted) = booted_generation() {
        keep_set.insert(booted);
    }

    for root in &gc_roots {
        keep_set.insert(*root);
    }

    // Keep the last N generations (by highest number)
    let start = all_numbers.len().saturating_sub(keep);
    for &num in &all_numbers[start..] {
        keep_set.insert(num);
    }

    let to_remove: Vec<i64> = all_numbers
        .iter()
        .filter(|n| !keep_set.contains(n))
        .copied()
        .collect();

    if to_remove.is_empty() {
        println!("Nothing to collect. All generations are kept.");
        return Ok(());
    }

    let mut removed_count = 0u64;
    let mut freed_bytes = 0u64;

    for gen_number in &to_remove {
        let gen_dir = generation_path(*gen_number);
        let size = dir_size_bytes(&gen_dir);

        match std::fs::remove_dir_all(&gen_dir) {
            Ok(()) => {
                info!("Removed generation {gen_number}");
                removed_count += 1;
                freed_bytes += size;
            }
            Err(e) => {
                eprintln!("Warning: failed to remove generation {gen_number}: {e}");
            }
        }

        // Remove corresponding BLS entry
        let bls_path =
            std::path::PathBuf::from(format!("/boot/loader/entries/conary-gen-{gen_number}.conf"));
        if bls_path.exists() {
            if let Err(e) = std::fs::remove_file(&bls_path) {
                eprintln!(
                    "Warning: failed to remove BLS entry {}: {e}",
                    bls_path.display()
                );
            } else {
                info!("Removed BLS entry for generation {gen_number}");
            }
        }
    }

    println!(
        "Collected {removed_count} generation(s), freed {}.",
        format_bytes(freed_bytes)
    );

    // --- CAS garbage collection ---
    // Determine which state IDs correspond to surviving generations, then
    // remove any CAS objects not referenced by those states.
    let surviving_numbers: Vec<i64> = all_numbers
        .iter()
        .filter(|n| keep_set.contains(n))
        .copied()
        .collect();

    cas_gc(db_path, &surviving_numbers)?;

    Ok(())
}

/// Run CAS garbage collection for the given surviving generation numbers.
///
/// Opens the database, maps generation numbers to state IDs, queries for
/// live CAS hashes, and removes unreferenced objects from the CAS store.
fn cas_gc(db_path: &str, surviving_gen_numbers: &[i64]) -> Result<()> {
    use conary_core::db::models::SystemState;
    use conary_core::db::paths::objects_dir;
    use conary_core::generation::gc::{gc_cas_objects, live_cas_hashes};

    let conn = crate::commands::open_db(db_path)?;

    // Map surviving generation numbers to system_state IDs.
    // Generation numbers correspond to state_number in system_states.
    let mut surviving_state_ids: Vec<i64> = Vec::new();
    for &gen_num in surviving_gen_numbers {
        if let Some(state) = SystemState::find_by_number(&conn, gen_num)?
            && let Some(id) = state.id
        {
            surviving_state_ids.push(id);
        }
    }

    if surviving_state_ids.is_empty() {
        info!("No surviving states found in database; skipping CAS GC.");
        println!("CAS GC: no surviving states in database, skipped.");
        return Ok(());
    }

    let live_hashes = live_cas_hashes(&conn, &surviving_state_ids)?;
    info!(
        "{} live CAS hashes across {} surviving states",
        live_hashes.len(),
        surviving_state_ids.len()
    );

    let obj_dir = objects_dir(db_path);
    let stats = gc_cas_objects(&obj_dir, &live_hashes)?;

    if stats.objects_removed > 0 {
        println!(
            "CAS GC: removed {} of {} objects, freed {}.",
            stats.objects_removed,
            stats.objects_checked,
            format_bytes(stats.bytes_freed)
        );
    } else {
        println!(
            "CAS GC: checked {} objects, all referenced.",
            stats.objects_checked
        );
    }

    Ok(())
}

/// Read the currently-booted generation from `/proc/cmdline`.
///
/// Returns `None` if no `conary.generation=N` parameter is present.
fn booted_generation() -> Option<i64> {
    let cmdline = std::fs::read_to_string("/proc/cmdline").ok()?;
    cmdline
        .split_whitespace()
        .find(|p| p.starts_with("conary.generation="))?
        .strip_prefix("conary.generation=")?
        .parse()
        .ok()
}

/// Read GC root entries from the gc-roots directory.
///
/// Each entry name is expected to parse as an i64 generation number.
fn load_gc_roots() -> Vec<i64> {
    let dir = gc_roots_dir();

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i64>().ok())
        .collect()
}

/// Calculate total size of all files under `path` recursively.
fn dir_size_bytes(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Build a new generation from the current system state and print its number.
pub fn cmd_generation_build(db_path: &str, summary: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let gen_number = super::builder::build_generation(&conn, db_path, summary)?;
    println!("Generation {} built.", gen_number);
    Ok(())
}

/// Switch the live system to `number`, update the boot entry, and optionally reboot.
pub fn cmd_generation_switch(number: i64, reboot: bool) -> Result<()> {
    super::switch::switch_live(number)?;
    if let Err(e) = super::boot::write_boot_entry(number) {
        eprintln!("Boot entry skipped: {}", e);
    }
    if reboot {
        println!("Rebooting...");
        std::process::Command::new("systemctl")
            .arg("reboot")
            .spawn()?;
    }
    Ok(())
}

/// Roll back to the highest-numbered generation below the currently active one.
pub fn cmd_generation_rollback() -> Result<()> {
    let current =
        current_generation(Path::new("/conary"))?.ok_or_else(|| anyhow!("No active generation"))?;

    // Find the highest generation below current that actually exists on disk.
    let gen_dir = generations_dir();
    let mut candidates: Vec<i64> = Vec::new();
    if gen_dir.exists() {
        for entry in std::fs::read_dir(&gen_dir)? {
            let entry = entry?;
            if let Ok(n) = entry.file_name().to_string_lossy().parse::<i64>()
                && n < current
            {
                candidates.push(n);
            }
        }
    }
    candidates.sort();
    let previous = candidates
        .last()
        .ok_or_else(|| anyhow!("No previous generation to roll back to"))?;

    super::switch::switch_live(*previous)?;
    if let Err(e) = super::boot::write_boot_entry(*previous) {
        eprintln!("Boot entry skipped: {}", e);
    }
    println!("Rolled back to generation {previous}");
    Ok(())
}

/// Recover any interrupted transaction using the database at `db_path`.
pub fn cmd_generation_recover(db_path: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let db_path_buf = std::path::PathBuf::from(db_path);
    // Root is the parent of the database file (e.g. /conary), not /.
    // This matches the install paths and ensures recover() reads/writes
    // the correct /conary/current symlink and generation directories.
    let conary_root = db_path_buf
        .parent()
        .unwrap_or(Path::new("/conary"))
        .to_path_buf();

    // Mount composefs at the staging point (<root>/mnt), not at /.
    // This matches the composefs_ops/switch.rs pattern and ensures the
    // staging mount exists for the /etc overlay lower path.
    let staging = conary_root.join("mnt");
    std::fs::create_dir_all(&staging)
        .map_err(|e| anyhow!("Failed to create staging directory: {e}"))?;

    let mut config =
        conary_core::transaction::TransactionConfig::from_paths(conary_root.clone(), db_path_buf);
    config.mount_point = staging.clone();
    let engine = conary_core::transaction::TransactionEngine::new(config)?;
    engine.recover(&conn)?;

    // Restore the /etc overlay after recovery mounts the generation.
    // recover() mounts the composefs image at <root>/mnt; the writable
    // /etc overlay uses staging/etc as lower and live /etc as target.
    if let Ok(Some(gen_num)) = current_generation(&conary_root) {
        let staging_etc = staging.join("etc");
        let upper = conary_root.join(format!("etc-state/{gen_num}"));
        let work = conary_root.join(format!("etc-state/{gen_num}-work"));
        if let Err(e) = conary_core::generation::mount::mount_etc_overlay(
            &staging_etc,
            Path::new("/etc"),
            &upper,
            &work,
        ) {
            tracing::warn!("Failed to restore /etc overlay after recovery: {e}");
        }
    }

    println!("Recovery complete.");
    Ok(())
}
