// src/commands/system.rs
//! System management commands (init, verify, rollback)

use super::TroveSnapshot;
use super::open_db;
use anyhow::Result;
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

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
            "https://packages.conary.io".to_string(),
        );
        remi_repo.priority = 110;
        remi_repo.default_strategy = Some("remi".to_string());
        remi_repo.default_strategy_endpoint = Some("https://packages.conary.io".to_string());
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
                "fedora-43",
                "https://dl.fedoraproject.org/pub/fedora/linux/releases/43/Everything/x86_64/os",
                90,
                "Fedora 43",
            ),
            (
                "arch-multilib",
                "https://geo.mirror.pkgbuild.com/multilib/os/x86_64",
                85,
                "Arch Linux",
            ),
            (
                "ubuntu-noble",
                "http://archive.ubuntu.com/ubuntu",
                80,
                "Ubuntu 24.04 LTS",
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
pub async fn cmd_rollback(changeset_id: i64, db_path: &str, _root: &str) -> Result<()> {
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

        // Atomically claim this changeset for rollback using a conditional UPDATE.
        // We set reversed_by_changeset_id to a sentinel (-1) as a claim marker;
        // the actual rollback transaction will overwrite it with the real rollback
        // changeset ID. This keeps status within the valid enum
        // (pending/applied/post_hooks_failed/rolled_back) while preventing a
        // second concurrent rollback from passing
        // the reversed_by_changeset_id IS NULL guard.
        let rollback_statuses = rollback_claim_statuses();
        let claimed = tx.execute(
            "UPDATE changesets SET reversed_by_changeset_id = -1
             WHERE id = ?1 AND status IN (?2, ?3) AND reversed_by_changeset_id IS NULL",
            rusqlite::params![changeset_id, rollback_statuses[0], rollback_statuses[1]],
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

    // Helper: clear the -1 claim sentinel if the rollback fails, so the
    // changeset doesn't get permanently wedged.
    let clear_claim = |conn: &rusqlite::Connection| {
        let _ = conn.execute(
            "UPDATE changesets SET reversed_by_changeset_id = NULL
             WHERE id = ?1 AND reversed_by_changeset_id = -1",
            [changeset_id],
        );
    };

    if let Some(ref json) = metadata {
        // Check if this changeset also has installed troves (= upgrade vs removal)
        let has_troves: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM troves WHERE installed_by_changeset_id = ?1)",
            [changeset_id],
            |row| row.get(0),
        )?;

        if has_troves {
            // Upgrade: remove new version, restore old version from snapshot
            return rollback_upgrade(changeset_id, json, &mut conn, &changeset)
                .inspect_err(|_| clear_claim(&conn));
        }
        // Removal: restore the package from snapshot
        return rollback_removal(changeset_id, json, &mut conn, &changeset)
            .inspect_err(|_| clear_claim(&conn));
    }

    // Otherwise, this is a fresh install - remove the installed packages
    let files_to_rollback = std::cell::RefCell::new(Vec::new());
    let removed_messages = std::cell::RefCell::new(Vec::new());

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
        let rollback_changeset_id = rollback_changeset.insert(tx)?;

        for trove in &troves {
            if let Some(trove_id) = trove.id {
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
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(troves.len())
    })
    .inspect_err(|_| clear_claim(&conn))?;

    let files_to_rollback = files_to_rollback.into_inner();
    let removed_messages = removed_messages.into_inner();

    // Composefs-native: rebuild EROFS image from updated DB state and remount
    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        &conn,
        &format!("Rollback changeset {}", changeset_id),
        None,
        std::path::Path::new("/conary"),
    )?;

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

/// Rollback a removal by restoring the package from snapshot
fn rollback_removal(
    changeset_id: i64,
    snapshot_json: &str,
    conn: &mut rusqlite::Connection,
    changeset: &conary_core::db::models::Changeset,
) -> Result<()> {
    info!("Rolling back removal changeset: {}", changeset_id);

    let snapshot: TroveSnapshot = serde_json::from_str(snapshot_json)?;
    println!(
        "Restoring package: {} version {}",
        snapshot.name, snapshot.version
    );

    let file_count = snapshot.files.len();

    conary_core::db::transaction(conn, |tx| {
        // Create rollback changeset
        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_changeset_id = rollback_changeset.insert(tx)?;

        // Restore the trove
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
        // Preserve repo provenance if the repo still exists; null out if deleted.
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

        // Restore file entries
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

            // Record in file history only for valid SHA256 hashes
            if file.sha256_hash.len() == 64
                && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
            {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                    [&rollback_changeset_id.to_string(), &file.path, &file.sha256_hash, "add"],
                )?;
            }
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;

        // Mark original changeset as rolled back
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(())
    })?;

    // Composefs-native: rebuild EROFS image from updated DB state and remount
    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        conn,
        &format!("Rollback removal of {}", snapshot.name),
        None,
        std::path::Path::new("/conary"),
    )?;

    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!("  Restored {} version {}", snapshot.name, snapshot.version);
    println!("  Files in DB: {}", file_count);

    Ok(())
}

