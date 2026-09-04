// crates/conary-core/src/automation/check/tests.rs

use super::*;

#[test]
fn test_check_results_total() {
    let mut results = CheckResults::default();
    assert_eq!(results.total(), 0);

    results.security.push(security_update_action(
        &["test".to_string()],
        "1.0.1",
        None,
        &[],
        "high",
    ));
    assert_eq!(results.total(), 1);
}

#[test]
fn test_find_corrupted_files_empty_db() {
    // Create in-memory database with schema
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                content_sha256 TEXT,
                trove_id INTEGER NOT NULL
            );
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL
            );",
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    // No files = no corruption
    let corrupted = checker.find_corrupted_files().unwrap();
    assert!(corrupted.is_empty());
}

#[test]
fn test_find_corrupted_files_detects_mismatch() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Create in-memory database
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                content_sha256 TEXT,
                trove_id INTEGER NOT NULL
            );",
    )
    .unwrap();

    // Create a temp file with known content
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "hello world").unwrap();
    temp_file.flush().unwrap();
    let temp_path = temp_file.path().to_str().unwrap();

    // The correct hash for "hello world" is:
    let correct_hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";

    // Insert file with WRONG hash - should be detected as corrupted
    conn.execute(
        "INSERT INTO files (path, content_sha256, trove_id)
             VALUES (?1, ?2, 1)",
        [temp_path, wrong_hash],
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    let corrupted = checker.find_corrupted_files().unwrap();
    assert_eq!(corrupted.len(), 1);
    assert_eq!(corrupted[0], temp_path);

    // Now update with correct hash - should NOT be detected
    conn.execute(
        "UPDATE files SET content_sha256 = ?1 WHERE path = ?2",
        [correct_hash, temp_path],
    )
    .unwrap();

    let corrupted = checker.find_corrupted_files().unwrap();
    assert!(corrupted.is_empty());
}

#[test]
fn test_find_corrupted_files_missing_file() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                content_sha256 TEXT,
                trove_id INTEGER NOT NULL
            );",
    )
    .unwrap();

    // Insert a file that doesn't exist
    conn.execute(
        "INSERT INTO files (path, content_sha256, trove_id)
             VALUES ('/nonexistent/file/path/abc123.txt', 'abc123', 1)",
        [],
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    let corrupted = checker.find_corrupted_files().unwrap();
    assert_eq!(corrupted.len(), 1);
    assert_eq!(corrupted[0], "/nonexistent/file/path/abc123.txt");
}

#[test]
fn test_filter_by_grace_period_empty() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    // Empty list returns empty
    let result = checker
        .filter_by_grace_period(&[], std::time::Duration::from_secs(3600))
        .unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_filter_by_grace_period_marks_new_orphans() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );
            INSERT INTO troves (name) VALUES ('libfoo');",
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    // First time detecting orphan - should mark it but not return it
    let result = checker
        .filter_by_grace_period(
            &["libfoo".to_string()],
            std::time::Duration::from_secs(3600),
        )
        .unwrap();
    assert!(
        result.is_empty(),
        "New orphan should not be returned immediately"
    );

    // Verify orphan_since was set
    let orphan_since: Option<String> = conn
        .query_row(
            "SELECT orphan_since FROM troves WHERE name = 'libfoo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(orphan_since.is_some(), "orphan_since should be set");
}

#[test]
fn test_filter_by_grace_period_respects_grace() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
    )
    .unwrap();

    // Insert package that was orphaned 2 hours ago
    let two_hours_ago = Utc::now() - Duration::hours(2);
    conn.execute(
        "INSERT INTO troves (name, orphan_since) VALUES ('libold', ?1)",
        [two_hours_ago.to_rfc3339()],
    )
    .unwrap();

    // Insert package that was orphaned 30 minutes ago
    let thirty_mins_ago = Utc::now() - Duration::minutes(30);
    conn.execute(
        "INSERT INTO troves (name, orphan_since) VALUES ('libnew', ?1)",
        [thirty_mins_ago.to_rfc3339()],
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    // With 1 hour grace period, only libold should be returned
    let result = checker
        .filter_by_grace_period(
            &["libold".to_string(), "libnew".to_string()],
            std::time::Duration::from_secs(3600), // 1 hour
        )
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], "libold");
}

#[test]
fn test_clear_orphan_status() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                orphan_since TEXT
            );",
    )
    .unwrap();

    // Insert package with orphan_since set
    let yesterday = Utc::now() - Duration::days(1);
    conn.execute(
        "INSERT INTO troves (name, orphan_since) VALUES ('libfoo', ?1)",
        [yesterday.to_rfc3339()],
    )
    .unwrap();

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);

    // Clear orphan status
    checker
        .clear_orphan_status(&["libfoo".to_string()])
        .unwrap();

    // Verify orphan_since is now NULL
    let orphan_since: Option<String> = conn
        .query_row(
            "SELECT orphan_since FROM troves WHERE name = 'libfoo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(orphan_since.is_none(), "orphan_since should be cleared");
}

#[test]
fn test_check_integrity_groups_corruption_by_package() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_a1 = temp_dir.path().join("pkg-a-bin");
    let missing_a2 = temp_dir.path().join("pkg-a-lib");
    let missing_b1 = temp_dir.path().join("pkg-b-bin");

    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                architecture TEXT
            );
            CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                content_sha256 TEXT,
                trove_id INTEGER NOT NULL
            );",
    )
    .unwrap();

    conn.execute(
            "INSERT INTO troves (id, name, version, architecture) VALUES (1, 'pkg-a', '1.0.0', 'x86_64')",
            [],
        )
        .unwrap();
    conn.execute(
            "INSERT INTO troves (id, name, version, architecture) VALUES (2, 'pkg-b', '2.0.0', 'x86_64')",
            [],
        )
        .unwrap();

    for (path, trove_id) in [
        (missing_a1, 1_i64),
        (missing_a2, 1_i64),
        (missing_b1, 2_i64),
    ] {
        conn.execute(
            "INSERT INTO files (path, content_sha256, trove_id)
                 VALUES (?1, 'deadbeef', ?2)",
            rusqlite::params![path.to_string_lossy(), trove_id],
        )
        .unwrap();
    }

    let config = AutomationConfig::default();
    let checker = AutomationChecker::new(&conn, &config);
    let mut results = CheckResults::default();

    checker.check_integrity(&mut results).unwrap();

    assert_eq!(results.integrity.len(), 2);
    let repaired_packages: Vec<_> = results
        .integrity
        .iter()
        .map(|action| match &action.payload {
            super::super::ActionPayload::RestorePackage { installed } => installed.name.as_str(),
            other => panic!("unexpected payload: {other:?}"),
        })
        .collect();
    assert!(repaired_packages.contains(&"pkg-a"));
    assert!(repaired_packages.contains(&"pkg-b"));
}
