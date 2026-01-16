// tests/integration_test.rs

//! Integration tests for Conary
//!
//! These tests verify end-to-end functionality across modules.

use conary::db;
use tempfile::NamedTempFile;

#[test]
fn test_database_lifecycle() {
    // Create a temporary database
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();

    // Remove the temp file so init can create it
    drop(temp_file);

    // Initialize the database
    let init_result = db::init(&db_path);
    assert!(
        init_result.is_ok(),
        "Database initialization should succeed"
    );

    // Verify database file exists
    assert!(
        std::path::Path::new(&db_path).exists(),
        "Database file should exist after initialization"
    );

    // Open the database
    let conn_result = db::open(&db_path);
    assert!(conn_result.is_ok(), "Opening database should succeed");

    // Verify we can execute a simple query
    let conn = conn_result.unwrap();
    let result: Result<i32, _> = conn.query_row("SELECT 1", [], |row| row.get(0));
    assert_eq!(result.unwrap(), 1, "Should be able to execute queries");
}

#[test]
fn test_database_init_creates_parent_directories() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir
        .path()
        .join("nested/path/to/conary.db")
        .to_str()
        .unwrap()
        .to_string();

    let result = db::init(&db_path);
    assert!(result.is_ok(), "Should create parent directories");
    assert!(
        std::path::Path::new(&db_path).exists(),
        "Database should exist in nested path"
    );
}

#[test]
fn test_database_pragmas_are_set() {
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Verify foreign keys are enabled
    let foreign_keys: i32 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(foreign_keys, 1, "Foreign keys should be enabled");

    // Verify WAL mode (on a fresh init)
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        journal_mode.to_lowercase(),
        "wal",
        "Journal mode should be WAL"
    );
}

#[test]
fn test_full_workflow_with_transaction() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus, FileEntry, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    // Initialize database with schema
    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Use transaction to install a package atomically
    let result = db::transaction(&mut conn, |tx| {
        // Create a changeset
        let mut changeset = Changeset::new("Install nginx-1.21.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        // Create a trove
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.description = Some("HTTP and reverse proxy server".to_string());
        trove.installed_by_changeset_id = Some(changeset_id);

        let trove_id = trove.insert(tx)?;

        // Add files
        let mut file1 = FileEntry::new(
            "/usr/bin/nginx".to_string(),
            "a1b2c3d4e5f6".to_string(),
            524288, // 512KB
            0o755,
            trove_id,
        );
        file1.owner = Some("root".to_string());
        file1.insert(tx)?;

        let mut file2 = FileEntry::new(
            "/etc/nginx/nginx.conf".to_string(),
            "f6e5d4c3b2a1".to_string(),
            4096,
            0o644,
            trove_id,
        );
        file2.owner = Some("root".to_string());
        file2.insert(tx)?;

        // Mark changeset as applied
        changeset.update_status(tx, ChangesetStatus::Applied)?;

        Ok(())
    });

    assert!(result.is_ok(), "Transaction should succeed");

    // Verify the data was committed
    let troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(troves.len(), 1);
    assert_eq!(troves[0].version, "1.21.0");

    let files = FileEntry::find_by_trove(&conn, troves[0].id.unwrap()).unwrap();
    assert_eq!(files.len(), 2);

    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(changesets[0].status, ChangesetStatus::Applied);
}

#[test]
fn test_transaction_rollback_on_error() {
    use conary::db;
    use conary::db::models::{Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Try a transaction that will fail
    let result = db::transaction(&mut conn, |tx| {
        let mut trove1 = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove1.architecture = Some("x86_64".to_string());
        trove1.insert(tx)?;

        // Try to insert duplicate (should fail due to UNIQUE constraint)
        let mut trove2 = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove2.architecture = Some("x86_64".to_string());
        trove2.insert(tx)?;

        Ok(())
    });

    assert!(result.is_err(), "Transaction should fail on duplicate");

    // Verify nothing was committed (rollback worked)
    let troves = Trove::find_by_name(&conn, "test-pkg").unwrap();
    assert_eq!(
        troves.len(),
        0,
        "No troves should be in database after rollback"
    );
}

#[test]
fn test_trove_with_flavors_and_provenance() {
    use conary::db;
    use conary::db::models::{Flavor, Provenance, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Create a trove with specific flavors (nginx with SSL and HTTP/3 support)
    let mut trove = Trove::new(
        "nginx".to_string(),
        "1.21.0".to_string(),
        TroveType::Package,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.description = Some("HTTP server with SSL and HTTP/3".to_string());

    let trove_id = trove.insert(&conn).unwrap();

    // Add flavors to represent build configuration
    let mut ssl_flavor = Flavor::new(trove_id, "ssl".to_string(), "openssl-3.0".to_string());
    ssl_flavor.insert(&conn).unwrap();

    let mut http3_flavor = Flavor::new(trove_id, "http3".to_string(), "enabled".to_string());
    http3_flavor.insert(&conn).unwrap();

    let mut arch_flavor = Flavor::new(trove_id, "arch".to_string(), "x86_64".to_string());
    arch_flavor.insert(&conn).unwrap();

    // Add provenance information
    let mut prov = Provenance::new(trove_id);
    prov.source_url = Some("https://github.com/nginx/nginx".to_string());
    prov.source_branch = Some("release-1.21".to_string());
    prov.source_commit = Some("abc123def456789".to_string());
    prov.build_host = Some("builder.example.com".to_string());
    prov.build_time = Some("2025-11-14T12:00:00Z".to_string());
    prov.builder = Some("ci-bot".to_string());
    prov.insert(&conn).unwrap();

    // Verify we can retrieve the full picture
    let retrieved_trove = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    assert_eq!(retrieved_trove.name, "nginx");
    assert_eq!(retrieved_trove.version, "1.21.0");

    let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(flavors.len(), 3);

    // Verify flavors are ordered by key
    assert_eq!(flavors[0].key, "arch");
    assert_eq!(flavors[1].key, "http3");
    assert_eq!(flavors[2].key, "ssl");
    assert_eq!(flavors[2].value, "openssl-3.0");

    let provenance = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
    assert_eq!(
        provenance.source_url,
        Some("https://github.com/nginx/nginx".to_string())
    );
    assert_eq!(
        provenance.source_commit,
        Some("abc123def456789".to_string())
    );
    assert_eq!(provenance.builder, Some("ci-bot".to_string()));

    // Test querying by flavor
    let ssl_packages = Flavor::find_by_key(&conn, "ssl").unwrap();
    assert_eq!(ssl_packages.len(), 1);
    assert_eq!(ssl_packages[0].trove_id, trove_id);
}

#[test]
#[ignore] // Ignored by default since it requires a real RPM file
fn test_rpm_install_workflow() {
    use conary::db;
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
    use conary::db;
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
    use conary::db;
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

#[test]
fn test_query_packages() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Install multiple packages
    for (name, version) in [("nginx", "1.21.0"), ("redis", "6.2.0"), ("postgres", "14.0")] {
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(format!("Install {}-{}", name, version));
            let changeset_id = changeset.insert(tx)?;

            let mut trove = Trove::new(name.to_string(), version.to_string(), TroveType::Package);
            trove.installed_by_changeset_id = Some(changeset_id);
            trove.insert(tx)?;

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })
        .unwrap();
    }

    // Query all packages
    let all_troves = Trove::list_all(&conn).unwrap();
    assert_eq!(all_troves.len(), 3);

    // Query specific package
    let nginx_troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_troves.len(), 1);
    assert_eq!(nginx_troves[0].version, "1.21.0");
}

