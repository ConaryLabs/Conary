// src/transaction/recovery.rs

//! Transaction recovery for crash safety
//!
//! This module handles recovery of incomplete transactions after a crash or
//! unexpected termination. The key principle is:
//!
//! - Before DB_APPLIED: Roll back (restore backups, remove staged files)
//! - After DB_APPLIED: Roll forward (cleanup temp dirs, archive journal)
//!
//! The database is the source of truth for whether DB_APPLIED actually succeeded,
//! since a crash can occur after SQLite commits but before the journal record
//! is written.

use crate::Result;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

use super::journal::{find_incomplete_journals, JournalRecord, TransactionJournal};
use super::{FileType, TransactionEngine, TransactionState};

/// Helper to join a root path with a potentially absolute path
fn safe_join(root: &Path, path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    let relative = path_str.strip_prefix('/').unwrap_or(&path_str);
    root.join(relative)
}

/// Outcome of recovering a transaction
#[derive(Debug, Clone)]
pub enum RecoveryOutcome {
    /// Transaction was rolled back (before DB commit)
    RolledBack { tx_uuid: String, reason: String },
    /// Transaction was rolled forward (after DB commit, completed cleanup)
    RolledForward { tx_uuid: String, changeset_id: i64 },
    /// Transaction was already complete, just needed cleanup
    CompletedPending { tx_uuid: String },
    /// Journal was corrupted, manual intervention needed
    Corrupted { tx_uuid: String, error: String },
    /// No recovery needed (clean journal or already handled)
    Clean { tx_uuid: String },
}

/// Recover all incomplete transactions
pub fn recover_all(engine: &TransactionEngine, conn: &mut Connection) -> Result<Vec<RecoveryOutcome>> {
    let journals = find_incomplete_journals(&engine.config().journal_dir)?;
    let mut outcomes = Vec::new();

    for journal_path in journals {
        let outcome = recover_single(engine, journal_path, conn)?;
        outcomes.push(outcome);
    }

    Ok(outcomes)
}

/// Recover a single transaction from its journal
fn recover_single(
    engine: &TransactionEngine,
    journal_path: std::path::PathBuf,
    conn: &mut Connection,
) -> Result<RecoveryOutcome> {
    let journal = match TransactionJournal::open(journal_path.clone()) {
        Ok(j) => j,
        Err(e) => {
            return Ok(RecoveryOutcome::Corrupted {
                tx_uuid: "unknown".to_string(),
                error: format!("Failed to open journal: {}", e),
            });
        }
    };

    let tx_uuid = journal.tx_uuid().to_string();
    let records = match journal.read_all() {
        Ok(r) => r,
        Err(e) => {
            return Ok(RecoveryOutcome::Corrupted {
                tx_uuid,
                error: format!("Failed to read journal: {}", e),
            });
        }
    };

    if records.is_empty() {
        // Empty journal, just delete it
        journal.delete()?;
        return Ok(RecoveryOutcome::Clean { tx_uuid });
    }

    let last_phase = journal.last_phase()?;
    log::info!(
        "Recovering transaction {} (last phase: {:?})",
        tx_uuid,
        last_phase
    );

    match last_phase {
        // Before DB commit: ROLL BACK
        TransactionState::New
        | TransactionState::Planned
        | TransactionState::Prepared
        | TransactionState::PreScriptsComplete
        | TransactionState::BackedUp
        | TransactionState::Staged => {
            rollback_transaction(engine, &tx_uuid, &records)?;
            cleanup_work_dir(engine, &tx_uuid)?;
            journal.delete()?;
            Ok(RecoveryOutcome::RolledBack {
                tx_uuid,
                reason: format!("Crashed before DB commit (phase: {:?})", last_phase),
            })
        }

        // Critical transition: FsApplied -> DbApplied
        // Must check DB to determine actual state
        TransactionState::FsApplied => {
            // Check if DB actually has this transaction
            let db_has_changeset = check_changeset_by_uuid(conn, &tx_uuid)?;

            if db_has_changeset {
                // DB commit succeeded, crash happened before journal updated
                // Roll forward
                cleanup_work_dir(engine, &tx_uuid)?;
                journal.archive()?;
                let changeset_id = get_changeset_id_by_uuid(conn, &tx_uuid)?;
                Ok(RecoveryOutcome::RolledForward {
                    tx_uuid,
                    changeset_id,
                })
            } else {
                // DB commit failed or never happened
                // Roll back
                rollback_transaction(engine, &tx_uuid, &records)?;
                cleanup_work_dir(engine, &tx_uuid)?;
                journal.delete()?;
                Ok(RecoveryOutcome::RolledBack {
                    tx_uuid,
                    reason: "DB commit was not durable".to_string(),
                })
            }
        }

        // After DB commit: ROLL FORWARD
        TransactionState::DbApplied | TransactionState::PostScriptsComplete => {
            // Verify DB actually has the changeset
            let db_has_changeset = check_changeset_by_uuid(conn, &tx_uuid)?;

            if db_has_changeset {
                cleanup_work_dir(engine, &tx_uuid)?;
                journal.archive()?;
                let _changeset_id = get_changeset_id_by_uuid(conn, &tx_uuid)?;
                Ok(RecoveryOutcome::CompletedPending {
                    tx_uuid,
                })
            } else {
                // This shouldn't happen - journal says DB committed but DB doesn't have it
                Ok(RecoveryOutcome::Corrupted {
                    tx_uuid,
                    error: "Journal says DB committed but changeset not found".to_string(),
                })
            }
        }

        TransactionState::Done => {
            // Transaction was complete, just archive the journal
            journal.archive()?;
            Ok(RecoveryOutcome::Clean { tx_uuid })
        }

        TransactionState::Aborted | TransactionState::Failed => {
            // Already handled, just cleanup
            cleanup_work_dir(engine, &tx_uuid)?;
            journal.delete()?;
            Ok(RecoveryOutcome::Clean { tx_uuid })
        }
    }
}

