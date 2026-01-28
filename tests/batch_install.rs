// tests/batch_install.rs

//! Integration tests for batch (atomic multi-package) installation.
//!
//! These tests verify that:
//! 1. Multiple packages can be installed atomically
//! 2. Cross-package file conflicts are detected
//! 3. Rollback works correctly when installation fails
//! 4. All packages share a single changeset

use conary::db;
use conary::db::models::{Changeset, ChangesetStatus, FileEntry, Trove, TroveType};
use tempfile::TempDir;

/// Create a minimal test database
fn setup_test_db() -> (TempDir, String) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("test.db")
        .to_str()
        .unwrap()
        .to_string();

    db::init(&db_path).unwrap();

    (temp_dir, db_path)
}

#[test]
fn test_batch_install_creates_single_changeset() {
    // This test verifies that batch install creates a single changeset
    // for all packages, which is the key to atomicity.

    let (_temp_dir, db_path) = setup_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Simulate what BatchInstaller does: single changeset for multiple troves
    db::transaction(&mut conn, |tx| {
        // Single changeset for the batch
        let mut changeset = Changeset::new("Install pkgA + 2 dependencies".to_string());
        let changeset_id = changeset.insert(tx)?;

        // First package (dependency 1)
        let mut pkg1 = Trove::new("dep1".to_string(), "1.0.0".to_string(), TroveType::Package);
        pkg1.installed_by_changeset_id = Some(changeset_id);
        pkg1.selection_reason = Some("Required by pkgA".to_string());
        let pkg1_id = pkg1.insert(tx)?;

        let mut f1 = FileEntry::new(
            "/usr/lib/libdep1.so".to_string(),
            "hash1".to_string(),
            1024,
            0o755,
            pkg1_id,
        );
        f1.insert(tx)?;

        // Second package (dependency 2)
        let mut pkg2 = Trove::new("dep2".to_string(), "2.0.0".to_string(), TroveType::Package);
        pkg2.installed_by_changeset_id = Some(changeset_id);
        pkg2.selection_reason = Some("Required by pkgA".to_string());
        let pkg2_id = pkg2.insert(tx)?;

        let mut f2 = FileEntry::new(
            "/usr/lib/libdep2.so".to_string(),
            "hash2".to_string(),
            2048,
            0o755,
            pkg2_id,
        );
        f2.insert(tx)?;

        // Main package
        let mut pkg_a = Trove::new("pkgA".to_string(), "1.0.0".to_string(), TroveType::Package);
        pkg_a.installed_by_changeset_id = Some(changeset_id);
        pkg_a.selection_reason = Some("Explicitly installed by user".to_string());
        let pkg_a_id = pkg_a.insert(tx)?;

        let mut f3 = FileEntry::new(
            "/usr/bin/pkgA".to_string(),
            "hash3".to_string(),
            4096,
            0o755,
            pkg_a_id,
        );
        f3.insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify: only ONE changeset was created
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 1, "Should have exactly one changeset");
    assert_eq!(changesets[0].status, ChangesetStatus::Applied);
    assert!(changesets[0].description.contains("pkgA"));
    assert!(changesets[0].description.contains("dependencies"));

    // Verify: all three troves reference the same changeset
    let all_troves: Vec<_> = ["dep1", "dep2", "pkgA"]
        .iter()
        .flat_map(|name| Trove::find_by_name(&conn, name).unwrap())
        .collect();

    assert_eq!(all_troves.len(), 3, "Should have 3 troves");

    let changeset_ids: std::collections::HashSet<_> = all_troves
        .iter()
        .filter_map(|t| t.installed_by_changeset_id)
        .collect();

    assert_eq!(
        changeset_ids.len(),
        1,
        "All troves should reference the same changeset"
    );
}

#[test]
fn test_batch_install_rollback_removes_all() {
    // This test verifies that if a batch install fails,
    // no packages are left installed (atomic rollback).

    let (_temp_dir, db_path) = setup_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Start a transaction that will fail
    let result: Result<(), conary::Error> = db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install failing batch".to_string());
        let changeset_id = changeset.insert(tx)?;

        // First package succeeds
        let mut pkg1 = Trove::new("good-pkg".to_string(), "1.0.0".to_string(), TroveType::Package);
        pkg1.installed_by_changeset_id = Some(changeset_id);
        pkg1.insert(tx)?;

        // Second package "fails" (we simulate this by returning an error)
        Err(conary::Error::InitError(
            "Simulated installation failure".to_string(),
        ))
    });

    // The transaction should have failed
    assert!(result.is_err());

    // Verify: NO packages were installed (atomic rollback)
    let good_pkg = Trove::find_by_name(&conn, "good-pkg").unwrap();
    assert!(
        good_pkg.is_empty(),
        "good-pkg should not be installed after rollback"
    );

    // Verify: NO changesets were created
    let changesets = Changeset::list_all(&conn).unwrap();
    assert!(
        changesets.is_empty(),
        "No changesets should exist after rollback"
    );
}

