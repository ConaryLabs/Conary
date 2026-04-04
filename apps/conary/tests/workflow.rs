// tests/workflow.rs

//! Package install, remove, and rollback workflow tests.

mod common;

use conary_core::db;
use tempfile::NamedTempFile;

#[test]
#[ignore] // Ignored by default since it requires a real RPM file
fn test_rpm_install_workflow() {
    use conary_core::db::models::{Changeset, ChangesetStatus, FileEntry, Trove};
    use conary_core::packages::PackageFormat;
    use conary_core::packages::rpm::RpmPackage;

    // This test requires a real RPM file to be present
    // To run: place an RPM file at /tmp/test.rpm and run:
    // cargo test test_rpm_install_workflow -- --ignored

    let rpm_path = "/tmp/test.rpm";
    if !std::path::Path::new(rpm_path).exists() {
        eprintln!("Skipping RPM install test: no RPM file at {}", rpm_path);
        return;
    }

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    // Initialize database
    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Parse the RPM
    let rpm = RpmPackage::parse(rpm_path).expect("Failed to parse RPM");

    // Verify basic metadata was extracted
    assert!(!rpm.name().is_empty(), "Package name should not be empty");
    assert!(
        !rpm.version().is_empty(),
        "Package version should not be empty"
    );

    // Perform installation within changeset (simulating the install command)
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new(format!("Install {}-{}", rpm.name(), rpm.version()));
        let changeset_id = changeset.insert(tx)?;

        let mut trove = rpm.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        // Store file metadata
        for file in rpm.files() {
            let mut file_entry = FileEntry::new(
                file.path.clone(),
                file.sha256.clone().unwrap_or_default(),
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.insert(tx)?;
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify it was stored correctly
    let troves = Trove::find_by_name(&conn, rpm.name()).unwrap();
    assert_eq!(troves.len(), 1);
    assert_eq!(troves[0].version, rpm.version());

    // Verify changeset was created
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(changesets[0].status, ChangesetStatus::Applied);

    // Verify files were stored
    let files = FileEntry::find_by_trove(&conn, troves[0].id.unwrap()).unwrap();
    assert_eq!(files.len(), rpm.files().len());

    println!("Successfully installed RPM package:");
    println!("  Name: {}", rpm.name());
    println!("  Version: {}", rpm.version());
    println!("  Files: {}", rpm.files().len());
    println!("  Dependencies: {}", rpm.dependencies().len());
}

#[test]
fn test_install_and_remove_workflow() {
    use conary_core::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Install a package
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install test-package-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(trove_id)
    })
    .unwrap();

    // Verify it's installed
    let troves = Trove::find_by_name(&conn, "test-package").unwrap();
    assert_eq!(troves.len(), 1);

    // Remove the package
    let trove_id = troves[0].id.unwrap();
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Remove test-package-1.0.0".to_string());
        changeset.insert(tx)?;

        Trove::delete(tx, trove_id)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify it's removed
    let troves = Trove::find_by_name(&conn, "test-package").unwrap();
    assert_eq!(troves.len(), 0);
}

#[test]
fn test_install_and_rollback() {
    use conary_core::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Install a package
    let changeset_id = db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install nginx-1.21.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })
    .unwrap();

    // Verify it's installed
    let troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(troves.len(), 1);

    // Rollback the installation
    db::transaction(&mut conn, |tx| {
        let mut rollback_changeset =
            Changeset::new(format!("Rollback of changeset {}", changeset_id));
        let rollback_id = rollback_changeset.insert(tx)?;

        // Delete the trove
        Trove::delete(tx, troves[0].id.unwrap())?;

        rollback_changeset.update_status(tx, ChangesetStatus::Applied)?;

        // Mark original as rolled back
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, changeset_id],
        )?;

        Ok(())
    })
    .unwrap();

    // Verify it's removed
    let troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(troves.len(), 0);

    // Verify changeset is marked as rolled back
    let changeset = Changeset::find_by_id(&conn, changeset_id).unwrap().unwrap();
    assert_eq!(changeset.status, ChangesetStatus::RolledBack);
}

