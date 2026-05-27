// crates/conary-core/tests/db_backup.rs

use conary_core::db;
use conary_core::db::backup::{
    CheckpointReason, GenerationDbRecoveryOptions, RecoveryOptions, backup_dir_for_db_path,
    create_checkpoint, create_generation_db_backup, recover_generation_db_backup, recover_latest,
    verify_backup, verify_generation_db_backup,
};
use conary_core::db::models::{
    GenerationPublication, GenerationPublicationPhase, GenerationPublicationStatus,
};
use conary_core::db::schema::SCHEMA_VERSION;
use conary_core::generation::artifact::{
    ArtifactWriteInputs, BootAssetsManifest, CasObjectRef, CasObjectVerification,
    write_generation_artifact,
};
use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[test]
fn default_live_db_uses_conary_runtime_backup_dir() {
    assert_eq!(
        backup_dir_for_db_path("/var/lib/conary/conary.db"),
        std::path::PathBuf::from("/conary/backups")
    );
}

#[test]
fn custom_db_uses_parent_backups_dir() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("nested/conary.db");

    assert_eq!(
        backup_dir_for_db_path(&db_path),
        db_path.parent().unwrap().join("backups")
    );
}

#[test]
fn checkpoint_writes_verified_sqlite_backup_and_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    db::init(&db_path).unwrap();

    let record = create_checkpoint(&db_path, CheckpointReason::PreMutation).unwrap();

    assert!(record.backup_path.exists());
    assert!(record.manifest_path.exists());
    assert_eq!(record.manifest.reason, CheckpointReason::PreMutation);
    assert_eq!(record.manifest.db_schema_version, SCHEMA_VERSION);
    assert_eq!(record.manifest.integrity_check, "ok");
    assert!(
        record
            .backup_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("pre-mutation")
    );

    let verification = verify_backup(&record).unwrap();
    assert_eq!(verification.db_schema_version, SCHEMA_VERSION);
    assert_eq!(verification.integrity_check, "ok");
}

#[test]
fn checkpoint_rotation_keeps_five_most_recent_verified_backups() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    db::init(&db_path).unwrap();

    let mut records = Vec::new();
    for _ in 0..6 {
        records.push(create_checkpoint(&db_path, CheckpointReason::PostSuccess).unwrap());
    }

    let remaining = conary_core::db::backup::list_backups(&db_path).unwrap();
    assert_eq!(remaining.len(), 5);
    assert!(!records[0].backup_path.exists());
    assert!(!records[0].manifest_path.exists());
    assert!(!records[0].checksum_path.exists());
    assert!(records[5].backup_path.exists());
}

#[test]
fn recovery_dry_run_verifies_latest_backup_without_live_db() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    db::init(&db_path).unwrap();
    let record = create_checkpoint(&db_path, CheckpointReason::PreMutation).unwrap();

    std::fs::remove_file(&db_path).unwrap();

    let outcome = recover_latest(
        &db_path,
        RecoveryOptions {
            dry_run: true,
            yes: false,
            replace_healthy_db: false,
        },
    )
    .unwrap();

    assert_eq!(outcome.backup_path, record.backup_path);
    assert!(!db_path.exists());
}

#[test]
fn recovery_apply_quarantines_corrupt_db_and_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    db::init(&db_path).unwrap();
    let record = create_checkpoint(&db_path, CheckpointReason::PreMutation).unwrap();

    let wal_path = temp.path().join("conary.db-wal");
    let shm_path = temp.path().join("conary.db-shm");
    std::fs::write(&db_path, b"not a sqlite database").unwrap();
    std::fs::write(&wal_path, b"stale wal").unwrap();
    std::fs::write(&shm_path, b"stale shm").unwrap();

    let outcome = recover_latest(
        &db_path,
        RecoveryOptions {
            dry_run: false,
            yes: true,
            replace_healthy_db: false,
        },
    )
    .unwrap();

    assert_eq!(outcome.backup_path, record.backup_path);
    assert!(outcome.restored);
    assert_eq!(outcome.quarantined_paths.len(), 3);
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());
    db::open(&db_path).unwrap();
}

