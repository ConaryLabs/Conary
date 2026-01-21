// src/transaction/mod.rs

//! Transaction engine for atomic package operations
//!
//! This module provides crash-safe, atomic semantics for package install/upgrade/remove
//! operations. Key features:
//!
//! - **Journal-based recovery**: Append-only log enables roll-forward or roll-back after crash
//! - **Backup-before-overwrite**: Original files preserved until DB commits
//! - **VFS preflight planning**: Conflict detection before any filesystem changes
//! - **State machine**: Clear phases with deterministic recovery at each point
//!
//! # Transaction Lifecycle
//!
//! ```text
//! NEW -> PLANNED -> PREPARED -> PRE_SCRIPTS -> BACKED_UP -> STAGED -> FS_APPLIED -> DB_APPLIED -> POST_SCRIPTS -> DONE
//!                                                                                       ^
//!                                                                     Point of no return (roll-forward after)
//! ```

mod journal;
mod planner;
mod recovery;

pub use journal::{JournalRecord, TransactionJournal};
pub use planner::{
    BackupInfo, ConflictInfo, PlannedOperation, StageInfo, TransactionPlan, TransactionPlanner,
};
pub use recovery::RecoveryOutcome;

use crate::db::paths::objects_dir;
use crate::filesystem::path::safe_join;
use crate::filesystem::{CasStore, FileDeployer};
use crate::hash::HashAlgorithm;
use crate::progress::ProgressTracker;
use crate::Result;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// Move a file atomically, falling back to copy+sync+delete for cross-filesystem moves.
///
/// This handles the EXDEV error that occurs when source and destination are on
/// different filesystems (e.g., staging on /var and target on /usr).
///
/// # Arguments
/// * `src` - Source file path
/// * `dst` - Destination file path
///
/// # Safety
/// Uses fsync to ensure data durability before removing source file.
pub(crate) fn move_file_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Cross-filesystem: copy + fsync + delete
            log::debug!(
                "Cross-filesystem move detected ({} -> {}), using copy fallback",
                src.display(),
                dst.display()
            );

            // Copy file content
            fs::copy(src, dst)?;

            // fsync the destination file to ensure data is on disk
            let file = File::open(dst)?;
            file.sync_all()?;
            drop(file);

            // fsync the parent directory to ensure directory entry is persisted
            if let Some(parent) = dst.parent() && let Ok(dir) = File::open(parent) {
                // Ignore errors from fsync on directory - not all filesystems support it
                let _ = dir.sync_all();
            }

            // Now safe to remove source
            fs::remove_file(src)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Transaction engine configuration
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// Root filesystem path (usually "/")
    pub root: PathBuf,
    /// Path to conary database
    pub db_path: PathBuf,
    /// Transaction working directory for backup/stage operations
    pub txn_dir: PathBuf,
    /// Directory for transaction journals
    pub journal_dir: PathBuf,
    /// Hash algorithm for CAS operations
    pub hash_algorithm: HashAlgorithm,
    /// Whether to preserve old file content in CAS for long-term rollback
    pub preserve_old_content: bool,
}

impl TransactionConfig {
    /// Create a new config with sensible defaults based on db_path
    pub fn new(root: PathBuf, db_path: PathBuf) -> Self {
        let db_dir = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Self {
            root,
            db_path,
            txn_dir: db_dir.join("txn"),
            journal_dir: db_dir.join("journal"),
            hash_algorithm: HashAlgorithm::Sha256,
            preserve_old_content: true,
        }
    }
}

/// Transaction state machine phases
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionState {
    /// Transaction created, no changes made
    New,
    /// Plan computed, conflicts checked
    Planned,
    /// Content prepared in CAS
    Prepared,
    /// Pre-install scriptlets executed
    PreScriptsComplete,
    /// Original files backed up
    BackedUp,
    /// New files staged from CAS
    Staged,
    /// Filesystem changes applied (atomic renames)
    FsApplied,
    /// Database transaction committed - POINT OF NO RETURN
    DbApplied,
    /// Post-install scriptlets and triggers executed
    PostScriptsComplete,
    /// Transaction complete, cleanup done
    Done,
    /// Transaction aborted (rolled back)
    Aborted,
    /// Transaction failed (may need recovery)
    Failed,
}

