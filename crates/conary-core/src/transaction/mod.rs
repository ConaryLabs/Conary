// conary-core/src/transaction/mod.rs

//! Composefs-native transaction engine.
//!
//! Package state and exact selected-root publication authority commit together.
//! Payload-only bootstrap paths can derive a root manifest from installed rows;
//! lifecycle-bearing mutations persist the cumulative selected-root manifest
//! before the database commit. EROFS build and selection are retryable from
//! that typed authority.
//!
//! # Transaction Lifecycle
//!
//! ```text
//! NEW -> RESOLVED -> FETCHED -> [root authority durable] -> COMMITTED -> BUILT -> SELECTED -> DONE
//! ```
//!
//! The point of no return is `Committed`: at that point the DB has the new
//! package state and any selected-root publication snapshot is already
//! durable. Building the EROFS image and selecting it are idempotent recovery
//! operations.

pub mod package_relations;
mod recovery;

use crate::Result;
use crate::filesystem::CasStore;
use crate::hash::HashAlgorithm;
use crate::runtime_root::ConaryRuntimeRoot;
use fs2::FileExt;
pub use package_relations::{
    IncomingPackageRelations, PackageRelationDeconfiguration, PackageRelationDeconfigurationCause,
    PackageRelationIncomingIdentity, PackageRelationInstalledIdentity, PackageRelationPlan,
    PackageRelationRemoval, plan_package_relation_batch_facts, plan_package_relation_facts,
    plan_package_relations, validate_package_relation_plan, validate_package_relation_removals,
    validate_package_relation_transitions,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// Transaction state machine phases.
///
/// The composefs-native lifecycle replaces the old 10-state journal-based
/// state machine. Recovery is simple: if `/conary/current` points to generation
/// N but the artifact is missing or invalid, rebuild the EROFS image and repair
/// the selected boot generation.
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
    /// New generation artifact built and selected via /conary/current
    Selected,
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
                | (Self::Built, Self::Selected)
                | (Self::Selected, Self::Done)
        )
    }

    /// Returns true if the transaction has not yet committed to the DB.
    /// Before commit, we can simply discard the transaction with no side effects.
    pub fn is_before_commit(&self) -> bool {
        matches!(self, Self::New | Self::Resolved | Self::Fetched)
    }

    /// Returns true if the transaction is past the point of no return.
    /// After commit, recovery means re-deriving the EROFS image and repairing
    /// the selected boot generation.
    pub fn is_committed(&self) -> bool {
        matches!(
            self,
            Self::Committed | Self::Built | Self::Selected | Self::Done
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
    /// This constructor derives runtime generation state from the canonical
    /// runtime root. The default DB lives under `/var/lib/conary`, but CAS,
    /// generation, mount, and `/etc` state live under `/conary`.
    pub fn from_paths(_root: PathBuf, db_path: PathBuf) -> Self {
        let runtime_root = ConaryRuntimeRoot::from_db_path(db_path);
        Self::for_runtime_root(&runtime_root)
    }

    /// Create a config for one explicit runtime root.
    ///
    /// Selected-root and try-session transactions use this constructor so the
    /// global mutation lock and CAS always belong to the live runtime even when
    /// package state is read from a copied database.
    pub fn for_runtime_root(runtime_root: &ConaryRuntimeRoot) -> Self {
        Self {
            root: runtime_root.root().to_path_buf(),
            db_path: runtime_root.db_path().to_path_buf(),
            objects_dir: runtime_root.objects_dir(),
            generations_dir: runtime_root.generations_dir(),
            etc_state_dir: runtime_root.etc_state_dir(),
            mount_point: runtime_root.mount_dir(),
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
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;

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
                 Wait for the active transaction to finish and then retry. \
                 Lock path: {}",
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
    use crate::db::models::{SystemState, Trove, TroveType};
    use crate::filesystem::CasStore;
    use crate::generation::builder::build_generation_from_db_with_boot_root;
    use crate::generation::builder::test_support::{
        insert_regular_file_with_parents, persist_test_host_capabilities,
    };
    use crate::generation::metadata::{
        GENERATION_FORMAT, GENERATION_METADATA_FILE, GenerationMetadata,
    };
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, rusqlite::Connection) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("db.sqlite3");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open(&db_path).unwrap();
        persist_test_host_capabilities(&conn);
        (tmp, conn)
    }

    #[test]
    fn full_transaction_round_trip() {
        let (tmp, conn) = setup_test_db();
        let root = tmp.path();
        let generations_dir = root.join("generations");
        let objects_dir = root.join("objects");
        let boot_root = root.join("boot");
        std::fs::create_dir_all(&generations_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        // Insert a mock trove
        let mut trove = Trove::new(
            "kernel".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();

        // Insert file entries for the trove
        let cas = CasStore::new(&objects_dir).unwrap();
        let hash = cas.store(b"hello").unwrap();
        let init_hash = cas.store(b"init").unwrap();
        insert_regular_file_with_parents(
            &conn,
            "/usr/bin/hello",
            hash,
            b"hello".len(),
            0o755,
            trove_id,
        );
        insert_regular_file_with_parents(
            &conn,
            "/sbin/init",
            init_hash,
            b"init".len(),
            0o755,
            trove_id,
        );

        // Run build_generation_from_db
        let result = build_generation_from_db_with_boot_root(
            &conn,
            &generations_dir,
            "Full transaction round-trip test",
            &boot_root,
        );
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
            build_result.cas_objects_referenced, 2,
            "Should reference CAS objects for the hello binary and init entrypoint"
        );

        // Verify metadata JSON was written
        let gen_dir = generations_dir.join(gen_num.to_string());
        let meta_path = gen_dir.join(GENERATION_METADATA_FILE);
        assert!(
            meta_path.exists(),
            "Metadata file must exist at {:?}",
            meta_path
        );
        let meta_json = std::fs::read_to_string(&meta_path).unwrap();
        let meta: GenerationMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.generation, gen_num);
        assert_eq!(meta.format, GENERATION_FORMAT);
        assert_eq!(meta.package_count, 1);
        assert!(meta.artifact_manifest_sha256.is_some());
        crate::generation::artifact::load_generation_artifact(&gen_dir).unwrap();

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
mod tests;
