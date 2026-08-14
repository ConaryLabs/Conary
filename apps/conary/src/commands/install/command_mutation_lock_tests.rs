// apps/conary/src/commands/install/command_mutation_lock_tests.rs

use super::*;
use conary_core::db::models::{InstallSource, Trove, TroveType};
use conary_core::packages::PackageFormat;
use conary_core::repository::versioning::VersionScheme;

#[test]
fn native_install_resolves_upgrade_identity_under_the_mutation_lock() {
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let install_root = temp.path().join("install-root");
    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let mut builder = rpm::PackageBuilder::new(
        "native-lock-fixture",
        "2.0.0",
        "MIT",
        "x86_64",
        "native install mutation-lock fixture",
    );
    builder
        .with_file_contents(
            b"fixture\n".to_vec(),
            rpm::FileOptions::new("/usr/lib/native-lock-fixture/payload").permissions(0o644),
        )
        .unwrap();
    let rpm_path = temp.path().join("native-lock-fixture.rpm");
    builder.build().unwrap().write_file(&rpm_path).unwrap();
    let incoming =
        conary_core::packages::rpm::RpmPackage::parse(rpm_path.to_str().unwrap()).unwrap();
    let incoming_version = incoming.version().to_string();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut installed = Trove::new_with_source(
        "native-lock-fixture".to_string(),
        "1.0.0-1".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        VersionScheme::Rpm,
    );
    installed.architecture = Some("x86_64".to_string());
    let installed_id = installed.insert(&conn).unwrap();
    drop(conn);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let install_root_string = install_root.to_string_lossy().into_owned();
    let rpm_path_string = rpm_path.to_string_lossy().into_owned();
    let locked = LockedRuntimeRoot::acquire(&db_path_string).unwrap();
    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let waiter_db_path = db_path_string.clone();
    let waiter = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        attempt_tx.send(()).unwrap();
        let result = runtime
            .block_on(cmd_install(
                &rpm_path_string,
                InstallOptions {
                    db_path: &waiter_db_path,
                    root: &install_root_string,
                    architecture: Some("x86_64".to_string()),
                    no_deps: true,
                    sandbox_mode: crate::commands::SandboxMode::Always,
                    yes: true,
                    ..InstallOptions::default()
                },
            ))
            .map_err(|error| format!("{error:#}"));
        result_tx.send(result).unwrap();
    });

    attempt_rx.recv().unwrap();
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "native install reached a verdict while another transaction held the mutation lock"
    );

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE troves SET version = ?1 WHERE id = ?2",
            rusqlite::params![incoming_version, installed_id],
        )
        .unwrap(),
        1
    );
    drop(conn);
    drop(locked);

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("native install must finish once the mutation lock is released")
        .expect_err("native install accepted upgrade authority changed before it locked");
    waiter.join().unwrap();
    assert!(error.contains("is already installed"), "{error}");
}
