// tests/features.rs

//! Feature-specific tests: language dependencies, install reasons, collections, config files.

mod common;

use conary::db;
use tempfile::NamedTempFile;

// =============================================================================
// LANGUAGE DEPENDENCY TESTS
// =============================================================================

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
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Python && p.name == "requests"),
        "Should detect python(requests) provide"
    );
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Python && p.name == "urllib3"),
        "Should detect python(urllib3) provide"
    );
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Python && p.name == "numpy"),
        "Should detect python(numpy) provide from .so"
    );

    // Should detect Perl modules
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Perl && p.name == "DBI"),
        "Should detect perl(DBI) provide"
    );
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Perl && p.name == "DBD::SQLite"),
        "Should detect perl(DBD::SQLite) provide"
    );

    // Should detect sonames
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::Soname && p.name == "libssl.so.3"),
        "Should detect soname(libssl.so.3) provide"
    );

    // Should detect file provides
    assert!(
        provides
            .iter()
            .any(|p| p.class == DependencyClass::File && p.name == "/usr/bin/python3"),
        "Should detect file(/usr/bin/python3) provide"
    );
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
}

/// Test language deps stored in database during install
#[test]
fn test_language_deps_in_database() {
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
    assert!(
        !language_provides.is_empty(),
        "Should detect at least one provide"
    );

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
            let mut entry = FileEntry::new(path.clone(), hash, 100, 0o644, trove_id);
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
    })
    .unwrap();

    // Verify: check that provides are stored
    let troves = Trove::find_by_name(&conn, "python-mymodule").unwrap();
    assert_eq!(troves.len(), 1);
    let trove_id = troves[0].id.unwrap();

    let provides = ProvideEntry::find_by_trove(&conn, trove_id).unwrap();

    // Should have package name + language provides
    assert!(
        provides.len() >= 2,
        "Should have at least package + module provide"
    );

    // Check for python(mymodule) provide
    assert!(
        provides
            .iter()
            .any(|p| p.capability.contains("python(mymodule)")),
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
}

// =============================================================================
// INSTALL REASON TESTS
// =============================================================================

/// Test InstallReason tracking for autoremove functionality
#[test]
fn test_install_reason_tracking() {
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
    })
    .unwrap();

    // Verify install reasons are stored correctly
    let explicit = Trove::find_by_name(&conn, "explicit-pkg").unwrap();
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].install_reason, InstallReason::Explicit);

    let dep = Trove::find_by_name(&conn, "dependency-pkg").unwrap();
    assert_eq!(dep.len(), 1);
    assert_eq!(dep[0].install_reason, InstallReason::Dependency);
}

/// Test install reason queries (equivalent to cmd_query_reason)
#[test]
fn test_install_reason_queries() {
    use conary::db::models::{InstallReason, InstallSource, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
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

// =============================================================================
// COLLECTION TESTS
// =============================================================================

/// Test collection creation and member management
#[test]
fn test_collection_management() {
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

        let mut m3 = CollectionMember::new(collection_id, "gdb".to_string()).optional();
        m3.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Verify collection was created
    let collections = Trove::find_by_name(&conn, "dev-tools").unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].trove_type, TroveType::Collection);
    assert_eq!(
        collections[0].description,
        Some("Development tools collection".to_string())
    );

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
}

/// Test collection operations (equivalent to cmd_collection_*)
#[test]
fn test_collection_operations() {
    use conary::db::models::{CollectionMember, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
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

// =============================================================================
// STATE SNAPSHOT TESTS
// =============================================================================

/// Test state snapshot operations (equivalent to cmd_state_*)
#[test]
fn test_state_snapshot_operations() {
    use conary::db::models::{StateEngine, SystemState};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let engine = StateEngine::new(&conn);

    // Create a snapshot
    let state = engine
        .create_snapshot("Test snapshot", None, None)
        .unwrap();
    assert!(state.id.is_some());
    assert_eq!(state.summary, "Test snapshot");
    // First state is numbered 0 (state_number starts at 0, not 1)
    assert_eq!(state.state_number, 0);

    // List snapshots using SystemState::list_all
    let states = SystemState::list_all(&conn).unwrap();
    assert!(!states.is_empty(), "Should have at least one state");

    // Create another snapshot
    let state2 = engine
        .create_snapshot("Second snapshot", None, None)
        .unwrap();
    assert!(state2.state_number > state.state_number);

    // Get latest state (highest state_number from list)
    let states = SystemState::list_all(&conn).unwrap();
    assert!(states.len() >= 2);
    // list_all returns DESC order, so first is latest
    assert_eq!(states[0].summary, "Second snapshot");
}

// =============================================================================
// CONFIG FILE TESTS
// =============================================================================

/// Test config file tracking (equivalent to config management commands)
#[test]
fn test_config_file_tracking() {
    use conary::db::models::{ConfigFile, ConfigSource, ConfigStatus, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
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
    found.mark_modified(&conn, "modified_hash_123").unwrap();

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
