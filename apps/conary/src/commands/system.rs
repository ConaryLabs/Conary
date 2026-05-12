// src/commands/system.rs
//! System management commands (init, verify, rollback)

use super::open_db;
use super::{FileSnapshot, RevertMetadata, TroveSnapshot};
use anyhow::{Context, Result};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

/// Initialize the Conary database and add default repositories
pub async fn cmd_init(db_path: &str) -> Result<()> {
    info!("Initializing Conary database at: {}", db_path);
    conary_core::db::init(db_path)?;
    println!("Database initialized successfully at: {}", db_path);

    let mut conn = open_db(db_path)?;
    info!("Adding default repositories...");

    // Collect messages inside the transaction; print after commit to avoid
    // interleaving output with a potential rollback log.
    let mut messages: Vec<String> = Vec::new();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut remi_repo = conary_core::db::models::Repository::new(
            "remi".to_string(),
            "https://remi.conary.io".to_string(),
        );
        remi_repo.priority = 110;
        remi_repo.default_strategy = Some("remi".to_string());
        remi_repo.default_strategy_endpoint = Some("https://remi.conary.io".to_string());
        remi_repo.default_strategy_distro = Some("fedora".to_string());
        match remi_repo.insert(tx) {
            Ok(_) => messages.push("  Added: remi (Conary Remi (CCS))".to_string()),
            Err(e) => messages.push(format!("  Warning: Could not add remi: {e}")),
        }

        let default_repos = [
            (
                "arch-core",
                "https://geo.mirror.pkgbuild.com/core/os/x86_64",
                100,
                "Arch Linux",
            ),
            (
                "arch-extra",
                "https://geo.mirror.pkgbuild.com/extra/os/x86_64",
                95,
                "Arch Linux",
            ),
            (
                "fedora-44",
                "https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Everything/x86_64/os",
                90,
                "Fedora 44",
            ),
            (
                "arch-multilib",
                "https://geo.mirror.pkgbuild.com/multilib/os/x86_64",
                85,
                "Arch Linux",
            ),
            (
                "ubuntu-26.04",
                "http://archive.ubuntu.com/ubuntu",
                80,
                "Ubuntu 26.04 LTS",
            ),
        ];

        for (name, url, priority, desc) in default_repos {
            match conary_core::repository::add_repository(
                tx,
                name.to_string(),
                url.to_string(),
                true,
                priority,
            ) {
                Ok(_) => messages.push(format!("  Added: {name} ({desc})")),
                Err(e) => messages.push(format!("  Warning: Could not add {name}: {e}")),
            }
        }

        Ok(())
    })?;

    for msg in &messages {
        if msg.contains("Warning:") {
            eprintln!("{msg}");
        } else {
            println!("{msg}");
        }
    }

    println!("\nDefault repositories added. Use 'conary repo sync' to download metadata.");
    Ok(())
}

fn rollback_claim_statuses() -> [&'static str; 2] {
    ["applied", "post_hooks_failed"]
}

fn is_rollback_eligible_status(status: &conary_core::db::models::ChangesetStatus) -> bool {
    matches!(
        status,
        conary_core::db::models::ChangesetStatus::Applied
            | conary_core::db::models::ChangesetStatus::PostHooksFailed
    )
}