impl TransactionState {
    /// Returns true if this state is before the point of no return
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::New
                | Self::Planned
                | Self::Prepared
                | Self::PreScriptsComplete
                | Self::BackedUp
                | Self::Staged
                | Self::FsApplied
        )
    }

    /// Returns true if transaction should roll forward on recovery
    pub fn should_roll_forward(&self) -> bool {
        matches!(
            self,
            Self::DbApplied | Self::PostScriptsComplete | Self::Done
        )
    }
}

/// Information about the package being transacted
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub release: Option<String>,
    pub arch: Option<String>,
}

/// Operations to perform in a transaction
#[derive(Debug, Clone)]
pub struct TransactionOperations {
    /// Package being installed/upgraded
    pub package: PackageInfo,
    /// Files to add (from extracted package)
    pub files_to_add: Vec<ExtractedFile>,
    /// Files to remove (from old version during upgrade)
    pub files_to_remove: Vec<FileToRemove>,
    /// Whether this is an upgrade of an existing package
    pub is_upgrade: bool,
    /// Old package info (for upgrades)
    pub old_package: Option<PackageInfo>,
}

/// A file extracted from a package
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
}

/// A file to be removed
#[derive(Debug, Clone)]
pub struct FileToRemove {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mode: u32,
}

/// Result of applying filesystem changes
#[derive(Debug, Clone)]
pub struct FsApplyResult {
    pub files_added: usize,
    pub files_replaced: usize,
    pub files_removed: usize,
    pub dirs_created: usize,
    pub dirs_removed: usize,
}

impl FsApplyResult {
    pub fn total_operations(&self) -> usize {
        self.files_added
            + self.files_replaced
            + self.files_removed
            + self.dirs_created
            + self.dirs_removed
    }
}

/// Result of a completed transaction
#[derive(Debug, Clone)]
pub struct TransactionResult {
    pub tx_uuid: String,
    pub changeset_id: i64,
    pub duration_ms: u64,
    pub fs_result: FsApplyResult,
}

/// File type for backup/restore operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

/// Operation type for planning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    Mkdir,
    AddFile,
    ReplaceFile,
    RemoveFile,
    AddSymlink,
    ReplaceSymlink,
    RemoveSymlink,
    Rmdir,
}

/// The main transaction engine
pub struct TransactionEngine {
    config: TransactionConfig,
    cas: CasStore,
    #[allow(dead_code)]
    deployer: FileDeployer,
}

impl TransactionEngine {
    /// Create a new transaction engine
    pub fn new(config: TransactionConfig) -> Result<Self> {
        // Ensure directories exist
        fs::create_dir_all(&config.txn_dir)?;
        fs::create_dir_all(&config.journal_dir)?;
        fs::create_dir_all(config.journal_dir.join("archive"))?;

        // Create CAS store
        let cas_dir = objects_dir(&config.db_path.to_string_lossy());
        let cas = CasStore::with_algorithm(cas_dir, config.hash_algorithm)?;
        let deployer = FileDeployer::with_cas(cas.clone(), &config.root)?;

        Ok(Self {
            config,
            cas,
            deployer,
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &TransactionConfig {
        &self.config
    }

    /// Get the CAS store
    pub fn cas(&self) -> &CasStore {
        &self.cas
    }

    /// Recover any incomplete transactions from previous runs
    pub fn recover(&self, conn: &mut Connection) -> Result<Vec<RecoveryOutcome>> {
        recovery::recover_all(self, conn)
    }

    /// Begin a new transaction
    pub fn begin(&self, description: &str) -> Result<Transaction<'_>> {
        let tx_uuid = Uuid::new_v4().to_string();

        // Acquire exclusive lock with retry logic
        let lock_path = self.config.txn_dir.join("conary.lock");
        let lock_file = File::create(&lock_path)?;

        // Retry lock acquisition with exponential backoff
        // Tries: 0ms, 100ms, 200ms, 400ms, 800ms (total ~1.5s wait)
        const MAX_RETRIES: u32 = 5;
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES - 1 {
                        let delay = std::time::Duration::from_millis(100 * (1 << attempt));
                        std::thread::sleep(delay);
                    }
                }
            }
        }