#[test]
fn test_history_shows_operations() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Create some changesets
    for desc in ["Install nginx", "Install redis", "Remove nginx"] {
        db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new(desc.to_string());
            changeset.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })
        .unwrap();
    }

    // Verify history
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 3);
    assert_eq!(changesets[0].description, "Remove nginx"); // Most recent first
    assert_eq!(changesets[1].description, "Install redis");
    assert_eq!(changesets[2].description, "Install nginx");

    for changeset in &changesets {
        assert_eq!(changeset.status, ChangesetStatus::Applied);
    }
}

// =============================================================================
// Component Selection Tests (Gemini's Smoke Test)
// =============================================================================

/// Test that the classifier correctly categorizes files into components
#[test]
fn test_component_classifier_categorization() {
    use conary::components::{ComponentClassifier, ComponentType};
    use std::path::Path;

    // :devel files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/include/myapp.h")),
        ComponentType::Devel,
        "Header files should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/pkgconfig/myapp.pc")),
        ComponentType::Devel,
        "pkg-config files should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/libmyapp.a")),
        ComponentType::Devel,
        "Static libraries should be :devel"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/cmake/myapp/myappConfig.cmake")),
        ComponentType::Devel,
        "CMake files should be :devel"
    );

    // :runtime files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/bin/myapp")),
        ComponentType::Runtime,
        "Binaries should be :runtime"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/sbin/myapp-daemon")),
        ComponentType::Runtime,
        "System binaries should be :runtime"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/myapp/helper.sh")),
        ComponentType::Runtime,
        "Helper scripts should be :runtime (not :data)"
    );

    // :lib files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib/libmyapp.so.1")),
        ComponentType::Lib,
        "Shared libraries should be :lib"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/lib64/libmyapp.so")),
        ComponentType::Lib,
        "64-bit shared libraries should be :lib"
    );

    // :doc files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/doc/myapp/README")),
        ComponentType::Doc,
        "Documentation should be :doc"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/usr/share/man/man1/myapp.1.gz")),
        ComponentType::Doc,
        "Man pages should be :doc"
    );

    // :config files
    assert_eq!(
        ComponentClassifier::classify(Path::new("/etc/myapp.conf")),
        ComponentType::Config,
        "Config files should be :config"
    );
    assert_eq!(
        ComponentClassifier::classify(Path::new("/etc/myapp/settings.ini")),
        ComponentType::Config,
        "Nested config files should be :config"
    );
}

/// Test that default components are correctly identified
#[test]
fn test_default_component_types() {
    use conary::components::ComponentType;

    // Default components (installed by default)
    assert!(ComponentType::Runtime.is_default(), ":runtime should be default");
    assert!(ComponentType::Lib.is_default(), ":lib should be default");
    assert!(ComponentType::Config.is_default(), ":config should be default");

    // Non-default components (require explicit request)
    assert!(!ComponentType::Devel.is_default(), ":devel should NOT be default");
    assert!(!ComponentType::Doc.is_default(), ":doc should NOT be default");
}

/// Test scriptlet gating: scriptlets only run when :runtime or :lib is installed
#[test]
fn test_scriptlet_gating() {
    use conary::components::{should_run_scriptlets, ComponentType};

    // Scriptlets SHOULD run when :runtime or :lib is present
    assert!(
        should_run_scriptlets(&[ComponentType::Runtime]),
        "Scriptlets should run when installing :runtime"
    );
    assert!(
        should_run_scriptlets(&[ComponentType::Lib]),
        "Scriptlets should run when installing :lib"
    );
    assert!(
        should_run_scriptlets(&[ComponentType::Runtime, ComponentType::Lib, ComponentType::Config]),
        "Scriptlets should run when installing defaults"
    );

    // Scriptlets should NOT run when only :devel, :doc, or :config
    assert!(
        !should_run_scriptlets(&[ComponentType::Devel]),
        "Scriptlets should NOT run when installing only :devel"
    );
    assert!(
        !should_run_scriptlets(&[ComponentType::Doc]),
        "Scriptlets should NOT run when installing only :doc"
    );
    assert!(
        !should_run_scriptlets(&[ComponentType::Devel, ComponentType::Doc]),
        "Scriptlets should NOT run when installing :devel + :doc"
    );
    // Note: :config alone also shouldn't run scriptlets (contentious, but safe)
    assert!(
        !should_run_scriptlets(&[ComponentType::Config]),
        "Scriptlets should NOT run when installing only :config"
    );
}

