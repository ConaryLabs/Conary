// src/commands/system.rs
//! System management commands (init, verify, rollback)

use super::TroveSnapshot;
use anyhow::Result;
use conary_core::db::paths::objects_dir;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Initialize the Conary database and add default repositories
pub fn cmd_init(db_path: &str) -> Result<()> {
    info!("Initializing Conary database at: {}", db_path);
    conary_core::db::init(db_path)?;
    println!("Database initialized successfully at: {}", db_path);

    let conn = conary_core::db::open(db_path)?;
    info!("Adding default repositories...");

    let default_repos = [
        (
            "remi",
            "https://packages.conary.io",
            110,
            "Conary Remi (CCS)",
        ),
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
            &conn,
            name.to_string(),
            url.to_string(),
            true,
            priority,
        ) {
            Ok(_) => println!("  Added: {} ({})", name, desc),
            Err(e) => eprintln!("  Warning: Could not add {}: {}", name, e),
        }
    }

    println!("\nDefault repositories added. Use 'conary repo-sync' to download metadata.");
    Ok(())
}

/// Rollback a changeset
pub fn cmd_rollback(changeset_id: i64, db_path: &str, root: &str) -> Result<()> {
    info!("Rolling back changeset: {}", changeset_id);

    let mut conn = conary_core::db::open(db_path)?;

    let objects_dir = objects_dir(db_path);
    let install_root = PathBuf::from(root);
    let deployer = conary_core::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let changeset = conary_core::db::models::Changeset::find_by_id(&conn, changeset_id)?
        .ok_or_else(|| anyhow::anyhow!("Changeset {} not found", changeset_id))?;

    // TODO: Move these status/reversal checks inside the transaction to eliminate the
    // TOCTOU gap between these reads and the actual rollback transaction below.
    if changeset.status == conary_core::db::models::ChangesetStatus::RolledBack {
        return Err(anyhow::anyhow!(
            "Changeset {} is already rolled back",
            changeset_id
        ));
    }
    if changeset.status == conary_core::db::models::ChangesetStatus::Pending {
        return Err(anyhow::anyhow!(
            "Cannot rollback pending changeset {}",
            changeset_id
        ));
    }

    // Prevent double rollback: check if another changeset already reversed this one
    let already_reversed: Option<i64> = conn.query_row(
        "SELECT reversed_by_changeset_id FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;
    if let Some(reverse_id) = already_reversed {
        return Err(anyhow::anyhow!(
            "Changeset {} has already been reversed by changeset {}",
            changeset_id,
            reverse_id
        ));
    }

    // Check if this changeset has metadata (trove snapshot for rollback)
    let metadata: Option<String> = conn.query_row(
        "SELECT metadata FROM changesets WHERE id = ?1",
        [changeset_id],
        |row| row.get(0),
    )?;

    if let Some(ref json) = metadata {
        // Check if this changeset also has installed troves (= upgrade vs removal)
        let has_troves: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM troves WHERE installed_by_changeset_id = ?1)",
            [changeset_id],
            |row| row.get(0),
        )?;

        if has_troves {
            // Upgrade: remove new version, restore old version from snapshot
            return rollback_upgrade(changeset_id, json, &mut conn, &deployer, &changeset);
        }
        // Removal: restore the package from snapshot
        return rollback_removal(changeset_id, json, &mut conn, &deployer, &changeset);
    }

    // Otherwise, this is a fresh install - remove the installed packages
    let files_to_rollback = std::cell::RefCell::new(Vec::new());

    conary_core::db::transaction(&mut conn, |tx| {
        // Read file history inside the transaction to ensure consistency (TOCTOU)
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
                "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, selection_reason, label_id, orphan_since
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
                println!("Removed {} version {}", trove.name, trove.version);
            }
        }

        rollback_changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;

        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(troves.len())
    })?;

    let files_to_rollback = files_to_rollback.into_inner();

    info!("Removing files from filesystem...");
    let mut removal_errors = Vec::new();
    for (path, action) in &files_to_rollback {
        if action == "add" || action == "modify" {
            match deployer.remove_file(path) {
                Ok(()) => info!("Removed file: {}", path),
                Err(e) => {
                    warn!("Failed to remove file during rollback: {}: {}", path, e);
                    removal_errors.push(format!("{}: {}", path, e));
                }
            }
        }
    }
    if !removal_errors.is_empty() {
        warn!(
            "{} file(s) could not be removed during rollback",
            removal_errors.len()
        );
    }

    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!(
        "  Removed {} files from filesystem",
        files_to_rollback.len()
    );

    Ok(())
}

