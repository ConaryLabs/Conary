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
                orphan_since: None,
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

// =============================================================================
// DERIVED PACKAGE TESTS
// =============================================================================

/// Test derived package creation and basic operations
#[test]
fn test_derived_package_creation() {
    use conary::db::models::{DerivedPackage, DerivedStatus, VersionPolicy};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create a derived package from nginx
    let mut derived = DerivedPackage::new(
        "nginx-custom".to_string(),
        "nginx".to_string(),
    );
    derived.description = Some("Custom nginx with patches".to_string());
    derived.version_policy = VersionPolicy::Suffix("+custom".to_string());

    let derived_id = derived.insert(&conn).unwrap();
    assert!(derived_id > 0);

    // Verify it was created
    let found = DerivedPackage::find_by_name(&conn, "nginx-custom").unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.parent_name, "nginx");
    assert_eq!(found.status, DerivedStatus::Pending);
    assert_eq!(found.version_policy, VersionPolicy::Suffix("+custom".to_string()));

    // List all derived packages
    let all = DerivedPackage::list_all(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "nginx-custom");
}

/// Test derived package patches
#[test]
fn test_derived_package_patches() {
    use conary::db::models::{DerivedPackage, DerivedPatch};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new(
        "nginx-patched".to_string(),
        "nginx".to_string(),
    );
    let derived_id = derived.insert(&conn).unwrap();

    // Add patches
    let mut patch1 = DerivedPatch::new(
        derived_id,
        1,
        "security-fix.patch".to_string(),
        "abc123hash".to_string(),
    );
    patch1.insert(&conn).unwrap();

    let mut patch2 = DerivedPatch::new(
        derived_id,
        2,
        "performance.patch".to_string(),
        "def456hash".to_string(),
    );
    patch2.strip_level = 2; // Use -p2
    patch2.insert(&conn).unwrap();

    // Retrieve patches
    let patches = DerivedPatch::find_by_derived(&conn, derived_id).unwrap();
    assert_eq!(patches.len(), 2);

    // Patches should be ordered
    assert_eq!(patches[0].patch_name, "security-fix.patch");
    assert_eq!(patches[0].patch_order, 1);
    assert_eq!(patches[0].strip_level, 1); // Default

    assert_eq!(patches[1].patch_name, "performance.patch");
    assert_eq!(patches[1].patch_order, 2);
    assert_eq!(patches[1].strip_level, 2);

    // Test cascade delete
    DerivedPackage::delete(&conn, derived_id).unwrap();
    let patches_after = DerivedPatch::find_by_derived(&conn, derived_id).unwrap();
    assert!(patches_after.is_empty());
}

/// Test derived package file overrides
#[test]
fn test_derived_package_overrides() {
    use conary::db::models::{DerivedOverride, DerivedPackage};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new(
        "nginx-config".to_string(),
        "nginx".to_string(),
    );
    let derived_id = derived.insert(&conn).unwrap();

    // Add file replacement
    let mut override1 = DerivedOverride::new_replace(
        derived_id,
        "/etc/nginx/nginx.conf".to_string(),
        "custom_config_hash".to_string(),
    );
    override1.source_path = Some("custom-nginx.conf".to_string());
    override1.permissions = Some(0o644);
    override1.insert(&conn).unwrap();

    // Add file removal
    let mut override2 = DerivedOverride::new_remove(
        derived_id,
        "/etc/nginx/sites-enabled/default".to_string(),
    );
    override2.insert(&conn).unwrap();

    // Retrieve overrides
    let overrides = DerivedOverride::find_by_derived(&conn, derived_id).unwrap();
    assert_eq!(overrides.len(), 2);

    // Check replacement
    let replacement = overrides.iter().find(|o| o.target_path == "/etc/nginx/nginx.conf").unwrap();
    assert!(!replacement.is_removal());
    assert_eq!(replacement.source_hash.as_deref(), Some("custom_config_hash"));
    assert_eq!(replacement.permissions, Some(0o644));

    // Check removal
    let removal = overrides.iter().find(|o| o.target_path == "/etc/nginx/sites-enabled/default").unwrap();
    assert!(removal.is_removal());
    assert!(removal.source_hash.is_none());

    // Test find by path
    let found = DerivedOverride::find_by_path(&conn, derived_id, "/etc/nginx/nginx.conf").unwrap();
    assert!(found.is_some());
}

