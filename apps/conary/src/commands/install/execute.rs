// src/commands/install/execute.rs
//! Transaction execution helpers - CAS storage and file tracking
//!
//! In the composefs-native model, files are stored in CAS and tracked in the DB.
//! Filesystem deployment happens via EROFS image build + composefs mount, not
//! direct file deployment. These helpers handle the CAS/DB side.

use anyhow::Result;
use conary_core::transaction::{ExtractedFile as TxExtractedFile, FileToRemove};
use rusqlite::Connection;
use std::collections::HashSet;

/// Convert package ExtractedFile to transaction ExtractedFile
///
/// Preserved for batch install compatibility (PreparedPackage uses ExtractedFile).
#[allow(dead_code)]
pub fn convert_extracted_files(
    files: &[conary_core::packages::traits::ExtractedFile],
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
    let old_files = conary_core::db::models::FileEntry::find_by_trove(conn, old_trove_id)?;
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