/// Rollback a changeset
pub async fn cmd_rollback(changeset_id: i64, db_path: &str, root: &str) -> Result<()> {
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
        let rollback_statuses = rollback_claim_statuses();
        let claimed = tx.execute(
            "UPDATE changesets SET reversed_by_changeset_id = ?2
             WHERE id = ?1 AND status IN (?3, ?4) AND reversed_by_changeset_id IS NULL",
            rusqlite::params![
                changeset_id,
                changeset_id,
                rollback_statuses[0],
                rollback_statuses[1]
            ],
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
        let snapshots = parse_rollback_snapshots(json)?;
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
            root,
        )
        .inspect_err(|_| clear_claim(&conn));
    }

    // Otherwise, this is a fresh install - remove the installed packages
    let files_to_rollback = RefCell::new(Vec::new());
    let removed_messages = RefCell::new(Vec::new());
    let removed_snapshots = RefCell::new(Vec::new());
    let rollback_changeset_id = Cell::new(0_i64);

    conary_core::db::transaction(&mut conn, |tx| {
        // Read file history inside the transaction to ensure consistency (TOCTOU)
        // Note: if this transaction fails, clear_claim (below) resets the -1 sentinel.
        {
            let mut stmt =
                tx.prepare("SELECT path, action FROM file_history WHERE changeset_id = ?1")?;
            let rows = stmt.query_map([changeset_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            *files_to_rollback.borrow_mut() = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        }

        let troves = {
            let mut stmt = tx.prepare(
                "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason, label_id, orphan_since, source_distro, version_scheme, installed_from_repository_id
                 FROM troves WHERE installed_by_changeset_id = ?1",
            )?;
            let rows = stmt.query_map([changeset_id], |row| {
                let source_str: Option<String> = row.get(8)?;
                let install_source = match source_str {
                    Some(s) => s.parse().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("Invalid install_source '{}': {}", s, e),
                            )),
                        )
                    })?,
                    None => conary_core::db::models::InstallSource::File,
                };
                let reason_str: Option<String> = row.get(9)?;
                let install_reason = match reason_str {
                    Some(s) => s.parse().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("Invalid install_reason '{}': {}", s, e),
                            )),
                        )
                    })?,
                    None => conary_core::db::models::InstallReason::Explicit,
                };
                let trove_type_str: String = row.get(3)?;
                let trove_type = trove_type_str.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Invalid trove_type '{}': {}", trove_type_str, e),
                        )),
                    )
                })?;
                let flavor_spec: Option<String> = row.get(10)?;
                let pinned: i32 = row.get(11).unwrap_or(0);
                let selection_reason: Option<String> = row.get(12).unwrap_or(None);
                let label_id: Option<i64> = row.get(13).unwrap_or(None);
                let orphan_since: Option<String> = row.get(14).unwrap_or(None);
                let source_distro: Option<String> = row.get(15).unwrap_or(None);
                let version_scheme: Option<String> = row.get(16).unwrap_or(None);
                Ok(conary_core::db::models::Trove {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    version: row.get(2)?,
                    trove_type,
                    architecture: row.get(4)?,
                    description: row.get(5)?,
                    installed_at: row.get(6)?,
                    installed_by_changeset_id: row.get(7)?,
                    install_source,
                    install_reason,
                    flavor_spec,
                    pinned: pinned != 0,
                    selection_reason,
                    label_id,
                    orphan_since,
                    source_distro,
                    version_scheme,
                    installed_from_repository_id: row.get(17).unwrap_or(None),
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if troves.is_empty() {
            return Err(conary_core::Error::InitError(
                "No troves found for this changeset.".to_string(),
            ));
        }

        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_id = rollback_changeset.insert(tx)?;
        rollback_changeset_id.set(rollback_id);

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                removed_snapshots
                    .borrow_mut()
                    .push(snapshot_trove(tx, trove)?);
                conary_core::db::models::Trove::delete(tx, trove_id)?;
                removed_messages
                    .borrow_mut()
                    .push(format!("Removed {} version {}", trove.name, trove.version));
            }
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;

        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, changeset_id],
        )?;

        Ok(troves.len())
    })
    .inspect_err(|_| clear_claim(&conn))?;

    let files_to_rollback = files_to_rollback.into_inner();
    let removed_messages = removed_messages.into_inner();
    let removed_snapshots = removed_snapshots.into_inner();

    let summary = format!("Rollback changeset {}", changeset_id);
    if has_active_generation(db_path) {
        // Composefs-native: rebuild EROFS image from updated DB state and remount
        let _gen_num =
            crate::commands::composefs_ops::rebuild_and_mount(&conn, db_path, &summary, None)?;
    } else {
        let stats = remove_snapshots_from_live_root(Path::new(root), &removed_snapshots)?;
        info!(
            "Removed {} file(s) and {} directories directly during rollback because no active generation exists",
            stats.files_removed, stats.dirs_removed
        );
        super::create_state_snapshot(&conn, rollback_changeset_id.get(), &summary)?;
    }

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

