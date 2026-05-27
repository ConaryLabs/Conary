// crates/conary-core/tests/db_backup.rs

use conary_core::db;
use conary_core::db::backup::{
    CheckpointReason, RecoveryOptions, backup_dir_for_db_path, create_checkpoint, recover_latest,
    verify_backup,
};
use conary_core::db::schema::SCHEMA_VERSION;

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
