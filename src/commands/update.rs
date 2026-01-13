// src/commands/update.rs
//! Update and delta statistics commands

use super::install_package_from_file;
use super::progress::{UpdatePhase, UpdateProgress};
use anyhow::Result;
use conary::db::models::{DeltaStats, PackageDelta, Repository, RepositoryPackage, Trove};
use conary::delta::DeltaApplier;
use conary::repository::{self, DownloadOptions};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Get the keyring directory based on db_path
fn get_keyring_dir(db_path: &str) -> PathBuf {
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| {
        Path::new(db_path)
            .parent()
            .unwrap_or(Path::new("/var/lib/conary"))
            .to_string_lossy()
            .to_string()
    });
    PathBuf::from(db_dir).join("keys")
}

/// Result of a download attempt for an update
#[derive(Debug)]
enum DownloadResult {
    /// Full package was downloaded
    Full { trove: Trove, pkg_path: PathBuf },
    /// Download failed completely
    Failed { name: String, error: String },
}

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

    let keyring_dir = get_keyring_dir(db_path);

    let installed_troves = if let Some(pkg_name) = package {
        Trove::find_by_name(&conn, &pkg_name)?
    } else {
        Trove::list_all(&conn)?
    };

    if installed_troves.is_empty() {
        println!("No packages to update");
        return Ok(());
    }

    // Collect updates with their repository info (needed for GPG verification)
    let mut updates_available: Vec<(Trove, RepositoryPackage, Repository)> = Vec::new();
    for trove in &installed_troves {
        let repo_packages = RepositoryPackage::find_by_name(&conn, &trove.name)?;
        for repo_pkg in repo_packages {
            if repo_pkg.version != trove.version
                && (repo_pkg.architecture == trove.architecture || repo_pkg.architecture.is_none())
            {
                // Get the repository for GPG verification
                if let Ok(Some(repo)) = Repository::find_by_id(&conn, repo_pkg.repository_id) {
                    info!(
                        "Update available: {} {} -> {}",
                        trove.name, trove.version, repo_pkg.version
                    );
                    updates_available.push((trove.clone(), repo_pkg, repo));
                    break;
                }
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
    for (trove, repo_pkg, _) in &updates_available {
        println!("  {} {} -> {}", trove.name, trove.version, repo_pkg.version);
    }

    // Phase 1: Check for deltas and categorize updates
    let mut delta_updates: Vec<(Trove, RepositoryPackage, PackageDelta)> = Vec::new();
    let mut full_updates: Vec<(Trove, RepositoryPackage, Repository)> = Vec::new();

    for (trove, repo_pkg, repo) in updates_available {
        if let Ok(Some(delta_info)) = PackageDelta::find_delta(
            &conn,
            &trove.name,
            &trove.version,
            &repo_pkg.version,
        ) {
            println!(
                "  {} has delta: {} bytes ({:.1}% of full)",
                trove.name,
                delta_info.delta_size,
                delta_info.compression_ratio * 100.0
            );
            delta_updates.push((trove, repo_pkg, delta_info));
        } else {
            full_updates.push((trove, repo_pkg, repo));
        }
    }

    let mut total_bytes_saved = 0i64;
    let mut deltas_applied = 0i32;
    let mut full_downloads = 0i32;
    let mut delta_failures = 0i32;

    // Save counts before consuming the vectors
    let delta_count = delta_updates.len();
    let initial_full_count = full_updates.len();

    let changeset_id = conary::db::transaction(&mut conn, |tx| {
        let mut changeset = conary::db::models::Changeset::new(format!(
            "Update {} package(s)",
            delta_count + initial_full_count
        ));
        changeset.insert(tx)
    })?;

    // Phase 2: Download and apply deltas (sequential - requires CAS access)
    for (trove, repo_pkg, delta_info) in delta_updates {
        println!("\nUpdating {} (delta)...", trove.name);

        let delta_path = temp_dir.join(format!(
            "{}-{}-to-{}.delta",
            trove.name, trove.version, repo_pkg.version
        ));

        match repository::download_delta(
            &repository::DeltaInfo {
                from_version: delta_info.from_version.clone(),
                from_hash: delta_info.from_hash.clone(),
                delta_url: delta_info.delta_url.clone(),
                delta_size: delta_info.delta_size,
                delta_checksum: delta_info.delta_checksum.clone(),
                compression_ratio: delta_info.compression_ratio,
            },
            &trove.name,
            &repo_pkg.version,
            &temp_dir,
        ) {
            Ok(_) => {
                let applier = DeltaApplier::new(&objects_dir)?;
                match applier.apply_delta(&delta_info.from_hash, &delta_path, &delta_info.to_hash)
                {
                    Ok(_) => {
                        println!("  [OK] Delta applied");
                        deltas_applied += 1;
                        total_bytes_saved += repo_pkg.size - delta_info.delta_size;
                    }
                    Err(e) => {
                        warn!("  Delta application failed: {}, will download full package", e);
                        delta_failures += 1;
                        // Get repository for fallback download
                        if let Ok(Some(repo)) = Repository::find_by_id(&conn, repo_pkg.repository_id) {
                            full_updates.push((trove, repo_pkg, repo));
                        }
                    }
                }
                let _ = std::fs::remove_file(delta_path);
            }
            Err(e) => {
                warn!("  Delta download failed: {}, will download full package", e);
                delta_failures += 1;
                // Get repository for fallback download
                if let Ok(Some(repo)) = Repository::find_by_id(&conn, repo_pkg.repository_id) {
                    full_updates.push((trove, repo_pkg, repo));
                }
            }
        }
    }

    // Phase 3 & 4: Download and install full packages with progress tracking
    if !full_updates.is_empty() {
        let total_to_install = full_updates.len() as u64;
        let mut progress = UpdateProgress::new(total_to_install);

        // Download full packages in parallel
        progress.set_status("Downloading packages...");

        // Pre-create progress bars for all downloads
        let progress_bars: Vec<_> = full_updates
            .iter()
            .map(|(trove, repo_pkg, _)| {
                progress.add_download_progress(&trove.name, repo_pkg.size as u64)
            })
            .collect();

        let download_results: Vec<_> = full_updates
            .par_iter()
            .zip(progress_bars.par_iter())
            .map(|((trove, repo_pkg, repo), pb)| {
                info!("Downloading {}", trove.name);

                // Build GPG options if enabled
                let gpg_options = if repo.gpg_check {
                    Some(DownloadOptions {
                        gpg_check: true,
                        gpg_strict: repo.gpg_strict,
                        keyring_dir: keyring_dir.clone(),
                        repository_name: repo.name.clone(),
                    })
                } else {
                    None
                };

                match repository::download_package_verified_with_progress(
                    repo_pkg,
                    &temp_dir,
                    gpg_options.as_ref(),
                    Some(pb),
                ) {
                    Ok(pkg_path) => {
                        pb.finish_with_message(format!("{} [done]", trove.name));
                        DownloadResult::Full {
                            trove: trove.clone(),
                            pkg_path,
                        }
                    }
                    Err(e) => {
                        pb.abandon_with_message(format!("{} [FAILED]", trove.name));
                        DownloadResult::Failed {
                            name: trove.name.clone(),
                            error: e.to_string(),
                        }
                    }
                }
            })
            .collect();

        // Install downloaded packages sequentially
        for result in download_results {
            match result {
                DownloadResult::Full { trove, pkg_path } => {
                    progress.set_phase(&trove.name, UpdatePhase::Installing);

                    if let Err(e) =
                        install_package_from_file(&pkg_path, &mut conn, root, db_path, Some(&trove))
                    {
                        progress.fail_package(&trove.name, &e.to_string());
                        warn!("  Package installation failed: {}", e);
                        let _ = std::fs::remove_file(&pkg_path);
                        continue;
                    }

                    full_downloads += 1;
                    progress.complete_package(&trove.name);
                    let _ = std::fs::remove_file(pkg_path);
                }
                DownloadResult::Failed { name, error } => {
                    progress.fail_package(&name, &error);
                    warn!("  Failed to download {}: {}", name, error);
                }
            }
        }

        progress.finish(&format!(
            "Updated {} package(s)",
            deltas_applied + full_downloads
        ));
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