fn parse_rollback_snapshots(snapshot_json: &str) -> Result<Vec<TroveSnapshot>> {
    if let Ok(wrapper) = serde_json::from_str::<RevertMetadata>(snapshot_json) {
        return Ok(wrapper.removed_troves);
    }
    Ok(vec![serde_json::from_str(snapshot_json)?])
}

fn has_active_generation(db_path: &str) -> bool {
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    conary_core::generation::mount::current_generation(runtime_root.root())
        .unwrap_or(None)
        .is_some()
}

fn snapshot_trove(
    conn: &rusqlite::Connection,
    trove: &conary_core::db::models::Trove,
) -> conary_core::Result<TroveSnapshot> {
    let trove_id = trove.id.ok_or_else(|| {
        conary_core::Error::MissingId("Cannot snapshot trove without ID".to_string())
    })?;
    let files = conary_core::db::models::FileEntry::find_by_trove(conn, trove_id)?;
    Ok(TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        architecture: trove.architecture.clone(),
        description: trove.description.clone(),
        install_source: trove.install_source.as_str().to_string(),
        installed_from_repository_id: trove.installed_from_repository_id,
        files: files
            .iter()
            .map(|file| FileSnapshot {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size: file.size,
                permissions: file.permissions,
                symlink_target: file.symlink_target.clone(),
            })
            .collect(),
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct LiveRootRollbackStats {
    files_removed: usize,
    dirs_removed: usize,
    files_restored: usize,
    dirs_restored: usize,
}

fn snapshot_path_under_root(root: &Path, path: &str) -> PathBuf {
    root.join(path.strip_prefix('/').unwrap_or(path))
}

fn snapshot_entry_is_dir(file: &FileSnapshot) -> bool {
    file.path.ends_with('/') || (file.permissions as u32 & 0o170000) == 0o040000
}

fn snapshot_entry_is_symlink(file: &FileSnapshot) -> bool {
    file.symlink_target.is_some() || (file.permissions as u32 & 0o170000) == 0o120000
}

fn remove_snapshots_from_live_root(
    root: &Path,
    snapshots: &[TroveSnapshot],
) -> Result<LiveRootRollbackStats> {
    let mut stats = LiveRootRollbackStats::default();
    let mut dirs = Vec::new();

    for snapshot in snapshots {
        for file in &snapshot.files {
            let path = snapshot_path_under_root(root, &file.path);
            if snapshot_entry_is_dir(file) {
                dirs.push(path);
                continue;
            }

            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {
                    dirs.push(path);
                }
                Ok(_) => {
                    std::fs::remove_file(&path).with_context(|| {
                        format!("Failed to remove rollback file {}", path.display())
                    })?;
                    stats.files_removed += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    warn!(
                        "Rollback file {} was already absent during direct live-root removal",
                        path.display()
                    );
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to inspect rollback file {}", path.display())
                    });
                }
            }
        }
    }

    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    dirs.dedup();
    for dir in dirs {
        match std::fs::remove_dir(&dir) {
            Ok(()) => stats.dirs_removed += 1,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to remove rollback dir {}", dir.display()));
            }
        }
    }

    Ok(stats)
}

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
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = (file.permissions as u32) & 0o7777;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                        .with_context(|| {
                            format!("Failed to set permissions on {}", path.display())
                        })?;
                }
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
                    let target = match file.symlink_target.as_deref() {
                        Some(target) => target.to_string(),
                        None => cas.retrieve_symlink(&file.sha256_hash).with_context(|| {
                            format!("Failed to retrieve symlink target for {}", file.path)
                        })?,
                    };
                    std::os::unix::fs::symlink(&target, &path).with_context(|| {
                        format!("Failed to restore symlink {} -> {}", path.display(), target)
                    })?;
                }
                #[cfg(not(unix))]
                {
                    anyhow::bail!("Cannot restore symlink {} on this platform", file.path);
                }
            } else {
                let content = cas
                    .retrieve(&file.sha256_hash)
                    .with_context(|| format!("Failed to retrieve CAS object for {}", file.path))?;
                std::fs::write(&path, content)
                    .with_context(|| format!("Failed to restore file {}", path.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = (file.permissions as u32) & 0o7777;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                        .with_context(|| {
                            format!("Failed to set permissions on {}", path.display())
                        })?;
                }
            }
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

    let mut trove = conary_core::db::models::Trove::new_with_source(
        snapshot.name.clone(),
        snapshot.version.clone(),
        conary_core::db::models::TroveType::Package,
        install_source,
    );
    trove.architecture = snapshot.architecture.clone();
    trove.description = snapshot.description.clone();
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

    for file in &snapshot.files {
        let mut file_entry = conary_core::db::models::FileEntry::new(
            file.path.clone(),
            file.sha256_hash.clone(),
            file.size,
            file.permissions,
            trove_id,
        );
        file_entry.symlink_target = file.symlink_target.clone();
        file_entry.insert(tx)?;

        if file.sha256_hash.len() == 64 && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&rollback_changeset_id.to_string(), &file.path, &file.sha256_hash, "add"],
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
    root: &str,
) -> Result<()> {
    if snapshots.is_empty() {
        anyhow::bail!(
            "Changeset {} metadata did not contain any rollback snapshots",
            changeset_id
        );
    }

    let removed_messages = RefCell::new(Vec::new());
    let removed_snapshots = RefCell::new(Vec::new());
    let rollback_changeset_id = Cell::new(0_i64);

    conary_core::db::transaction(conn, |tx| {
        if remove_new_troves {
            let new_trove_ids: Vec<i64> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM troves WHERE installed_by_changeset_id = ?1")?;
                stmt.query_map([changeset_id], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };

            for trove_id in &new_trove_ids {
                let trove = conary_core::db::models::Trove::find_by_id(tx, *trove_id)?.ok_or_else(
                    || {
                        conary_core::Error::InitError(format!(
                            "Trove {} disappeared during rollback",
                            trove_id
                        ))
                    },
                )?;
                removed_snapshots
                    .borrow_mut()
                    .push(snapshot_trove(tx, &trove)?);
                conary_core::db::models::Trove::delete(tx, *trove_id)?;
                removed_messages.borrow_mut().push(format!(
                    "  Removed reverted package {} {}",
                    trove.name, trove.version
                ));
            }
        }

        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_id = rollback_changeset.insert(tx)?;
        rollback_changeset_id.set(rollback_id);

        for snapshot in snapshots {
            restore_snapshot(tx, rollback_id, snapshot)?;
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, changeset_id],
        )?;

        Ok(())
    })?;

    let summary = if remove_new_troves {
        format!("Rollback changeset {}", changeset_id)
    } else {
        format!("Rollback removal of {}", snapshots[0].name)
    };
    if has_active_generation(db_path) {
        let _gen_num =
            crate::commands::composefs_ops::rebuild_and_mount(conn, db_path, &summary, None)?;
    } else {
        let root_path = Path::new(root);
        let remove_stats =
            remove_snapshots_from_live_root(root_path, &removed_snapshots.into_inner())?;
        let restore_stats = restore_snapshots_to_live_root(root_path, db_path, snapshots)?;
        info!(
            "Applied rollback directly because no active generation exists: removed {} file(s), restored {} file(s)",
            remove_stats.files_removed, restore_stats.files_restored
        );
        super::create_state_snapshot(conn, rollback_changeset_id.get(), &summary)?;
    }

    let removed_messages = removed_messages.into_inner();
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

    let files: Vec<(String, String, String)> = if let Some(pkg_name) = package {
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
                    all_files.push((file.path, file.sha256_hash, trove.name.clone()));
                }
            }
        }
        all_files
    } else {
        let mut stmt = conn.prepare(
            "SELECT f.path, f.sha256_hash, t.name FROM files f
             JOIN troves t ON f.trove_id = t.id ORDER BY t.name, f.path",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if files.is_empty() {
        println!("No files to verify");
        return Ok(());
    }

    let mut ok_count = 0;
    let mut missing_count = 0;

    for (path, expected_hash, pkg_name) in &files {
        // Composefs-native verify: check that the CAS object exists
        if cas.exists(expected_hash) {
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

/// Garbage collect unreferenced files from CAS storage
///
/// This removes files from the content-addressable store that are no longer
/// referenced by any installed package or recent file history (for rollback).
pub async fn cmd_gc(
    db_path: &str,
    objects_dir: &str,
    keep_days: u32,
    dry_run: bool,
    chunks: bool,
) -> Result<()> {
    use std::collections::HashSet;
    use std::fs;

    info!(
        "Starting CAS garbage collection (keep_days={}, dry_run={})",
        keep_days, dry_run
    );

    let conn = open_db(db_path)?;
    let objects_path = Path::new(objects_dir);

    if !objects_path.exists() {
        println!("CAS directory does not exist: {}", objects_dir);
        return Ok(());
    }

    // Step 1: Collect all referenced hashes from installed files
    println!("Collecting referenced hashes from installed packages...");
    let mut referenced_hashes: HashSet<String> = HashSet::new();

    let file_hashes: Vec<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT sha256_hash FROM files WHERE sha256_hash IS NOT NULL AND sha256_hash != ''")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for hash in file_hashes {
        referenced_hashes.insert(hash);
    }
    println!(
        "  Found {} hashes from installed files",
        referenced_hashes.len()
    );

    // Step 2: Collect hashes from file_history within retention period
    println!(
        "Collecting hashes from recent file history ({}+ days)...",
        keep_days
    );
    let history_hashes: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT fh.sha256_hash FROM file_history fh
             JOIN changesets c ON fh.changeset_id = c.id
             WHERE fh.sha256_hash IS NOT NULL AND fh.sha256_hash != ''
             AND c.applied_at >= datetime('now', ?1)",
        )?;
        let days_param = format!("-{} days", keep_days);
        let rows = stmt.query_map([days_param], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for hash in history_hashes {
        referenced_hashes.insert(hash);
    }
    println!("  Total referenced hashes: {}", referenced_hashes.len());

    // Step 3: Scan CAS directory for all stored objects
    println!("Scanning CAS directory for objects...");
    let mut cas_objects: Vec<(PathBuf, String)> = Vec::new();
    let mut total_cas_size: u64 = 0;

    let cas = CasStore::new(objects_path)?;
    for result in cas.iter_objects() {
        let (hash, path) = result?;
        let metadata = fs::metadata(&path)?;
        total_cas_size += metadata.len();
        cas_objects.push((path, hash));
    }
    println!(
        "  Found {} objects in CAS ({} total)",
        cas_objects.len(),
        format_bytes(total_cas_size)
    );

    // Step 4: Find unreferenced objects
    let unreferenced: Vec<(PathBuf, String)> = cas_objects
        .into_iter()
        .filter(|(_, hash)| !referenced_hashes.contains(hash))
        .collect();

    if unreferenced.is_empty() {
        println!("\nNo unreferenced objects found. CAS is clean.");
        return Ok(());
    }

    // Calculate space to reclaim
    let mut reclaimable_size: u64 = 0;
    for (path, _) in &unreferenced {
        if let Ok(metadata) = fs::metadata(path) {
            reclaimable_size += metadata.len();
        }
    }

    println!(
        "\nFound {} unreferenced objects ({})",
        unreferenced.len(),
        format_bytes(reclaimable_size)
    );

    // Step 5: Delete unreferenced objects (or just report if dry_run)
    if dry_run {
        println!("\nDry run - would remove {} objects:", unreferenced.len());
        for (_, hash) in unreferenced.iter().take(10) {
            println!("  {}", hash);
        }
        if unreferenced.len() > 10 {
            println!("  ... and {} more", unreferenced.len() - 10);
        }
        println!("\nRun without --dry-run to actually remove these objects.");
    } else {
        println!("\nRemoving unreferenced objects...");
        let mut removed_count = 0;
        let mut error_count = 0;

        for (path, hash) in &unreferenced {
            match fs::remove_file(path) {
                Ok(()) => {
                    removed_count += 1;
                    info!("Removed: {}", hash);
                }
                Err(e) => {
                    error_count += 1;
                    info!("Failed to remove {}: {}", hash, e);
                }
            }
        }

        // Clean up empty prefix directories
        for prefix_entry in fs::read_dir(objects_path)? {
            let prefix_entry = prefix_entry?;
            let prefix_path = prefix_entry.path();

            if prefix_path.is_dir() {
                // Try to remove if empty (will fail silently if not empty)
                let _ = fs::remove_dir(&prefix_path);
            }
        }

        println!("\nGarbage collection complete:");
        println!("  Removed: {} objects", removed_count);
        println!("  Errors: {}", error_count);
        println!("  Space reclaimed: {}", format_bytes(reclaimable_size));
    }

    if chunks {
        gc_orphaned_chunks(&conn, db_path, dry_run)?;
    }

    Ok(())
}

/// Local-only chunk GC for the CLI. The full async version with R2 support
/// lives in apps/remi/src/server/chunk_gc.rs.
///
/// Scans the CAS objects directory for chunk files that are not referenced by
/// any converted package (`chunk_hashes_json`) or protected in `chunk_access`.
fn gc_orphaned_chunks(conn: &rusqlite::Connection, db_path: &str, dry_run: bool) -> Result<()> {
    use std::collections::HashSet;

    let objects_dir = conary_core::db::paths::objects_dir(db_path);

    println!("\nChunk GC: collecting referenced chunk hashes...");

    // Build referenced set from converted_packages
    let mut referenced = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT chunk_hashes_json FROM converted_packages WHERE chunk_hashes_json IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let json_str: String = row.get(0)?;
        if let Ok(hashes) = serde_json::from_str::<Vec<String>>(&json_str) {
            for hash in hashes {
                referenced.insert(hash);
            }
        }
    }

    // Add protected chunks from chunk_access
    let mut stmt = conn.prepare("SELECT hash FROM chunk_access WHERE protected = 1")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add live file hashes from installed packages so we never delete CAS
    // objects that are still referenced by troves, rollback history, etc.
    let mut stmt = conn.prepare("SELECT DISTINCT sha256_hash FROM files WHERE sha256_hash IS NOT NULL AND sha256_hash != ''")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add config backup hashes (used by config restore/diff)
    let mut stmt = conn.prepare(
        "SELECT DISTINCT backup_hash FROM config_backups WHERE backup_hash IS NOT NULL AND backup_hash != ''",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        referenced.insert(hash);
    }

    // Add config file original hashes (used by config diff)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT original_hash FROM config_files WHERE original_hash IS NOT NULL AND original_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Add derived package CAS objects (patches and overrides)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT patch_hash FROM derived_patches WHERE patch_hash IS NOT NULL AND patch_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT source_hash FROM derived_overrides WHERE source_hash IS NOT NULL AND source_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Add derivation CAS objects (manifest + provenance hashes)
    if let Ok(mut stmt) = conn.prepare(
        "SELECT manifest_cas_hash FROM derivation_index WHERE manifest_cas_hash IS NOT NULL AND manifest_cas_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT provenance_cas_hash FROM derivation_index WHERE provenance_cas_hash IS NOT NULL AND provenance_cas_hash != ''",
    ) {
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let hash: String = row.get(0)?;
            referenced.insert(hash);
        }
    }

    // Also protect hashes referenced by state snapshots (rollback/restore)
    let mut stmt = conn.prepare("SELECT metadata FROM changesets WHERE metadata IS NOT NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let json_str: String = row.get(0)?;
        // Snapshot metadata contains file hashes — extract them conservatively
        // by matching hex-like strings that look like CAS hashes.
        for word in json_str.split('"') {
            // CAS hashes are hex strings of 64 chars (SHA-256)
            if word.len() == 64 && word.chars().all(|c| c.is_ascii_hexdigit()) {
                referenced.insert(word.to_string());
            }
        }
    }

    // Scan local chunks in the objects directory
    let mut orphaned = 0usize;
    let mut freed = 0u64;
    if objects_dir.exists() {
        let cas = CasStore::new(&objects_dir)?;
        for result in cas.iter_objects() {
            let (hash, path) = result?;
            if !referenced.contains(&hash) {
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                if dry_run {
                    println!("[dry-run] Would delete: {} ({} bytes)", hash, size);
                } else {
                    let _ = std::fs::remove_file(&path);
                    // Try to remove empty parent directory
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
                orphaned += 1;
                freed += size;
            }
        }
    }

    println!(
        "Chunk GC: {} referenced, {} orphaned, {} freed",
        referenced.len(),
        orphaned,
        format_bytes(freed)
    );

    Ok(())
}

use super::format_bytes;

#[cfg(test)]
mod tests {
    use super::{
        cmd_init, cmd_rollback, parse_rollback_snapshots, restore_snapshots_to_live_root,
        rollback_claim_statuses,
    };
    use crate::commands::{FileSnapshot, RevertMetadata, TroveSnapshot};
    use conary_core::db::models::{
        Changeset, ChangesetStatus, FileEntry, InstallSource, Trove, TroveType,
    };
    use conary_core::db::paths::objects_dir;
    use conary_core::filesystem::CasStore;
    use rusqlite::params;
    use std::path::Path;

    #[tokio::test]
    async fn init_adds_remi_with_strategy_defaults() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();

        cmd_init(db_path_str).await.unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        let repo = conary_core::db::models::Repository::find_by_name(&conn, "remi")
            .unwrap()
            .unwrap();
        assert_eq!(repo.url, "https://remi.conary.io");
        assert_eq!(repo.default_strategy.as_deref(), Some("remi"));
        assert_eq!(
            repo.default_strategy_endpoint.as_deref(),
            Some("https://remi.conary.io")
        );
        assert_eq!(repo.default_strategy_distro.as_deref(), Some("fedora"));
    }

    #[test]
    fn rollback_claim_statuses_include_post_hooks_failed() {
        assert_eq!(rollback_claim_statuses(), ["applied", "post_hooks_failed"]);
    }

    #[test]
    fn rollback_metadata_parser_accepts_legacy_and_revert_wrapper_formats() {
        let single = TroveSnapshot {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: Some("web server".to_string()),
            install_source: "repository".to_string(),
            installed_from_repository_id: Some(7),
            files: vec![FileSnapshot {
                path: "/usr/sbin/nginx".to_string(),
                sha256_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                size: 1024,
                permissions: 0o755,
                symlink_target: None,
            }],
        };

        let parsed_single =
            parse_rollback_snapshots(&serde_json::to_string(&single).unwrap()).unwrap();
        assert_eq!(parsed_single.len(), 1);
        assert_eq!(parsed_single[0].name, "nginx");

        let wrapper = RevertMetadata {
            removed_troves: vec![
                single.clone(),
                TroveSnapshot {
                    name: "vim".to_string(),
                    version: "9.1.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    description: Some("editor".to_string()),
                    install_source: "repository".to_string(),
                    installed_from_repository_id: None,
                    files: Vec::new(),
                },
            ],
        };

        let parsed_wrapper =
            parse_rollback_snapshots(&serde_json::to_string(&wrapper).unwrap()).unwrap();
        assert_eq!(parsed_wrapper.len(), 2);
        assert_eq!(parsed_wrapper[0].name, "nginx");
        assert_eq!(parsed_wrapper[1].name, "vim");
    }

    fn store_test_object(conn: &rusqlite::Connection, db_path: &Path, content: &[u8]) -> String {
        let cas = CasStore::new(objects_dir(&db_path.to_string_lossy())).unwrap();
        let hash = cas.store(content).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
            params![
                &hash,
                format!("objects/{}/{}", &hash[0..2], &hash[2..]),
                content.len() as i64
            ],
        )
        .unwrap();
        hash
    }

    fn insert_test_trove(
        conn: &rusqlite::Connection,
        changeset_id: i64,
        name: &str,
        version: &str,
        files: &[(&str, &str, i64)],
    ) {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            InstallSource::File,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(conn).unwrap();

        for (path, hash, size) in files {
            let mut file = FileEntry::new(
                (*path).to_string(),
                (*hash).to_string(),
                *size,
                0o100644,
                trove_id,
            );
            file.insert(conn).unwrap();
        }
    }

    #[tokio::test]
    async fn rollback_update_without_active_generation_mutates_live_root_directly() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_string_lossy().to_string();
        conary_core::db::init(&db_path_str).unwrap();
        let conn = conary_core::db::open(&db_path_str).unwrap();

        let root = temp_dir.path().join("root");
        std::fs::create_dir_all(root.join("usr/share/conary-test")).unwrap();
        let hello_path = root.join("usr/share/conary-test/hello.txt");
        let added_path = root.join("usr/share/conary-test/added.txt");
        std::fs::write(&hello_path, b"hello from v2\n").unwrap();
        std::fs::write(&added_path, b"added in v2\n").unwrap();

        let v1_hash = store_test_object(&conn, &db_path, b"hello from v1\n");
        let v2_hash = store_test_object(&conn, &db_path, b"hello from v2\n");
        let added_hash = store_test_object(&conn, &db_path, b"added in v2\n");

        let old_snapshot = TroveSnapshot {
            name: "conary-test-fixture".to_string(),
            version: "1.0.0".to_string(),
            architecture: Some("x86_64".to_string()),
            description: None,
            install_source: InstallSource::File.as_str().to_string(),
            installed_from_repository_id: None,
            files: vec![FileSnapshot {
                path: "/usr/share/conary-test/hello.txt".to_string(),
                sha256_hash: v1_hash,
                size: "hello from v1\n".len() as i64,
                permissions: 0o100644,
                symlink_target: None,
            }],
        };

        let mut update_changeset =
            Changeset::new("CCS upgrade conary-test-fixture 1.0.0 -> 2.0.0".to_string());
        let update_changeset_id = update_changeset.insert(&conn).unwrap();
        conn.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(&old_snapshot).unwrap(),
                update_changeset_id
            ],
        )
        .unwrap();
        update_changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        insert_test_trove(
            &conn,
            update_changeset_id,
            "conary-test-fixture",
            "2.0.0",
            &[
                (
                    "/usr/share/conary-test/hello.txt",
                    &v2_hash,
                    "hello from v2\n".len() as i64,
                ),
                (
                    "/usr/share/conary-test/added.txt",
                    &added_hash,
                    "added in v2\n".len() as i64,
                ),
            ],
        );
        drop(conn);

        cmd_rollback(
            update_changeset_id,
            &db_path_str,
            root.to_string_lossy().as_ref(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&hello_path).unwrap(),
            "hello from v1\n"
        );
        assert!(!added_path.exists());

        let conn = conary_core::db::open(&db_path_str).unwrap();
        let troves = Trove::find_by_name(&conn, "conary-test-fixture").unwrap();
        assert_eq!(troves.len(), 1);
        assert_eq!(troves[0].version, "1.0.0");
        let status: String = conn
            .query_row(
                "SELECT status FROM changesets WHERE id = ?1",
                [update_changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "rolled_back");
    }

    #[test]
    fn direct_live_root_restore_recreates_regular_files_and_symlinks() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_string_lossy().to_string();
        conary_core::db::init(&db_path_str).unwrap();
        let conn = conary_core::db::open(&db_path_str).unwrap();
        let file_hash = store_test_object(&conn, &db_path, b"restored\n");
        let link_hash = {
            let cas = CasStore::new(objects_dir(&db_path_str)).unwrap();
            cas.store_symlink("tool").unwrap()
        };
        conn.execute(
            "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
             VALUES (?1, ?2, ?3)",
            params![
                &link_hash,
                format!("objects/{}/{}", &link_hash[0..2], &link_hash[2..]),
                "tool".len() as i64
            ],
        )
        .unwrap();

        let root = temp_dir.path().join("root");
        let snapshot = TroveSnapshot {
            name: "fixture".to_string(),
            version: "1.0.0".to_string(),
            architecture: None,
            description: None,
            install_source: InstallSource::File.as_str().to_string(),
            installed_from_repository_id: None,
            files: vec![
                FileSnapshot {
                    path: "/usr/bin/tool".to_string(),
                    sha256_hash: file_hash,
                    size: "restored\n".len() as i64,
                    permissions: 0o100755,
                    symlink_target: None,
                },
                FileSnapshot {
                    path: "/usr/bin/tool-link".to_string(),
                    sha256_hash: link_hash,
                    size: "tool".len() as i64,
                    permissions: 0o120777,
                    symlink_target: Some("tool".to_string()),
                },
            ],
        };

        let stats = restore_snapshots_to_live_root(&root, &db_path_str, &[snapshot]).unwrap();

        assert_eq!(stats.files_restored, 2);
        assert_eq!(
            std::fs::read_to_string(root.join("usr/bin/tool")).unwrap(),
            "restored\n"
        );
        assert_eq!(
            std::fs::read_link(root.join("usr/bin/tool-link")).unwrap(),
            Path::new("tool")
        );
    }
}
