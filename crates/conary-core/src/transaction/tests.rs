// crates/conary-core/src/transaction/tests.rs

use super::*;
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectRef, CasObjectVerification, stage_boot_assets,
    write_generation_artifact,
};
use crate::generation::metadata::EROFS_IMAGE_NAME;
use crate::generation::metadata::{
    GENERATION_FORMAT, GENERATION_METADATA_FILE, GenerationMetadata,
};
use crate::generation::test_support::write_root_manifests_with_objects;
use tempfile::TempDir;

fn write_valid_generation_artifact(root: &Path, generation: i64) {
    let generations_dir = root.join("generations");
    let generation_dir = generations_dir.join(generation.to_string());
    let objects_dir = root.join("objects");
    let boot_source = root.join("boot-source");
    std::fs::create_dir_all(&generation_dir).unwrap();
    std::fs::create_dir_all(&objects_dir).unwrap();
    std::fs::create_dir_all(boot_source.join("EFI/BOOT")).unwrap();

    let cas_bytes = b"selected-generation-cas";
    let cas_hash = crate::filesystem::CasStore::new(&objects_dir)
        .unwrap()
        .store(cas_bytes)
        .unwrap();
    let cas_objects = vec![CasObjectRef {
        sha256: cas_hash,
        size: cas_bytes.len() as u64,
    }];
    write_root_manifests_with_objects(&generation_dir, &cas_objects);

    let erofs_path = generation_dir.join(EROFS_IMAGE_NAME);
    std::fs::write(&erofs_path, b"root-erofs").unwrap();
    let kernel = boot_source.join("vmlinuz");
    let initramfs = boot_source.join("initramfs.img");
    let efi = boot_source.join("EFI/BOOT/BOOTX64.EFI");
    std::fs::write(&kernel, b"kernel").unwrap();
    std::fs::write(&initramfs, b"initramfs").unwrap();
    std::fs::write(&efi, b"efi").unwrap();

    let boot_assets = stage_boot_assets(BootAssetSources {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        kernel_version: "6.19.8-conary",
        kernel: &kernel,
        initramfs: &initramfs,
        efi_bootloader: &efi,
    })
    .unwrap();
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &generation_dir,
        generation,
        architecture: "x86_64",
        erofs_path: &erofs_path,
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::AlreadyVerified,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();

    GenerationMetadata {
        generation,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(std::fs::metadata(&erofs_path).unwrap().len() as i64),
        cas_objects_referenced: Some(1),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        security_capability_xattr_count: None,
        created_at: "2026-05-14T00:00:00Z".to_string(),
        package_count: 1,
        kernel_version: Some("6.19.8-conary".to_string()),
        summary: "selected generation fixture".to_string(),
    }
    .write_to(&generation_dir)
    .unwrap();

    assert!(generation_dir.join(GENERATION_METADATA_FILE).is_file());
}

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
fn transaction_config_from_paths_keeps_default_runtime_state_under_conary() {
    let config = TransactionConfig::from_paths(
        PathBuf::from("/"),
        PathBuf::from("/var/lib/conary/conary.db"),
    );
    assert_eq!(config.root, PathBuf::from("/conary"));
    assert_eq!(config.db_path, PathBuf::from("/var/lib/conary/conary.db"));
    assert_eq!(config.objects_dir, PathBuf::from("/conary/objects"));
    assert_eq!(config.generations_dir, PathBuf::from("/conary/generations"));
    assert_eq!(config.etc_state_dir, PathBuf::from("/conary/etc-state"));
    assert_eq!(config.mount_point, PathBuf::from("/conary/mnt"));
}

#[test]
fn transaction_config_from_paths_keeps_explicit_test_root_self_contained() {
    let config = TransactionConfig::from_paths(
        PathBuf::from("/tmp/conary-test"),
        PathBuf::from("/tmp/conary-test/conary.db"),
    );

    assert_eq!(config.root, PathBuf::from("/tmp/conary-test"));
    assert_eq!(config.db_path, PathBuf::from("/tmp/conary-test/conary.db"));
    assert_eq!(
        config.objects_dir,
        PathBuf::from("/tmp/conary-test/objects")
    );
    assert_eq!(
        config.generations_dir,
        PathBuf::from("/tmp/conary-test/generations")
    );
    assert_eq!(
        config.etc_state_dir,
        PathBuf::from("/tmp/conary-test/etc-state")
    );
    assert_eq!(config.mount_point, PathBuf::from("/tmp/conary-test/mnt"));
}

