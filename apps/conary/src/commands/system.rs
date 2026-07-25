// apps/conary/src/commands/system.rs
//! System management commands (init, verify, rollback)

mod gc;
mod init;

pub use gc::cmd_gc;
pub use init::cmd_init;
#[cfg(test)]
use init::{NATIVE_REPOSITORY_SEEDS, paths_refer_to_same_location, validate_init_privileges};

#[cfg(test)]
use super::FileSnapshot;
use super::TroveSnapshot;
use super::open_db;
use super::progress::RemoveProgress;
use super::remove::{RemoveLifecycleOptions, execute_installed_trove_remove_graph};
#[cfg(test)]
use anyhow::Context;
use anyhow::{Result, anyhow};
use conary_core::db::models::{PackageResolution, Repository, RepositoryPackage, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
#[cfg(test)]
use conary_core::payload::PayloadNodeKind;
use conary_core::repository::RepositoryFormat;
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::scriptlet::SandboxMode;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

fn rollback_claim_status() -> &'static str {
    "applied"
}

fn is_rollback_eligible_status(status: &conary_core::db::models::ChangesetStatus) -> bool {
    matches!(status, conary_core::db::models::ChangesetStatus::Applied)
}

/// Rollback a changeset
pub async fn cmd_rollback(changeset_id: i64, db_path: &str) -> Result<()> {
    info!("Rolling back changeset: {}", changeset_id);
    println!("Rolling back changeset: {}", changeset_id);
    std::io::stdout().flush()?;
    if let Ok(delay_ms) = std::env::var("CONARY_TEST_HOLD_DURING_ROLLBACK_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
        && delay_ms > 0
    {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    let mut conn = open_db(db_path)?;
    require_active_generation_for_rollback(changeset_id, db_path)?;

    // All preflight checks run inside a transaction to eliminate the TOCTOU gap
    // that would allow two concurrent rollbacks to both pass checks.
    let (changeset, metadata) = conary_core::db::transaction(&mut conn, |tx| {
        let changeset = conary_core::db::models::Changeset::find_by_id(tx, changeset_id)?
            .ok_or_else(|| {
                conary_core::Error::InitError(format!("Changeset {} not found", changeset_id))
            })?;

        if changeset.status == conary_core::db::models::ChangesetStatus::RolledBack {
            return Err(conary_core::Error::InitError(format!(
                "Changeset {} is already rolled back",
                changeset_id
            )));
        }
        if changeset.status == conary_core::db::models::ChangesetStatus::Pending {
            return Err(conary_core::Error::InitError(format!(
                "Cannot rollback pending changeset {}",
                changeset_id
            )));
        }
        if !is_rollback_eligible_status(&changeset.status) {
            return Err(conary_core::Error::InitError(format!(
                "Changeset {} is not eligible for rollback (status: {})",
                changeset_id, changeset.status
            )));
        }

        let already_reversed: Option<i64> = tx.query_row(
            "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )?;
        if let Some(reverse_id) = already_reversed {
            return Err(conary_core::Error::InitError(format!(
                "Changeset {} has already been reversed by changeset {}",
                changeset_id, reverse_id
            )));
        }

        // Atomically claim this changeset for rollback using a conditional
        // UPDATE. We temporarily set reversed_by_changeset_id to the changeset's
        // own ID as an in-band claim marker; the actual rollback transaction
        // overwrites it with the real rollback changeset ID. This keeps status
        // within the valid enum while avoiding invalid foreign-key sentinels and
        // still prevents a second concurrent rollback from passing the
        // reversed_by_changeset_id IS NULL guard.
        let rollback_status = rollback_claim_status();
        let claimed = tx.execute(
            "UPDATE changesets SET reversed_by_changeset_id = ?2
             WHERE id = ?1 AND status = ?3 AND reversed_by_changeset_id IS NULL",
            rusqlite::params![changeset_id, changeset_id, rollback_status],
        )?;
        if claimed == 0 {
            return Err(conary_core::Error::InitError(format!(
                "Changeset {} is no longer eligible for rollback (concurrent rollback?)",
                changeset_id
            )));
        }

        let metadata: Option<String> = tx.query_row(
            "SELECT metadata FROM changesets WHERE id = ?1",
            [changeset_id],
            |row| row.get(0),
        )?;

        Ok((changeset, metadata))
    })?;

    // Helper: clear the self-reference claim marker if the rollback fails, so
    // the changeset doesn't get permanently wedged.
    let clear_claim = |conn: &rusqlite::Connection| {
        let _ = conn.execute(
            "UPDATE changesets SET reversed_by_changeset_id = NULL
             WHERE id = ?1 AND reversed_by_changeset_id = ?1",
            [changeset_id],
        );
    };

    if let Some(ref json) = metadata {
        let snapshots = crate::commands::parse_rollback_snapshots(json)?;
        // Check if this changeset also has installed troves (= upgrade vs removal)
        let has_troves: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM troves WHERE installed_by_changeset_id = ?1)",
            [changeset_id],
            |row| row.get(0),
        )?;

        return rollback_changeset_with_snapshots(
            changeset_id,
            &snapshots,
            has_troves,
            &mut conn,
            &changeset,
            db_path,
        )
        .inspect_err(|_| clear_claim(&conn));
    }

    // Otherwise, this is a fresh install. Revert each installed package as
    // its own exact native transaction graph, in reverse installation order.
    let files_to_rollback = {
        let mut stmt =
            conn.prepare("SELECT path, action FROM file_history WHERE changeset_id = ?1")?;
        let rows = stmt.query_map([changeset_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let troves = troves_installed_by_changeset(&conn, changeset_id)?;
    if troves.is_empty() {
        clear_claim(&conn);
        anyhow::bail!("No troves found for this changeset.");
    }
    validate_rollback_removal_targets(&conn, &troves)?;
    let removed_messages = (|| -> Result<Vec<String>> {
        let mut messages = Vec::with_capacity(troves.len());
        for trove in troves.iter().rev() {
            remove_rollback_trove_through_graph(&conn, trove, db_path)?;
            messages.push(format!("Removed {} version {}", trove.name, trove.version));
        }
        Ok(messages)
    })()
    .inspect_err(|_| clear_claim(&conn))?;

    let _rollback_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_id = rollback_changeset.insert(tx)?;
        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, changeset_id],
        )?;
        Ok(rollback_id)
    })
    .inspect_err(|_| clear_claim(&conn))?;

    for message in &removed_messages {
        println!("{message}");
    }
    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!("  {} files affected by rollback", files_to_rollback.len());

    Ok(())
}

