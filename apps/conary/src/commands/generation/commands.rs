// apps/conary/src/commands/generation/commands.rs
//! CLI implementations for generation list, info, build, switch, rollback,
//! and recover commands.

use super::metadata::{GenerationMetadata, is_generation_pending};
use crate::commands::format_bytes;
use anyhow::{Context, Result, anyhow};
use conary_core::generation::mount::{
    current_generation, unmount_generation, update_current_symlink,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SideEffectPackageWarning {
    name: String,
    version: String,
    reasons: Vec<&'static str>,
}

fn default_runtime_root() -> ConaryRuntimeRoot {
    ConaryRuntimeRoot::default()
}

fn runtime_root_for_generation_db_path(db_path: &str) -> ConaryRuntimeRoot {
    ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path))
}

fn mark_generation_state_active(runtime_root: &ConaryRuntimeRoot, number: i64) -> Result<()> {
    let db_path = runtime_root
        .db_path()
        .to_str()
        .ok_or_else(|| anyhow!("Generation database path is not valid UTF-8"))?;
    let conn = crate::commands::open_db(db_path)?;
    let state = conary_core::db::models::SystemState::find_by_number(&conn, number)?
        .ok_or_else(|| anyhow!("Generation {number} has no DB state snapshot"))?;
    state.set_active(&conn)?;
    Ok(())
}

fn validate_generation_activation_artifact(
    runtime_root: &ConaryRuntimeRoot,
    number: i64,
) -> Result<()> {
    let gen_dir = runtime_root.generation_path(number);
    let artifact =
        conary_core::generation::artifact::load_generation_artifact_with_verified_cas(&gen_dir)
            .with_context(|| {
                format!("Generation {number} is not an activatable composefs artifact")
            })?;
    if artifact.generation != number {
        return Err(anyhow!(
            "Generation artifact mismatch: requested {number}, artifact declares {}",
            artifact.generation
        ));
    }
    Ok(())
}