#[test]
fn transaction_config_from_paths_uses_test_db_parent_not_install_root() {
    let config = TransactionConfig::from_paths(
        PathBuf::from("/tmp/install-root"),
        PathBuf::from("/tmp/conary-runtime/conary.db"),
    );

    assert_eq!(config.root, PathBuf::from("/tmp/conary-runtime"));
    assert_eq!(
        config.objects_dir,
        PathBuf::from("/tmp/conary-runtime/objects")
    );
    assert_eq!(
        config.generations_dir,
        PathBuf::from("/tmp/conary-runtime/generations")
    );
    assert_eq!(config.mount_point, PathBuf::from("/tmp/conary-runtime/mnt"));
}

#[test]
fn state_valid_transitions() {
    assert!(TransactionState::New.can_transition_to(&TransactionState::Resolved));
    assert!(TransactionState::Resolved.can_transition_to(&TransactionState::Fetched));
    assert!(TransactionState::Fetched.can_transition_to(&TransactionState::Committed));
    assert!(TransactionState::Committed.can_transition_to(&TransactionState::Built));
    assert!(TransactionState::Built.can_transition_to(&TransactionState::Selected));
    assert!(TransactionState::Selected.can_transition_to(&TransactionState::Done));
}

#[test]
fn state_invalid_transitions() {
    assert!(!TransactionState::New.can_transition_to(&TransactionState::Built));
    assert!(!TransactionState::New.can_transition_to(&TransactionState::Committed));
    assert!(!TransactionState::Fetched.can_transition_to(&TransactionState::Selected));
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
    assert!(TransactionState::Selected.is_committed());
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
fn engine_begin_preserves_existing_lockfile_contents() {
    let temp_dir = TempDir::new().unwrap();
    let config = TransactionConfig::new(temp_dir.path());
    let lock_path = config.objects_dir.join("conary.lock");
    std::fs::create_dir_all(&config.objects_dir).unwrap();
    std::fs::write(&lock_path, b"keep-me").unwrap();

    let mut engine = TransactionEngine::new(config).unwrap();
    engine.begin().unwrap();
    engine.release_lock();

    assert_eq!(std::fs::read(&lock_path).unwrap(), b"keep-me");
}

#[test]
fn engine_begin_timeout_message_does_not_suggest_deleting_lockfile() {
    let temp_dir = TempDir::new().unwrap();
    let mut config = TransactionConfig::new(temp_dir.path());
    config.lock_timeout_secs = 0;
    let lock_path = config.objects_dir.join("conary.lock");
    std::fs::create_dir_all(&config.objects_dir).unwrap();

    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock_file.try_lock_exclusive().unwrap();

    let mut engine = TransactionEngine::new(config).unwrap();
    let err = engine.begin().unwrap_err().to_string();
    assert!(!err.contains("remove the lock file"));
    assert!(err.contains("Wait for the active transaction to finish"));
}

/// Write an EROFS-looking image stub with only the magic at offset 1024.
fn write_stub_erofs(path: &std::path::Path) {
    const EROFS_MAGIC: u32 = 0xE0F5_E1E2;
    let mut data = vec![0u8; 4096];
    let magic = EROFS_MAGIC.to_le_bytes();
    data[1024..1028].copy_from_slice(&magic);
    std::fs::write(path, &data).expect("write stub erofs");
}

#[test]
fn test_recover_does_not_promote_magic_only_generation() {
    // Arrange: set up a generations directory with an EROFS-like image and
    // a `current` symlink pointing to generation 2.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let generations_dir = root.join("generations");
    std::fs::create_dir_all(generations_dir.join("2")).unwrap();

    let image_path = generations_dir.join("2").join(EROFS_IMAGE_NAME);
    write_stub_erofs(&image_path);

    // Create `current -> generations/2`
    let link = root.join("current");
    std::os::unix::fs::symlink("generations/2", &link).unwrap();

    // Act: call find_latest_intact_generation directly (mount would fail in test)
    let config = TransactionConfig {
        root: root.clone(),
        db_path: root.join("conary.db"),
        objects_dir: root.join("objects"),
        generations_dir: generations_dir.clone(),
        etc_state_dir: root.join("etc-state"),
        mount_point: PathBuf::from("/"),
        hash_algorithm: crate::hash::HashAlgorithm::Sha256,
        lock_timeout_secs: TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS,
    };
    let cas = crate::filesystem::CasStore::with_algorithm(
        config.objects_dir.clone(),
        config.hash_algorithm,
    )
    .unwrap();
    let engine = TransactionEngine {
        config,
        cas,
        lock_file: None,
    };

    // EROFS magic alone is not enough; recovery scanning now requires the
    // generation artifact contract and metadata.
    let found = engine.find_latest_intact_generation().unwrap();
    assert!(found.is_none(), "magic-only generation must be skipped");
}

#[test]
fn test_transaction_recover_does_not_promote_db_active_without_current_symlink() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();

    let mut state = crate::db::models::SystemState::new(7, "build-only state".to_string());
    state.insert(&conn).unwrap();
    state.set_active(&conn).unwrap();

    let config = TransactionConfig::new(&root);
    let engine = TransactionEngine::new(config).unwrap();

    engine.recover(&conn).unwrap();

    assert!(
        !root.join("current").exists(),
        "transaction recovery must not promote DB-active build-only states without /conary/current"
    );
}