        if let Some(e) = last_error {
            return Err(crate::Error::IoError(format!(
                "Failed to acquire transaction lock after {} retries. \
                 Another transaction may be in progress or a previous transaction \
                 crashed without releasing the lock. Error: {}",
                MAX_RETRIES, e
            )));
        }

        // Create transaction working directory
        let txn_work_dir = self.config.txn_dir.join(&tx_uuid);
        fs::create_dir_all(txn_work_dir.join("backup"))?;
        fs::create_dir_all(txn_work_dir.join("stage"))?;

        // Create journal
        let journal = TransactionJournal::create(&self.config.journal_dir, &tx_uuid)?;

        let mut txn = Transaction {
            engine: self,
            uuid: tx_uuid.clone(),
            journal,
            state: TransactionState::New,
            plan: None,
            start_time: Utc::now(),
            description: description.to_string(),
            lock_file: Some(lock_file),
            options: TransactionOptions::default(),
        };

        // Write begin record
        txn.journal.write_barrier(JournalRecord::Begin {
            tx_uuid,
            root: self.config.root.clone(),
            db_path: self.config.db_path.clone(),
            description: description.to_string(),
            timestamp: Utc::now(),
        })?;

        Ok(txn)
    }

    /// Get the working directory for a transaction
    pub fn txn_work_dir(&self, tx_uuid: &str) -> PathBuf {
        self.config.txn_dir.join(tx_uuid)
    }
}

/// Options for controlling transaction execution
#[derive(Default)]
pub struct TransactionOptions {
    /// Cancel token - set to true to request cancellation
    pub cancel: Option<Arc<AtomicBool>>,
    /// Progress tracker for reporting operation progress
    pub progress: Option<Arc<dyn ProgressTracker>>,
}

impl TransactionOptions {
    /// Create new transaction options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the cancel token
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Set the progress tracker
    pub fn with_progress(mut self, progress: Arc<dyn ProgressTracker>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Check if cancellation has been requested
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Return Cancelled error if cancellation requested
    fn check_cancelled(&self, operation: &str) -> crate::Result<()> {
        if self.is_cancelled() {
            Err(crate::Error::Cancelled(operation.to_string()))
        } else {
            Ok(())
        }
    }

    /// Report progress if a tracker is available
    fn report_progress(&self, current: u64, total: u64, message: &str) {
        if let Some(ref progress) = self.progress {
            progress.set_position(current);
            progress.set_length(total);
            progress.set_message(message);
        }
    }
}

/// Represents an active transaction
pub struct Transaction<'a> {
    engine: &'a TransactionEngine,
    uuid: String,
    journal: TransactionJournal,
    state: TransactionState,
    plan: Option<TransactionPlan>,
    start_time: DateTime<Utc>,
    #[allow(dead_code)]
    description: String,
    lock_file: Option<File>,
    /// Options for controlling execution (cancel, progress)
    options: TransactionOptions,
}

impl<'a> Transaction<'a> {
    /// Get the transaction UUID
    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// Get the current state
    pub fn state(&self) -> TransactionState {
        self.state
    }

    /// Get the transaction plan (if planned)
    pub fn plan(&self) -> Option<&TransactionPlan> {
        self.plan.as_ref()
    }

    /// Set execution options (cancel token, progress tracker)
    pub fn set_options(&mut self, options: TransactionOptions) {
        self.options = options;
    }

    /// Set the cancel token for this transaction
    pub fn set_cancel_token(&mut self, cancel: Arc<AtomicBool>) {
        self.options.cancel = Some(cancel);
    }

