// apps/conary/src/commands/remove/native_graph_tests.rs

use super::*;
use conary_core::db::models::{InstallSource, TroveType};
use conary_core::repository::versioning::VersionScheme;

#[test]
fn remove_graph_resolves_package_identity_under_the_mutation_lock() {
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut trove = Trove::new_with_source(
        "remove-lock-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        VersionScheme::Conary,
    );
    trove.architecture = Some("x86_64".to_string());
    let trove_id = trove.insert(&conn).unwrap();
    let trove = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    drop(conn);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let locked = LockedRuntimeRoot::acquire(&db_path_string).unwrap();
    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let waiter_db_path = db_path_string.clone();
    let waiter = std::thread::spawn(move || {
        let conn = conary_core::db::open(&waiter_db_path).unwrap();
        let progress = RemoveProgress::new("remove-lock-fixture");
        attempt_tx.send(()).unwrap();
        let result = execute_installed_trove_remove_graph(
            &conn,
            &trove,
            &waiter_db_path,
            "remove-lock-fixture",
            RemoveLifecycleOptions::new(crate::commands::SandboxMode::Always),
            &progress,
        )
        .map_err(|error| format!("{error:#}"));
        result_tx.send(result).unwrap();
    });

    attempt_rx.recv().unwrap();
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "remove reached a verdict while another transaction held the mutation lock"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE troves SET version = '2.0.0' WHERE id = ?1",
            [trove_id],
        )
        .unwrap(),
        1
    );
    drop(conn);
    drop(locked);

    let result = result_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("remove must finish once the mutation lock is released");
    waiter.join().unwrap();
    let error = match result {
        Ok(_) => panic!("remove accepted package identity changed before it locked"),
        Err(error) => error,
    };
    assert!(
        error.contains("identity changed during removal preparation"),
        "{error}"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        Trove::find_by_id(&conn, trove_id).unwrap().unwrap().version,
        "2.0.0"
    );
}