#[test]
fn test_transaction_recover_accepts_valid_selected_artifact_without_live_mounting() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();

    let generation = 1;
    let mut state =
        crate::db::models::SystemState::new(generation, "selected generation".to_string());
    state.insert(&conn).unwrap();
    write_valid_generation_artifact(&root, generation);

    #[cfg(unix)]
    std::os::unix::fs::symlink("generations/1", root.join("current")).unwrap();

    let mut config = TransactionConfig::new(&root);
    config.mount_point = root.join("mount-should-not-be-used");
    let engine = TransactionEngine::new(config).unwrap();

    engine.recover(&conn).unwrap();

    assert!(
        !root.join("mount-should-not-be-used").exists(),
        "ordinary transaction recovery must not live-mount the selected generation"
    );
    assert_eq!(
        std::fs::read_link(root.join("current")).unwrap(),
        PathBuf::from("generations/1")
    );
}

#[test]
fn test_boot_selection_recovery_fails_without_valid_artifacts_and_preserves_missing_current() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let db_path = root.join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();

    let generation_dir = root.join("generations/9");
    std::fs::create_dir_all(&generation_dir).unwrap();
    write_stub_erofs(&generation_dir.join(EROFS_IMAGE_NAME));

    let config = TransactionConfig::new(&root);
    let engine = TransactionEngine::new(config).unwrap();

    let err = engine.recover_boot_selection(&conn).unwrap_err();

    assert!(
        err.to_string().contains("no valid generation artifact"),
        "unexpected recovery error: {err}"
    );
    assert!(
        !root.join("current").exists(),
        "failed boot-selection recovery must not create /conary/current"
    );
}

#[test]
fn test_recover_rebuilds_when_image_missing() {
    // Arrange: generation directory exists but root.erofs is absent.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let generations_dir = root.join("generations");
    // Create generation 5 directory without an EROFS image
    std::fs::create_dir_all(generations_dir.join("5")).unwrap();

    // find_latest_intact_generation should return None (no intact image)
    let config = TransactionConfig {
        root: root.clone(),
        db_path: root.join("conary.db"),
        objects_dir: root.join("objects"),
        generations_dir: generations_dir.clone(),
        etc_state_dir: root.join("etc-state"),
        mount_point: PathBuf::from("/"),
        hash_algorithm: crate::hash::HashAlgorithm::Sha256,
        lock_timeout_secs: TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS,
    };
    std::fs::create_dir_all(&config.objects_dir).unwrap();
    let cas = crate::filesystem::CasStore::with_algorithm(
        config.objects_dir.clone(),
        config.hash_algorithm,
    )
    .unwrap();
    let engine = TransactionEngine {
        config,
        cas,
        lock_file: None,
    };

    let found = engine.find_latest_intact_generation().unwrap();
    assert!(
        found.is_none(),
        "no intact image should result in None from find_latest_intact_generation"
    );
}

#[test]
fn test_find_latest_intact_generation_skips_pending_generation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let generations_dir = root.join("generations");
    std::fs::create_dir_all(generations_dir.join("6")).unwrap();

    let image_path = generations_dir.join("6").join(EROFS_IMAGE_NAME);
    write_stub_erofs(&image_path);
    crate::generation::metadata::mark_generation_pending(&generations_dir.join("6")).unwrap();

    let config = TransactionConfig {
        root: root.clone(),
        db_path: root.join("conary.db"),
        objects_dir: root.join("objects"),
        generations_dir: generations_dir.clone(),
        etc_state_dir: root.join("etc-state"),
        mount_point: PathBuf::from("/"),
        hash_algorithm: crate::hash::HashAlgorithm::Sha256,
        lock_timeout_secs: TransactionConfig::DEFAULT_LOCK_TIMEOUT_SECS,
    };
    std::fs::create_dir_all(&config.objects_dir).unwrap();
    let cas = crate::filesystem::CasStore::with_algorithm(
        config.objects_dir.clone(),
        config.hash_algorithm,
    )
    .unwrap();
    let engine = TransactionEngine {
        config,
        cas,
        lock_file: None,
    };

    assert!(engine.find_latest_intact_generation().unwrap().is_none());
}