/// Test rollback changeset tracking
#[test]
fn test_rollback_tracking() {
    use conary_core::db::models::{Changeset, ChangesetStatus, Trove};

    let (temp_dir, db_path) = common::setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Find nginx and record its changeset
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_id = nginx.id.unwrap();
    let nginx_cs_id = nginx.installed_by_changeset_id.unwrap();

    // Roll back the install
    db::transaction(&mut conn, |tx| {
        // Create rollback changeset
        let mut rollback_cs = Changeset::new("Rollback nginx install".to_string());
        let rollback_id = rollback_cs.insert(tx)?;

        // Delete the package
        Trove::delete(tx, nginx_id)?;

        // Mark both changesets
        rollback_cs.update_status(tx, ChangesetStatus::Applied)?;
        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_id, nginx_cs_id],
        )?;

        Ok(())
    })
    .unwrap();

    // Verify rollback
    let nginx_after = Trove::find_by_name(&conn, "nginx").unwrap();
    assert!(
        nginx_after.is_empty(),
        "nginx should be removed after rollback"
    );

    let cs_after = Changeset::find_by_id(&conn, nginx_cs_id).unwrap().unwrap();
    assert_eq!(cs_after.status, ChangesetStatus::RolledBack);

    // Keep temp_dir alive
    drop(temp_dir);
}

#[test]
fn test_derive_build_cli_surfaces_persisted_artifact() {
    use conary_core::db::models::{DerivedOverride, DerivedPackage, VersionPolicy};
    use conary_core::filesystem::CasStore;
    use std::process::Command;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let objects_dir = conary_core::db::paths::objects_dir(&db_path);
    let cas = CasStore::new(&objects_dir).unwrap();

    let mut derived = DerivedPackage::new("nginx-cli-derived".to_string(), "nginx".to_string());
    derived.version_policy = VersionPolicy::Suffix("+cli".to_string());
    let derived_id = derived.insert(&conn).unwrap();

    let override_content = b"events {}\nhttp { server_tokens off; }\n".to_vec();
    let override_hash = cas.store(&override_content).unwrap();

    let mut override_entry = DerivedOverride::new_replace(
        derived_id,
        "/etc/nginx/nginx.conf".to_string(),
        override_hash,
    );
    override_entry.insert(&conn).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("derive")
        .arg("build")
        .arg("nginx-cli-derived")
        .arg("--db-path")
        .arg(&db_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "derive build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Artifact: cas://"));
    assert!(stdout.contains("Parent Version: 1.24.0"));
}

#[test]
fn test_parent_upgrade_marks_built_derived_package_stale_via_install_cli() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{
        BuildResult, CcsManifest, ComponentData, FileEntry as CcsFileEntry, FileType,
    };
    use conary_core::db::models::{DerivedOverride, DerivedPackage, DerivedStatus, VersionPolicy};
    use conary_core::derived::{build_from_definition, persist_build_artifact};
    use conary_core::filesystem::CasStore;
    use conary_core::hash;
    use std::collections::HashMap;
    use std::process::Command;

    let install_temp = tempfile::tempdir().unwrap();
    let install_root = install_temp.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let (_db_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();
    let objects_dir = conary_core::db::paths::objects_dir(&db_path);
    let cas = CasStore::new(&objects_dir).unwrap();

    let mut derived = DerivedPackage::new("nginx-derived".to_string(), "nginx".to_string());
    derived.version_policy = VersionPolicy::Suffix("+corp".to_string());
    let derived_id = derived.insert(&conn).unwrap();

    let override_content = b"user nginx;\nworker_processes auto;\n".to_vec();
    let override_hash = cas.store(&override_content).unwrap();
    let mut override_entry = DerivedOverride::new_replace(
        derived_id,
        "/etc/nginx/nginx.conf".to_string(),
        override_hash,
    );
    override_entry.insert(&conn).unwrap();

    let build_result = build_from_definition(&conn, &derived, &cas).unwrap();
    persist_build_artifact(&conn, &mut derived, &build_result, &cas).unwrap();

    let binary_content = b"#!/bin/sh\necho upgraded nginx\n".to_vec();
    let binary_hash = hash::sha256(&binary_content);
    let config_content = b"worker_processes 4;\n".to_vec();
    let config_hash = hash::sha256(&config_content);
    let files = vec![
        CcsFileEntry {
            path: "/usr/sbin/nginx".to_string(),
            hash: binary_hash.clone(),
            size: binary_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        CcsFileEntry {
            path: "/etc/nginx/nginx.conf".to_string(),
            hash: config_hash.clone(),
            size: config_content.len() as u64,
            mode: 0o100644,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let package_path = install_temp.path().join("nginx-1.25.0.ccs");
    let result = BuildResult {
        manifest: CcsManifest::new_minimal("nginx", "1.25.0"),
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime-component".to_string(),
                size: (binary_content.len() + config_content.len()) as u64,
            },
        )]),
        files,
        blobs: HashMap::from([(binary_hash, binary_content), (config_hash, config_content)]),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let install_output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("--allow-live-system-mutation")
        .arg("install")
        .arg(package_path.to_str().unwrap())
        .arg("--db-path")
        .arg(&db_path)
        .arg("--root")
        .arg(install_root.to_str().unwrap())
        .arg("--sandbox")
        .arg("never")
        .arg("--yes")
        .output()
        .unwrap();

    assert!(
        install_output.status.success(),
        "install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install_output.stdout),
        String::from_utf8_lossy(&install_output.stderr)
    );

    let derived_after = DerivedPackage::find_by_name(&conn, "nginx-derived")
        .unwrap()
        .unwrap();
    assert_eq!(derived_after.status, DerivedStatus::Stale);

    let stale_output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("derive")
        .arg("stale")
        .arg("--db-path")
        .arg(&db_path)
        .output()
        .unwrap();

    assert!(stale_output.status.success());
    let stdout = String::from_utf8_lossy(&stale_output.stdout);
    assert!(stdout.contains("nginx-derived <- nginx"));
}