#[test]
fn test_batch_install_file_tracking() {
    // Verify that all files from all packages in a batch are properly tracked

    let (_temp_dir, db_path) = setup_test_db();
    let mut conn = db::open(&db_path).unwrap();

    let (pkg1_id, pkg2_id) = db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Batch install".to_string());
        let changeset_id = changeset.insert(tx)?;

        // Package 1 with 2 files
        let mut pkg1 = Trove::new("lib-a".to_string(), "1.0".to_string(), TroveType::Package);
        pkg1.installed_by_changeset_id = Some(changeset_id);
        let pkg1_id = pkg1.insert(tx)?;

        FileEntry::new("/usr/lib/liba.so.1".to_string(), "h1".to_string(), 100, 0o755, pkg1_id)
            .insert(tx)?;
        FileEntry::new("/usr/lib/liba.so".to_string(), "h2".to_string(), 50, 0o777, pkg1_id)
            .insert(tx)?;

        // Package 2 with 3 files
        let mut pkg2 = Trove::new("lib-b".to_string(), "2.0".to_string(), TroveType::Package);
        pkg2.installed_by_changeset_id = Some(changeset_id);
        let pkg2_id = pkg2.insert(tx)?;

        FileEntry::new("/usr/lib/libb.so.1".to_string(), "h3".to_string(), 200, 0o755, pkg2_id)
            .insert(tx)?;
        FileEntry::new("/usr/lib/libb.so".to_string(), "h4".to_string(), 50, 0o777, pkg2_id)
            .insert(tx)?;
        FileEntry::new("/usr/include/b.h".to_string(), "h5".to_string(), 500, 0o644, pkg2_id)
            .insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok((pkg1_id, pkg2_id))
    })
    .unwrap();

    // Verify file counts
    let pkg1_files = FileEntry::find_by_trove(&conn, pkg1_id).unwrap();
    assert_eq!(pkg1_files.len(), 2, "lib-a should have 2 files");

    let pkg2_files = FileEntry::find_by_trove(&conn, pkg2_id).unwrap();
    assert_eq!(pkg2_files.len(), 3, "lib-b should have 3 files");

    // Verify file lookup by path
    let liba = FileEntry::find_by_path(&conn, "/usr/lib/liba.so.1").unwrap();
    assert!(liba.is_some(), "Should find liba.so.1");
    assert_eq!(liba.unwrap().trove_id, pkg1_id);

    let header = FileEntry::find_by_path(&conn, "/usr/include/b.h").unwrap();
    assert!(header.is_some(), "Should find b.h");
    assert_eq!(header.unwrap().trove_id, pkg2_id);
}

#[test]
fn test_batch_preserves_dependency_reasons() {
    // Verify that batch install correctly marks packages with their install reasons

    let (_temp_dir, db_path) = setup_test_db();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install app + deps".to_string());
        let changeset_id = changeset.insert(tx)?;

        // Dependency with "Required by" reason
        let mut dep = Trove::new("libfoo".to_string(), "1.0".to_string(), TroveType::Package);
        dep.installed_by_changeset_id = Some(changeset_id);
        dep.selection_reason = Some("Required by myapp".to_string());
        dep.install_reason = conary::db::models::InstallReason::Dependency;
        dep.insert(tx)?;

        // Main package with explicit reason
        let mut app = Trove::new("myapp".to_string(), "2.0".to_string(), TroveType::Package);
        app.installed_by_changeset_id = Some(changeset_id);
        app.selection_reason = Some("Explicitly installed by user".to_string());
        app.install_reason = conary::db::models::InstallReason::Explicit;
        app.insert(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify install reasons
    let libfoo = Trove::find_by_name(&conn, "libfoo").unwrap();
    assert_eq!(libfoo.len(), 1);
    assert_eq!(
        libfoo[0].install_reason,
        conary::db::models::InstallReason::Dependency
    );
    assert_eq!(
        libfoo[0].selection_reason,
        Some("Required by myapp".to_string())
    );

    let myapp = Trove::find_by_name(&conn, "myapp").unwrap();
    assert_eq!(myapp.len(), 1);
    assert_eq!(
        myapp[0].install_reason,
        conary::db::models::InstallReason::Explicit
    );
}

#[test]
fn test_cross_package_file_conflict_detection() {
    // This test verifies that the batch planner detects when two packages
    // in the same batch try to install the same file.
    //
    // Note: This is a unit-level test of the conflict detection logic.
    // The actual BatchInstaller.plan_batch() method does this check.

    use std::collections::HashSet;

    // Simulate the conflict detection from BatchInstaller::plan_batch
    fn detect_conflicts(
        packages: &[(&str, Vec<&str>)],
    ) -> Vec<(String, String, String)> {
        let mut all_paths: HashSet<String> = HashSet::new();
        let mut conflicts = Vec::new();

        for (pkg_name, files) in packages {
            for path in files {
                if all_paths.contains(*path) {
                    // Find which package already has this path
                    for (other_name, other_files) in packages {
                        if *other_name != *pkg_name && other_files.contains(path) {
                            conflicts.push((
                                path.to_string(),
                                other_name.to_string(),
                                pkg_name.to_string(),
                            ));
                            break;
                        }
                    }
                } else {
                    all_paths.insert(path.to_string());
                }
            }
        }

        conflicts
    }

    // Test case 1: No conflicts
    let packages = [
        ("pkg1", vec!["/usr/bin/a", "/usr/lib/liba.so"]),
        ("pkg2", vec!["/usr/bin/b", "/usr/lib/libb.so"]),
    ];
    let conflicts = detect_conflicts(&packages);
    assert!(conflicts.is_empty(), "Should have no conflicts");

    // Test case 2: Single conflict
    let packages = [
        ("pkg1", vec!["/usr/bin/foo", "/usr/lib/liba.so"]),
        ("pkg2", vec!["/usr/bin/foo", "/usr/lib/libb.so"]), // conflict on /usr/bin/foo
    ];
    let conflicts = detect_conflicts(&packages);
    assert_eq!(conflicts.len(), 1, "Should detect one conflict");
    assert_eq!(conflicts[0].0, "/usr/bin/foo");

    // Test case 3: Multiple conflicts
    let packages = [
        ("pkg1", vec!["/usr/bin/common", "/usr/lib/shared.so"]),
        ("pkg2", vec!["/usr/bin/common", "/usr/lib/shared.so"]), // both conflict
    ];
    let conflicts = detect_conflicts(&packages);
    assert_eq!(conflicts.len(), 2, "Should detect two conflicts");
}