    /// Set the progress tracker for this transaction
    pub fn set_progress(&mut self, progress: Arc<dyn ProgressTracker>) {
        self.options.progress = Some(progress);
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.options.is_cancelled()
    }

    /// Plan the transaction using VfsTree for conflict detection
    pub fn plan_operations(
        &mut self,
        operations: TransactionOperations,
        conn: &Connection,
    ) -> Result<&TransactionPlan> {
        if self.state != TransactionState::New {
            return Err(crate::Error::IoError(format!(
                "Cannot plan transaction in state {:?}",
                self.state
            )));
        }

        let mut planner =
            TransactionPlanner::new(conn, &self.engine.config.root, self.engine.cas());

        let plan = planner.plan_install(
            &operations.files_to_add,
            &operations.files_to_remove,
            &operations.package.name,
            operations.is_upgrade,
        )?;

        // Write plan to journal
        self.journal.write_barrier(JournalRecord::Plan {
            operations: plan.operations.clone(),
            package_name: operations.package.name.clone(),
            package_version: operations.package.version.clone(),
            is_upgrade: operations.is_upgrade,
            old_version: operations.old_package.map(|p| p.version),
        })?;

        self.plan = Some(plan);
        self.state = TransactionState::Planned;

        self.plan.as_ref().ok_or_else(|| {
            crate::Error::TransactionError("Failed to store plan in transaction".to_string())
        })
    }

    /// Prepare content in CAS (store all new file content)
    pub fn prepare(&mut self, files: &[ExtractedFile]) -> Result<()> {
        if self.state != TransactionState::Planned {
            return Err(crate::Error::IoError(format!(
                "Cannot prepare transaction in state {:?}",
                self.state
            )));
        }

        let mut total_bytes = 0u64;
        for file in files {
            if !file.is_symlink {
                self.engine.cas.store(&file.content)?;
                total_bytes += file.content.len() as u64;
            }
        }

        self.journal.write_barrier(JournalRecord::Prepared {
            files_in_cas: files.len(),
            total_bytes,
        })?;

        self.state = TransactionState::Prepared;
        Ok(())
    }

    /// Execute filesystem backup phase
    pub fn backup_files(&mut self) -> Result<()> {
        let plan = self.plan.as_ref().ok_or_else(|| {
            crate::Error::IoError("Transaction not planned".to_string())
        })?;

        let backup_dir = self.engine.txn_work_dir(&self.uuid).join("backup");
        let total = plan.files_to_backup.len() as u64;

        for (i, backup_info) in plan.files_to_backup.iter().enumerate() {
            // Check for cancellation
            self.options.check_cancelled("backup")?;

            // Report progress
            self.options.report_progress(
                i as u64,
                total,
                &format!("Backing up {}", backup_info.path.display()),
            );

            let source = safe_join(&self.engine.config.root, &backup_info.path)?;
            let backup_path = backup_dir.join(backup_info.path.strip_prefix("/").unwrap_or(&backup_info.path));

            // Create parent directories in backup area
            if let Some(parent) = backup_path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Handle different file types
            let metadata = fs::symlink_metadata(&source)?;
            if metadata.is_symlink() {
                // Backup symlink target
                let target = fs::read_link(&source)?;
                // Store symlink info as a special file
                let symlink_info = format!("SYMLINK:{}", target.display());
                fs::write(&backup_path, symlink_info)?;
            } else if metadata.is_dir() {
                // For directories, we just note they exist
                fs::create_dir_all(&backup_path)?;
            } else {
                // Regular file - move to backup location (handles cross-FS)
                move_file_atomic(&source, &backup_path)?;
            }

            self.journal.write(JournalRecord::Backup {
                path: backup_info.path.clone(),
                backup_path: backup_path.clone(),
                old_type: backup_info.file_type,
                old_hash: backup_info.current_hash.clone(),
                old_mode: backup_info.mode,
                old_size: backup_info.size,
            })?;
        }

        self.journal.write_barrier(JournalRecord::BackupsComplete {
            count: plan.files_to_backup.len(),
        })?;

        self.state = TransactionState::BackedUp;
        Ok(())
    }