fn has_active_generation(db_path: &str) -> bool {
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap_or(None)
        .is_some()
}

fn require_active_generation_for_rollback(changeset_id: i64, db_path: &str) -> Result<()> {
    if !has_active_generation(db_path) {
        anyhow::bail!(
            "Cannot roll back changeset {changeset_id} without an active composefs generation. \
             Build or activate a generation first, then retry rollback."
        );
    }
    Ok(())
}

fn troves_installed_by_changeset(
    conn: &rusqlite::Connection,
    changeset_id: i64,
) -> Result<Vec<Trove>> {
    Ok(Trove::list_all(conn)?
        .into_iter()
        .filter(|trove| trove.installed_by_changeset_id == Some(changeset_id))
        .collect())
}

fn remove_rollback_trove_through_graph(
    conn: &rusqlite::Connection,
    trove: &Trove,
    db_path: &str,
) -> Result<()> {
    let progress = RemoveProgress::new(&trove.name);
    execute_installed_trove_remove_graph(
        conn,
        trove,
        db_path,
        &trove.name,
        RemoveLifecycleOptions::new(SandboxMode::Always),
        &progress,
    )?;
    Ok(())
}

fn validate_rollback_removal_targets(conn: &rusqlite::Connection, troves: &[Trove]) -> Result<()> {
    for expected in troves {
        let trove_id = expected
            .id
            .ok_or_else(|| anyhow!("rollback removal target has no database identity"))?;
        let current = Trove::find_by_id(conn, trove_id)?
            .ok_or_else(|| anyhow!("rollback removal target {trove_id} disappeared"))?;
        if current.name != expected.name
            || current.version != expected.version
            || current.architecture != expected.architecture
            || current.installed_by_changeset_id != expected.installed_by_changeset_id
        {
            anyhow::bail!("rollback removal target {trove_id} changed after lifecycle preflight");
        }
    }
    Ok(())
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
struct LiveRootRollbackStats {
    files_restored: usize,
    dirs_restored: usize,
}

#[cfg(test)]
fn snapshot_path_under_root(root: &Path, path: &str) -> PathBuf {
    root.join(path.strip_prefix('/').unwrap_or(path))
}

#[cfg(test)]
fn snapshot_entry_is_dir(file: &FileSnapshot) -> bool {
    matches!(file.node.source.kind, PayloadNodeKind::Directory)
}

#[cfg(test)]
fn snapshot_entry_is_symlink(file: &FileSnapshot) -> bool {
    matches!(file.node.source.kind, PayloadNodeKind::Symlink { .. })
}

#[cfg(test)]
fn remove_existing_leaf_for_restore(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir(path)
                .with_context(|| format!("Failed to replace directory {}", path.display()))?;
        }
        Ok(_) => {
            std::fs::remove_file(path)
                .with_context(|| format!("Failed to replace file {}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect restore path {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
fn restore_snapshots_to_live_root(
    root: &Path,
    db_path: &str,
    snapshots: &[TroveSnapshot],
) -> Result<LiveRootRollbackStats> {
    let mut stats = LiveRootRollbackStats::default();
    let cas = CasStore::new(objects_dir(db_path))?;

    for snapshot in snapshots {
        for file in &snapshot.files {
            let path = snapshot_path_under_root(root, &file.path);

            if snapshot_entry_is_dir(file) {
                std::fs::create_dir_all(&path)
                    .with_context(|| format!("Failed to restore directory {}", path.display()))?;
                conary_core::generation::root_manifest::apply_resolved_payload_metadata(
                    &path, &file.node,
                )
                .with_context(|| {
                    format!("Failed to restore directory metadata on {}", path.display())
                })?;
                stats.dirs_restored += 1;
                continue;
            }

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory {}", parent.display())
                })?;
            }
            remove_existing_leaf_for_restore(&path)?;

            if snapshot_entry_is_symlink(file) {
                #[cfg(unix)]
                {
                    let PayloadNodeKind::Symlink { target } = &file.node.source.kind else {
                        unreachable!("typed symlink check and payload node diverged")
                    };
                    std::os::unix::fs::symlink(target, &path).with_context(|| {
                        format!("Failed to restore symlink {} -> {}", path.display(), target)
                    })?;
                }
                #[cfg(not(unix))]
                {
                    anyhow::bail!("Cannot restore symlink {} on this platform", file.path);
                }
            } else {
                let PayloadNodeKind::Regular { .. } = &file.node.source.kind else {
                    anyhow::bail!(
                        "test rollback helper does not materialize {:?} payload nodes",
                        file.node.source.kind
                    );
                };
                let authority = file.content.as_ref().ok_or_else(|| {
                    anyhow!(
                        "regular rollback snapshot {} has no content authority",
                        file.path
                    )
                })?;
                let content = cas
                    .retrieve(&authority.sha256)
                    .with_context(|| format!("Failed to retrieve CAS object for {}", file.path))?;
                if content.len() as u64 != authority.size
                    || conary_core::hash::sha256(&content) != authority.sha256
                {
                    anyhow::bail!(
                        "rollback snapshot content for {} differs from exact authority",
                        file.path
                    );
                }
                std::fs::write(&path, content)
                    .with_context(|| format!("Failed to restore file {}", path.display()))?;
            }
            conary_core::generation::root_manifest::apply_resolved_payload_metadata(
                &path, &file.node,
            )
            .with_context(|| format!("Failed to restore payload metadata on {}", path.display()))?;
            stats.files_restored += 1;
        }
    }

    Ok(stats)
}

fn restore_snapshot(
    tx: &rusqlite::Transaction<'_>,
    rollback_changeset_id: i64,
    snapshot: &TroveSnapshot,
) -> conary_core::Result<()> {
    let install_source: conary_core::db::models::InstallSource =
        snapshot.install_source.parse().map_err(|e| {
            conary_core::Error::InitError(format!(
                "Invalid install_source in snapshot '{}': {}",
                snapshot.install_source, e
            ))
        })?;
    let version_scheme = snapshot.version_scheme;

    let mut trove = conary_core::db::models::Trove::new_with_source(
        snapshot.name.clone(),
        snapshot.version.clone(),
        conary_core::db::models::TroveType::Package,
        install_source,
        version_scheme,
    );
    trove.architecture = snapshot.architecture.clone();
    trove.description = snapshot.description.clone();
    trove.source_distro = snapshot.source_distro.clone();
    trove.installed_by_changeset_id = Some(rollback_changeset_id);
    if let Some(repo_id) = snapshot.installed_from_repository_id {
        let repo_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM repositories WHERE id = ?1)",
                [repo_id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        trove.installed_from_repository_id = if repo_exists { Some(repo_id) } else { None };
    }

    let trove_id = trove.insert(tx)?;

    if let Some(native) = &snapshot.native_lifecycle {
        let bundle: conary_core::ccs::native_lifecycle::NativeLifecycleBundle =
            toml::from_str(&native.bundle_toml)
                .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
        if bundle.source_package != snapshot.name
            || bundle.source_version != snapshot.version
            || bundle.version_scheme.as_str() != version_scheme.as_str()
        {
            return Err(conary_core::Error::InitError(format!(
                "rollback snapshot '{}' native lifecycle identity does not match its installed package identity",
                snapshot.name
            )));
        }
        let lifecycle_state =
            conary_core::ccs::native_transaction::DebPackageState::parse(&native.lifecycle_state)
                .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
        let mut installed = conary_core::db::models::InstalledNativeLifecycleBundle::new(
            trove_id,
            Some(rollback_changeset_id),
            &bundle,
        )
        .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
        installed.lifecycle_state = lifecycle_state;
        installed.pending_triggers = native.pending_triggers.clone();
        installed.awaited_packages = native.awaited_packages.clone();
        installed
            .insert_or_replace(tx)
            .map_err(|error| conary_core::Error::InitError(error.to_string()))?;
    }
    if let Some(hook) = &snapshot.ccs_remove_hook {
        if version_scheme != conary_core::repository::versioning::VersionScheme::Conary {
            return Err(conary_core::Error::InitError(format!(
                "rollback snapshot '{}' carries a CCS remove hook with non-Conary version scheme '{}'",
                snapshot.name,
                version_scheme.as_str()
            )));
        }
        conary_core::db::models::InstalledCcsRemoveHook::new(
            trove_id,
            hook.script.clone(),
            hook.reversible,
        )
        .insert_or_replace(tx)?;
    }

    for file in &snapshot.files {
        let mut file_entry = conary_core::db::models::FileEntry::new(
            file.path.clone(),
            file.node.clone(),
            file.content.clone(),
            trove_id,
        );
        file_entry.insert(tx)?;

        if let Some(content) = file.content.as_ref() {
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![rollback_changeset_id, &file.path, &content.sha256, "add"],
            )?;
        }
    }

    Ok(())
}