/// Smoke test: Simulate devel-only installation and verify correct behavior
#[test]
fn test_devel_only_install_smoke_test() {
    use conary::components::{should_run_scriptlets, ComponentClassifier, ComponentType};
    use std::collections::HashSet;

    // Simulate a package with files in all component types
    let package_files = vec![
        // :runtime files
        "/usr/bin/zlib-tool".to_string(),
        "/usr/sbin/zlibd".to_string(),
        // :lib files
        "/usr/lib/libz.so.1".to_string(),
        "/usr/lib/libz.so.1.2.13".to_string(),
        "/usr/lib64/libz.so".to_string(),
        // :devel files (what we want)
        "/usr/include/zlib.h".to_string(),
        "/usr/include/zconf.h".to_string(),
        "/usr/lib/pkgconfig/zlib.pc".to_string(),
        "/usr/lib/libz.a".to_string(),
        // :doc files
        "/usr/share/doc/zlib/README".to_string(),
        "/usr/share/man/man3/zlib.3.gz".to_string(),
        // :config files
        "/etc/zlib.conf".to_string(),
    ];

    // Classify all files
    let classified = ComponentClassifier::classify_all(&package_files);

    // Verify classification counts
    assert!(classified.contains_key(&ComponentType::Runtime), "Should have :runtime files");
    assert!(classified.contains_key(&ComponentType::Lib), "Should have :lib files");
    assert!(classified.contains_key(&ComponentType::Devel), "Should have :devel files");
    assert!(classified.contains_key(&ComponentType::Doc), "Should have :doc files");
    assert!(classified.contains_key(&ComponentType::Config), "Should have :config files");

    // Simulate selecting ONLY :devel
    let selected_component = ComponentType::Devel;
    let selected_files: HashSet<&str> = classified
        .get(&selected_component)
        .map(|files| files.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    // Verify ONLY :devel files would be installed
    assert!(
        selected_files.contains("/usr/include/zlib.h"),
        "Header file should be selected"
    );
    assert!(
        selected_files.contains("/usr/include/zconf.h"),
        "Header file should be selected"
    );
    assert!(
        selected_files.contains("/usr/lib/pkgconfig/zlib.pc"),
        "pkg-config file should be selected"
    );
    assert!(
        selected_files.contains("/usr/lib/libz.a"),
        "Static library should be selected"
    );

    // Verify :runtime files are NOT selected
    assert!(
        !selected_files.contains("/usr/bin/zlib-tool"),
        "Binary should NOT be selected for :devel-only"
    );
    assert!(
        !selected_files.contains("/usr/sbin/zlibd"),
        "Daemon should NOT be selected for :devel-only"
    );

    // Verify :lib files are NOT selected
    assert!(
        !selected_files.contains("/usr/lib/libz.so.1"),
        "Shared library should NOT be selected for :devel-only"
    );

    // Verify :doc files are NOT selected
    assert!(
        !selected_files.contains("/usr/share/doc/zlib/README"),
        "Documentation should NOT be selected for :devel-only"
    );

    // Verify :config files are NOT selected
    assert!(
        !selected_files.contains("/etc/zlib.conf"),
        "Config should NOT be selected for :devel-only"
    );

    // CRITICAL: Verify scriptlets would NOT run for :devel-only install
    let installed_components = vec![ComponentType::Devel];
    assert!(
        !should_run_scriptlets(&installed_components),
        "Scriptlets should NOT run for :devel-only install (would likely fail without /usr/bin)"
    );

    println!("Smoke test passed: :devel-only install behavior is correct");
    println!("  - Selected {} files from :devel component", selected_files.len());
    println!("  - Excluded :runtime, :lib, :doc, :config files");
    println!("  - Scriptlet execution: SKIPPED (correct!)");
}

/// Test default installation behavior (runtime + lib + config only)
#[test]
fn test_default_install_excludes_devel_and_doc() {
    use conary::components::{ComponentClassifier, ComponentType};
    use std::collections::HashSet;

    // Simulate a package
    let package_files = vec![
        "/usr/bin/nginx".to_string(),           // :runtime
        "/usr/lib/libnginx.so".to_string(),     // :lib
        "/etc/nginx/nginx.conf".to_string(),    // :config
        "/usr/include/nginx.h".to_string(),     // :devel
        "/usr/lib/pkgconfig/nginx.pc".to_string(), // :devel
        "/usr/share/doc/nginx/README".to_string(), // :doc
        "/usr/share/man/man8/nginx.8.gz".to_string(), // :doc
    ];

    let classified = ComponentClassifier::classify_all(&package_files);

    // Simulate default selection (runtime + lib + config)
    let default_types: HashSet<ComponentType> = [
        ComponentType::Runtime,
        ComponentType::Lib,
        ComponentType::Config,
    ].into_iter().collect();

    let selected_files: HashSet<&str> = classified
        .iter()
        .filter(|(comp_type, _)| default_types.contains(comp_type))
        .flat_map(|(_, files)| files.iter().map(|s| s.as_str()))
        .collect();

    // Should include defaults
    assert!(selected_files.contains("/usr/bin/nginx"), "Binary should be included");
    assert!(selected_files.contains("/usr/lib/libnginx.so"), "Shared lib should be included");
    assert!(selected_files.contains("/etc/nginx/nginx.conf"), "Config should be included");

    // Should exclude non-defaults
    assert!(!selected_files.contains("/usr/include/nginx.h"), "Headers should be excluded");
    assert!(!selected_files.contains("/usr/lib/pkgconfig/nginx.pc"), "pkg-config should be excluded");
    assert!(!selected_files.contains("/usr/share/doc/nginx/README"), "Docs should be excluded");
    assert!(!selected_files.contains("/usr/share/man/man8/nginx.8.gz"), "Man pages should be excluded");

    println!("Default install test passed:");
    println!("  - Included: {} files (runtime + lib + config)", selected_files.len());
    println!("  - Excluded: :devel and :doc components");
}

/// Test component selection with database (full integration)
#[test]
fn test_component_selective_install_database() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus, Component, FileEntry, Trove, TroveType};
    use conary::components::{ComponentClassifier, ComponentType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Simulate installing only :devel component
    let package_files = vec![
        "/usr/bin/myapp".to_string(),           // :runtime - NOT installed
        "/usr/lib/libmyapp.so".to_string(),     // :lib - NOT installed
        "/usr/include/myapp.h".to_string(),     // :devel - INSTALLED
        "/usr/lib/pkgconfig/myapp.pc".to_string(), // :devel - INSTALLED
    ];

    let classified = ComponentClassifier::classify_all(&package_files);
    let selected_component = ComponentType::Devel;

    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install myapp:devel".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "myapp".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        // Only create the :devel component
        let mut devel_comp = Component::from_type(trove_id, ComponentType::Devel);
        let devel_comp_id = devel_comp.insert(tx)?;

        // Only insert files from :devel component
        if let Some(devel_files) = classified.get(&selected_component) {
            for path in devel_files {
                let mut file_entry = FileEntry::new(
                    path.clone(),
                    "fakehash123".to_string(),
                    100,
                    0o644,
                    trove_id,
                );
                file_entry.component_id = Some(devel_comp_id);
                file_entry.insert(tx)?;
            }
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })
    .unwrap();

    // Verify: only :devel component exists
    let troves = Trove::find_by_name(&conn, "myapp").unwrap();
    assert_eq!(troves.len(), 1);
    let trove_id = troves[0].id.unwrap();

    let components = Component::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(components.len(), 1, "Should have exactly one component");
    assert_eq!(components[0].name, "devel", "Should be :devel component");

    // Verify: only :devel files are in the database
    let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(files.len(), 2, "Should have 2 files (header + pkg-config)");

    let file_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert!(file_paths.contains(&"/usr/include/myapp.h"), "Should have header");
    assert!(file_paths.contains(&"/usr/lib/pkgconfig/myapp.pc"), "Should have pkg-config");
    assert!(!file_paths.contains(&"/usr/bin/myapp"), "Should NOT have binary");
    assert!(!file_paths.contains(&"/usr/lib/libmyapp.so"), "Should NOT have shared lib");

    println!("Database integration test passed:");
    println!("  - Package: myapp:devel");
    println!("  - Components in DB: {:?}", components.iter().map(|c| &c.name).collect::<Vec<_>>());
    println!("  - Files in DB: {:?}", file_paths);
}

