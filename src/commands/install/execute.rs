// src/commands/install/execute.rs
//! Transaction execution helpers - file deployment and rollback

use anyhow::Result;
use conary::transaction::{ExtractedFile as TxExtractedFile, FileToRemove};
use rusqlite::Connection;
use std::collections::HashSet;
use tracing::{info, warn};

/// Deploy files to filesystem with rollback capability
///
/// Returns the list of (path, hash, size, mode) for all deployed files
///
/// NOTE: This function is kept for backwards compatibility with other code paths.
/// The main install flow now uses TransactionEngine for crash-safe operations.
#[allow(dead_code)]
pub fn deploy_files(
    deployer: &conary::filesystem::FileDeployer,
    extracted_files: &[conary::packages::traits::ExtractedFile],
    is_upgrade: bool,
    conn: &Connection,
    pkg_name: &str,
) -> Result<Vec<(String, String, i64, i32)>> {
    // Phase 1: Check file conflicts BEFORE any changes
    for file in extracted_files {
        if deployer.file_exists(&file.path) {
            if let Some(existing) =
                conary::db::models::FileEntry::find_by_path(conn, &file.path)?
            {
                let owner_trove =
                    conary::db::models::Trove::find_by_id(conn, existing.trove_id)?;
                if let Some(owner) = owner_trove
                    && owner.name != pkg_name
                {
                    return Err(anyhow::anyhow!(
                        "File conflict: {} is owned by package {}",
                        file.path, owner.name
                    ));
                }
            } else if !is_upgrade {
                return Err(anyhow::anyhow!(
                    "File conflict: {} exists but is not tracked by any package",
                    file.path
                ));
            }
        }
    }

    // Phase 2: Store content in CAS and pre-compute hashes
    let mut file_hashes: Vec<(String, String, i64, i32)> = Vec::with_capacity(extracted_files.len());
    for file in extracted_files {
        let hash = deployer.cas().store(&file.content)?;
        file_hashes.push((file.path.clone(), hash, file.size, file.mode));
    }

    // Phase 3: Deploy files, tracking what we've deployed for rollback
    let mut deployed_files: Vec<String> = Vec::with_capacity(extracted_files.len());
    let deploy_result: Result<()> = (|| {
        for (path, hash, _size, mode) in &file_hashes {
            deployer.deploy_file(path, hash, *mode as u32)?;
            deployed_files.push(path.clone());
        }
        Ok(())
    })();

    // If deployment failed, rollback deployed files
    if let Err(e) = deploy_result {
        warn!(
            "File deployment failed, rolling back {} deployed files",
            deployed_files.len()
        );
        for path in &deployed_files {
            if let Err(remove_err) = deployer.remove_file(path) {
                warn!("Failed to rollback file {}: {}", path, remove_err);
            }
        }
        return Err(anyhow::anyhow!("File deployment failed: {}", e));
    }

    info!("Successfully deployed {} files", deployed_files.len());
    Ok(file_hashes)
}

/// Rollback deployed files on failure (legacy - kept for non-transaction code paths)
#[allow(dead_code)]
pub fn rollback_deployed_files(deployer: &conary::filesystem::FileDeployer, files: &[(String, String, i64, i32)]) {
    warn!("Rolling back {} deployed files", files.len());
    for (path, _, _, _) in files {
        if let Err(e) = deployer.remove_file(path) {
            warn!("Failed to rollback file {}: {}", path, e);
        }
    }
}

/// Convert package ExtractedFile to transaction ExtractedFile
pub fn convert_extracted_files(
    files: &[conary::packages::traits::ExtractedFile],
) -> Vec<TxExtractedFile> {
    files
        .iter()
        .map(|f| {
            // Detect symlinks by checking if content starts with symlink marker
            // (package parsers store symlink target as content prefixed with special marker)
            let is_symlink = f.mode & 0o120000 == 0o120000; // S_IFLNK check
            let symlink_target = if is_symlink {
                // For symlinks, the content is the target path
                String::from_utf8(f.content.clone()).ok()
            } else {
                None
            };

            TxExtractedFile {
                path: f.path.clone(),
                content: f.content.clone(),
                mode: f.mode as u32,
                is_symlink,
                symlink_target,
            }
        })
        .collect()
}

/// Get list of files to remove from old trove (for upgrades)
pub fn get_files_to_remove(
    conn: &Connection,
    old_trove_id: i64,
    new_file_paths: &HashSet<&str>,
) -> Result<Vec<FileToRemove>> {
    let old_files = conary::db::models::FileEntry::find_by_trove(conn, old_trove_id)?;
    let mut to_remove = Vec::new();

    for old_file in old_files {
        // Only remove files that aren't in the new package
        if !new_file_paths.contains(old_file.path.as_str()) {
            to_remove.push(FileToRemove {
                path: old_file.path,
                hash: old_file.sha256_hash,
                size: old_file.size,
                mode: old_file.permissions as u32,
            });
        }
    }

    Ok(to_remove)
}