/// Roll back a transaction by restoring backups and removing staged files
pub fn rollback_transaction(
    engine: &TransactionEngine,
    tx_uuid: &str,
    records: &[JournalRecord],
) -> Result<()> {
    log::info!("Rolling back transaction {}", tx_uuid);

    let root = &engine.config().root;
    let _work_dir = engine.txn_work_dir(tx_uuid);

    // Process records in reverse order to undo changes
    for record in records.iter().rev() {
        match record {
            JournalRecord::Stage {
                path, stage_path, ..
            } => {
                // Remove staged file if it exists
                if stage_path.exists()
                    && let Err(e) = fs::remove_file(stage_path)
                {
                    log::warn!("Failed to remove staged file {:?}: {}", stage_path, e);
                }

                // If file was already renamed to final location, check if we need to remove it
                let final_path = safe_join(root, Path::new(path));
                if final_path.exists() {
                    // Check if there's a corresponding backup - if not, this was a new file
                    let has_backup = records.iter().any(|r| {
                        matches!(r, JournalRecord::Backup { path: bp, .. } if bp == path)
                    });

                    if !has_backup {
                        // New file with no backup - remove it
                        if let Err(e) = fs::remove_file(&final_path) {
                            log::warn!("Failed to remove new file {:?}: {}", final_path, e);
                        }
                    }
                }
            }

            JournalRecord::Backup {
                path,
                backup_path,
                old_type,
                ..
            } => {
                // Restore backup to original location
                if backup_path.exists() {
                    let final_path = safe_join(root, Path::new(path));

                    // Remove any new file at the target location first
                    if final_path.exists() || final_path.symlink_metadata().is_ok() {
                        match old_type {
                            FileType::Directory => {
                                if let Err(e) = fs::remove_dir(&final_path) {
                                    log::warn!(
                                        "Failed to remove directory {:?}: {}",
                                        final_path,
                                        e
                                    );
                                }
                            }
                            _ => {
                                if let Err(e) = fs::remove_file(&final_path) {
                                    log::warn!("Failed to remove file {:?}: {}", final_path, e);
                                }
                            }
                        }
                    }

                    // Ensure parent directory exists
                    if let Some(parent) = final_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }

                    // Check if backup is a symlink marker
                    #[allow(clippy::collapsible_if)]
                    if backup_path.is_file() {
                        if let Ok(content) = fs::read_to_string(backup_path) {
                            if let Some(target) = content.strip_prefix("SYMLINK:") {
                                // Restore symlink
                                #[cfg(unix)]
                                {
                                    if let Err(e) =
                                        std::os::unix::fs::symlink(target, &final_path)
                                    {
                                        log::warn!(
                                            "Failed to restore symlink {:?}: {}",
                                            final_path,
                                            e
                                        );
                                    }
                                }
                                continue;
                            }
                        }
                    }

                    // Regular file - rename back
                    if let Err(e) = fs::rename(backup_path, &final_path) {
                        log::warn!(
                            "Failed to restore backup {:?} -> {:?}: {}",
                            backup_path,
                            final_path,
                            e
                        );
                    }
                }
            }

            _ => {}
        }
    }

    // Remove any directories that were created for new files
    // (in reverse order so children are removed before parents)
    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
    for record in records {
        if let JournalRecord::Stage { path, .. } = record {
            // Check if this was a new file (no backup)
            let has_backup = records
                .iter()
                .any(|r| matches!(r, JournalRecord::Backup { path: bp, .. } if bp == path));

            if !has_backup
                && let Some(parent) = path.parent()
            {
                dirs_to_check.push(parent.to_path_buf());
            }
        }
    }

    // Sort by depth (deepest first) and try to remove empty directories
    dirs_to_check.sort_by(|a, b| {
        let a_depth = a.components().count();
        let b_depth = b.components().count();
        b_depth.cmp(&a_depth)
    });

    for dir in dirs_to_check {
        let full_path = safe_join(root, &dir);
        if full_path.is_dir()
            && let Ok(mut entries) = fs::read_dir(&full_path)
            && entries.next().is_none()
        {
            // Directory is empty, remove it
            let _ = fs::remove_dir(&full_path);
        }
    }

    Ok(())
}