/// Test language-specific dependency detection
#[test]
fn test_language_deps_detection() {
    use conary::dependencies::{DependencyClass, LanguageDepDetector};

    // Simulate a Python package with multiple modules
    let package_files = vec![
        "/usr/lib/python3.11/site-packages/requests/__init__.py".to_string(),
        "/usr/lib/python3.11/site-packages/requests/api.py".to_string(),
        "/usr/lib/python3.11/site-packages/urllib3.py".to_string(),
        "/usr/lib64/python3.11/site-packages/numpy.cpython-311-x86_64-linux-gnu.so".to_string(),
        "/usr/share/perl5/vendor_perl/DBI.pm".to_string(),
        "/usr/share/perl5/vendor_perl/DBD/SQLite.pm".to_string(),
        "/usr/lib64/libssl.so.3".to_string(),
        "/usr/bin/python3".to_string(),
    ];

    let provides = LanguageDepDetector::detect_all_provides(&package_files);

    // Should detect Python modules
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Python && p.name == "requests"),
        "Should detect python(requests) provide"
    );
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Python && p.name == "urllib3"),
        "Should detect python(urllib3) provide"
    );
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Python && p.name == "numpy"),
        "Should detect python(numpy) provide from .so"
    );

    // Should detect Perl modules
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Perl && p.name == "DBI"),
        "Should detect perl(DBI) provide"
    );
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Perl && p.name == "DBD::SQLite"),
        "Should detect perl(DBD::SQLite) provide"
    );

    // Should detect sonames
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::Soname && p.name == "libssl.so.3"),
        "Should detect soname(libssl.so.3) provide"
    );

    // Should detect file provides
    assert!(
        provides.iter().any(|p| p.class == DependencyClass::File && p.name == "/usr/bin/python3"),
        "Should detect file(/usr/bin/python3) provide"
    );

    println!("Language deps detection test passed:");
    println!("  - Detected {} provides from {} files", provides.len(), package_files.len());
    for p in &provides {
        println!("    - {}", p.to_dep_string());
    }
}

/// Test LanguageDep parsing and formatting
#[test]
fn test_language_dep_parsing() {
    use conary::dependencies::{DependencyClass, LanguageDep};

    // Test parsing with version constraint
    let dep = LanguageDep::parse("python(requests>=2.0)").unwrap();
    assert_eq!(dep.class, DependencyClass::Python);
    assert_eq!(dep.name, "requests");
    assert_eq!(dep.version_constraint, Some(">=2.0".to_string()));

    // Test roundtrip
    let roundtrip = LanguageDep::parse(&dep.to_dep_string()).unwrap();
    assert_eq!(roundtrip.class, dep.class);
    assert_eq!(roundtrip.name, dep.name);
    assert_eq!(roundtrip.version_constraint, dep.version_constraint);

    // Test Perl module with double colons
    let perl_dep = LanguageDep::parse("perl(DBD::SQLite>=1.0)").unwrap();
    assert_eq!(perl_dep.class, DependencyClass::Perl);
    assert_eq!(perl_dep.name, "DBD::SQLite");
    assert_eq!(perl_dep.version_constraint, Some(">=1.0".to_string()));

    // Test soname
    let soname_dep = LanguageDep::parse("soname(libssl.so.3)").unwrap();
    assert_eq!(soname_dep.class, DependencyClass::Soname);
    assert_eq!(soname_dep.name, "libssl.so.3");

    // Test file dependency
    let file_dep = LanguageDep::parse("file(/usr/bin/python3)").unwrap();
    assert_eq!(file_dep.class, DependencyClass::File);
    assert_eq!(file_dep.name, "/usr/bin/python3");

    println!("Language dep parsing test passed");
}

