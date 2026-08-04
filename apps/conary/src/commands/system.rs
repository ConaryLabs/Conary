// apps/conary/src/commands/system.rs
//! System management commands (init, verify, rollback)

mod init;
mod rebuild_database;
mod rollback_command;
mod rollback_restore;

pub use init::cmd_init;
#[cfg(test)]
use init::{NATIVE_REPOSITORY_SEEDS, paths_refer_to_same_location, validate_init_privileges};
pub use rebuild_database::cmd_rebuild_database;
pub use rollback_command::cmd_rollback;
#[cfg(test)]
use rollback_command::cmd_rollback_with_forced_precommit_failure;

#[cfg(test)]
use super::FileSnapshot;
#[cfg(test)]
use super::TroveSnapshot;
use super::open_db;
#[cfg(test)]
use anyhow::Context;
use anyhow::{Result, anyhow};
use conary_core::db::models::{
    PackageResolution, Repository, RepositoryPackage, RepositoryPackageKey,
    RepositoryPackageKeyStatus,
};
use conary_core::db::paths::objects_dir;
#[cfg(test)]
use conary_core::filesystem::CasStore;
#[cfg(test)]
use conary_core::payload::PayloadNodeKind;
use conary_core::repository::RepositoryFormat;
use std::path::{Path, PathBuf};
use tracing::info;

#[cfg(test)]
use rollback_restore::restore_snapshot;

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
                let payload =
                    conary_core::db::models::PackagePayloadOwnership::load(&conn, trove_id)?;
                for file in payload.entries() {
                    all_files.push((
                        file.path.clone(),
                        file.content.as_ref().map(|content| content.sha256.clone()),
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