    /// Stage files from CAS to staging directory
    pub fn stage_files(&mut self) -> Result<()> {
        let plan = self.plan.as_ref().ok_or_else(|| {
            crate::Error::IoError("Transaction not planned".to_string())
        })?;

        let stage_dir = self.engine.txn_work_dir(&self.uuid).join("stage");
        let total = plan.files_to_stage.len() as u64;

        for (i, stage_info) in plan.files_to_stage.iter().enumerate() {
            // Check for cancellation
            self.options.check_cancelled("stage")?;

            // Report progress
            self.options.report_progress(
                i as u64,
                total,
                &format!("Staging {}", stage_info.path.display()),
            );

            let relative_path = stage_info.path.strip_prefix("/").unwrap_or(&stage_info.path);
            let stage_path = stage_dir.join(relative_path);

            // Create parent directories
            if let Some(parent) = stage_path.parent() {
                fs::create_dir_all(parent)?;
            }

            if let Some(ref target) = stage_info.symlink_target {
                // Create symlink in stage area
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &stage_path)?;
                #[cfg(not(unix))]
                fs::write(&stage_path, format!("SYMLINK:{}", target.display()))?;
            } else {
                // Try hardlink from CAS, fall back to copy
                let cas_path = self.engine.cas.hash_to_path(&stage_info.hash);
                if fs::hard_link(&cas_path, &stage_path).is_err() {
                    let content = self.engine.cas.retrieve(&stage_info.hash)?;
                    fs::write(&stage_path, content)?;
                }

                // Set permissions
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = fs::Permissions::from_mode(stage_info.mode);
                    fs::set_permissions(&stage_path, perms)?;
                }
            }

