// src/commands/generation/commands.rs
//! CLI implementations for generation list, info, gc, build, switch, rollback,
//! and recover commands

use super::metadata::{
    GenerationMetadata, generation_path, generations_dir, is_generation_pending,
};
use crate::commands::format_bytes;
use anyhow::{Result, anyhow};
use conary_core::generation::mount::current_generation;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use rusqlite::Connection;
use std::path::Path;
use tracing::{info, warn};

const GENERATION_DB_CANDIDATES: &[&str] = &["/conary/conary.db", "/var/lib/conary/conary.db"];
const GC_ROOTS_SETTING_KEY: &str = "generation.gc_roots";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SideEffectPackageWarning {
    name: String,
    version: String,
    reasons: Vec<&'static str>,
}

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
            if is_generation_pending(&gen_dir) {
                eprintln!("Warning: skipping incomplete generation {number}");
                continue;
            }
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
    let dir = generations_dir();
    let conary_root = dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("/conary"));
    let mut engine = TransactionEngine::new(TransactionConfig::from_paths(
        conary_root.clone(),
        db_path.into(),
    ))?;
    engine.begin()?;
    let result = cmd_generation_gc_locked(keep, db_path, &conary_root);
    engine.release_lock();
    result
}

fn cmd_generation_gc_locked(keep: usize, db_path: &str, conary_root: &Path) -> Result<()> {
    let current = current_generation(Path::new("/conary"))?;
    let conn = crate::commands::open_db(db_path)?;
    let gc_roots = load_gc_roots(&conn)?;
    let dir = generations_dir();

    if !dir.exists() {
        println!("No generations directory found. Nothing to collect.");
        return Ok(());
    }

    let mut all_numbers: Vec<i64> = Vec::new();
    let mut pending_numbers: Vec<i64> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(number) = name_str.parse::<i64>() {
            if is_generation_pending(&entry.path()) {
                pending_numbers.push(number);
            } else {
                all_numbers.push(number);
            }
        }
    }

    all_numbers.sort();
    pending_numbers.sort();

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

    for gen_number in &pending_numbers {
        let gen_dir = generation_path(*gen_number);
        let size = dir_size_bytes(&gen_dir);
        match std::fs::remove_dir_all(&gen_dir) {
            Ok(()) => {
                info!("Removed incomplete pending generation {gen_number}");
                removed_count += 1;
                freed_bytes += size;
                if let Err(error) = remove_generation_etc_state(conary_root, *gen_number) {
                    eprintln!(
                        "Warning: failed to remove etc-state directories for incomplete generation {gen_number}: {error}"
                    );
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to remove incomplete generation {gen_number}: {e}");
            }
        }
    }

    for gen_number in &to_remove {
        let gen_dir = generation_path(*gen_number);
        let size = dir_size_bytes(&gen_dir);

        match std::fs::remove_dir_all(&gen_dir) {
            Ok(()) => {
                info!("Removed generation {gen_number}");
                removed_count += 1;
                freed_bytes += size;
                if let Err(error) = remove_generation_etc_state(conary_root, *gen_number) {
                    eprintln!(
                        "Warning: failed to remove etc-state directories for generation {gen_number}: {error}"
                    );
                }
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
    booted_generation_from_cmdline(&cmdline, Path::new("/conary"))
}

fn booted_generation_from_cmdline(cmdline: &str, conary_root: &Path) -> Option<i64> {
    let generation: i64 = cmdline
        .split_whitespace()
        .find(|p| p.starts_with("conary.generation="))?
        .strip_prefix("conary.generation=")?
        .parse()
        .ok()?;

    let generation_dir = conary_root.join("generations").join(generation.to_string());
    if generation_dir.is_dir() {
        Some(generation)
    } else {
        warn!(
            "Ignoring booted generation {} because {} does not exist",
            generation,
            generation_dir.display()
        );
        None
    }
}

/// Read GC root entries from the database.
///
/// Raw filesystem entries under `/conary/gc-roots` are intentionally ignored;
/// callers must register pins in the database before GC will honor them.
fn load_gc_roots(conn: &Connection) -> Result<Vec<i64>> {
    use conary_core::db::models::settings;

    Ok(settings::get(conn, GC_ROOTS_SETTING_KEY)?
        .map(|serialized| parse_gc_root_setting(&serialized))
        .unwrap_or_default())
}

fn parse_gc_root_setting(serialized: &str) -> Vec<i64> {
    let mut generations = serde_json::from_str::<Vec<i64>>(serialized).unwrap_or_default();
    generations.sort_unstable();
    generations.dedup();
    generations
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

fn open_generation_db() -> Result<rusqlite::Connection> {
    let mut last_error = None;

    for path in GENERATION_DB_CANDIDATES {
        match crate::commands::open_db(path) {
            Ok(conn) => return Ok(conn),
            Err(err) => last_error = Some((path, err)),
        }
    }

    let (path, err) = last_error.expect("generation DB candidate list must not be empty");
    Err(anyhow!(
        "Failed to open generation state database at {path}: {err}"
    ))
}

fn removed_members_for_side_effect_warning(
    diff: &conary_core::db::models::StateDiff,
) -> Vec<conary_core::db::models::StateMember> {
    let mut removed = diff.removed.clone();
    removed.extend(diff.upgraded.iter().map(|(old, _)| old.clone()));
    removed.sort_by(|left, right| {
        (&left.trove_name, &left.trove_version, &left.architecture).cmp(&(
            &right.trove_name,
            &right.trove_version,
            &right.architecture,
        ))
    });
    removed.dedup_by(|left, right| {
        left.trove_name == right.trove_name
            && left.trove_version == right.trove_version
            && left.architecture == right.architecture
    });
    removed
}

fn has_user_group_side_effect(script: &str) -> bool {
    [
        "useradd", "usermod", "userdel", "adduser", "deluser", "groupadd", "groupmod", "groupdel",
        "addgroup", "delgroup",
    ]
    .iter()
    .any(|needle| script.contains(needle))
}

fn classify_side_effect_reasons<'a>(
    file_paths: impl IntoIterator<Item = &'a str>,
    script_contents: impl IntoIterator<Item = &'a str>,
) -> Vec<&'static str> {
    let file_paths: Vec<&str> = file_paths.into_iter().collect();
    let lowercased_scripts: Vec<String> = script_contents
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();

    let mut reasons = Vec::new();

    let has_user_group_state = file_paths.iter().any(|path| {
        path.starts_with("/usr/lib/sysusers.d/") || path.starts_with("/etc/sysusers.d/")
    }) || lowercased_scripts
        .iter()
        .any(|script| has_user_group_side_effect(script));
    if has_user_group_state {
        reasons.push("users/groups");
    }

    let has_systemd_state = file_paths.iter().any(|path| {
        path.starts_with("/usr/lib/systemd/system/")
            || path.starts_with("/etc/systemd/system/")
            || path.starts_with("/usr/lib/systemd/user/")
            || path.starts_with("/etc/systemd/user/")
    }) || lowercased_scripts.iter().any(|script| {
        script.contains("systemctl ")
            || script.contains("daemon-reload")
            || script.contains("preset ")
    });
    if has_systemd_state {
        reasons.push("systemd units");
    }

    let has_cron_state = file_paths.iter().any(|path| {
        path == &"/etc/crontab"
            || path.starts_with("/etc/cron.")
            || path.starts_with("/etc/cron/")
            || path.starts_with("/var/spool/cron/")
            || path.starts_with("/usr/lib/cron/")
    }) || lowercased_scripts
        .iter()
        .any(|script| script.contains("crontab "));
    if has_cron_state {
        reasons.push("cron jobs");
    }

    reasons
}

fn find_side_effect_package_warning(
    conn: &rusqlite::Connection,
    member: &conary_core::db::models::StateMember,
) -> Result<Option<SideEffectPackageWarning>> {
    let trove = conary_core::db::models::Trove::find_by_name(conn, &member.trove_name)?
        .into_iter()
        .filter(|trove| {
            trove.version == member.trove_version && trove.architecture == member.architecture
        })
        .max_by_key(|trove| trove.id.unwrap_or_default());

    let Some(trove) = trove else {
        return Ok(None);
    };
    let Some(trove_id) = trove.id else {
        return Ok(None);
    };

    let files = conary_core::db::models::FileEntry::find_by_trove(conn, trove_id)?;
    let scriptlets = conary_core::db::models::ScriptletEntry::find_by_trove(conn, trove_id)?;
    let reasons = classify_side_effect_reasons(
        files.iter().map(|file| file.path.as_str()),
        scriptlets
            .iter()
            .map(|scriptlet| scriptlet.content.as_str()),
    );

    if reasons.is_empty() {
        return Ok(None);
    }

    Ok(Some(SideEffectPackageWarning {
        name: member.trove_name.clone(),
        version: member.trove_version.clone(),
        reasons,
    }))
}

fn collect_side_effect_package_warnings(
    from_generation: i64,
    to_generation: i64,
) -> Result<Vec<SideEffectPackageWarning>> {
    let conn = open_generation_db()?;
    let from_state = conary_core::db::models::SystemState::find_by_number(&conn, from_generation)?
        .ok_or_else(|| anyhow!("State {from_generation} not found in generation database"))?;
    let to_state = conary_core::db::models::SystemState::find_by_number(&conn, to_generation)?
        .ok_or_else(|| anyhow!("State {to_generation} not found in generation database"))?;
    let from_id = from_state
        .id
        .ok_or_else(|| anyhow!("State {from_generation} is missing an ID"))?;
    let to_id = to_state
        .id
        .ok_or_else(|| anyhow!("State {to_generation} is missing an ID"))?;

    let diff = conary_core::db::models::StateDiff::compare(&conn, from_id, to_id)?;
    let mut warnings = Vec::new();

    for member in removed_members_for_side_effect_warning(&diff) {
        if let Some(package) = find_side_effect_package_warning(&conn, &member)? {
            warnings.push(package);
        }
    }

    warnings.sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    Ok(warnings)
}

fn warn_removed_side_effect_packages(from_generation: i64, to_generation: i64) {
    match collect_side_effect_package_warnings(from_generation, to_generation) {
        Ok(packages) if !packages.is_empty() => {
            eprintln!(
                "WARNING: Generation switch {} -> {} removed package versions without running removal scriptlets.",
                from_generation, to_generation
            );
            eprintln!(
                "WARNING: Persistent side effects are not automatically undone during rollback."
            );
            for package in packages {
                eprintln!(
                    "  - {} {} ({})",
                    package.name,
                    package.version,
                    package.reasons.join(", ")
                );
            }
            eprintln!(
                "WARNING: Review those packages manually; `--undo-scriptlets` is not implemented yet."
            );
        }
        Ok(_) => {}
        Err(error) => {
            warn!(
                from_generation,
                to_generation,
                "Failed to inspect removed package side effects during generation switch: {}",
                error
            );
        }
    }
}

fn etc_state_paths(conary_root: &Path, generation: i64) -> [std::path::PathBuf; 2] {
    [
        conary_root.join(format!("etc-state/{generation}")),
        conary_root.join(format!("etc-state/{generation}-work")),
    ]
}

fn remove_generation_etc_state(conary_root: &Path, generation: i64) -> Result<()> {
    for path in etc_state_paths(conary_root, generation) {
        if !path.exists() {
            continue;
        }

        std::fs::remove_dir_all(&path)
            .map_err(|error| anyhow!("failed to remove {}: {error}", path.display()))?;
    }

    Ok(())
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
    let current = current_generation(Path::new("/conary"))?;
    super::switch::switch_live(number)?;
    if let Err(e) = super::boot::write_boot_entry(number) {
        eprintln!("Boot entry skipped: {}", e);
    }
    if let Some(current) = current {
        warn_removed_side_effect_packages(current, number);
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
    warn_removed_side_effect_packages(current, *previous);
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

#[cfg(test)]
mod tests {
    use super::{
        booted_generation_from_cmdline, classify_side_effect_reasons, etc_state_paths,
        load_gc_roots, parse_gc_root_setting, remove_generation_etc_state,
        removed_members_for_side_effect_warning,
    };
    use conary_core::db::models::settings;
    use conary_core::db::models::{StateDiff, StateMember};
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn member(name: &str, version: &str) -> StateMember {
        StateMember {
            id: None,
            state_id: 1,
            trove_name: name.to_string(),
            trove_version: version.to_string(),
            architecture: Some("x86_64".to_string()),
            install_reason: "explicit".to_string(),
            selection_reason: None,
        }
    }

    #[test]
    fn classify_side_effect_reasons_detects_all_requested_categories() {
        let reasons = classify_side_effect_reasons(
            [
                "/usr/lib/systemd/system/example.service",
                "/etc/cron.d/example",
                "/usr/lib/sysusers.d/example.conf",
            ],
            [
                "groupadd example",
                "systemctl preset example.service",
                "crontab -r",
            ],
        );

        assert_eq!(reasons, vec!["users/groups", "systemd units", "cron jobs"]);
    }

    #[test]
    fn removed_members_for_side_effect_warning_includes_replaced_versions_once() {
        let removed = member("removed-only", "1.0.0");
        let upgraded_old = member("replaced", "2.0.0");
        let upgraded_new = member("replaced", "1.5.0");
        let diff = StateDiff {
            added: Vec::new(),
            removed: vec![removed.clone()],
            upgraded: vec![
                (upgraded_old.clone(), upgraded_new),
                (upgraded_old.clone(), member("replaced", "1.0.0")),
            ],
        };

        let members = removed_members_for_side_effect_warning(&diff);
        let rendered: Vec<_> = members
            .into_iter()
            .map(|member| (member.trove_name, member.trove_version))
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("removed-only".to_string(), "1.0.0".to_string()),
                ("replaced".to_string(), "2.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn remove_generation_etc_state_deletes_both_overlay_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conary_root = tmp.path();
        let [upper, work] = etc_state_paths(conary_root, 7);
        std::fs::create_dir_all(&upper).unwrap();
        std::fs::create_dir_all(&work).unwrap();

        remove_generation_etc_state(conary_root, 7).unwrap();

        assert!(!upper.exists());
        assert!(!work.exists());
    }

    #[test]
    fn remove_generation_etc_state_is_noop_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        remove_generation_etc_state(tmp.path(), 11).unwrap();
    }

    #[test]
    fn parse_gc_root_setting_sorts_and_deduplicates_values() {
        assert_eq!(parse_gc_root_setting("[7,3,7,5]"), vec![3, 5, 7]);
    }

    #[test]
    fn load_gc_roots_ignores_filesystem_entries_without_db_registration() {
        let temp_dir = TempDir::new().unwrap();
        let gc_roots_dir = temp_dir.path().join("gc-roots");
        std::fs::create_dir_all(&gc_roots_dir).unwrap();
        std::fs::write(gc_roots_dir.join("7"), b"").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        assert_eq!(load_gc_roots(&conn).unwrap(), Vec::<i64>::new());

        settings::set(&conn, "generation.gc_roots", "[7,5]").unwrap();
        assert_eq!(load_gc_roots(&conn).unwrap(), vec![5, 7]);
    }

    #[test]
    fn booted_generation_ignores_missing_generation_directory() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("generations")).unwrap();

        assert_eq!(
            booted_generation_from_cmdline("quiet conary.generation=7", temp_dir.path()),
            None
        );
    }

    #[test]
    fn booted_generation_accepts_existing_generation_directory() {
        let temp_dir = TempDir::new().unwrap();
        let gen_dir = temp_dir.path().join("generations/7");
        std::fs::create_dir_all(&gen_dir).unwrap();

        assert_eq!(
            booted_generation_from_cmdline("quiet conary.generation=7", temp_dir.path()),
            Some(7)
        );
    }
}
