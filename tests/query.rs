// tests/query.rs

//! Query operation tests: package queries, dependency lookups, provides, changesets.

mod common;

use conary::db;
use tempfile::NamedTempFile;

#[test]
fn test_query_packages() {
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

/// Test whatprovides query capability
#[test]
fn test_whatprovides_query() {
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
    })
    .unwrap();

    // Test exact capability lookup
    let providers = ProvideEntry::find_all_by_capability(&conn, "openssl").unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].version, Some("3.0.0".to_string()));

    // Test soname lookup
    let ssl_providers =
        ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(ssl_providers.len(), 1);

    // Test pattern search
    let pattern_results = ProvideEntry::search_capability(&conn, "soname%").unwrap();
    assert_eq!(pattern_results.len(), 2);

    // Test satisfying provider lookup
    let (provider_name, _version) = ProvideEntry::find_satisfying_provider(&conn, "openssl")
        .unwrap()
        .expect("Should find provider");
    assert_eq!(provider_name, "openssl");
}

// =============================================================================
// COMMAND-LEVEL QUERY TESTS
// =============================================================================

/// Test package query operations (equivalent to cmd_query)
#[test]
fn test_query_operations() {
    use conary::db::models::{FileEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test listing all packages
    let all_troves = Trove::list_all(&conn).unwrap();
    assert_eq!(all_troves.len(), 2, "Should have 2 packages");

    // Test pattern matching
    let nginx_troves = Trove::find_by_name(&conn, "nginx").unwrap();
    assert_eq!(nginx_troves.len(), 1, "Should find nginx");
    assert_eq!(nginx_troves[0].version, "1.24.0");
    assert_eq!(
        nginx_troves[0].description,
        Some("High performance web server".to_string())
    );

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
    use conary::db::models::{DependencyEntry, ProvideEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
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
    let libssl_providers =
        ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
    assert_eq!(libssl_providers.len(), 1, "Should find libssl.so.3 provider");
}

/// Test changeset history (equivalent to cmd_history)
#[test]
fn test_changeset_history() {
    use conary::db::models::{Changeset, ChangesetStatus};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // List all changesets
    let changesets = Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 2, "Should have 2 changesets");

    // Verify changeset details
    let nginx_cs = changesets
        .iter()
        .find(|c| c.description.contains("nginx"))
        .unwrap();
    assert_eq!(nginx_cs.status, ChangesetStatus::Applied);

    let openssl_cs = changesets
        .iter()
        .find(|c| c.description.contains("openssl"))
        .unwrap();
    assert_eq!(openssl_cs.status, ChangesetStatus::Applied);

    // Test finding by ID
    let cs_by_id = Changeset::find_by_id(&conn, nginx_cs.id.unwrap()).unwrap();
    assert!(cs_by_id.is_some());
    assert_eq!(cs_by_id.unwrap().description, nginx_cs.description);
}

/// Test whatprovides functionality
#[test]
fn test_whatprovides_operations() {
    use conary::db::models::ProvideEntry;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Test finding provider by capability
    let webserver_providers = ProvideEntry::find_all_by_capability(&conn, "webserver").unwrap();
    assert_eq!(webserver_providers.len(), 1);

    // Test soname lookup
    let ssl_providers =
        ProvideEntry::find_all_by_capability(&conn, "soname(libssl.so.3)").unwrap();
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

/// Test dependency tree building
#[test]
fn test_dependency_tree() {
    use conary::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Build dependency tree for nginx
    let nginx = Trove::find_by_name(&conn, "nginx").unwrap().pop().unwrap();
    let nginx_deps = DependencyEntry::find_by_trove(&conn, nginx.id.unwrap()).unwrap();

    // nginx depends on openssl
    assert_eq!(nginx_deps.len(), 1);
    assert_eq!(nginx_deps[0].depends_on_name, "openssl");

    // openssl has no dependencies in our test setup
    let openssl = Trove::find_by_name(&conn, "openssl")
        .unwrap()
        .pop()
        .unwrap();
    let openssl_deps = DependencyEntry::find_by_trove(&conn, openssl.id.unwrap()).unwrap();
    assert!(openssl_deps.is_empty(), "openssl should have no deps in test");

    // This verifies the structure needed for deptree command
}

/// Test what-breaks analysis (reverse dependency check)
#[test]
fn test_what_breaks_analysis() {
    use conary::db::models::{DependencyEntry, Trove};

    let (_temp_dir, db_path) = common::setup_command_test_db();
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