/// Test language deps stored in database during install
#[test]
fn test_language_deps_in_database() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus, FileEntry, ProvideEntry, Trove, TroveType};
    use conary::dependencies::LanguageDepDetector;

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    // Simulate installing a Python package
    let package_files = vec![
        "/usr/lib/python3.11/site-packages/mymodule/__init__.py".to_string(),
        "/usr/lib/python3.11/site-packages/mymodule/core.py".to_string(),
        "/usr/lib64/libmymodule.so.1".to_string(),
    ];

    // Detect language provides
    let language_provides = LanguageDepDetector::detect_all_provides(&package_files);
    assert!(!language_provides.is_empty(), "Should detect at least one provide");

    // Store package and provides in DB
    db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install python-mymodule-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;

        let mut trove = Trove::new(
            "python-mymodule".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        // Insert files
        for (i, path) in package_files.iter().enumerate() {
            let hash = format!("{:064x}", i);
            let mut entry = FileEntry::new(
                path.clone(),
                hash,
                100,
                0o644,
                trove_id,
            );
            entry.insert(tx)?;
        }

        // Insert language provides
        for lang_dep in &language_provides {
            let mut provide = ProvideEntry::new(
                trove_id,
                lang_dep.to_dep_string(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        // Also store package name as provide
        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            "python-mymodule".to_string(),
            Some("1.0.0".to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    }).unwrap();

    // Verify: check that provides are stored
    let troves = Trove::find_by_name(&conn, "python-mymodule").unwrap();
    assert_eq!(troves.len(), 1);
    let trove_id = troves[0].id.unwrap();

    let provides = ProvideEntry::find_by_trove(&conn, trove_id).unwrap();

    // Should have package name + language provides
    assert!(provides.len() >= 2, "Should have at least package + module provide");

    // Check for python(mymodule) provide
    assert!(
        provides.iter().any(|p| p.capability.contains("python(mymodule)")),
        "Should have python(mymodule) provide"
    );

    // Check for package name provide
    assert!(
        provides.iter().any(|p| p.capability == "python-mymodule"),
        "Should have package name provide"
    );

    // Verify capability lookup works
    let satisfied = ProvideEntry::is_capability_satisfied(&conn, "python-mymodule").unwrap();
    assert!(satisfied, "Package should satisfy its own name");

    println!("Language deps database test passed:");
    println!("  - Stored {} provides for python-mymodule", provides.len());
    for p in &provides {
        println!("    - {} (version: {:?})", p.capability, p.version);
    }
}

/// Test InstallReason tracking for autoremove functionality
#[test]
fn test_install_reason_tracking() {
    use conary::db;
    use conary::db::models::{InstallReason, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Install package explicitly
        let mut explicit_pkg = Trove::new(
            "explicit-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        explicit_pkg.install_reason = InstallReason::Explicit;
        explicit_pkg.insert(tx)?;

        // Install package as dependency
        let mut dep_pkg = Trove::new(
            "dependency-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        dep_pkg.install_reason = InstallReason::Dependency;
        dep_pkg.insert(tx)?;

        Ok(())
    }).unwrap();

    // Verify install reasons are stored correctly
    let explicit = Trove::find_by_name(&conn, "explicit-pkg").unwrap();
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].install_reason, InstallReason::Explicit);

    let dep = Trove::find_by_name(&conn, "dependency-pkg").unwrap();
    assert_eq!(dep.len(), 1);
    assert_eq!(dep[0].install_reason, InstallReason::Dependency);

    println!("Install reason tracking test passed");
}

/// Test collection creation and member management
#[test]
fn test_collection_management() {
    use conary::db;
    use conary::db::models::{CollectionMember, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Create a collection
        let mut collection = Trove::new(
            "dev-tools".to_string(),
            "1.0".to_string(),
            TroveType::Collection,
        );
        collection.description = Some("Development tools collection".to_string());
        let collection_id = collection.insert(tx)?;

        // Add members
        let mut m1 = CollectionMember::new(collection_id, "gcc".to_string());
        m1.insert(tx)?;

        let mut m2 = CollectionMember::new(collection_id, "make".to_string());
        m2.insert(tx)?;

        let mut m3 = CollectionMember::new(collection_id, "gdb".to_string())
            .optional();
        m3.insert(tx)?;

        Ok(())
    }).unwrap();

    // Verify collection was created
    let collections = Trove::find_by_name(&conn, "dev-tools").unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].trove_type, TroveType::Collection);
    assert_eq!(collections[0].description, Some("Development tools collection".to_string()));

    let collection_id = collections[0].id.unwrap();

    // Verify members
    let members = CollectionMember::find_by_collection(&conn, collection_id).unwrap();
    assert_eq!(members.len(), 3);

    // Check member names (should be ordered by name)
    let names: Vec<&str> = members.iter().map(|m| m.member_name.as_str()).collect();
    assert!(names.contains(&"gcc"));
    assert!(names.contains(&"make"));
    assert!(names.contains(&"gdb"));

    // Check that gdb is optional
    let gdb_member = members.iter().find(|m| m.member_name == "gdb").unwrap();
    assert!(gdb_member.is_optional);

    // Check is_member function
    assert!(CollectionMember::is_member(&conn, collection_id, "gcc").unwrap());
    assert!(!CollectionMember::is_member(&conn, collection_id, "clang").unwrap());

    // Check find_collections_containing
    let gcc_collections = CollectionMember::find_collections_containing(&conn, "gcc").unwrap();
    assert_eq!(gcc_collections.len(), 1);
    assert_eq!(gcc_collections[0], collection_id);

    println!("Collection management test passed:");
    println!("  - Created collection: dev-tools");
    println!("  - Members: {:?}", names);
}