#[test]
fn test_capability_run_uses_installed_package_declaration() {
    use std::path::PathBuf;
    use std::process::Command;

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();

    let db_path = temp_dir.path().join("capability.db");
    conary_core::db::init(db_path.to_str().unwrap()).unwrap();

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/adversarial/malicious/cap-net-raw/output/cap-net-raw.ccs");

    let install_output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("--allow-live-system-mutation")
        .arg("ccs")
        .arg("install")
        .arg(fixture_path)
        .arg("--allow-unsigned")
        .arg("--allow-capabilities")
        .arg("--sandbox")
        .arg("never")
        .arg("--reinstall")
        .arg("--db-path")
        .arg(db_path.to_str().unwrap())
        .arg("--root")
        .arg(install_root.to_str().unwrap())
        .output()
        .unwrap();

    assert!(
        install_output.status.success(),
        "capability fixture install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install_output.stdout),
        String::from_utf8_lossy(&install_output.stderr)
    );

    let show_output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("capability")
        .arg("show")
        .arg("cap-net-raw")
        .arg("--db-path")
        .arg(db_path.to_str().unwrap())
        .output()
        .unwrap();

    assert!(show_output.status.success());
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(show_stdout.contains("Capability Declaration for: cap-net-raw"));
    assert!(show_stdout.contains("Schema Version: 1"));

    let run_output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("capability")
        .arg("run")
        .arg("cap-net-raw")
        .arg("--db-path")
        .arg(db_path.to_str().unwrap())
        .arg("--")
        .arg("/bin/echo")
        .arg("capability-ok")
        .output()
        .unwrap();

    let run_stderr = String::from_utf8_lossy(&run_output.stderr);
    let missing_mount_privilege = run_stderr.contains("mount --make-rprivate failed: EACCES")
        || run_stderr.contains("mount --make-rprivate failed: EPERM");
    let missing_readonly_remount_privilege = run_stderr.contains("read-only remount failed")
        && (run_stderr.contains("EACCES") || run_stderr.contains("EPERM"));
    if !run_output.status.success()
        && (missing_mount_privilege || missing_readonly_remount_privilege)
    {
        eprintln!(
            "skipping capability run assertion on a host without the mount privileges required for capability sandboxing"
        );
        return;
    }

    assert!(
        run_output.status.success(),
        "capability run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        run_stderr
    );
    let run_stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(run_stdout.contains("capability-ok"));
}

#[test]
fn test_local_native_fixture_builder_generates_all_supported_formats() {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn find_generated_package(output_dir: &Path, extension: &str) -> PathBuf {
        fs::read_dir(output_dir)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| path.to_string_lossy().ends_with(extension))
            .unwrap_or_else(|| panic!("no generated package with extension {extension}"))
    }

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = repo_root.join("tests/fixtures/phase4-runtime-fixture");
    let build_script = repo_root.join("tests/fixtures/native/build-native-fixtures.sh");
    let temp_dir = tempfile::tempdir().unwrap();
    let native_output_root = temp_dir.path().join("native-output");

    for (target, extension) in [("rpm", ".rpm"), ("deb", ".deb"), ("arch", ".pkg.tar.zst")] {
        let output_dir = native_output_root.join(target);
        std::fs::create_dir_all(&output_dir).unwrap();

        let build_output = Command::new("bash")
            .arg(&build_script)
            .arg(target)
            .arg(&output_dir)
            .arg(&fixture_dir)
            .env("CONARY_BIN", env!("CARGO_BIN_EXE_conary"))
            .output()
            .unwrap();

        assert!(
            build_output.status.success(),
            "native fixture build failed for {target}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let package_path = find_generated_package(&output_dir, extension);
        let file_name = package_path.file_name().unwrap().to_string_lossy();
        assert!(file_name.contains("phase4-runtime-fixture"));
        assert!(file_name.contains("1.0.0"));
    }
}
