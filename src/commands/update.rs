// src/commands/update.rs
//! Update and delta statistics commands

use super::install_package_from_file;
use anyhow::Result;
use conary::db::models::{DeltaStats, PackageDelta};
use conary::delta::DeltaApplier;
use conary::repository;
use std::path::Path;
use tracing::{info, warn};

/// Check for and apply package updates
pub fn cmd_update(package: Option<String>, db_path: &str, root: &str) -> Result<()> {
    info!("Checking for package updates");

    let mut conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let temp_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("tmp");
    std::fs::create_dir_all(&temp_dir)?;

    let installed_troves = if let Some(pkg_name) = package {
        conary::db::models::Trove::find_by_name(&conn, &pkg_name)?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
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

    if installed_troves.is_empty() {
        println!("No packages to update");
        return Ok(());
    }

    let mut updates_available = Vec::new();
    for trove in &installed_troves {
        let repo_packages = conary::db::models::RepositoryPackage::find_by_name(&conn, &trove.name)?;
        for repo_pkg in repo_packages {
            if repo_pkg.version != trove.version
                && (repo_pkg.architecture == trove.architecture || repo_pkg.architecture.is_none())
            {
                info!(
                    "Update available: {} {} -> {}",
                    trove.name, trove.version, repo_pkg.version
                );
                updates_available.push((trove.clone(), repo_pkg));
                break;
            }
        }
    }

    if updates_available.is_empty() {
        println!("All packages are up to date");
        return Ok(());
    }

    println!(
        "Found {} package(s) with updates available:",
        updates_available.len()
    );
    for (trove, repo_pkg) in &updates_available {
        println!("  {} {} -> {}", trove.name, trove.version, repo_pkg.version);
    }

    let mut total_bytes_saved = 0i64;
    let mut deltas_applied = 0i32;
    let mut full_downloads = 0i32;
    let mut delta_failures = 0i32;

    let changeset_id = conary::db::transaction(&mut conn, |tx| {
        let mut changeset = conary::db::models::Changeset::new(format!(
            "Update {} package(s)",
            updates_available.len()
        ));
        changeset.insert(tx)
    })?;

    for (installed_trove, repo_pkg) in updates_available {
        println!("\nUpdating {} ...", installed_trove.name);

        let mut delta_success = false;

        if let Ok(Some(delta_info)) = PackageDelta::find_delta(
            &conn,
            &installed_trove.name,
            &installed_trove.version,
            &repo_pkg.version,
        ) {
            println!(
                "  Delta available: {} bytes ({:.1}% of full size)",
                delta_info.delta_size,
                delta_info.compression_ratio * 100.0
            );

            let delta_path = temp_dir.join(format!(
                "{}-{}-to-{}.delta",
                installed_trove.name, installed_trove.version, repo_pkg.version
            ));

            match repository::download_delta(
                &repository::DeltaInfo {
                    from_version: delta_info.from_version,
                    from_hash: delta_info.from_hash.clone(),
                    delta_url: delta_info.delta_url,
                    delta_size: delta_info.delta_size,
                    delta_checksum: delta_info.delta_checksum,
                    compression_ratio: delta_info.compression_ratio,
                },
                &installed_trove.name,
                &repo_pkg.version,
                &temp_dir,
            ) {
                Ok(_) => {
                    let applier = DeltaApplier::new(&objects_dir)?;
                    match applier.apply_delta(&delta_info.from_hash, &delta_path, &delta_info.to_hash)
                    {
                        Ok(_) => {
                            println!("  [OK] Delta applied successfully");
                            delta_success = true;
                            deltas_applied += 1;
                            total_bytes_saved += repo_pkg.size - delta_info.delta_size;
                        }
                        Err(e) => {
                            warn!("  Delta application failed: {}", e);
                            delta_failures += 1;
                        }
                    }
                    let _ = std::fs::remove_file(delta_path);
                }
                Err(e) => {
                    warn!("  Delta download failed: {}", e);
                    delta_failures += 1;
                }
            }
        }

        if !delta_success {
            println!("  Downloading full package...");
            match repository::download_package(&repo_pkg, &temp_dir) {
                Ok(pkg_path) => {
                    println!("  [OK] Downloaded {} bytes", repo_pkg.size);
                    full_downloads += 1;

                    if let Err(e) =
                        install_package_from_file(&pkg_path, &mut conn, root, Some(&installed_trove))
                    {
                        warn!("  Package installation failed: {}", e);
                        let _ = std::fs::remove_file(pkg_path);
                        continue;
                    }

                    println!("  [OK] Package installed successfully");
                    let _ = std::fs::remove_file(pkg_path);
                }
                Err(e) => {
                    warn!("  Full download failed: {}", e);
                    continue;
                }
            }
        }
    }

    conary::db::transaction(&mut conn, |tx| {
        let mut stats = DeltaStats::new(changeset_id);
        stats.total_bytes_saved = total_bytes_saved;
        stats.deltas_applied = deltas_applied;
        stats.full_downloads = full_downloads;
        stats.delta_failures = delta_failures;
        stats.insert(tx)?;

        let mut changeset = conary::db::models::Changeset::find_by_id(tx, changeset_id)?
            .ok_or_else(|| conary::Error::NotFoundError("Changeset not found".to_string()))?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

        Ok(())
    })?;

    println!("\n=== Update Summary ===");
    println!("Delta updates: {}", deltas_applied);
    println!("Full downloads: {}", full_downloads);
    println!("Delta failures: {}", delta_failures);
    if total_bytes_saved > 0 {
        let saved_mb = total_bytes_saved as f64 / 1_048_576.0;
        println!("Bandwidth saved: {:.2} MB", saved_mb);
    }

    Ok(())
}

/// Show delta update statistics
pub fn cmd_delta_stats(db_path: &str) -> Result<()> {
    info!("Showing delta update statistics");

    let conn = conary::db::open(db_path)?;
    let total_stats = DeltaStats::get_total_stats(&conn)?;

    let all_stats = {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DeltaStats {
                id: Some(row.get(0)?),
                changeset_id: row.get(1)?,
                total_bytes_saved: row.get(2)?,
                deltas_applied: row.get(3)?,
                full_downloads: row.get(4)?,
                delta_failures: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if all_stats.is_empty() {
        println!("No delta statistics available");
        println!("Run 'conary update' to start tracking delta usage");
        return Ok(());
    }

    println!("=== Delta Update Statistics ===\n");
    println!("Total Statistics:");
    println!("  Delta updates applied: {}", total_stats.deltas_applied);
    println!("  Full downloads: {}", total_stats.full_downloads);
    println!("  Delta failures: {}", total_stats.delta_failures);

    let total_mb = total_stats.total_bytes_saved as f64 / 1_048_576.0;
    println!("  Total bandwidth saved: {:.2} MB", total_mb);

    let total_updates = total_stats.deltas_applied + total_stats.full_downloads;
    if total_updates > 0 {
        let success_rate = (total_stats.deltas_applied as f64 / total_updates as f64) * 100.0;
        println!("  Delta success rate: {:.1}%", success_rate);
    }

    println!("\nRecent Operations:");
    for (idx, stats) in all_stats.iter().take(10).enumerate() {
        if idx > 0 {
            println!();
        }

        let timestamp = stats.created_at.as_deref().unwrap_or("unknown");
        println!("  [Changeset {}] {}", stats.changeset_id, timestamp);
        println!("    Deltas applied: {}", stats.deltas_applied);
        println!("    Full downloads: {}", stats.full_downloads);

        if stats.delta_failures > 0 {
            println!("    Delta failures: {}", stats.delta_failures);
        }

        if stats.total_bytes_saved > 0 {
            let saved_mb = stats.total_bytes_saved as f64 / 1_048_576.0;
            println!("    Bandwidth saved: {:.2} MB", saved_mb);
        }
    }

    if all_stats.len() > 10 {
        println!("\n... and {} more operations", all_stats.len() - 10);
    }

    Ok(())
}
