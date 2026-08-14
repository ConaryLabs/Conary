// apps/conary/src/commands/install/batch/tests/mutation_lock.rs

//! Certification reads installed state with the mutation lock already held.
//!
//! A batch is admitted on facts it reads from installed state: which
//! requirements the end-state universe satisfies, which promises it relies on,
//! which installed packages the incoming relations remove. Reading those facts
//! before waiting for the runtime mutation lock leaves a window -- another
//! transaction can delete a provider the certification leaned on, and this
//! batch still commits against a universe that no longer exists. The lock is
//! what closes that window, so the facts have to be read on its far side.

use super::*;

/// Delete the only provider while the batch waits for the mutation lock.
///
/// The deletion lands after the batch has started and before it holds the
/// lock, which is exactly the interval a pre-lock snapshot would have read
/// across. Certification must see the deletion, because the database it is
/// about to commit into is the one without the provider.
#[test]
fn a_batch_certifies_against_the_installed_state_the_mutation_lock_protects() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let (db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![prepared_test_package(
            "provider",
            "/usr/lib64/libprovided.so.1",
            b"provided",
        )],
    );

    // Hold the mutation lock without materializing a root, which is the state
    // the installer itself is in while it certifies.
    let locked =
        crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(&db_path_string)
            .unwrap();

    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let waiter_db_path = db_path_string.clone();
    let waiter = std::thread::spawn(move || {
        let mut dependent = prepared_test_package("dependent", "/usr/bin/dependent", b"dependent");
        dependent.requirements = vec![depends_on("provider")];
        attempt_tx.send(()).unwrap();
        let result = BatchInstaller::new(&waiter_db_path, SandboxMode::Always)
            .install_batch(vec![dependent])
            .map_err(|error| format!("{error:#}"));
        result_tx.send(result).unwrap();
    });

    // No verdict is reachable while another holder owns the lock.
    attempt_rx.recv().unwrap();
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "a batch reached a verdict while another transaction held the mutation lock"
    );

    // Remove the only provider under the held lock. Nothing this batch will
    // certify has been read yet, so this is the state it is obliged to see.
    let conn = conary_core::db::open(&db_path).unwrap();
    let deleted = conn
        .execute("DELETE FROM troves WHERE name = 'provider'", [])
        .unwrap();
    assert_eq!(
        deleted, 1,
        "the prior transaction must have installed exactly one provider"
    );
    drop(conn);
    drop(locked);

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the batch must reach a verdict once the mutation lock is released")
        .expect_err("a batch certified a requirement against a provider deleted before it locked");
    waiter.join().unwrap();
    assert!(
        error.contains("does not satisfy exact depends requirement for dependent"),
        "{error}"
    );
}