#[test]
fn generation_backup_writes_manifest_and_verifies_artifact_and_publication_state() {
    let fixture = GenerationBackupFixture::new(3);

    let record =
        create_generation_db_backup(&fixture.db_path, &fixture.generation_dir, 3, 3).unwrap();

    assert_eq!(record.manifest.format, "conary.generation-db-backup.v1");
    assert_eq!(record.manifest.manifest_version, 1);
    assert_eq!(record.manifest.generation_number, 3);
    assert_eq!(record.manifest.state_number, 3);
    assert_eq!(record.manifest.db_schema_version, SCHEMA_VERSION);
    assert_eq!(record.manifest.backup_file, "conary.db.backup");
    assert!(record.backup_path.ends_with("state/conary.db.backup"));
    assert!(
        record
            .checksum_path
            .ends_with("state/conary.db.backup.sha256")
    );
    assert!(
        record
            .manifest_path
            .ends_with("state/conary-db-backup.json")
    );

    let verification = verify_generation_db_backup(&fixture.generation_dir, None).unwrap();
    assert_eq!(verification.generation_number, 3);
    assert_eq!(verification.state_number, 3);
    assert_eq!(verification.db_schema_version, SCHEMA_VERSION);
    assert_eq!(verification.integrity_check, "ok");
}

#[test]
fn generation_backup_verification_rejects_tampered_backup_file() {
    let fixture = GenerationBackupFixture::new(4);
    let record =
        create_generation_db_backup(&fixture.db_path, &fixture.generation_dir, 4, 4).unwrap();

    std::fs::write(&record.backup_path, b"not the original backup").unwrap();

    let error = verify_generation_db_backup(&fixture.generation_dir, None).unwrap_err();
    assert!(error.to_string().to_lowercase().contains("checksum"));
}

#[test]
fn generation_backup_dry_run_recovery_verifies_copy_without_live_db() {
    let fixture = GenerationBackupFixture::new(5);
    create_generation_db_backup(&fixture.db_path, &fixture.generation_dir, 5, 5).unwrap();
    std::fs::remove_file(&fixture.db_path).unwrap();

    let outcome = recover_generation_db_backup(
        &fixture.db_path,
        &fixture.generation_dir,
        GenerationDbRecoveryOptions {
            dry_run: true,
            yes: false,
            keep_temp: false,
            replace_healthy_db: false,
        },
    )
    .unwrap();

    assert!(outcome.dry_run);
    assert!(!outcome.restored);
    assert!(!fixture.db_path.exists());
    assert!(outcome.verified_temp_path.is_none());
}

#[test]
fn generation_backup_apply_quarantines_corrupt_db_and_sidecars() {
    let fixture = GenerationBackupFixture::new(6);
    create_generation_db_backup(&fixture.db_path, &fixture.generation_dir, 6, 6).unwrap();

    let wal_path = fixture.db_path.with_file_name("conary.db-wal");
    let shm_path = fixture.db_path.with_file_name("conary.db-shm");
    std::fs::write(&fixture.db_path, b"not a sqlite database").unwrap();
    std::fs::write(&wal_path, b"stale wal").unwrap();
    std::fs::write(&shm_path, b"stale shm").unwrap();

    let outcome = recover_generation_db_backup(
        &fixture.db_path,
        &fixture.generation_dir,
        GenerationDbRecoveryOptions {
            dry_run: false,
            yes: true,
            keep_temp: false,
            replace_healthy_db: false,
        },
    )
    .unwrap();

    assert!(outcome.restored);
    assert_eq!(outcome.quarantined_paths.len(), 3);
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());
    db::open(&fixture.db_path).unwrap();
}