fn rollback_changeset_with_snapshots(
    changeset_id: i64,
    snapshots: &[TroveSnapshot],
    remove_new_troves: bool,
    conn: &mut rusqlite::Connection,
    changeset: &conary_core::db::models::Changeset,
    db_path: &str,
) -> Result<()> {
    if snapshots.is_empty() {
        anyhow::bail!(
            "Changeset {} metadata did not contain any rollback snapshots",
            changeset_id
        );
    }

    require_active_generation_for_rollback(changeset_id, db_path)?;
    let new_troves = if remove_new_troves {
        troves_installed_by_changeset(conn, changeset_id)?
    } else {
        Vec::new()
    };
    validate_rollback_removal_targets(conn, &new_troves)?;
    let mut removed_messages = Vec::with_capacity(new_troves.len());
    for trove in new_troves.iter().rev() {
        remove_rollback_trove_through_graph(conn, trove, db_path)?;
        removed_messages.push(format!(
            "  Removed reverted package {} {}",
            trove.name, trove.version
        ));
    }

    let _rollback_id = conary_core::db::transaction(conn, |tx| {
        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_id = rollback_changeset.insert(tx)?;

        for snapshot in snapshots {
            restore_snapshot(tx, rollback_id, snapshot)?;
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, changeset_id],
        )?;

        Ok(rollback_id)
    })?;

    let summary = if remove_new_troves {
        format!("Rollback changeset {}", changeset_id)
    } else {
        format!("Rollback removal of {}", snapshots[0].name)
    };
    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount_from_installed_state(
        conn, db_path, &summary,
    )?;

    let restored_file_count: usize = snapshots.iter().map(|snapshot| snapshot.files.len()).sum();

    for message in &removed_messages {
        println!("{message}");
    }
    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    for snapshot in snapshots {
        println!("  Restored {} version {}", snapshot.name, snapshot.version);
    }
    println!("  Files in DB: {}", restored_file_count);

    Ok(())
}

