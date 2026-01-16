// tests/workflow.rs

//! Package install, remove, and rollback workflow tests.

mod common;

use conary::db;
use tempfile::NamedTempFile;

#[test]
#[ignore] // Ignored by default since it requires a real RPM file
fn test_rpm_install_workflow() {
    use conary::db::models::{Changeset, ChangesetStatus, FileEntry, Trove};
    use conary::packages::rpm::RpmPackage;
    use conary::packages::PackageFormat;

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
    use conary::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

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
    use conary::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Install a package
    let changeset_id = db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install nginx-1.21.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new("nginx".to_string(), "1.21.0".to_string(), TroveType::Package);
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
    use conary::db::models::{Changeset, ChangesetStatus, Trove};

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
    assert!(nginx_after.is_empty(), "nginx should be removed after rollback");

    let cs_after = Changeset::find_by_id(&conn, nginx_cs_id).unwrap().unwrap();
    assert_eq!(cs_after.status, ChangesetStatus::RolledBack);

    // Keep temp_dir alive
    drop(temp_dir);
}
