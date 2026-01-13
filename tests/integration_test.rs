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