/// Test whatprovides query capability
#[test]
fn test_whatprovides_query() {
    use conary::db;
    use conary::db::models::{ProvideEntry, Trove, TroveType};

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Create a package with various provides
        let mut trove = Trove::new(
            "openssl".to_string(),
            "3.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(tx)?;

        // Add provides
        let mut p1 = ProvideEntry::new(trove_id, "openssl".to_string(), Some("3.0.0".to_string()));
        p1.insert(tx)?;

        let mut p2 = ProvideEntry::new(trove_id, "soname(libssl.so.3)".to_string(), None);
        p2.insert(tx)?;

        let mut p3 = ProvideEntry::new(trove_id, "soname(libcrypto.so.3)".to_string(), None);
        p3.insert(tx)?;

        Ok(())
    }).unwrap();

    // Test exact capability lookup
    let providers = ProvideEntry::find_all_by_capability(&conn, "openssl").unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].version, Some("3.0.0".to_string()));

    // Test soname lookup
    let ssl_providers = ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(ssl_providers.len(), 1);

    // Test pattern search
    let pattern_results = ProvideEntry::search_capability(&conn, "soname%").unwrap();
    assert_eq!(pattern_results.len(), 2);

    // Test satisfying provider lookup
    let (provider_name, _version) = ProvideEntry::find_satisfying_provider(&conn, "openssl")
        .unwrap()
        .expect("Should find provider");
    assert_eq!(provider_name, "openssl");

    println!("whatprovides query test passed");
}

// =============================================================================
// COMMAND-LEVEL INTEGRATION TESTS
// =============================================================================
// These tests verify command-equivalent functionality through the library API.
// Commands in src/commands/ are thin wrappers around these operations.

/// Helper to create a test database with packages for command tests
fn setup_command_test_db() -> (tempfile::TempDir, String) {
    use conary::db;
    use conary::db::models::{
        Changeset, ChangesetStatus, Component, DependencyEntry, FileEntry, ProvideEntry, Trove,
        TroveType,
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db").to_str().unwrap().to_string();

    db::init(&db_path).unwrap();
    let mut conn = db::open(&db_path).unwrap();

    db::transaction(&mut conn, |tx| {
        // Create nginx package with files
        let mut changeset1 = Changeset::new("Install nginx-1.24.0".to_string());
        let changeset1_id = changeset1.insert(tx)?;

        let mut nginx = Trove::new("nginx".to_string(), "1.24.0".to_string(), TroveType::Package);
        nginx.architecture = Some("x86_64".to_string());
        nginx.description = Some("High performance web server".to_string());
        nginx.installed_by_changeset_id = Some(changeset1_id);
        let nginx_id = nginx.insert(tx)?;

        // Add nginx components
        let mut nginx_runtime = Component::new(nginx_id, "runtime".to_string());
        let runtime_id = nginx_runtime.insert(tx)?;

        let mut nginx_config = Component::new(nginx_id, "config".to_string());
        let config_id = nginx_config.insert(tx)?;

        // Add nginx files
        let mut f1 = FileEntry::new(
            "/usr/sbin/nginx".to_string(),
            "abc123def456789012345678901234567890123456789012345678901234".to_string(),
            1024000,
            0o755,
            nginx_id,
        );
        f1.component_id = Some(runtime_id);
        f1.insert(tx)?;

        let mut f2 = FileEntry::new(
            "/etc/nginx/nginx.conf".to_string(),
            "def456abc123789012345678901234567890123456789012345678901234".to_string(),
            2048,
            0o644,
            nginx_id,
        );
        f2.component_id = Some(config_id);
        f2.insert(tx)?;

        // Add nginx provides
        let mut p1 = ProvideEntry::new(nginx_id, "nginx".to_string(), Some("1.24.0".to_string()));
        p1.insert(tx)?;
        let mut p2 = ProvideEntry::new(nginx_id, "webserver".to_string(), None);
        p2.insert(tx)?;

        // Add nginx dependency
        let mut dep = DependencyEntry::new(
            nginx_id,
            "openssl".to_string(),
            Some(">= 3.0".to_string()),
            "runtime".to_string(),
            None,
        );
        dep.insert(tx)?;

        changeset1.update_status(tx, ChangesetStatus::Applied)?;

        // Create openssl package
        let mut changeset2 = Changeset::new("Install openssl-3.0.0".to_string());
        let changeset2_id = changeset2.insert(tx)?;

        let mut openssl = Trove::new("openssl".to_string(), "3.0.0".to_string(), TroveType::Package);
        openssl.architecture = Some("x86_64".to_string());
        openssl.description = Some("Cryptography and SSL/TLS toolkit".to_string());
        openssl.installed_by_changeset_id = Some(changeset2_id);
        let openssl_id = openssl.insert(tx)?;

        let mut openssl_runtime = Component::new(openssl_id, "runtime".to_string());
        openssl_runtime.insert(tx)?;

        let mut p3 = ProvideEntry::new(openssl_id, "openssl".to_string(), Some("3.0.0".to_string()));
        p3.insert(tx)?;
        let mut p4 =
            ProvideEntry::new(openssl_id, "soname(libssl.so.3)".to_string(), None);
        p4.insert(tx)?;

        changeset2.update_status(tx, ChangesetStatus::Applied)?;

        Ok(())
    })
    .unwrap();

    (temp_dir, db_path)
}

/// Test package query operations (equivalent to cmd_query)
#[test]
fn test_query_operations() {
    use conary::db;
    use conary::db::models::{FileEntry, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test listing all packages
    let all_troves = Trove::list_all(&conn).unwrap();
    assert_eq!(all_troves.len(), 2, "Should have 2 packages");

    // Test pattern matching
    let nginx_troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_troves.len(), 1, "Should find nginx");
    assert_eq!(nginx_troves[0].version, "1.24.0");
    assert_eq!(nginx_troves[0].description, Some("High performance web server".to_string()));

    // Test file path query
    let file = FileEntry::find_by_path(&conn, "/usr/sbin/nginx").unwrap();
    assert!(file.is_some(), "Should find file by path");
    let file = file.unwrap();
    assert_eq!(file.size, 1024000);
    assert_eq!(file.permissions, 0o755 as i32);

    // Test finding files by package
    let nginx_id = nginx_troves[0].id.unwrap();
    let files = FileEntry::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(files.len(), 2, "nginx should have 2 files");

    // Test non-existent package
    let nonexistent = Trove::find_by_name(&conn, "nonexistent").unwrap();
    assert!(nonexistent.is_empty(), "Should not find nonexistent package");
}

/// Test dependency query operations (equivalent to cmd_depends/cmd_rdepends)
#[test]
fn test_dependency_queries() {
    use conary::db;
    use conary::db::models::{DependencyEntry, ProvideEntry, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx's dependencies
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap();
    let nginx_id = nginx[0].id.unwrap();
    let deps = DependencyEntry::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(deps.len(), 1, "nginx should have 1 dependency");
    assert_eq!(deps[0].depends_on_name, "openssl");
    assert_eq!(deps[0].depends_on_version, Some(">= 3.0".to_string()));

    // Test reverse dependency lookup via provides
    let openssl_providers = ProvideEntry::find_all_by_capability(&conn, "openssl").unwrap();
    assert!(!openssl_providers.is_empty(), "Should find openssl provider");

    // Verify soname provides
    let libssl_providers = ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(libssl_providers.len(), 1, "Should find libssl.so.3 provider");
}

/// Test changeset history (equivalent to cmd_history)
#[test]
fn test_changeset_history() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // List all changesets
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 2, "Should have 2 changesets");

    // Verify changeset details
    let nginx_cs = changesets.iter().find(|c| c.description.contains("nginx")).unwrap();
    assert_eq!(nginx_cs.status, ChangesetStatus::Applied);

    let openssl_cs = changesets.iter().find(|c| c.description.contains("openssl")).unwrap();
    assert_eq!(openssl_cs.status, ChangesetStatus::Applied);

    // Test finding by ID
    let cs_by_id = Changeset::find_by_id(&conn, nginx_cs.id.unwrap()).unwrap();
    assert!(cs_by_id.is_some());
    assert_eq!(cs_by_id.unwrap().description, nginx_cs.description);
}