/// Rollback an upgrade by removing the new version and restoring the old
fn rollback_upgrade(
    changeset_id: i64,
    snapshot_json: &str,
    conn: &mut rusqlite::Connection,
    changeset: &conary_core::db::models::Changeset,
) -> Result<()> {
    info!("Rolling back upgrade changeset: {}", changeset_id);

    let snapshot: TroveSnapshot = serde_json::from_str(snapshot_json)?;
    println!(
        "Rolling back upgrade: restoring {} version {}",
        snapshot.name, snapshot.version
    );

    // Collect new version's files for filesystem cleanup
    let files_to_remove = std::cell::RefCell::new(Vec::new());
    let removed_messages = std::cell::RefCell::new(Vec::new());

    conary_core::db::transaction(conn, |tx| {
        // Find and remove the new trove installed by this changeset
        let new_troves: Vec<(i64, String)> = {
            let mut stmt =
                tx.prepare("SELECT id, version FROM troves WHERE installed_by_changeset_id = ?1")?;
            stmt.query_map([changeset_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (trove_id, version) in &new_troves {
            // Collect files for filesystem removal
            let files = conary_core::db::models::FileEntry::find_by_trove(tx, *trove_id)?;
            files_to_remove
                .borrow_mut()
                .extend(files.iter().map(|f| f.path.clone()));

            // Delete from DB
            conary_core::db::models::Trove::delete(tx, *trove_id)?;
            removed_messages
                .borrow_mut()
                .push(format!("  Removed new version {}", version));
        }

        // Create rollback changeset
        let mut rollback_changeset = conary_core::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_changeset_id = rollback_changeset.insert(tx)?;

        // Restore the old trove from snapshot
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
        // Preserve repo provenance if the repo still exists; null out if deleted.
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

        // Restore file entries
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
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;

        // Mark original changeset as rolled back
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(())
    })?;

    // Composefs-native: rebuild EROFS image from DB state and remount
    let _files_to_remove = files_to_remove.into_inner();
    let removed_messages = removed_messages.into_inner();
    let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
        conn,
        &format!("Rollback upgrade of {}", snapshot.name),
        None,
        std::path::Path::new("/conary"),
    )?;

    for message in &removed_messages {
        println!("{message}");
    }
    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!(
        "  Restored {} version {} ({} files in DB)",
        snapshot.name,
        snapshot.version,
        snapshot.files.len()
    );

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
/// is in conary-server/src/server/chunk_gc.rs.
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

#[cfg(feature = "server")]
/// Generate sparse package indices from the CAS chunk store.
pub fn cmd_index_gen(
    db_path: String,
    chunk_dir: String,
    output_dir: String,
    distro: Option<String>,
    sign_key: Option<String>,
) -> anyhow::Result<()> {
    use conary_server::server::{IndexGenConfig, generate_indices};

    let config = IndexGenConfig {
        db_path,
        chunk_dir,
        output_dir,
        distro,
        sign_key,
    };

    let results = generate_indices(&config)?;
    if results.is_empty() {
        println!("No indices generated.");
    } else {
        for result in results {
            println!(
                "{}: {} packages ({} versions) -> {}{}",
                result.distro,
                result.package_count,
                result.version_count,
                result.index_path,
                if result.signed { " [signed]" } else { "" }
            );
        }
    }
    Ok(())
}

#[cfg(feature = "server")]
/// Pre-warm the CAS chunk cache by eagerly converting popular packages.
#[allow(clippy::too_many_arguments)]
pub fn cmd_prewarm(
    db_path: String,
    chunk_dir: String,
    cache_dir: String,
    distro: String,
    max_packages: usize,
    popularity_file: Option<String>,
    pattern: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    use conary_server::server::{PrewarmConfig, run_prewarm};

    let config = PrewarmConfig {
        db_path,
        chunk_dir,
        cache_dir,
        distro,
        max_packages,
        popularity_file,
        pattern,
        dry_run,
    };

    let result = run_prewarm(&config)?;
    println!("Pre-warm complete:");
    println!("  Processed:  {}", result.packages_processed);
    println!("  Converted:  {}", result.packages_converted);
    println!("  Skipped:    {}", result.packages_skipped);
    println!("  Failed:     {}", result.packages_failed);
    println!("  Total size: {} bytes", result.total_bytes);

    if !result.converted.is_empty() {
        println!("\nConverted packages:");
        for pkg in &result.converted {
            println!("  {}", pkg);
        }
    }

    if !result.failed.is_empty() {
        println!("\nFailed packages:");
        for (pkg, err) in &result.failed {
            println!("  {}: {}", pkg, err);
        }
    }

    Ok(())
}

use super::format_bytes;

#[cfg(test)]
mod tests {
    use super::{cmd_init, rollback_claim_statuses};

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
        assert_eq!(repo.url, "https://packages.conary.io");
        assert_eq!(repo.default_strategy.as_deref(), Some("remi"));
        assert_eq!(
            repo.default_strategy_endpoint.as_deref(),
            Some("https://packages.conary.io")
        );
        assert_eq!(repo.default_strategy_distro.as_deref(), Some("fedora"));
    }

    #[test]
    fn rollback_claim_statuses_include_post_hooks_failed() {
        assert_eq!(rollback_claim_statuses(), ["applied", "post_hooks_failed"]);
    }
}