/// Rollback a removal by restoring the package from snapshot
fn rollback_removal(
    changeset_id: i64,
    snapshot_json: &str,
    conn: &mut rusqlite::Connection,
    deployer: &conary_core::filesystem::FileDeployer,
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

    // Deploy files from CAS to filesystem
    info!("Restoring files to filesystem...");
    let mut restored_count = 0;
    for file in &snapshot.files {
        match deployer.deploy_file(&file.path, &file.sha256_hash, file.permissions as u32) {
            Ok(()) => {
                restored_count += 1;
                info!("Restored file: {}", file.path);
            }
            Err(e) => {
                // Log but continue - file might already exist or CAS might not have it
                info!("Could not restore file {}: {}", file.path, e);
            }
        }
    }

    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!("  Restored {} version {}", snapshot.name, snapshot.version);
    println!("  Files restored: {}/{}", restored_count, file_count);

    Ok(())
}

/// Rollback an upgrade by removing the new version and restoring the old
fn rollback_upgrade(
    changeset_id: i64,
    snapshot_json: &str,
    conn: &mut rusqlite::Connection,
    deployer: &conary_core::filesystem::FileDeployer,
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

    conary_core::db::transaction(conn, |tx| {
        // Find and remove the new trove installed by this changeset
        let new_troves: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, version FROM troves WHERE installed_by_changeset_id = ?1",
            )?;
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
            println!("  Removed new version {}", version);
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

    // Remove new version's files that aren't in the old version
    let old_paths: std::collections::HashSet<&str> =
        snapshot.files.iter().map(|f| f.path.as_str()).collect();
    let files_to_remove = files_to_remove.into_inner();
    for path in &files_to_remove {
        if !old_paths.contains(path.as_str()) {
            match deployer.remove_file(path) {
                Ok(()) => info!("Removed new file: {}", path),
                Err(e) => warn!("Failed to remove file during rollback: {}: {}", path, e),
            }
        }
    }

    // Restore old version's files from CAS
    info!("Restoring old version files from CAS...");
    let mut restored_count = 0;
    for file in &snapshot.files {
        match deployer.deploy_file(&file.path, &file.sha256_hash, file.permissions as u32) {
            Ok(()) => {
                restored_count += 1;
                info!("Restored file: {}", file.path);
            }
            Err(e) => {
                info!("Could not restore file {}: {}", file.path, e);
            }
        }
    }

    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!(
        "  Restored {} version {} ({}/{} files)",
        snapshot.name,
        snapshot.version,
        restored_count,
        snapshot.files.len()
    );

    Ok(())
}

/// Verify installed files
pub fn cmd_verify(package: Option<String>, db_path: &str, root: &str, use_rpm: bool) -> Result<()> {
    info!("Verifying installed files...");

    let conn = conary_core::db::open(db_path)?;

    // If --rpm flag, verify adopted packages against RPM database
    if use_rpm {
        return verify_against_rpm(&conn, package);
    }

    let objects_dir = objects_dir(db_path);
    let install_root = PathBuf::from(root);
    let deployer = conary_core::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let files: Vec<(String, String, String)> = if let Some(pkg_name) = package {
        let troves = conary_core::db::models::Trove::find_by_name(&conn, &pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }

        let mut all_files = Vec::new();
        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let trove_files = conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
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
    let mut modified_count = 0;
    let mut missing_count = 0;

    for (path, expected_hash, pkg_name) in &files {
        match deployer.verify_file(path, expected_hash) {
            Ok(true) => {
                ok_count += 1;
                info!("OK: {} (from {})", path, pkg_name);
            }
            Ok(false) => {
                modified_count += 1;
                println!("MODIFIED: {} (from {})", path, pkg_name);
            }
            Err(_) => {
                missing_count += 1;
                println!("MISSING: {} (from {})", path, pkg_name);
            }
        }
    }

    println!("\nVerification summary:");
    println!("  OK: {} files", ok_count);
    println!("  Modified: {} files", modified_count);
    println!("  Missing: {} files", missing_count);
    println!("  Total: {} files", files.len());

    if modified_count > 0 || missing_count > 0 {
        return Err(anyhow::anyhow!("Verification failed"));
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
pub fn cmd_gc(db_path: &str, objects_dir: &str, keep_days: u32, dry_run: bool) -> Result<()> {
    use std::collections::HashSet;
    use std::fs;

    info!(
        "Starting CAS garbage collection (keep_days={}, dry_run={})",
        keep_days, dry_run
    );

    let conn = conary_core::db::open(db_path)?;
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

    for prefix_entry in fs::read_dir(objects_path)? {
        let prefix_entry = prefix_entry?;
        let prefix_path = prefix_entry.path();

        if !prefix_path.is_dir() {
            continue;
        }

        let prefix = prefix_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Skip if not a 2-char hex prefix
        if prefix.len() != 2 || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        for object_entry in fs::read_dir(&prefix_path)? {
            let object_entry = object_entry?;
            let object_path = object_entry.path();

            if object_path.is_file() {
                let suffix = object_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");

                // Skip temp files
                if suffix.ends_with(".tmp") {
                    continue;
                }

                let hash = format!("{}{}", prefix, suffix);
                let metadata = fs::metadata(&object_path)?;
                total_cas_size += metadata.len();
                cas_objects.push((object_path, hash));
            }
        }
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

    Ok(())
}

use super::format_bytes;
