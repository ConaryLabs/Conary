// conary-core/src/transaction/mod.rs

//! Composefs-native transaction engine.
//!
//! Every transaction follows: resolve -> fetch -> DB commit -> EROFS build -> mount.
//! No journal, no backup phase, no staging. Database is the source of truth.
//! Everything after DB commit is re-derivable from the DB state.
//!
//! # Transaction Lifecycle
//!
//! ```text
//! NEW -> RESOLVED -> FETCHED -> COMMITTED -> BUILT -> MOUNTED -> DONE
//! ```
//!
//! The point of no return is `Committed` — at that point the DB has the new
//! package state. Building the EROFS image and mounting it are idempotent
//! recovery operations that can be retried if they fail.

pub mod planner;

pub use planner::{
    BackupInfo, ConflictInfo, PlannedOperation, StageInfo, TransactionPlan, TransactionPlanner,
};

use crate::Result;
use crate::filesystem::CasStore;
use crate::hash::HashAlgorithm;
use fs2::FileExt;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// Transaction state machine phases.
///
/// The composefs-native lifecycle replaces the old 10-state journal-based
/// state machine. Recovery is simple: if the DB says generation N should be
/// active but the mount does not match, rebuild the EROFS image and remount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionState {
    /// Transaction created, nothing resolved yet
    New,
    /// Dependencies resolved, install plan computed
    Resolved,
    /// Package content fetched into CAS
    Fetched,
    /// Database transaction committed (point of no return)
    Committed,
    /// EROFS image built for the new generation
    Built,
    /// New generation mounted and symlink updated
    Mounted,
    /// Transaction complete
    Done,
}

impl TransactionState {
    /// Check whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::New, Self::Resolved)
                | (Self::Resolved, Self::Fetched)
                | (Self::Fetched, Self::Committed)
                | (Self::Committed, Self::Built)
                | (Self::Built, Self::Mounted)
                | (Self::Mounted, Self::Done)
        )
    }

    /// Returns true if the transaction has not yet committed to the DB.
    /// Before commit, we can simply discard the transaction with no side effects.
    pub fn is_before_commit(&self) -> bool {
        matches!(self, Self::New | Self::Resolved | Self::Fetched)
    }

    /// Returns true if the transaction is past the point of no return.
    /// After commit, recovery means re-deriving the EROFS image and remounting.
    pub fn is_committed(&self) -> bool {
        matches!(
            self,
            Self::Committed | Self::Built | Self::Mounted | Self::Done
        )
    }
}

/// Transaction engine configuration.
///
/// In the composefs-native model, there is no staging directory or journal
/// directory. The CAS (`objects_dir`) stores file content, the database
/// records package state, and `generations_dir` holds EROFS images.
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// Root filesystem path (usually "/")
    pub root: PathBuf,
    /// Path to conary database
    pub db_path: PathBuf,
    /// CAS objects directory
    pub objects_dir: PathBuf,
    /// Directory for EROFS generation images
    pub generations_dir: PathBuf,
    /// Directory for /etc state (three-way merge)
    pub etc_state_dir: PathBuf,
    /// Mount point for the active generation
    pub mount_point: PathBuf,
    /// Hash algorithm for CAS operations
    pub hash_algorithm: HashAlgorithm,
    /// Maximum time to wait for the transaction lock, in seconds
    pub lock_timeout_secs: u64,
}

impl TransactionConfig {
    /// Default lock timeout in seconds (30s)
    pub const DEFAULT_LOCK_TIMEOUT_SECS: u64 = 30;

    /// Create a new config with sensible defaults rooted at the given path.
    ///
    /// Layout:
    /// ```text
    /// {root}/
    ///   conary.db
    ///   objects/          # CAS
    ///   generations/      # EROFS images
    ///   etc-state/        # /etc merge state
    /// ```
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            db_path: root.join("conary.db"),
            objects_dir: root.join("objects"),
            generations_dir: root.join("generations"),
            etc_state_dir: root.join("etc-state"),
            mount_point: PathBuf::from("/"),
            hash_algorithm: HashAlgorithm::Sha256,
            lock_timeout_secs: Self::DEFAULT_LOCK_TIMEOUT_SECS,
        }
    }

    /// Create a config from explicit root and db_path.
    ///
    /// This constructor derives `objects_dir` and `generations_dir` from the
    /// database directory, matching the layout used by `conary system init`.
    pub fn from_paths(root: PathBuf, db_path: PathBuf) -> Self {
        let db_dir = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Self {
            root,
            db_path,
            objects_dir: db_dir.join("objects"),
            generations_dir: db_dir.join("generations"),
            etc_state_dir: db_dir.join("etc-state"),
            mount_point: PathBuf::from("/"),
            hash_algorithm: HashAlgorithm::Sha256,
            lock_timeout_secs: Self::DEFAULT_LOCK_TIMEOUT_SECS,
        }
    }
}