/// Clean up the transaction working directory
fn cleanup_work_dir(engine: &TransactionEngine, tx_uuid: &str) -> Result<()> {
    let work_dir = engine.txn_work_dir(tx_uuid);
    if work_dir.exists() {
        fs::remove_dir_all(&work_dir)?;
    }
    Ok(())
}

/// Check if a changeset with the given tx_uuid exists in the database
fn check_changeset_by_uuid(conn: &Connection, tx_uuid: &str) -> Result<bool> {
    // First check if the tx_uuid column exists
    let has_column: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('changesets') WHERE name = 'tx_uuid'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !has_column {
        // Column doesn't exist yet, can't have a matching changeset
        return Ok(false);
    }

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM changesets WHERE tx_uuid = ?1",
        [tx_uuid],
        |row| row.get(0),
    )?;

    Ok(count > 0)
}

/// Get the changeset ID for a transaction UUID
fn get_changeset_id_by_uuid(conn: &Connection, tx_uuid: &str) -> Result<i64> {
    // First check if the tx_uuid column exists
    let has_column: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('changesets') WHERE name = 'tx_uuid'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !has_column {
        return Ok(0);
    }

    let id: i64 = conn
        .query_row(
            "SELECT id FROM changesets WHERE tx_uuid = ?1",
            [tx_uuid],
            |row| row.get(0),
        )
        .unwrap_or(0);

    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{TransactionConfig, TransactionEngine};
    use chrono::Utc;
    use tempfile::TempDir;

    fn setup_test_engine() -> (TempDir, TransactionEngine) {
        let temp_dir = TempDir::new().unwrap();
        let config = TransactionConfig::new(
            temp_dir.path().to_path_buf(),
            temp_dir.path().join("conary.db"),
        );
        let engine = TransactionEngine::new(config).unwrap();
        (temp_dir, engine)
    }

    #[test]
    fn test_recover_empty_journal() {
        let (_temp_dir, engine) = setup_test_engine();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();

        // Create empty journal
        let _journal =
            TransactionJournal::create(&engine.config().journal_dir, "empty-test").unwrap();

        let outcomes = recover_all(&engine, &mut conn).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], RecoveryOutcome::Clean { .. }));
    }

    #[test]
    fn test_recover_planned_transaction() {
        let (temp_dir, engine) = setup_test_engine();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();

        // Create journal with only Begin and Plan records
        let mut journal =
            TransactionJournal::create(&engine.config().journal_dir, "planned-test").unwrap();

        journal
            .write_barrier(JournalRecord::Begin {
                tx_uuid: "planned-test".to_string(),
                root: temp_dir.path().to_path_buf(),
                db_path: temp_dir.path().join("conary.db"),
                description: "Test".to_string(),
                timestamp: Utc::now(),
            })
            .unwrap();

        journal
            .write_barrier(JournalRecord::Plan {
                operations: vec![],
                package_name: "test".to_string(),
                package_version: "1.0".to_string(),
                is_upgrade: false,
                old_version: None,
            })
            .unwrap();

        drop(journal);

        let outcomes = recover_all(&engine, &mut conn).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0],
            RecoveryOutcome::RolledBack { ref reason, .. } if reason.contains("Planned")
        ));
    }

    #[test]
    fn test_recover_with_backup_restore() {
        let (temp_dir, engine) = setup_test_engine();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();

        // Create a file that will be "backed up"
        let original_file = temp_dir.path().join("usr/bin/test");
        fs::create_dir_all(original_file.parent().unwrap()).unwrap();
        fs::write(&original_file, "original content").unwrap();

        // Create work directory with backup
        let work_dir = engine.txn_work_dir("backup-test");
        let backup_dir = work_dir.join("backup");
        fs::create_dir_all(&backup_dir.join("usr/bin")).unwrap();

        // "Back up" the file (simulate what transaction would do)
        fs::rename(&original_file, backup_dir.join("usr/bin/test")).unwrap();

        // Create a "new" file in the original location
        fs::write(&original_file, "new content").unwrap();

        // Create journal with backup records
        let mut journal =
            TransactionJournal::create(&engine.config().journal_dir, "backup-test").unwrap();

        journal
            .write_barrier(JournalRecord::Begin {
                tx_uuid: "backup-test".to_string(),
                root: temp_dir.path().to_path_buf(),
                db_path: temp_dir.path().join("conary.db"),
                description: "Test".to_string(),
                timestamp: Utc::now(),
            })
            .unwrap();

        journal
            .write(JournalRecord::Backup {
                path: std::path::PathBuf::from("usr/bin/test"),
                backup_path: backup_dir.join("usr/bin/test"),
                old_type: FileType::Regular,
                old_hash: Some("oldhash".to_string()),
                old_mode: 0o755,
                old_size: 16,
            })
            .unwrap();

        journal
            .write_barrier(JournalRecord::BackupsComplete { count: 1 })
            .unwrap();

        drop(journal);

        // Recover
        let outcomes = recover_all(&engine, &mut conn).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], RecoveryOutcome::RolledBack { .. }));

        // Verify original file was restored
        let content = fs::read_to_string(&original_file).unwrap();
        assert_eq!(content, "original content");
    }

    #[test]
    fn test_rollback_removes_new_files() {
        let (temp_dir, engine) = setup_test_engine();

        // Create a "new" file that was staged
        let new_file = temp_dir.path().join("usr/bin/newfile");
        fs::create_dir_all(new_file.parent().unwrap()).unwrap();
        fs::write(&new_file, "new content").unwrap();

        // Create journal with only stage record (no backup = new file)
        let records = vec![
            JournalRecord::Begin {
                tx_uuid: "rollback-test".to_string(),
                root: temp_dir.path().to_path_buf(),
                db_path: temp_dir.path().join("conary.db"),
                description: "Test".to_string(),
                timestamp: Utc::now(),
            },
            JournalRecord::Stage {
                path: std::path::PathBuf::from("usr/bin/newfile"),
                stage_path: temp_dir.path().join("stage/usr/bin/newfile"),
                new_hash: "newhash".to_string(),
                new_mode: 0o755,
                new_type: FileType::Regular,
            },
        ];

        rollback_transaction(&engine, "rollback-test", &records).unwrap();

        // New file should be removed
        assert!(!new_file.exists());
    }

}