/// Test whatprovides functionality
#[test]
fn test_whatprovides_operations() {
    use conary::db;
    use conary::db::models::ProvideEntry;

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test finding provider by capability
    let webserver_providers = ProvideEntry::find_all_by_capability(&conn, "webserver").unwrap();
    assert_eq!(webserver_providers.len(), 1);

    // Test soname lookup
    let ssl_providers = ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(ssl_providers.len(), 1);

    // Test pattern search
    let soname_results = ProvideEntry::search_capability(&conn, "soname%").unwrap();
    assert_eq!(soname_results.len(), 1, "Should find 1 soname provide");

    // Test satisfying provider
    let (name, _version) = ProvideEntry::find_satisfying_provider(&conn, "openssl")
        .unwrap()
        .expect("Should find openssl provider");
    assert_eq!(name, "openssl");

    // Test non-existent capability
    let nonexistent = ProvideEntry::find_all_by_capability(&conn, "nonexistent").unwrap();
    assert!(nonexistent.is_empty());
}

/// Test component listing (equivalent to cmd_list_components)
#[test]
fn test_component_listing() {
    use conary::db;
    use conary::db::models::{Component, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx components
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap();
    let nginx_id = nginx[0].id.unwrap();
    let components = Component::find_by_trove(&conn, nginx_id).unwrap();

    assert_eq!(components.len(), 2, "nginx should have 2 components");
    let comp_names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
    assert!(comp_names.contains(&"runtime"));
    assert!(comp_names.contains(&"config"));
}

/// Test state snapshot operations (equivalent to cmd_state_*)
#[test]
fn test_state_snapshot_operations() {
    use conary::db;
    use conary::db::models::{StateEngine, SystemState};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let engine = StateEngine::new(&conn);

    // Create a snapshot
    let state = engine.create_snapshot("Test snapshot", None, None).unwrap();
    assert!(state.id.is_some());
    assert_eq!(state.summary, "Test snapshot");
    // First state is numbered 0 (state_number starts at 0, not 1)
    assert_eq!(state.state_number, 0);

    // List snapshots using SystemState::list_all
    let states = SystemState::list_all(&conn).unwrap();
    assert!(!states.is_empty(), "Should have at least one state");

    // Create another snapshot
    let state2 = engine.create_snapshot("Second snapshot", None, None).unwrap();
    assert!(state2.state_number > state.state_number);

    // Get latest state (highest state_number from list)
    let states = SystemState::list_all(&conn).unwrap();
    assert!(states.len() >= 2);
    // list_all returns DESC order, so first is latest
    assert_eq!(states[0].summary, "Second snapshot");
}

/// Test collection operations (equivalent to cmd_collection_*)
#[test]
fn test_collection_operations() {
    use conary::db;
    use conary::db::models::{CollectionMember, Trove, TroveType};

    let (_temp_dir, db_path) = setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Create a collection
    db::transaction(&mut conn, |tx| {
        let mut collection = Trove::new(
            "webstack".to_string(),
            "1.0".to_string(),
            TroveType::Collection,
        );
        collection.description = Some("Web server stack".to_string());
        let coll_id = collection.insert(tx)?;

        // Add members
        let mut m1 = CollectionMember::new(coll_id, "nginx".to_string());
        m1.insert(tx)?;

        let mut m2 = CollectionMember::new(coll_id, "openssl".to_string());
        m2.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Verify collection
    let collections = Trove::find_by_name(&conn, "webstack").unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].trove_type, TroveType::Collection);

    let coll_id = collections[0].id.unwrap();
    let members = CollectionMember::find_by_collection(&conn, coll_id).unwrap();
    assert_eq!(members.len(), 2);

    // Test membership check
    assert!(CollectionMember::is_member(&conn, coll_id, "nginx").unwrap());
    assert!(CollectionMember::is_member(&conn, coll_id, "openssl").unwrap());
    assert!(!CollectionMember::is_member(&conn, coll_id, "postgresql").unwrap());

    // Test find_collections_containing
    let nginx_collections = CollectionMember::find_collections_containing(&conn, "nginx").unwrap();
    assert_eq!(nginx_collections.len(), 1);
    assert_eq!(nginx_collections[0], coll_id);
}

/// Test changeset rollback tracking
#[test]
fn test_rollback_tracking() {
    use conary::db;
    use conary::db::models::{Changeset, ChangesetStatus, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Get nginx changeset
    let changesets = Changeset::list_all(&conn).unwrap();
    let nginx_cs = changesets
        .iter()
        .find(|c| c.description.contains("nginx"))
        .unwrap();
    let nginx_cs_id = nginx_cs.id.unwrap();

    // Verify nginx exists
    let nginx_before = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_before.len(), 1);

    // Simulate rollback by updating changeset status and removing trove
    db::transaction(&mut conn, |tx| {
        // Mark changeset as rolled back
        let mut cs = Changeset::find_by_id(tx, nginx_cs_id)?.unwrap();
        cs.update_status(tx, ChangesetStatus::RolledBack)?;

        // Remove the trove (simulating rollback)
        Trove::delete(tx, nginx_before[0].id.unwrap())?;

        Ok(())
    })
    .unwrap();

    // Verify rollback
    let nginx_after = Trove::find_by_name(&conn, "nginx").unwrap();
    assert!(nginx_after.is_empty(), "nginx should be removed after rollback");

    let cs_after = Changeset::find_by_id(&conn, nginx_cs_id).unwrap().unwrap();
    assert_eq!(cs_after.status, ChangesetStatus::RolledBack);
}

/// Test install reason queries (equivalent to cmd_query_reason)
#[test]
fn test_install_reason_queries() {
    use conary::db;
    use conary::db::models::{InstallReason, InstallSource, Trove, TroveType};

    let (_temp_dir, db_path) = setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Set install reasons
    db::transaction(&mut conn, |tx| {
        let nginx = Trove::find_by_name(tx, "nginx")?.pop().unwrap();
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE id = ?2",
            rusqlite::params![InstallReason::Explicit.as_str(), nginx.id],
        )?;

        let openssl = Trove::find_by_name(tx, "openssl")?.pop().unwrap();
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE id = ?2",
            rusqlite::params![InstallReason::Dependency.as_str(), openssl.id],
        )?;

        Ok(())
    })
    .unwrap();

    // Query by install reason
    let explicit: Vec<Trove> = conn
        .prepare("SELECT * FROM troves WHERE install_reason = ?1")
        .unwrap()
        .query_map([InstallReason::Explicit.as_str()], |row| {
            Ok(Trove {
                id: row.get("id")?,
                name: row.get("name")?,
                version: row.get("version")?,
                trove_type: TroveType::Package,
                architecture: row.get("architecture").ok(),
                description: row.get("description").ok(),
                installed_at: row.get("installed_at").ok(),
                installed_by_changeset_id: row.get("installed_by_changeset_id").ok(),
                install_source: InstallSource::Repository,
                install_reason: InstallReason::Explicit,
                selection_reason: None,
                pinned: false,
                flavor_spec: None,
                label_id: None,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].name, "nginx");
}

/// Test dependency tree building
#[test]
fn test_dependency_tree() {
    use conary::db;
    use conary::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Build dependency tree for nginx
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_deps = DependencyEntry::find_by_trove(&conn, nginx.id.unwrap()).unwrap();

    // nginx depends on openssl
    assert_eq!(nginx_deps.len(), 1);
    assert_eq!(nginx_deps[0].depends_on_name, "openssl");

    // openssl has no dependencies in our test setup
    let openssl = Trove::find_by_name(&conn, "openssl").unwrap().pop().unwrap();
    let openssl_deps = DependencyEntry::find_by_trove(&conn, openssl.id.unwrap()).unwrap();
    assert!(openssl_deps.is_empty(), "openssl should have no deps in test");

    // This verifies the structure needed for deptree command
}

/// Test what-breaks analysis (reverse dependency check)
#[test]
fn test_what_breaks_analysis() {
    use conary::db;
    use conary::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Find what depends on openssl
    // This requires checking all packages' dependencies
    let all_troves = Trove::list_all(&conn).unwrap();
    let mut dependents = Vec::new();

    for trove in &all_troves {
        if let Some(id) = trove.id {
            let deps = DependencyEntry::find_by_trove(&conn, id).unwrap();
            for dep in deps {
                if dep.depends_on_name == "openssl" {
                    dependents.push(trove.name.clone());
                }
            }
        }
    }

    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0], "nginx");

    // This verifies: removing openssl would break nginx
}