/// The composefs-native transaction engine.
///
/// Replaces the old journal/backup/stage/apply pipeline with:
/// 1. CAS store for content
/// 2. SQLite DB as authoritative package state
/// 3. EROFS image build from DB state
/// 4. composefs mount of the new generation
pub struct TransactionEngine {
    config: TransactionConfig,
    cas: CasStore,
    lock_file: Option<File>,
}

impl TransactionEngine {
    /// Create a new transaction engine, ensuring required directories exist.
    pub fn new(config: TransactionConfig) -> Result<Self> {
        fs::create_dir_all(&config.objects_dir)?;
        fs::create_dir_all(&config.generations_dir)?;
        fs::create_dir_all(&config.etc_state_dir)?;

        let cas = CasStore::with_algorithm(config.objects_dir.clone(), config.hash_algorithm)?;

        Ok(Self {
            config,
            cas,
            lock_file: None,
        })
    }

    /// Get the configuration.
    pub fn config(&self) -> &TransactionConfig {
        &self.config
    }

    /// Get the CAS store.
    pub fn cas(&self) -> &CasStore {
        &self.cas
    }

    /// Acquire the transaction lock.
    ///
    /// Only one transaction can be active at a time. Uses file locking with
    /// exponential backoff up to the configured timeout.
    pub fn begin(&mut self) -> Result<()> {
        let lock_path = self.config.objects_dir.join("conary.lock");
        let lock_file = File::create(&lock_path)?;

        let timeout = std::time::Duration::from_secs(self.config.lock_timeout_secs);
        let start = std::time::Instant::now();
        let mut attempt = 0u32;
        let mut lock_acquired = false;

        loop {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    lock_acquired = true;
                    break;
                }
                Err(_) => {
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        break;
                    }
                    let delay_ms = std::cmp::min(
                        100u64.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX)),
                        2000,
                    );
                    let delay = std::time::Duration::from_millis(delay_ms);
                    let remaining = timeout.saturating_sub(elapsed);
                    std::thread::sleep(std::cmp::min(delay, remaining));
                    attempt += 1;
                }
            }
        }

        if !lock_acquired {
            let waited = start.elapsed();
            return Err(crate::Error::IoError(format!(
                "Failed to acquire transaction lock after {:.1}s (timeout: {}s). \
                 Another conary transaction is likely in progress. \
                 If you are sure no other transaction is running, remove the lock file \
                 at {} and try again.",
                waited.as_secs_f64(),
                self.config.lock_timeout_secs,
                lock_path.display(),
            )));
        }

        self.lock_file = Some(lock_file);
        Ok(())
    }

    /// Release the transaction lock.
    pub fn release_lock(&mut self) {
        if let Some(ref lock) = self.lock_file {
            let _ = lock.unlock();
        }
        self.lock_file = None;
    }

    /// Recover from an interrupted transaction.
    ///
    /// In the composefs-native model, recovery is simple:
    /// - Check what generation the DB says should be active
    /// - If the mounted generation does not match, rebuild the EROFS image
    ///   from the DB state and remount
    ///
    /// This replaces the old journal-based roll-forward/roll-back recovery.
    pub fn recover(&self, conn: &Connection) -> Result<()> {
        use crate::generation::builder::build_generation_from_db;
        use crate::generation::mount::current_generation;

        // Early return if no generations directory or it's empty
        if !self.config.generations_dir.exists() {
            return Ok(());
        }
        let has_entries = self
            .config
            .generations_dir
            .read_dir()
            .map(|mut rd| rd.next().is_some())
            .unwrap_or(false);
        if !has_entries {
            return Ok(());
        }

        // Query the DB for the expected generation number
        let expected_gen: Option<i64> = match conn.query_row(
            "SELECT MAX(state_number) FROM system_states WHERE is_active = 1",
            [],
            |row| row.get(0),
        ) {
            Ok(val) => val,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };

        let Some(expected) = expected_gen else {
            // No active system state in DB — nothing to recover
            return Ok(());
        };

        // Check what is currently mounted
        let current = current_generation(&self.config.root).unwrap_or(None);

        if current == Some(expected) {
            // Already consistent — no recovery needed
            tracing::debug!(
                "Generation {} already mounted, no recovery needed",
                expected
            );
            return Ok(());
        }

        tracing::info!(
            "Recovery: expected generation {} but found {:?}, rebuilding",
            expected,
            current
        );

        // Rebuild EROFS image from DB state
        let (gen_num, _build_result) = build_generation_from_db(
            conn,
            &self.config.generations_dir,
            &format!("Recovery rebuild of generation {expected}"),
        )?;

        let gen_dir = self.config.generations_dir.join(gen_num.to_string());

        // Mount the rebuilt generation
        crate::generation::mount::mount_generation(&crate::generation::mount::MountOptions {
            image_path: gen_dir.join("root.erofs"),
            basedir: self.config.objects_dir.clone(),
            mount_point: self.config.mount_point.clone(),
            verity: false,
            digest: None,
            upperdir: Some(self.config.etc_state_dir.join(gen_num.to_string())),
            workdir: Some(self.config.etc_state_dir.join(format!("{gen_num}-work"))),
        })?;

        // Update the current symlink
        crate::generation::mount::update_current_symlink(&self.config.root, gen_num)?;

        tracing::info!("Recovery: generation {} rebuilt and mounted", gen_num);
        Ok(())
    }
}