/// Test derived package status transitions
#[test]
fn test_derived_package_status() {
    use conary::db::models::{DerivedPackage, DerivedStatus, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new(
        "nginx-status-test".to_string(),
        "nginx".to_string(),
    );
    let _derived_id = derived.insert(&conn).unwrap();

    // Initial status is Pending
    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test").unwrap().unwrap();
    assert_eq!(found.status, DerivedStatus::Pending);

    // Mark as built (need to create a trove first)
    let mut built_trove = Trove::new(
        "nginx-status-test".to_string(),
        "1.24.0+custom".to_string(),
        TroveType::Package,
    );
    let trove_id = built_trove.insert(&conn).unwrap();

    // Re-fetch as mutable to call mark_built
    let mut found = DerivedPackage::find_by_name(&conn, "nginx-status-test").unwrap().unwrap();
    found.mark_built(&conn, trove_id).unwrap();

    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test").unwrap().unwrap();
    assert_eq!(found.status, DerivedStatus::Built);
    assert_eq!(found.built_trove_id, Some(trove_id));

    // Mark parent as stale
    DerivedPackage::mark_stale(&conn, "nginx").unwrap();
    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test").unwrap().unwrap();
    assert_eq!(found.status, DerivedStatus::Stale);

    // Find stale packages
    let stale = DerivedPackage::find_by_status(&conn, DerivedStatus::Stale).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].name, "nginx-status-test");
}

/// Test version policy computation
#[test]
fn test_derived_version_policy() {
    use conary::db::models::VersionPolicy;

    // Inherit policy
    let inherit = VersionPolicy::Inherit;
    assert_eq!(inherit.compute_version("1.24.0"), "1.24.0");

    // Suffix policy
    let suffix = VersionPolicy::Suffix("+custom".to_string());
    assert_eq!(suffix.compute_version("1.24.0"), "1.24.0+custom");

    // Specific policy
    let specific = VersionPolicy::Specific("2.0.0".to_string());
    assert_eq!(specific.compute_version("1.24.0"), "2.0.0");
}

// =============================================================================
// SYSTEM MODEL TESTS
// =============================================================================

/// Test system model parsing
#[test]
fn test_system_model_parsing() {
    use conary::model::parse_model_file;
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a test model file
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(file, r#"
[model]
version = 1
search = ["fedora@f41:stable"]
install = ["nginx", "postgresql"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"

[optional]
packages = ["nginx-module-geoip"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = []
"#).unwrap();

    // Parse the model
    let model = parse_model_file(&model_path).unwrap();

    assert_eq!(model.config.version, 1);
    assert_eq!(model.config.install, vec!["nginx", "postgresql"]);
    assert_eq!(model.config.exclude, vec!["sendmail"]);
    assert_eq!(model.config.search, vec!["fedora@f41:stable"]);
    assert_eq!(model.pin.get("openssl"), Some(&"3.0.*".to_string()));
    assert_eq!(model.optional.packages, vec!["nginx-module-geoip"]);
    assert_eq!(model.derive.len(), 1);
    assert_eq!(model.derive[0].name, "nginx-custom");
    assert_eq!(model.derive[0].from, "nginx");
}

/// Test system model diff computation
#[test]
fn test_system_model_diff() {
    use conary::model::{compute_diff, DiffAction, SystemState};
    use conary::model::parse_model_file;
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a model requesting nginx and redis
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(file, r#"
[model]
version = 1
install = ["nginx", "redis"]
exclude = ["sendmail"]
"#).unwrap();

    let model = parse_model_file(&model_path).unwrap();

    // Create a state with only nginx installed
    let mut state = SystemState::new();
    state.installed.insert("nginx".to_string(), conary::model::InstalledPackage {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        architecture: None,
        explicit: true,
        label: None,
    });
    state.explicit.insert("nginx".to_string());

    // Also have sendmail installed (should be removed)
    state.installed.insert("sendmail".to_string(), conary::model::InstalledPackage {
        name: "sendmail".to_string(),
        version: "8.0.0".to_string(),
        architecture: None,
        explicit: true,
        label: None,
    });
    state.explicit.insert("sendmail".to_string());

    // Compute diff
    let diff = compute_diff(&model, &state);

    // Should need to install redis
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Install { package, .. } if package == "redis"
    )), "Should need to install redis");

    // Should need to remove sendmail (excluded)
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Remove { package, .. } if package == "sendmail"
    )), "Should need to remove sendmail");

    // nginx is already installed, no action needed for it
    assert!(!diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::Install { package, .. } if package == "nginx"
    )), "Should not need to install nginx again");
}