/// Verify installed files
pub async fn cmd_verify(
    package: Option<String>,
    db_path: &str,
    _root: &str,
    use_rpm: bool,
) -> Result<()> {
    info!("Verifying installed files...");

    let conn = open_db(db_path)?;

    // If --rpm flag, verify adopted packages against RPM database
    if use_rpm {
        return verify_against_rpm(&conn, package);
    }

    // In composefs-native, verify means checking that CAS objects exist
    // for all file_entries in the DB. The EROFS image is built from these.
    let objects_dir = objects_dir(db_path);
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;

    let files: Vec<(String, Option<String>, String)> = if let Some(pkg_name) = package {
        let troves = conary_core::db::models::Trove::find_by_name(&conn, &pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }

        let mut all_files = Vec::new();
        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let trove_files =
                    conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
                for file in trove_files {
                    all_files.push((
                        file.path,
                        file.content.map(|content| content.sha256),
                        trove.name.clone(),
                    ));
                }
            }
        }
        all_files
    } else {
        let troves = conary_core::db::models::Trove::list_all(&conn)?
            .into_iter()
            .filter_map(|trove| trove.id.map(|id| (id, trove.name)))
            .collect::<std::collections::HashMap<_, _>>();
        conary_core::db::models::FileEntry::find_all_ordered(&conn)?
            .into_iter()
            .map(|file| {
                let package = troves
                    .get(&file.trove_id)
                    .cloned()
                    .unwrap_or_else(|| format!("<missing-trove:{}>", file.trove_id));
                (
                    file.path,
                    file.content.map(|content| content.sha256),
                    package,
                )
            })
            .collect()
    };

    if files.is_empty() {
        println!("No files to verify");
        return Ok(());
    }

    let mut ok_count = 0;
    let mut missing_count = 0;

    for (path, expected_hash, pkg_name) in &files {
        // Composefs-native verify: check that the CAS object exists
        if expected_hash
            .as_deref()
            .is_none_or(|expected_hash| cas.exists(expected_hash))
        {
            ok_count += 1;
            info!("OK: {} (from {})", path, pkg_name);
        } else {
            missing_count += 1;
            println!("MISSING from CAS: {} (from {})", path, pkg_name);
        }
    }

    println!("\nVerification summary:");
    println!("  OK (in CAS): {} files", ok_count);
    println!("  Missing from CAS: {} files", missing_count);
    println!("  Total: {} files", files.len());

    if missing_count > 0 {
        return Err(anyhow::anyhow!(
            "Verification failed: {} files missing from CAS",
            missing_count
        ));
    }

    Ok(())
}

