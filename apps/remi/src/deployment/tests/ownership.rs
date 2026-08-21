// apps/remi/src/deployment/tests/ownership.rs

use super::fixtures::arrange;
use crate::deployment::{prepare, rollback};
use crate::server::runtime_lock::{RuntimeRootLock, RuntimeRootLockError};
use std::fs;

#[test]
fn live_runtime_rejects_prepare_before_any_deployment_mutation() {
    let (temp, options) = arrange();
    let original_config = fs::read(&options.config_path).unwrap();
    let owner = RuntimeRootLock::acquire(temp.path()).unwrap();

    let error = prepare(&options).expect_err("live runtime must fence prepare");

    assert!(matches!(
        error.downcast_ref::<RuntimeRootLockError>(),
        Some(RuntimeRootLockError::AlreadyOwned { .. })
    ));
    assert_eq!(fs::read(&options.config_path).unwrap(), original_config);
    assert!(!options.repository_manifest_target.exists());
    assert!(!temp.path().join("metadata/conary.db").exists());
    assert!(!temp.path().join("deployment-backups").exists());
    assert_eq!(
        fs::read_dir(&options.repository_keys_dir).unwrap().count(),
        0
    );

    drop(owner);
    prepare(&options).expect("released runtime root should permit prepare");
}

#[test]
fn live_runtime_rejects_rollback_before_consistent_state_is_restored() {
    let (temp, options) = arrange();
    let original_config = fs::read(&options.config_path).unwrap();
    fs::write(
        &options.repository_manifest_target,
        b"previous repository authority",
    )
    .unwrap();
    let db_path = temp.path().join("metadata/conary.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE deployment_marker (value TEXT NOT NULL);
         INSERT INTO deployment_marker (value) VALUES ('before');",
    )
    .unwrap();
    drop(conn);

    let manifest = prepare(&options).unwrap();
    fs::write(&options.config_path, b"after config").unwrap();
    fs::write(
        &options.repository_manifest_target,
        b"after repository authority",
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("UPDATE deployment_marker SET value = 'after'", [])
        .unwrap();
    drop(conn);

    let owner = RuntimeRootLock::acquire(temp.path()).unwrap();
    let error = rollback(&manifest).expect_err("live runtime must fence rollback");
    assert!(matches!(
        error.downcast_ref::<RuntimeRootLockError>(),
        Some(RuntimeRootLockError::AlreadyOwned { .. })
    ));
    assert_eq!(fs::read(&options.config_path).unwrap(), b"after config");
    assert_eq!(
        fs::read(&options.repository_manifest_target).unwrap(),
        b"after repository authority"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let marker: String = conn
        .query_row("SELECT value FROM deployment_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(marker, "after");
    drop(conn);
    assert!(!manifest.parent().unwrap().join("failed-current").exists());

    drop(owner);
    rollback(&manifest).expect("released runtime root should permit rollback");
    assert_eq!(fs::read(&options.config_path).unwrap(), original_config);
    assert_eq!(
        fs::read(&options.repository_manifest_target).unwrap(),
        b"previous repository authority"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let marker: String = conn
        .query_row("SELECT value FROM deployment_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(marker, "before");
}