/// Test config file tracking (equivalent to config management commands)
#[test]
fn test_config_file_tracking() {
    use conary::db;
    use conary::db::models::{ConfigFile, ConfigSource, ConfigStatus, Trove};

    let (_temp_dir, db_path) = setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Get nginx trove id
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_id = nginx.id.unwrap();

    // Create config file entries
    let mut config1 = ConfigFile::new(
        "/etc/nginx/nginx.conf".to_string(),
        nginx_id,
        "abc123hash".to_string(),
    );
    config1.source = ConfigSource::Auto;
    config1.insert(&conn).unwrap();

    let mut config2 = ConfigFile::new_noreplace(
        "/etc/nginx/sites-enabled/default".to_string(),
        nginx_id,
        "def456hash".to_string(),
    );
    config2.source = ConfigSource::Rpm;
    config2.insert(&conn).unwrap();

    // List all configs for nginx
    let configs = ConfigFile::find_by_trove(&conn, nginx_id).unwrap();
    assert_eq!(configs.len(), 2);

    // Find by path
    let found = ConfigFile::find_by_path(&conn, "/etc/nginx/nginx.conf")
        .unwrap()
        .unwrap();
    assert_eq!(found.original_hash, "abc123hash");
    assert_eq!(found.status, ConfigStatus::Pristine);

    // Mark as modified (simulating user edit)
    found
        .mark_modified(&conn, "modified_hash_123")
        .unwrap();

    // Find modified configs
    let modified = ConfigFile::find_modified(&conn).unwrap();
    assert_eq!(modified.len(), 1);
    assert_eq!(modified[0].path, "/etc/nginx/nginx.conf");

    // Verify noreplace flag
    let sites_config = ConfigFile::find_by_path(&conn, "/etc/nginx/sites-enabled/default")
        .unwrap()
        .unwrap();
    assert!(sites_config.noreplace);
    assert_eq!(sites_config.source, ConfigSource::Rpm);

    // Mark as missing
    sites_config.mark_missing(&conn).unwrap();
    let missing = ConfigFile::find_by_status(&conn, ConfigStatus::Missing).unwrap();
    assert_eq!(missing.len(), 1);
}
