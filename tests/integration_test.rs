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
