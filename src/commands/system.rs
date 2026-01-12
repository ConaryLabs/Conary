// src/commands/system.rs
//! System management commands (init, verify, rollback)

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::info;

/// Initialize the Conary database and add default repositories
pub fn cmd_init(db_path: &str) -> Result<()> {
    info!("Initializing Conary database at: {}", db_path);
    conary::db::init(db_path)?;
    println!("Database initialized successfully at: {}", db_path);

    let conn = conary::db::open(db_path)?;
    info!("Adding default repositories...");

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
        match conary::repository::add_repository(
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

    let mut conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let changeset = conary::db::models::Changeset::find_by_id(&conn, changeset_id)?
        .ok_or_else(|| anyhow::anyhow!("Changeset {} not found", changeset_id))?;

    if changeset.status == conary::db::models::ChangesetStatus::RolledBack {
        return Err(anyhow::anyhow!(
            "Changeset {} is already rolled back",
            changeset_id
        ));
    }
    if changeset.status == conary::db::models::ChangesetStatus::Pending {
        return Err(anyhow::anyhow!(
            "Cannot rollback pending changeset {}",
            changeset_id
        ));
    }

    let files_to_rollback: Vec<(String, String)> = {
        let mut stmt =
            conn.prepare("SELECT path, action FROM file_history WHERE changeset_id = ?1")?;
        let rows = stmt.query_map([changeset_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    conary::db::transaction(&mut conn, |tx| {
        let troves = {
            let mut stmt = tx.prepare(
                "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
                 FROM troves WHERE installed_by_changeset_id = ?1",
            )?;
            let rows = stmt.query_map([changeset_id], |row| {
                Ok(conary::db::models::Trove {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    version: row.get(2)?,
                    trove_type: row.get::<_, String>(3)?
                        .parse()
                        .unwrap_or(conary::db::models::TroveType::Package),
                    architecture: row.get(4)?,
                    description: row.get(5)?,
                    installed_at: row.get(6)?,
                    installed_by_changeset_id: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if troves.is_empty() {
            return Err(conary::Error::InitError(
                "No troves found for this changeset. Cannot rollback Remove operations yet."
                    .to_string(),
            ));
        }

        let mut rollback_changeset = conary::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})",
            changeset_id, changeset.description
        ));
        let rollback_changeset_id = rollback_changeset.insert(tx)?;

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                conary::db::models::Trove::delete(tx, trove_id)?;
                println!("Removed {} version {}", trove.name, trove.version);
            }
        }

        rollback_changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(troves.len())
    })?;

    info!("Removing files from filesystem...");
    for (path, action) in &files_to_rollback {
        if action == "add" || action == "modify" {
            deployer.remove_file(path)?;
            info!("Removed file: {}", path);
        }
    }

    println!(
        "Rollback complete. Changeset {} has been reversed.",
        changeset_id
    );
    println!("  Removed {} files from filesystem", files_to_rollback.len());

    Ok(())
}

/// Verify installed files
pub fn cmd_verify(package: Option<String>, db_path: &str, root: &str) -> Result<()> {
    info!("Verifying installed files...");

    let conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let files: Vec<(String, String, String)> = if let Some(pkg_name) = package {
        let troves = conary::db::models::Trove::find_by_name(&conn, &pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }

        let mut all_files = Vec::new();
        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let trove_files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
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
