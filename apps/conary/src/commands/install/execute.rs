// src/commands/install/execute.rs
//! Transaction execution helpers - CAS storage and file tracking
//!
//! In the composefs-native model, files are stored in CAS and tracked in the DB.
//! Filesystem deployment happens via EROFS image build + composefs mount, not
//! direct file deployment. These helpers handle the CAS/DB side.

use super::ExtractionResult;
use super::inner;
use crate::commands::LiveRootFile;
use anyhow::{Context, Result};
use conary_core::filesystem::CasStore;
use conary_core::packages::PackageFormat;
use conary_core::transaction::{ExtractedFile as TxExtractedFile, FileToRemove};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PackageExecutionPath {
    GenerationAware,
    MutableLiveRoot,
}

fn package_execution_path(db_path: &str) -> Result<PackageExecutionPath> {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let current_link = runtime_root.root().join("current");
    let has_current_link = match std::fs::symlink_metadata(&current_link) {
        Ok(metadata) if metadata.file_type().is_symlink() && !current_link.exists() => {
            let target = std::fs::read_link(&current_link)
                .with_context(|| format!("Failed to read {}", current_link.display()))?;
            anyhow::bail!(
                "current generation symlink {} -> {} is dangling",
                current_link.display(),
                target.display()
            );
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", current_link.display()));
        }
    };
    if !has_current_link && std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        return Ok(PackageExecutionPath::GenerationAware);
    }
    let current = conary_core::generation::mount::current_generation(runtime_root.root())?;
    Ok(match current {
        Some(_) => PackageExecutionPath::GenerationAware,
        None => PackageExecutionPath::MutableLiveRoot,
    })
}

pub(super) fn prepare_install_environment_before_scriptlets(
    conn: &rusqlite::Connection,
    db_path: &str,
    root: &str,
) -> Result<PackageExecutionPath> {
    let execution_path = package_execution_path(db_path)?;
    recover_mutable_journals_before_scriptlets(conn, db_path, root, execution_path)?;
    Ok(execution_path)
}

fn recover_mutable_journals_before_scriptlets(
    conn: &rusqlite::Connection,
    db_path: &str,
    root: &str,
    execution_path: PackageExecutionPath,
) -> Result<()> {
    if execution_path == PackageExecutionPath::MutableLiveRoot {
        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
        crate::commands::live_root::recover_pending_journals_with_changesets(
            runtime_root.root(),
            Path::new(root),
            conn,
        )?;
    }
    Ok(())
}

pub(super) fn preflight_extracted_live_root_file_ownership(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    execution_path: PackageExecutionPath,
) -> Result<()> {
    if execution_path == PackageExecutionPath::MutableLiveRoot {
        inner::preflight_live_root_file_ownership(
            conn,
            extraction
                .extracted_files
                .iter()
                .map(|file| file.path.as_str()),
            pkg.name(),
        )?;
    }
    Ok(())
}

pub(super) fn live_root_files_from_stored_files(
    cas: &CasStore,
    stored_files: &[inner::StoredInstallFile],
) -> Result<Vec<LiveRootFile>> {
    stored_files
        .iter()
        .map(|file| {
            let content = if let Some(target) = file.symlink_target.as_deref() {
                let stored_target = cas
                    .retrieve_symlink(&file.hash)
                    .with_context(|| format!("Failed to read symlink {} from CAS", file.path))?;
                if stored_target != target {
                    anyhow::bail!(
                        "CAS symlink target mismatch for {}: expected {}, got {}",
                        file.path,
                        target,
                        stored_target
                    );
                }
                Vec::new()
            } else {
                let content = cas
                    .retrieve(&file.hash)
                    .with_context(|| format!("Failed to read {} from CAS", file.path))?;
                if content.len() as i64 != file.size {
                    anyhow::bail!(
                        "CAS object size mismatch for {}: expected {}, got {}",
                        file.path,
                        file.size,
                        content.len()
                    );
                }
                content
            };
            Ok(LiveRootFile {
                path: file.path.clone(),
                content,
                mode: file.mode,
                symlink_target: file.symlink_target.clone(),
            })
        })
        .collect()
}

pub(super) fn run_triggers(
    conn: &rusqlite::Connection,
    root: &Path,
    changeset_id: i64,
    file_paths: &[String],
) {
    let trigger_executor = conary_core::trigger::TriggerExecutor::new(conn, root);

    let triggered = trigger_executor
        .record_triggers(changeset_id, file_paths)
        .unwrap_or_else(|e| {
            warn!("Failed to record triggers: {}", e);
            Vec::new()
        });

    if !triggered.is_empty() {
        info!("Recorded {} trigger(s) for execution", triggered.len());
        match trigger_executor.execute_pending(changeset_id) {
            Ok(results) => {
                if results.total() > 0 {
                    info!(
                        "Triggers: {} succeeded, {} failed, {} skipped",
                        results.succeeded, results.failed, results.skipped
                    );
                    for error in &results.errors {
                        warn!("Trigger error: {}", error);
                    }
                }
            }
            Err(e) => {
                warn!("Trigger execution failed: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_mutable_journals_runs_before_scriptlets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        let live_file = root.join("usr/bin/fixture");
        std::fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        std::fs::write(&live_file, "before").unwrap();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();

        let tx_uuid = uuid::Uuid::new_v4().to_string();
        let mut live_tx = crate::commands::LiveRootTransaction::begin(
            temp.path(),
            &root,
            tx_uuid,
            "install fixture",
        )
        .unwrap();
        live_tx
            .apply_install_files(&[crate::commands::LiveRootFile {
                path: "/usr/bin/fixture".to_string(),
                content: b"after".to_vec(),
                mode: 0o100644,
                symlink_target: None,
            }])
            .unwrap();
        std::mem::forget(live_tx);
        assert_eq!(std::fs::read_to_string(&live_file).unwrap(), "after");

        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();
        recover_mutable_journals_before_scriptlets(
            &conn,
            &db_path_string,
            &root_string,
            PackageExecutionPath::MutableLiveRoot,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&live_file).unwrap(), "before");
    }

    #[test]
    fn live_root_files_are_loaded_from_stored_cas_objects() {
        let temp = tempfile::tempdir().unwrap();
        let cas = conary_core::filesystem::CasStore::new(temp.path().join("objects")).unwrap();
        let hash = cas.store(b"from cas").unwrap();
        let files = live_root_files_from_stored_files(
            &cas,
            &[inner::StoredInstallFile {
                path: "/usr/bin/fixture".to_string(),
                hash,
                size: 8,
                mode: 0o100755,
                symlink_target: None,
            }],
        )
        .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, b"from cas");
    }

    #[test]
    fn package_execution_path_fails_closed_on_invalid_generation_state() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("generations/not-a-generation")).unwrap();
        std::os::unix::fs::symlink("generations/not-a-generation", temp.path().join("current"))
            .unwrap();
        let db_path = temp.path().join("conary.db");
        let db_path_string = db_path.to_string_lossy().into_owned();

        let error = package_execution_path(&db_path_string).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Failed to parse generation number"),
            "{error}"
        );
    }

    #[test]
    fn package_execution_path_fails_closed_on_dangling_current_symlink() {
        let temp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("generations/7", temp.path().join("current")).unwrap();
        let db_path = temp.path().join("conary.db");
        let db_path_string = db_path.to_string_lossy().into_owned();

        let error = package_execution_path(&db_path_string).unwrap_err();

        assert!(error.to_string().contains("dangling"), "{error}");
    }
}