            self.journal.write(JournalRecord::Stage {
                path: stage_info.path.clone(),
                stage_path: stage_path.clone(),
                new_hash: stage_info.hash.clone(),
                new_mode: stage_info.mode,
                new_type: stage_info.file_type,
            })?;
        }

        self.journal.write_barrier(JournalRecord::StagingComplete {
            count: plan.files_to_stage.len(),
        })?;

        self.state = TransactionState::Staged;
        Ok(())
    }

    /// Apply filesystem changes (atomic renames from stage to final)
    pub fn apply_filesystem(&mut self) -> Result<FsApplyResult> {
        let plan = self.plan.as_ref().ok_or_else(|| {
            crate::Error::IoError("Transaction not planned".to_string())
        })?;

        let stage_dir = self.engine.txn_work_dir(&self.uuid).join("stage");
        let mut result = FsApplyResult {
            files_added: 0,
            files_replaced: 0,
            files_removed: 0,
            dirs_created: 0,
            dirs_removed: 0,
        };

        // Calculate total operations for progress tracking
        let total_ops = (plan.dirs_to_create.len()
            + plan.files_to_stage.len()
            + plan.operations.iter().filter(|op| {
                matches!(op.op_type, OperationType::RemoveFile | OperationType::RemoveSymlink)
            }).count()
            + plan.dirs_to_remove.len()) as u64;
        let mut current_op = 0u64;

        // Create directories first (parent to child order)
        for dir in &plan.dirs_to_create {
            self.options.check_cancelled("apply")?;
            self.options.report_progress(current_op, total_ops, &format!("Creating {}", dir.display()));
            current_op += 1;

            let target = safe_join(&self.engine.config.root, dir)?;
            fs::create_dir_all(&target)?;
            result.dirs_created += 1;
        }

        // Move staged files to final locations
        for stage_info in &plan.files_to_stage {
            self.options.check_cancelled("apply")?;
            self.options.report_progress(
                current_op,
                total_ops,
                &format!("Installing {}", stage_info.path.display()),
            );
            current_op += 1;
            let relative_path = stage_info.path.strip_prefix("/").unwrap_or(&stage_info.path);
            let stage_path = stage_dir.join(relative_path);
            let target = safe_join(&self.engine.config.root, &stage_info.path)?;

            // Ensure parent exists
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }

            // Move from stage to final location (handles cross-FS)
            move_file_atomic(&stage_path, &target)?;

            // Count as add or replace based on whether we backed it up
            let was_replaced = plan
                .files_to_backup
                .iter()
                .any(|b| b.path == stage_info.path);
            if was_replaced {
                result.files_replaced += 1;
            } else {
                result.files_added += 1;
            }
        }

        // Remove files that should be deleted (upgrade case)
        for op in &plan.operations {
            if matches!(
                op.op_type,
                OperationType::RemoveFile | OperationType::RemoveSymlink
            ) {
                self.options.check_cancelled("apply")?;
                self.options.report_progress(
                    current_op,
                    total_ops,
                    &format!("Removing {}", op.path.display()),
                );
                current_op += 1;

                let target = safe_join(&self.engine.config.root, &op.path)?;
                if target.exists() || target.symlink_metadata().is_ok() {
                    fs::remove_file(&target)?;
                    result.files_removed += 1;
                }
            }
        }

        // Remove empty directories (child to parent order)
        for dir in plan.dirs_to_remove.iter().rev() {
            self.options.check_cancelled("apply")?;
            self.options.report_progress(current_op, total_ops, &format!("Cleanup {}", dir.display()));
            current_op += 1;

            let target = safe_join(&self.engine.config.root, dir)?;
            if target.is_dir() && fs::read_dir(&target)?.next().is_none() {
                fs::remove_dir(&target)?;
                result.dirs_removed += 1;
            }
        }

        self.journal.write_barrier(JournalRecord::FsApplied {
            files_added: result.files_added,
            files_replaced: result.files_replaced,
            files_removed: result.files_removed,
            dirs_created: result.dirs_created,
        })?;

        self.state = TransactionState::FsApplied;
        Ok(result)
    }

    /// Write DB commit intent (for recovery correlation)
    pub fn write_db_commit_intent(&mut self) -> Result<()> {
        self.journal.write_barrier(JournalRecord::DbCommitIntent {
            tx_uuid: self.uuid.clone(),
        })?;
        Ok(())
    }

    /// Record successful DB commit
    pub fn record_db_commit(&mut self, changeset_id: i64, trove_id: i64) -> Result<()> {
        self.journal.write_barrier(JournalRecord::DbApplied {
            changeset_id,
            trove_id,
        })?;
        self.state = TransactionState::DbApplied;
        Ok(())
    }

    /// Mark post-scripts complete
    pub fn mark_post_scripts_complete(&mut self) -> Result<()> {
        self.state = TransactionState::PostScriptsComplete;
        Ok(())
    }

    /// Finish the transaction (cleanup and finalize)
    pub fn finish(mut self) -> Result<TransactionResult> {
        let duration = Utc::now()
            .signed_duration_since(self.start_time)
            .num_milliseconds() as u64;

        // Clean up working directory
        let work_dir = self.engine.txn_work_dir(&self.uuid);
        if work_dir.exists() {
            fs::remove_dir_all(&work_dir)?;
        }

        // Archive journal
        self.journal.write_barrier(JournalRecord::Done {
            duration_ms: duration,
            success: true,
        })?;

        // Take ownership of journal for archiving
        let journal = std::mem::replace(
            &mut self.journal,
            TransactionJournal::create_placeholder()?,
        );
        journal.archive()?;

        self.state = TransactionState::Done;

        // Release lock
        if let Some(lock) = self.lock_file.take() {
            lock.unlock()?;
        }

        Ok(TransactionResult {
            tx_uuid: self.uuid.clone(),
            changeset_id: 0, // Should be set from DB commit
            duration_ms: duration,
            fs_result: FsApplyResult {
                files_added: 0,
                files_replaced: 0,
                files_removed: 0,
                dirs_created: 0,
                dirs_removed: 0,
            },
        })
    }

    /// Abort the transaction (rollback all changes)
    pub fn abort(mut self) -> Result<()> {
        // Read records before replacing journal
        let records = self.journal.read_all()?;

        // Perform rollback based on current state
        recovery::rollback_transaction(self.engine, &self.uuid, &records)?;

        // Clean up working directory
        let work_dir = self.engine.txn_work_dir(&self.uuid);
        if work_dir.exists() {
            fs::remove_dir_all(&work_dir)?;
        }

        // Take ownership of journal for deletion
        let journal = std::mem::replace(
            &mut self.journal,
            TransactionJournal::create_placeholder()?,
        );
        journal.delete()?;

        self.state = TransactionState::Aborted;

        // Release lock
        if let Some(lock) = self.lock_file.take() {
            lock.unlock()?;
        }

        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // Release lock if still held
        if let Some(ref lock) = self.lock_file {
            let _ = lock.unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_transaction_config_defaults() {
        let config = TransactionConfig::new(
            PathBuf::from("/"),
            PathBuf::from("/var/lib/conary/conary.db"),
        );

        assert_eq!(config.root, PathBuf::from("/"));
        assert_eq!(
            config.txn_dir,
            PathBuf::from("/var/lib/conary/txn")
        );
        assert_eq!(
            config.journal_dir,
            PathBuf::from("/var/lib/conary/journal")
        );
    }

    #[test]
    fn test_transaction_state_recovery() {
        assert!(TransactionState::New.is_recoverable());
        assert!(TransactionState::Planned.is_recoverable());
        assert!(TransactionState::FsApplied.is_recoverable());
        assert!(!TransactionState::DbApplied.is_recoverable());
        assert!(!TransactionState::Done.is_recoverable());
    }

    #[test]
    fn test_transaction_state_roll_forward() {
        assert!(!TransactionState::FsApplied.should_roll_forward());
        assert!(TransactionState::DbApplied.should_roll_forward());
        assert!(TransactionState::PostScriptsComplete.should_roll_forward());
        assert!(TransactionState::Done.should_roll_forward());
    }

    #[test]
    fn test_engine_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("conary.db");

        let config = TransactionConfig::new(temp_dir.path().to_path_buf(), db_path);

        let engine = TransactionEngine::new(config).unwrap();

        assert!(engine.config.txn_dir.exists());
        assert!(engine.config.journal_dir.exists());
    }

    #[test]
    fn test_begin_transaction() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("conary.db");

        let config = TransactionConfig::new(temp_dir.path().to_path_buf(), db_path);
        let engine = TransactionEngine::new(config).unwrap();

        let txn = engine.begin("Test transaction").unwrap();

        assert_eq!(txn.state(), TransactionState::New);
        assert!(!txn.uuid().is_empty());

        // Work directory should exist
        let work_dir = engine.txn_work_dir(txn.uuid());
        assert!(work_dir.join("backup").exists());
        assert!(work_dir.join("stage").exists());
    }

    #[test]
    fn test_fs_apply_result() {
        let result = FsApplyResult {
            files_added: 5,
            files_replaced: 3,
            files_removed: 2,
            dirs_created: 1,
            dirs_removed: 0,
        };

        assert_eq!(result.total_operations(), 11);
    }

    #[test]
    fn test_move_file_atomic_same_fs() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("source.txt");
        let dst = temp_dir.path().join("dest.txt");

        fs::write(&src, "test content").unwrap();
        assert!(src.exists());

        move_file_atomic(&src, &dst).unwrap();

        assert!(!src.exists());
        assert!(dst.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "test content");
    }

    #[test]
    fn test_move_file_atomic_preserves_content() {
        let temp_dir = TempDir::new().unwrap();
        let src = temp_dir.path().join("binary_file");
        let dst = temp_dir.path().join("moved_binary");

        // Create a file with binary content
        let content: Vec<u8> = (0..=255).collect();
        fs::write(&src, &content).unwrap();

        move_file_atomic(&src, &dst).unwrap();

        assert!(!src.exists());
        let read_content = fs::read(&dst).unwrap();
        assert_eq!(read_content, content);
    }
}