#[test]
fn generation_backup_apply_refuses_healthy_live_db_without_debug_override() {
    let fixture = GenerationBackupFixture::new(7);
    create_generation_db_backup(&fixture.db_path, &fixture.generation_dir, 7, 7).unwrap();

    let error = recover_generation_db_backup(
        &fixture.db_path,
        &fixture.generation_dir,
        GenerationDbRecoveryOptions {
            dry_run: false,
            yes: true,
            keep_temp: false,
            replace_healthy_db: false,
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("refusing to replace"));
}

struct GenerationBackupFixture {
    _temp: tempfile::TempDir,
    db_path: PathBuf,
    generation_dir: PathBuf,
}

impl GenerationBackupFixture {
    fn new(generation_number: i64) -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime_root = temp.path().join("runtime");
        let db_path = runtime_root.join("conary.db");
        let generation_dir = runtime_root
            .join("generations")
            .join(generation_number.to_string());
        let objects_dir = runtime_root.join("objects");

        db::init(&db_path).unwrap();
        seed_publication_state(&db_path, &runtime_root, generation_number);
        write_generation_artifact_fixture(&generation_dir, &objects_dir, generation_number);

        Self {
            _temp: temp,
            db_path,
            generation_dir,
        }
    }
}

fn seed_publication_state(db_path: &Path, runtime_root: &Path, generation_number: i64) {
    let conn = db::open(db_path).unwrap();
    let debt = GenerationPublication::create_pending(
        &conn,
        None,
        None,
        db_path.to_str().unwrap(),
        runtime_root.to_str().unwrap(),
        "fixture generation publication",
    )
    .unwrap();
    debt.set_phase(
        &conn,
        GenerationPublicationPhase::CurrentPublished,
        GenerationPublicationStatus::Running,
        Some(generation_number),
        Some(generation_number),
    )
    .unwrap();
}

fn write_generation_artifact_fixture(
    generation_dir: &Path,
    objects_dir: &Path,
    generation_number: i64,
) {
    let boot_assets_dir = generation_dir.join("boot-assets");
    std::fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();
    std::fs::create_dir_all(objects_dir).unwrap();
    std::fs::write(generation_dir.join("root.erofs"), b"root-erofs").unwrap();
    std::fs::write(boot_assets_dir.join("vmlinuz"), b"kernel").unwrap();
    std::fs::write(boot_assets_dir.join("initramfs.img"), b"initramfs").unwrap();
    std::fs::write(boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

    let cas_object = write_cas_object(objects_dir, b"cas object");
    let boot_assets = BootAssetsManifest {
        version: 1,
        generation: generation_number,
        architecture: "x86_64".to_string(),
        kernel_version: "6.19.8-conary".to_string(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: digest_bytes(b"kernel"),
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: digest_bytes(b"initramfs"),
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: digest_bytes(b"efi"),
        created_at: "2026-05-27T00:00:00Z".to_string(),
    };
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir,
        generation: generation_number,
        architecture: "x86_64",
        erofs_path: &generation_dir.join("root.erofs"),
        cas_base_rel: "../../objects",
        cas_objects: vec![cas_object],
        cas_verification: CasObjectVerification::Deep,
        boot_assets,
    })
    .unwrap();

    GenerationMetadata {
        generation: generation_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(10),
        cas_objects_referenced: Some(1),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        created_at: "2026-05-27T00:00:00Z".to_string(),
        package_count: 1,
        kernel_version: Some("6.19.8-conary".to_string()),
        summary: "fixture".to_string(),
    }
    .write_to(generation_dir)
    .unwrap();
}

fn write_cas_object(objects_dir: &Path, bytes: &[u8]) -> CasObjectRef {
    let sha256 = digest_bytes(bytes);
    let object_path = conary_core::filesystem::object_path(objects_dir, &sha256).unwrap();
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(object_path, bytes).unwrap();
    CasObjectRef {
        sha256,
        size: bytes.len() as u64,
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