/// Test system model diff with derived packages
#[test]
fn test_system_model_diff_derived() {
    use conary::model::{compute_diff, DiffAction, SystemState};
    use conary::model::parse_model_file;
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("system.toml");

    // Write a model with a derived package
    let mut file = std::fs::File::create(&model_path).unwrap();
    writeln!(file, r#"
[model]
version = 1
install = ["nginx"]

[[derive]]
name = "nginx-custom"
from = "nginx"
version = "inherit"
patches = []
"#).unwrap();

    let model = parse_model_file(&model_path).unwrap();

    // State with nginx installed but not the derived package
    let mut state = SystemState::new();
    state.installed.insert("nginx".to_string(), conary::model::InstalledPackage {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        architecture: None,
        explicit: true,
        label: None,
    });
    state.explicit.insert("nginx".to_string());

    // Compute diff
    let diff = compute_diff(&model, &state);

    // Should need to build the derived package
    assert!(diff.actions.iter().any(|a| matches!(
        a,
        DiffAction::BuildDerived { name, parent, needs_parent }
        if name == "nginx-custom" && parent == "nginx" && !*needs_parent
    )), "Should need to build derived package with parent already installed");
}

/// Test system model state capture
#[test]
fn test_system_model_state_capture() {
    use conary::model::capture_current_state;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Capture current state
    let state = capture_current_state(&conn).unwrap();

    // Should have nginx and openssl from the test db
    assert!(state.is_installed("nginx"), "nginx should be installed");
    assert!(state.is_installed("openssl"), "openssl should be installed");

    // Check nginx details
    let nginx = state.installed.get("nginx").unwrap();
    assert_eq!(nginx.version, "1.24.0");
}

/// Test system model snapshot to model conversion
#[test]
fn test_system_model_snapshot() {
    use conary::model::{capture_current_state, snapshot_to_model};
    use conary::db::models::InstallReason;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Mark nginx as explicit
    db::transaction(&mut conn, |tx| {
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE name = ?2",
            rusqlite::params![InstallReason::Explicit.as_str(), "nginx"],
        )?;
        Ok(())
    }).unwrap();

    // Capture state and convert to model
    let state = capture_current_state(&conn).unwrap();
    let model = snapshot_to_model(&state);

    // Model should include explicitly installed packages
    assert!(model.config.install.contains(&"nginx".to_string()),
        "Model should include explicitly installed nginx");
}

// =============================================================================
// REFERENCE MIRROR TESTS
// =============================================================================

/// Test repository with content mirror (reference mirror pattern)
#[test]
fn test_reference_mirror_creation() {
    use conary::db::models::Repository;

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Create repository with separate metadata and content URLs
    let mut repo = Repository::with_content_mirror(
        "fedora-mirror".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
        "https://local-cache.example.com/fedora".to_string(),
    );

    repo.insert(&conn).unwrap();

    // Retrieve and verify
    let found = Repository::find_by_name(&conn, "fedora-mirror").unwrap().unwrap();
    assert_eq!(found.url, "https://mirrors.fedoraproject.org/metalink");
    assert_eq!(found.content_url, Some("https://local-cache.example.com/fedora".to_string()));

    // Effective content URL should be the content_url
    assert_eq!(found.effective_content_url(), "https://local-cache.example.com/fedora");
}

/// Test repository without content mirror (standard pattern)
#[test]
fn test_repository_without_content_mirror() {
    use conary::db::models::Repository;

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Create standard repository (no content mirror)
    let mut repo = Repository::new(
        "fedora".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
    );

    repo.insert(&conn).unwrap();

    // Retrieve and verify
    let found = Repository::find_by_name(&conn, "fedora").unwrap().unwrap();
    assert_eq!(found.url, "https://mirrors.fedoraproject.org/metalink");
    assert!(found.content_url.is_none());

    // Effective content URL should fall back to url
    assert_eq!(found.effective_content_url(), "https://mirrors.fedoraproject.org/metalink");
}

/// Test multiple repositories with different mirror configurations
#[test]
fn test_mixed_mirror_configurations() {
    use conary::db::models::Repository;

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Standard repo
    let mut repo1 = Repository::new(
        "updates".to_string(),
        "https://updates.example.com".to_string(),
    );
    repo1.priority = 10;
    repo1.insert(&conn).unwrap();

    // Reference mirror repo
    let mut repo2 = Repository::with_content_mirror(
        "base".to_string(),
        "https://metadata.example.com".to_string(),
        "https://cdn.example.com/packages".to_string(),
    );
    repo2.priority = 5;
    repo2.insert(&conn).unwrap();

    // List all and verify ordering (by priority DESC)
    let repos = Repository::list_all(&conn).unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].name, "updates"); // Higher priority
    assert_eq!(repos[1].name, "base");

    // Verify effective URLs
    assert_eq!(repos[0].effective_content_url(), "https://updates.example.com");
    assert_eq!(repos[1].effective_content_url(), "https://cdn.example.com/packages");
}

/// Test repository update preserves content_url
#[test]
fn test_repository_update_content_mirror() {
    use conary::db::models::Repository;

    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();
    drop(temp_file);

    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();

    // Create repo with content mirror
    let mut repo = Repository::with_content_mirror(
        "test-repo".to_string(),
        "https://old-metadata.example.com".to_string(),
        "https://old-cdn.example.com".to_string(),
    );
    repo.insert(&conn).unwrap();

    // Update URLs
    let mut found = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
    found.url = "https://new-metadata.example.com".to_string();
    found.content_url = Some("https://new-cdn.example.com".to_string());
    found.update(&conn).unwrap();

    // Verify update
    let updated = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
    assert_eq!(updated.url, "https://new-metadata.example.com");
    assert_eq!(updated.content_url, Some("https://new-cdn.example.com".to_string()));
}
