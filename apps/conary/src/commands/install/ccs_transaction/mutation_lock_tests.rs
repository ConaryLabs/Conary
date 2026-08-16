// apps/conary/src/commands/install/ccs_transaction/mutation_lock_tests.rs

use super::super::*;
use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::{BuildResult, CcsManifest, CcsPackage, SigningKeyPair};
use conary_core::db::models::{InstallSource, Trove, TroveType};
use conary_core::packages::PackageFormat;
use conary_core::repository::versioning::VersionScheme;
use std::collections::HashMap;

fn metadata_only_ccs(temp: &std::path::Path) -> (CcsPackage, SigningKeyPair) {
    let package_path = temp.join("ccs-lock-fixture.ccs");
    let manifest = CcsManifest::new_minimal("ccs-lock-fixture", "2.0.0");
    let result = BuildResult {
        manifest,
        components: HashMap::new(),
        files: Vec::new(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &[],
            HashMap::new(),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = SigningKeyPair::generate().with_key_id("ccs-lock-fixture");
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    let verification = crate::commands::install::verify_ccs_package_authority(
        temp.join("conary.db").to_str().unwrap(),
        &package_path,
        &crate::commands::install::CcsEnvelopeAuthority::ExactKey(signing_key.public_key_base64()),
        None,
    )
    .unwrap();
    let package =
        CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verification).unwrap();
    (package, signing_key)
}

#[test]
fn standalone_ccs_install_resolves_upgrade_identity_under_the_mutation_lock() {
    let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let install_root = temp.path().join("install-root");
    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let (package, _signing_key) = metadata_only_ccs(temp.path());

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut installed = Trove::new_with_source(
        package.name().to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        InstallSource::Repository,
        VersionScheme::Conary,
    );
    installed.architecture = package.architecture().map(str::to_string);
    installed.package_release = package.package_release().map(str::to_string);
    let installed_id = installed.insert(&conn).unwrap();
    drop(conn);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let install_root_string = install_root.to_string_lossy().into_owned();
    let incoming_version = package.version().to_string();
    let locked =
        crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(&db_path_string)
            .unwrap();
    let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
    let waiter_db_path = db_path_string.clone();
    let waiter = std::thread::spawn(move || {
        let mut conn = conary_core::db::open(&waiter_db_path).unwrap();
        attempt_tx.send(()).unwrap();
        let result = install_ccs_package_transactionally(
            &mut conn,
            &package,
            CcsTransactionInstallOptions {
                db_path: &waiter_db_path,
                root: &install_root_string,
                dry_run: false,
                defer_generation: false,
                quiet: true,
                sandbox_mode: conary_core::scriptlet::SandboxMode::Always,
                allow_downgrade: false,
                intent: InstallIntent::PackageChange,
                reinstall: false,
                selection_reason: None,
                selected_manifest_components: None,
                repository_provenance: None,
                requested_source_identity: None,
            },
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
        "CCS install reached a verdict while another transaction held the mutation lock"
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

    let result = result_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("CCS install must finish once the mutation lock is released");
    waiter.join().unwrap();
    let error = match result {
        Ok(_) => panic!("CCS install accepted upgrade authority changed before it locked"),
        Err(error) => error,
    };
    assert!(error.contains("is already installed"), "{error}");
}