impl Drop for TransactionEngine {
    fn drop(&mut self) {
        self.release_lock();
    }
}

// ---------------------------------------------------------------------------
// Compatibility types for CLI consumers (Task 6 will replace these usages)
// ---------------------------------------------------------------------------

/// Information about the package being transacted.
///
/// Preserved for CLI install/batch compatibility. Task 6 will adapt these
/// call sites to the new composefs-native flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub release: Option<String>,
    pub arch: Option<String>,
}

/// Operations to perform in a transaction.
///
/// Preserved for CLI install/batch compatibility.
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

/// A file extracted from a package.
///
/// Used by both the transaction planner and the CLI install commands.
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
}

/// A file to be removed.
#[derive(Debug, Clone)]
pub struct FileToRemove {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mode: u32,
}

/// File type for planning operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

/// Operation type for planning.
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

/// Result of applying filesystem changes.
///
/// Preserved for CLI compatibility. In the composefs-native model, the
/// equivalent information comes from the EROFS build result.
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

/// Result of a completed transaction.
#[derive(Debug, Clone)]
pub struct TransactionResult {
    pub generation_number: i64,
    pub duration_ms: u64,
    pub packages_changed: usize,
}

#[cfg(all(test, feature = "composefs-rs"))]
mod integration_tests {
    use crate::db::models::{FileEntry, SystemState, Trove, TroveType};
    use crate::generation::builder::build_generation_from_db;
    use crate::generation::metadata::GenerationMetadata;
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, rusqlite::Connection) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("db.sqlite3");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open(&db_path).unwrap();
        (tmp, conn)
    }

    #[test]
    fn full_transaction_round_trip() {
        let (tmp, conn) = setup_test_db();
        let root = tmp.path();
        let generations_dir = root.join("generations");
        std::fs::create_dir_all(&generations_dir).unwrap();

        // Insert a mock trove
        let mut trove = Trove::new(
            "hello-world".to_string(),
            "1.0.0-1".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();

        // Insert file entries for the trove
        // Use a 64-char hex hash that the EROFS builder will accept
        let hash = "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd";
        let mut fe = FileEntry::new(
            "/usr/bin/hello".to_string(),
            hash.to_string(),
            1024,
            0o755,
            trove_id,
        );
        fe.insert(&conn).unwrap();

        // Run build_generation_from_db
        let result =
            build_generation_from_db(&conn, &generations_dir, "Full transaction round-trip test");
        assert!(
            result.is_ok(),
            "build_generation_from_db failed: {:?}",
            result.err()
        );
        let (gen_num, build_result) = result.unwrap();

        // Verify generation number (first generation is 0 per next_state_number logic)
        assert_eq!(gen_num, 0);

        // Verify EROFS image exists and is non-empty
        assert!(
            build_result.image_path.exists(),
            "EROFS image must exist at {:?}",
            build_result.image_path
        );
        assert!(build_result.image_size > 0, "EROFS image must be non-empty");

        // Verify at least one CAS object was referenced
        assert_eq!(
            build_result.cas_objects_referenced, 1,
            "Should reference 1 CAS object for the hello binary"
        );

        // Verify metadata JSON was written
        let gen_dir = generations_dir.join(gen_num.to_string());
        let meta_path = gen_dir.join(".conary-gen.json");
        assert!(
            meta_path.exists(),
            "Metadata file must exist at {:?}",
            meta_path
        );
        let meta_json = std::fs::read_to_string(&meta_path).unwrap();
        let meta: GenerationMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.generation, gen_num);
        assert_eq!(meta.format, "composefs");
        assert_eq!(meta.package_count, 1);

        // Verify SystemState was created and is active
        let active_state = SystemState::get_active(&conn).unwrap();
        assert!(
            active_state.is_some(),
            "An active SystemState must exist after build_generation_from_db"
        );
        let active_state = active_state.unwrap();
        assert_eq!(active_state.state_number, gen_num);
        assert_eq!(active_state.package_count, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn transaction_config_new_defaults() {
        let config = TransactionConfig::new(Path::new("/conary"));
        assert_eq!(config.root, PathBuf::from("/conary"));
        assert_eq!(config.db_path, PathBuf::from("/conary/conary.db"));
        assert_eq!(config.objects_dir, PathBuf::from("/conary/objects"));
        assert_eq!(config.generations_dir, PathBuf::from("/conary/generations"));
        assert_eq!(config.etc_state_dir, PathBuf::from("/conary/etc-state"));
        assert_eq!(config.mount_point, PathBuf::from("/"));
        assert_eq!(
            config.lock_timeout_secs,
            TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS
        );
    }

    #[test]
    fn transaction_config_from_paths() {
        let config = TransactionConfig::from_paths(
            PathBuf::from("/"),
            PathBuf::from("/var/lib/conary/conary.db"),
        );
        assert_eq!(config.root, PathBuf::from("/"));
        assert_eq!(config.objects_dir, PathBuf::from("/var/lib/conary/objects"));
        assert_eq!(
            config.generations_dir,
            PathBuf::from("/var/lib/conary/generations")
        );
    }

    #[test]
    fn state_valid_transitions() {
        assert!(TransactionState::New.can_transition_to(&TransactionState::Resolved));
        assert!(TransactionState::Resolved.can_transition_to(&TransactionState::Fetched));
        assert!(TransactionState::Fetched.can_transition_to(&TransactionState::Committed));
        assert!(TransactionState::Committed.can_transition_to(&TransactionState::Built));
        assert!(TransactionState::Built.can_transition_to(&TransactionState::Mounted));
        assert!(TransactionState::Mounted.can_transition_to(&TransactionState::Done));
    }

    #[test]
    fn state_invalid_transitions() {
        assert!(!TransactionState::New.can_transition_to(&TransactionState::Built));
        assert!(!TransactionState::New.can_transition_to(&TransactionState::Committed));
        assert!(!TransactionState::Fetched.can_transition_to(&TransactionState::Mounted));
        assert!(!TransactionState::Done.can_transition_to(&TransactionState::New));
    }

    #[test]
    fn state_before_commit() {
        assert!(TransactionState::New.is_before_commit());
        assert!(TransactionState::Resolved.is_before_commit());
        assert!(TransactionState::Fetched.is_before_commit());
        assert!(!TransactionState::Committed.is_before_commit());
        assert!(!TransactionState::Built.is_before_commit());
        assert!(!TransactionState::Done.is_before_commit());
    }

    #[test]
    fn state_is_committed() {
        assert!(!TransactionState::New.is_committed());
        assert!(!TransactionState::Fetched.is_committed());
        assert!(TransactionState::Committed.is_committed());
        assert!(TransactionState::Built.is_committed());
        assert!(TransactionState::Mounted.is_committed());
        assert!(TransactionState::Done.is_committed());
    }

    #[test]
    fn engine_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = TransactionConfig::new(temp_dir.path());
        let engine = TransactionEngine::new(config).unwrap();

        assert!(engine.config.objects_dir.exists());
        assert!(engine.config.generations_dir.exists());
        assert!(engine.config.etc_state_dir.exists());
    }

    #[test]
    fn engine_begin_and_release_lock() {
        let temp_dir = TempDir::new().unwrap();
        let config = TransactionConfig::new(temp_dir.path());
        let mut engine = TransactionEngine::new(config).unwrap();

        engine.begin().unwrap();
        assert!(engine.lock_file.is_some());

        engine.release_lock();
        assert!(engine.lock_file.is_none());
    }

    #[test]
    fn fs_apply_result_total() {
        let result = FsApplyResult {
            files_added: 5,
            files_replaced: 3,
            files_removed: 2,
            dirs_created: 1,
            dirs_removed: 0,
        };
        assert_eq!(result.total_operations(), 11);
    }
}