/// Verify adopted packages against RPM database using `rpm -V`
fn verify_against_rpm(conn: &rusqlite::Connection, package: Option<String>) -> Result<()> {
    use std::process::Command;

    // Check if RPM is available
    if !conary_core::packages::rpm_query::is_rpm_available() {
        return Err(anyhow::anyhow!("RPM is not available on this system"));
    }

    // Get adopted packages to verify
    let packages: Vec<String> = if let Some(pkg_name) = package {
        let troves = conary_core::db::models::Trove::find_by_name(conn, &pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not tracked", pkg_name));
        }
        // Check if it's adopted
        let adopted: Vec<_> = troves
            .iter()
            .filter(|t| {
                matches!(
                    t.install_source,
                    conary_core::db::models::InstallSource::AdoptedTrack
                        | conary_core::db::models::InstallSource::AdoptedFull
                )
            })
            .collect();
        if adopted.is_empty() {
            return Err(anyhow::anyhow!(
                "Package '{}' is not an adopted package. Use --rpm only for adopted packages.",
                pkg_name
            ));
        }
        vec![pkg_name]
    } else {
        // Get all adopted packages
        let mut stmt = conn.prepare(
            "SELECT name FROM troves WHERE install_source LIKE 'adopted%' ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if packages.is_empty() {
        println!("No adopted packages to verify");
        return Ok(());
    }

    println!(
        "Verifying {} adopted packages against RPM database...\n",
        packages.len()
    );

    let mut verified_count = 0;
    let mut failed_count = 0;
    let mut total_issues = 0;

    for pkg_name in &packages {
        // Run rpm -V <package>
        let output = Command::new("rpm").args(["-V", pkg_name]).output();

        match output {
            Ok(result) => {
                if result.status.success() && result.stdout.is_empty() {
                    // No output means all files verified OK
                    verified_count += 1;
                    info!("OK: {}", pkg_name);
                } else {
                    // There were verification failures
                    failed_count += 1;
                    let issues = String::from_utf8_lossy(&result.stdout);
                    let issue_count = issues.lines().count();
                    total_issues += issue_count;
                    println!("FAILED: {} ({} issues)", pkg_name, issue_count);
                    for line in issues.lines().take(5) {
                        println!("  {}", line);
                    }
                    if issue_count > 5 {
                        println!("  ... and {} more", issue_count - 5);
                    }
                }
            }
            Err(e) => {
                failed_count += 1;
                println!("ERROR: {} - {}", pkg_name, e);
            }
        }
    }

    println!("\nRPM Verification summary:");
    println!("  OK: {} packages", verified_count);
    println!("  Failed: {} packages", failed_count);
    println!("  Total issues: {}", total_issues);
    println!("  Total packages: {}", packages.len());

    if failed_count > 0 {
        return Err(anyhow::anyhow!("RPM verification failed"));
    }

    Ok(())
}

#[cfg(test)]
mod tests;