/// List all generations with a summary table.
///
/// Prints each generation's number, creation date, package count, kernel version,
/// and whether it is the currently active generation.
pub async fn cmd_generation_list() -> Result<()> {
    let runtime_root = default_runtime_root();
    let dir = runtime_root.generations_dir();

    if !dir.exists() {
        println!("No generations found. Run 'conary system takeover' to create the first.");
        return Ok(());
    }

    let current = current_generation(runtime_root.root())?;

    let mut generations: Vec<(i64, GenerationMetadata)> = Vec::new();

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Ok(number) = name_str.parse::<i64>() {
            let gen_dir = entry.path();
            if is_generation_pending(&gen_dir) {
                crate::ui::warn(&format!("skipping incomplete generation {number}"));
                continue;
            }
            match GenerationMetadata::read_from(&gen_dir) {
                Ok(meta) => generations.push((number, meta)),
                Err(e) => {
                    crate::ui::warn(&format!("skipping generation {number}: {e}"));
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
    let runtime_root = default_runtime_root();
    let gen_dir = runtime_root.generation_path(gen_number);

    if !gen_dir.exists() {
        return Err(anyhow!("Generation {gen_number} does not exist"));
    }

    let meta = GenerationMetadata::read_from(&gen_dir)?;
    let current = current_generation(runtime_root.root())?;
    let is_active = current == Some(gen_number);

    print!(
        "{}",
        render_generation_info(gen_number, &meta, is_active, dir_size_bytes(&gen_dir))
    );

    Ok(())
}

fn render_generation_info(
    gen_number: i64,
    meta: &GenerationMetadata,
    is_active: bool,
    generation_dir_size: u64,
) -> String {
    let status = if is_active { "active" } else { "inactive" };
    let kernel = meta.kernel_version.as_deref().unwrap_or("none");
    let format = if meta.format.is_empty() {
        "reflink"
    } else {
        &meta.format
    };

    let mut rendered = String::new();
    let _ = writeln!(&mut rendered, "Generation {gen_number}");
    let _ = writeln!(&mut rendered, "  Status:   {status}");
    let _ = writeln!(&mut rendered, "  Format:   {format}");
    let _ = writeln!(&mut rendered, "  Created:  {}", meta.created_at);
    let _ = writeln!(&mut rendered, "  Packages: {}", meta.package_count);
    let _ = writeln!(&mut rendered, "  Kernel:   {kernel}");
    let _ = writeln!(&mut rendered, "  Summary:  {}", meta.summary);

    if let Some(erofs_size) = meta.erofs_size {
        let _ = writeln!(
            &mut rendered,
            "  Image:    {} (root.erofs)",
            format_bytes(erofs_size as u64)
        );
    } else {
        let _ = writeln!(
            &mut rendered,
            "  Size:     {}",
            format_bytes(generation_dir_size)
        );
    }
    if let Some(cas_refs) = meta.cas_objects_referenced {
        let _ = writeln!(&mut rendered, "  CAS refs: {cas_refs}");
    }
    if let Some(cap_xattrs) = meta
        .security_capability_xattr_count
        .filter(|count| *count > 0)
    {
        let _ = writeln!(&mut rendered, "  Cap xattrs: {cap_xattrs}");
    }
    if meta.fsverity_enabled {
        let _ = writeln!(&mut rendered, "  Verity:   enabled");
    }

    rendered
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
    let runtime_root = default_runtime_root();
    let db_path = runtime_root.db_path().to_string_lossy();
    crate::commands::open_db(db_path.as_ref()).map_err(|err| {
        anyhow!(
            "Failed to open generation state database at {}: {err}",
            runtime_root.db_path().display()
        )
    })
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

    let payload = conary_core::db::models::PackagePayloadOwnership::load(conn, trove_id)?;
    let ccs_remove_hook =
        conary_core::db::models::InstalledCcsRemoveHook::find_by_trove(conn, trove_id)?;
    let reasons = classify_side_effect_reasons(
        payload.entries().iter().map(|file| file.path.as_str()),
        ccs_remove_hook.iter().map(|hook| hook.script.as_str()),
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

/// Build a new generation from the current system state and print its number.
pub fn cmd_generation_build(db_path: &str, summary: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let gen_number = super::builder::build_generation(&conn, db_path, summary)?;
    println!("Generation {} built.", gen_number);
    Ok(())
}

pub fn cmd_generation_publish(db_path: &str, changeset: Option<i64>) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    if let Some(changeset_id) = changeset
        && conary_core::db::models::GenerationPublication::pending_for_changeset(
            &conn,
            changeset_id,
        )?
        .is_none()
    {
        return Err(anyhow!(
            "No pending generation publication debt found for changeset {changeset_id}"
        ));
    }

    let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?;
    if debts.is_empty() {
        println!("Generation publication is already current.");
        return Ok(());
    }

    let runtime_root = runtime_root_for_generation_db_path(db_path);
    let mut engine = TransactionEngine::new(TransactionConfig::for_runtime_root(&runtime_root))?;
    engine.begin()?;
    let result = crate::commands::generation::publication::retry_pending_publication(
        &conn,
        db_path,
        "Retry pending generation publication",
    );
    engine.release_lock();

    let outcome = result?;
    if outcome.needs_publication {
        return Err(pending_publication_error(
            "Generation publication",
            &outcome,
        ));
    }

    println!(
        "Generation publication complete: generation {} selected.",
        outcome.generation_number.unwrap_or_default()
    );
    Ok(())
}

fn pending_publication_error(
    context: &str,
    outcome: &crate::commands::generation::publication::PublicationOutcome,
) -> anyhow::Error {
    let cause = outcome
        .failure_reason
        .as_deref()
        .unwrap_or("publication failed without a recorded cause");
    let retry = outcome.retry_command.clone().unwrap_or_else(
        crate::commands::generation::publication::PublicationOutcome::default_retry_command,
    );
    anyhow!("{context} is still pending.\nCause: {cause}\nRetry with: {retry}")
}

pub fn cmd_generation_pending(db_path: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?;
    if debts.is_empty() {
        println!("No pending generation publication debt.");
        return Ok(());
    }

    println!("Pending generation publication debt:");
    for debt in debts {
        let id = debt.id.unwrap_or_default();
        let changeset = debt
            .trigger_changeset_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  [{id}] changeset={changeset} status={} phase={} generation={} state={} retry=\"{}\"",
            debt.status.as_str(),
            debt.phase.as_str(),
            debt.generation_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            debt.state_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            crate::commands::generation::publication::PublicationOutcome::default_retry_command()
        );
    }
    Ok(())
}

pub fn cmd_generation_verify_db_backup(
    db_path: &str,
    generation: Option<i64>,
    current: bool,
) -> Result<()> {
    let runtime_root = runtime_root_for_generation_db_path(db_path);
    let generation_number = if current {
        current_generation(runtime_root.root())?
            .ok_or_else(|| anyhow!("No currently selected generation found at /conary/current"))?
    } else {
        generation.ok_or_else(|| anyhow!("Specify --generation <N> or --current"))?
    };

    let gen_dir = runtime_root.generation_path(generation_number);
    let current_root = current.then_some(runtime_root.root());
    let verification =
        conary_core::db::backup::verify_generation_db_backup(&gen_dir, current_root)?;
    crate::ui::status(
        "Verified",
        &format!(
            "generation {} database backup",
            verification.generation_number
        ),
    );
    crate::ui::field("Authority", &verification.backup_path.display().to_string());
    crate::ui::field("Schema", &verification.db_schema_version.to_string());
    crate::ui::field("Integrity", &verification.integrity_check);
    crate::ui::field(
        "Base SQLite pages",
        &verification
            .sqlite_page_count
            .map(|pages| pages.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
    );
    crate::ui::field(
        "Transaction high-water mark",
        &verification
            .transaction_high_water_mark
            .map(|changeset| changeset.to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
    crate::ui::field(
        "Base snapshot method",
        verification.snapshot.method.as_str(),
    );
    crate::ui::field(
        "Base snapshot payload bytes",
        &verification.snapshot.payload_bytes_written.to_string(),
    );
    crate::ui::field("Deltas", &verification.delta_count.to_string());
    let fallbacks = verification
        .snapshot
        .fallbacks
        .iter()
        .map(|fallback| format!("{}:{}", fallback.method.as_str(), fallback.reason.as_str()))
        .collect::<Vec<_>>()
        .join(",");
    crate::ui::field(
        "Base snapshot fallbacks",
        if fallbacks.is_empty() {
            "none"
        } else {
            &fallbacks
        },
    );
    crate::ui::field("Chain SHA-256", &verification.backup_sha256);
    Ok(())
}

pub fn cmd_generation_recover_db(
    db_path: &str,
    generation: i64,
    dry_run: bool,
    keep_temp: bool,
    yes: bool,
    replace_healthy_db: bool,
) -> Result<()> {
    let runtime_root = runtime_root_for_generation_db_path(db_path);
    let gen_dir = runtime_root.generation_path(generation);
    let options = conary_core::db::backup::GenerationDbRecoveryOptions {
        dry_run,
        yes,
        keep_temp,
        replace_healthy_db,
    };
    let outcome = if dry_run {
        conary_core::db::backup::recover_generation_db_backup(
            runtime_root.db_path(),
            &gen_dir,
            options,
        )?
    } else {
        let mut engine = TransactionEngine::new(TransactionConfig::from_paths(
            runtime_root.root().to_path_buf(),
            runtime_root.db_path().to_path_buf(),
        ))?;
        engine.begin()?;
        let result = conary_core::db::backup::recover_generation_db_backup(
            runtime_root.db_path(),
            &gen_dir,
            options,
        );
        engine.release_lock();
        result?
    };

    if outcome.dry_run {
        println!(
            "Generation {generation} DB recovery dry-run verified: {}",
            outcome.backup_path.display()
        );
        if let Some(temp_path) = outcome.verified_temp_path {
            println!("  verified temp copy={}", temp_path.display());
        }
    } else {
        println!(
            "Recovered Conary DB from generation {generation} backup: {}",
            outcome.backup_path.display()
        );
        if outcome.quarantined_paths.is_empty() {
            println!("  no previous DB files needed quarantine");
        } else {
            for path in outcome.quarantined_paths {
                println!("  quarantined={}", path.display());
            }
        }
    }
    println!("  manifest={}", outcome.manifest_path.display());
    Ok(())
}

/// Select `number` as the next boot generation, update the boot entry, and optionally reboot.
pub fn cmd_generation_switch(number: i64, reboot: bool) -> Result<()> {
    let runtime_root = default_runtime_root();
    let current = current_generation(runtime_root.root())?;
    let gen_dir = runtime_root.generation_path(number);
    if !gen_dir.exists() {
        return Err(anyhow!(
            "Generation {number} does not exist at {}",
            gen_dir.display()
        ));
    }
    validate_generation_activation_artifact(&runtime_root, number)?;
    let bootloader = super::boot::detect_bootloader();
    super::boot::write_boot_entry(number, &bootloader)
        .with_context(|| format!("Failed to prepare boot entry for generation {number}"))?;

    update_current_symlink(runtime_root.root(), number)
        .map_err(|e| anyhow!("Failed to update current generation symlink: {e}"))?;
    mark_generation_state_active(&runtime_root, number)?;
    if let Some(current) = current {
        warn_removed_side_effect_packages(current, number);
    }
    println!("Generation {number} selected for next boot.");
    println!("Reboot to activate the selected composefs generation.");
    if reboot {
        println!("Rebooting...");
        std::process::Command::new("systemctl")
            .arg("reboot")
            .spawn()?;
    }
    Ok(())
}

/// Roll back to the highest-numbered generation below the currently selected one.
pub fn cmd_generation_rollback() -> Result<()> {
    let runtime_root = default_runtime_root();
    let current =
        current_generation(runtime_root.root())?.ok_or_else(|| anyhow!("No active generation"))?;

    // Find the highest generation below current that actually exists on disk.
    let gen_dir = runtime_root.generations_dir();
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
    validate_generation_activation_artifact(&runtime_root, *previous)?;
    let bootloader = super::boot::detect_bootloader();
    super::boot::write_boot_entry(*previous, &bootloader)
        .with_context(|| format!("Failed to prepare boot entry for generation {previous}"))?;

    update_current_symlink(runtime_root.root(), *previous)
        .map_err(|e| anyhow!("Failed to update current generation symlink: {e}"))?;
    mark_generation_state_active(&runtime_root, *previous)?;
    warn_removed_side_effect_packages(current, *previous);
    println!("Generation {previous} selected for next boot.");
    println!("Reboot to activate the rollback generation.");
    Ok(())
}

/// Recover any interrupted transaction using the database at `db_path`.
pub fn cmd_generation_recover(db_path: &str) -> Result<()> {
    let conn = crate::commands::open_db(db_path)?;
    let runtime_root = runtime_root_for_generation_db_path(db_path);

    // Mount composefs at the runtime staging point, not at /.
    let staging = runtime_root.mount_dir();
    std::fs::create_dir_all(&staging)
        .map_err(|e| anyhow!("Failed to create staging directory: {e}"))?;

    let mut config = conary_core::transaction::TransactionConfig::for_runtime_root(&runtime_root);
    config.mount_point = staging.clone();
    let mut engine = conary_core::transaction::TransactionEngine::new(config)?;
    engine.begin()?;

    if !conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?.is_empty() {
        let outcome = crate::commands::generation::publication::retry_pending_publication(
            &conn,
            db_path,
            "Recover interrupted generation publication",
        )?;
        if outcome.needs_publication {
            return Err(pending_publication_error(
                "Generation publication recovery",
                &outcome,
            ));
        }
    }
    engine.recover_boot_selection(&conn)?;

    // Restore the /etc overlay after recovery mounts the generation.
    // recover() mounts the composefs image at <root>/mnt; the writable
    // /etc overlay uses staging/etc as lower and live /etc as target.
    if let Ok(Some(gen_num)) = current_generation(runtime_root.root()) {
        let staging_etc = staging.join("etc");
        let upper = runtime_root.etc_state_dir().join(gen_num.to_string());
        let work = runtime_root.etc_state_dir().join(format!("{gen_num}-work"));
        conary_core::generation::mount::mount_etc_overlay(
            &staging_etc,
            Path::new("/etc"),
            &upper,
            &work,
        )
        .map_err(|e| {
            let _ = unmount_generation(&staging);
            anyhow!("Failed to restore /etc overlay after recovery for generation {gen_num}: {e}")
        })?;
    }

    println!("Recovery complete.");
    Ok(())
}

#[cfg(test)]
mod tests;
