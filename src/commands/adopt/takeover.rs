// src/commands/adopt/takeover.rs

//! Take over adopted packages from the system package manager
//!
//! Migrates packages from system PM tracking to full Conary ownership.
//! After takeover, Conary owns the files and the system PM no longer knows
//! about the package.

use super::super::create_state_snapshot;
use super::super::open_db;
use super::system::{FileInfoTuple, compute_file_hash};
use anyhow::Result;
use conary_core::db::models::{Changeset, ChangesetStatus, InstallSource, Trove};
use conary_core::model;
use conary_core::packages::{SystemPackageManager, dpkg_query, pacman_query, rpm_query};
use std::io::{self, Write};
use tracing::{info, warn};

/// Take over adopted packages from the system package manager.
///
/// This implements the ownership ladder transition: `AdoptedTrack` / `AdoptedFull` -> `Taken`.
/// The ownership ladder is: `AdoptedTrack` -> `AdoptedFull` -> `Taken` -> `Repository`.
/// - `AdoptedTrack`: metadata-only tracking (no CAS content)
/// - `AdoptedFull`: CAS-backed adoption (content hardlinked into CAS)
/// - `Taken`: full Conary ownership (removed from system PM)
/// - `Repository`: installed fresh from Remi CCS repository
///
/// Operation order (chosen for safest failure mode):
/// 1. Pre-capture file lists from PM while it still has the metadata
/// 2. DB transaction: hardlink into CAS, mark as Taken
/// 3. Remove from system PM database (post-commit)
///
/// If step 3 fails, Conary owns the files and the PM has a harmless ghost
/// record. This is safer than the reverse (PM removed but DB not updated)
/// which would lose tracking entirely.
pub async fn cmd_adopt_takeover(
    packages: &[String],
    db_path: &str,
    system_wide: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let pkg_mgr = SystemPackageManager::detect();
    if !pkg_mgr.is_available() {
        return Err(anyhow::anyhow!(
            "No supported package manager found. Conary supports RPM, dpkg, and pacman."
        ));
    }

    let mut conn = open_db(db_path)?;

    // Collect target packages
    println!("Scanning installed packages...");
    let targets: Vec<Trove> = if system_wide {
        let all = Trove::list_all(&conn)?;
        all.into_iter()
            .filter(|t| t.install_source.is_adopted())
            .collect()
    } else {
        let mut found = Vec::new();
        for name in packages {
            match Trove::find_one_by_name(&conn, name)? {
                Some(t) if t.install_source.is_adopted() => found.push(t),
                Some(t) => {
                    println!(
                        "Skipping '{}': install source is '{}', not adopted",
                        name,
                        t.install_source.as_str()
                    );
                }
                None => {
                    println!("Skipping '{}': not found in Conary database", name);
                }
            }
        }
        found
    };

    if targets.is_empty() {
        println!("No adopted packages to take over.");
        return Ok(());
    }

    // Log convergence context from system model if available
    let convergence_intent = if model::model_exists(None) {
        match model::load_model(None) {
            Ok(m) => {
                let intent = &m.system.convergence;
                info!(
                    "System model convergence intent: {} (target: {})",
                    intent.display_name(),
                    intent.target_install_source()
                );
                Some(intent.clone())
            }
            Err(e) => {
                info!("Could not load system model for convergence context: {e}");
                None
            }
        }
    } else {
        None
    };

    println!(
        "Will take over {} package(s) from {} to Conary ownership:",
        targets.len(),
        pkg_mgr.display_name()
    );
    if let Some(ref intent) = convergence_intent {
        println!(
            "  Convergence intent: {} (target state: {})",
            intent.display_name(),
            intent.target_install_source()
        );
    }
    for t in &targets {
        println!("  {} {} ({})", t.name, t.version, t.install_source.as_str());
    }

    if dry_run {
        println!("\nDry run -- no changes made.");
        return Ok(());
    }

    // Interactive confirmation unless --yes
    if !yes {
        print!(
            "\nThis will remove these packages from the {} database.\nConary will fully own the files. Continue? [y/N] ",
            pkg_mgr.display_name()
        );
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Set up CAS for AdoptedTrack -> hardlink
    let objects_dir = std::path::PathBuf::from(db_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("objects");
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;

    let mut changeset = Changeset::new(format!(
        "Takeover {} package(s) from {}",
        targets.len(),
        pkg_mgr.display_name()
    ));

    let mut taken_count = 0u32;
    let mut failed_count = 0u32;

    // Step 1: Pre-capture file lists from PM while it still has metadata.
    // This MUST happen before any PM removal, otherwise the PM can't answer
    // file queries for AdoptedTrack packages that need CAS hardlinking.
    // Keyed by trove ID (not name) so arch/version variants are independent.
    let mut captured_files: std::collections::HashMap<i64, Vec<FileInfoTuple>> =
        std::collections::HashMap::new();
    let mut skip_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for trove in &targets {
        if trove.install_source == InstallSource::AdoptedTrack {
            let trove_id = match trove.id {
                Some(id) => id,
                None => continue,
            };
            let files = query_package_files(pkg_mgr, &trove.name);
            if files.is_empty() {
                // Check if this trove actually has file records in the DB.
                // Meta-packages (no files) legitimately have empty file lists
                // and should not be blocked from takeover.
                // DB errors are treated conservatively as "has files" to avoid
                // accidentally deleting file tracking on a transient DB issue.
                let has_db_files =
                    match conary_core::db::models::FileEntry::find_by_trove(&conn, trove_id) {
                        Ok(f) => !f.is_empty(),
                        Err(e) => {
                            warn!(
                                "DB error checking files for {}: {} -- assuming files exist",
                                trove.name, e
                            );
                            true
                        }
                    };
                if has_db_files {
                    eprintln!(
                        "WARNING: Could not retrieve file list for '{}' from {} -- \
                         skipping takeover to preserve existing file tracking.",
                        trove.name,
                        pkg_mgr.display_name(),
                    );
                    skip_ids.insert(trove_id);
                    failed_count += 1;
                    continue;
                }
                // Meta-package with no files: proceed normally
            }
            captured_files.insert(trove_id, files);
        }
    }

    // Check if any candidates remain after pre-capture filtering
    let actionable_count = targets
        .iter()
        .filter(|t| t.id.is_some_and(|id| !skip_ids.contains(&id)))
        .count();
    if actionable_count == 0 {
        println!("\nAll packages were skipped due to errors. No changes made.");
        return Ok(());
    }

    // Step 2: DB transaction — mark as Taken, insert CAS files from pre-captured data.
    // This happens BEFORE PM removal so that if the transaction fails, the PM
    // metadata is untouched and the system is in a consistent state.
    println!("Converting {} packages...", actionable_count);
    println!("Recording in database...");
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;

        for trove in &targets {
            let trove_id = match trove.id {
                Some(id) => id,
                None => {
                    warn!("Trove {} has no id, skipping", trove.name);
                    failed_count += 1;
                    continue;
                }
            };

            // Skip packages whose file pre-capture failed
            if skip_ids.contains(&trove_id) {
                continue;
            }

            // If AdoptedTrack, hardlink pre-captured files into CAS
            if trove.install_source == InstallSource::AdoptedTrack {
                info!("Hardlinking files for {} into CAS...", trove.name);

                let files = captured_files.get(&trove_id).cloned().unwrap_or_default();
                tx.execute("DELETE FROM files WHERE trove_id = ?1", [trove_id])?;

                for (
                    file_path,
                    file_size,
                    file_mode,
                    file_digest,
                    file_user,
                    file_group,
                    link_target,
                ) in &files
                {
                    let hash = compute_file_hash(
                        file_path,
                        *file_mode,
                        file_digest.as_deref(),
                        link_target.as_deref(),
                        true, // full mode -- hardlink into CAS
                        Some(&cas),
                    );
                    let mut fe = conary_core::db::models::FileEntry::new(
                        file_path.clone(),
                        hash,
                        *file_size,
                        *file_mode,
                        trove_id,
                    );
                    fe.owner = file_user.clone();
                    fe.group_name = file_group.clone();
                    if let Err(e) = fe.insert_or_replace(tx) {
                        warn!(
                            "Failed to insert file {} for {}: {}",
                            file_path, trove.name, e
                        );
                    }
                }
            }

            // Mark as Taken
            tx.execute(
                "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 WHERE id = ?3",
                rusqlite::params![
                    InstallSource::Taken.as_str(),
                    changeset_id,
                    trove_id,
                ],
            )?;

            taken_count += 1;
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Step 3: Remove from system PM database (post-commit).
    // If this fails, Conary already owns the files and the PM has a harmless
    // ghost record. This is the safer failure mode vs the reverse (PM removed
    // but DB not updated = lost tracking).
    let mut pm_fail_count = 0u32;
    for trove in &targets {
        if trove.id.is_some_and(|id| skip_ids.contains(&id)) {
            continue;
        }
        if let Err(e) = remove_from_system_pm(pkg_mgr, &trove.name) {
            warn!(
                "Failed to remove {} from {} database: {}",
                trove.name,
                pkg_mgr.display_name(),
                e
            );
            eprintln!(
                "WARNING: Could not remove '{}' from {} database: {}\n\
                 Conary owns the files, but {} still has a ghost record.\n\
                 Remove it manually with: {}",
                trove.name,
                pkg_mgr.display_name(),
                e,
                pkg_mgr.display_name(),
                pkg_mgr.remove_command(&trove.name),
            );
            pm_fail_count += 1;
        } else {
            println!(
                "  [OK] {} removed from {} database",
                trove.name,
                pkg_mgr.display_name()
            );
        }
    }

    // State snapshot for rollback
    if taken_count > 0 {
        create_state_snapshot(
            &conn,
            changeset_id,
            &format!("Takeover {} package(s)", taken_count),
        )?;
    }

    println!(
        "\nTakeover complete: {} taken over, {} failed.",
        taken_count, failed_count
    );
    if pm_fail_count > 0 {
        println!(
            "WARNING: {} package(s) could not be removed from {}. See warnings above.",
            pm_fail_count,
            pkg_mgr.display_name()
        );
    }

    Ok(())
}

/// Remove a package from the system package manager's database only.
fn remove_from_system_pm(pkg_mgr: SystemPackageManager, name: &str) -> Result<()> {
    match pkg_mgr {
        SystemPackageManager::Rpm => {
            rpm_query::remove_from_db_only(name).map_err(|e| anyhow::anyhow!("{}", e))
        }
        SystemPackageManager::Dpkg => {
            dpkg_query::remove_from_db_only(name).map_err(|e| anyhow::anyhow!("{}", e))
        }
        SystemPackageManager::Pacman => {
            pacman_query::remove_from_db_only(name).map_err(|e| anyhow::anyhow!("{}", e))
        }
        SystemPackageManager::Unknown => Err(anyhow::anyhow!("No supported package manager")),
    }
}

/// Query files for a package from the active package manager.
fn query_package_files(pkg_mgr: SystemPackageManager, name: &str) -> Vec<FileInfoTuple> {
    match pkg_mgr {
        SystemPackageManager::Rpm => rpm_query::query_package_files(name)
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                (
                    f.path,
                    f.size,
                    f.mode,
                    f.digest,
                    f.user,
                    f.group,
                    f.link_target,
                )
            })
            .collect(),
        SystemPackageManager::Dpkg => dpkg_query::query_package_files(name)
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                (
                    f.path,
                    f.size,
                    f.mode,
                    f.digest,
                    f.user,
                    f.group,
                    f.link_target,
                )
            })
            .collect(),
        SystemPackageManager::Pacman => pacman_query::query_package_files(name)
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                (
                    f.path,
                    f.size,
                    f.mode,
                    f.digest,
                    f.user,
                    f.group,
                    f.link_target,
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use conary_core::db::models::InstallSource;

    #[test]
    fn test_taken_variant_roundtrip() {
        let taken = InstallSource::Taken;
        let s = taken.as_str();
        assert_eq!(s, "taken");
        let parsed: InstallSource = s.parse().unwrap();
        assert_eq!(parsed, InstallSource::Taken);
    }

    #[test]
    fn test_taken_is_conary_owned() {
        assert!(InstallSource::Taken.is_conary_owned());
        assert!(InstallSource::File.is_conary_owned());
        assert!(InstallSource::Repository.is_conary_owned());
        assert!(!InstallSource::AdoptedTrack.is_conary_owned());
        assert!(!InstallSource::AdoptedFull.is_conary_owned());
    }

    #[test]
    fn test_taken_is_not_adopted() {
        assert!(!InstallSource::Taken.is_adopted());
    }
}
