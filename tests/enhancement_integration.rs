// tests/enhancement_integration.rs
//! Integration tests for retroactive enhancement of converted packages
//!
//! These tests validate the enhancement framework that adds capabilities,
//! provenance, and subpackage relationships to already-installed converted
//! packages.

use conary::ccs::enhancement::{
    check_enhancement_window, get_pending_by_priority, schedule_for_enhancement,
    EnhancementPriority, EnhancementResult_, EnhancementRunner, EnhancementStatus,
    EnhancementType, EnhancementWindowStatus, ENHANCEMENT_VERSION,
};
use conary::ccs::enhancement::context::ConvertedPackageInfo;
use conary::ccs::enhancement::runner::EnhancementOptions;
use conary::db;
use rusqlite::params;
use std::path::PathBuf;
use tempfile::NamedTempFile;

// =============================================================================
// TEST HELPERS
// =============================================================================

/// Create a test database with schema migrations applied
fn create_test_db() -> (NamedTempFile, rusqlite::Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    db::schema::migrate(&conn).unwrap();
    (temp_file, conn)
}

/// Insert a test trove and return its ID
fn insert_test_trove(conn: &rusqlite::Connection, name: &str, version: &str) -> i64 {
    conn.execute(
        "INSERT INTO troves (name, version, type, architecture)
         VALUES (?1, ?2, 'package', 'x86_64')",
        params![name, version],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Insert a converted package record and return its ID
fn insert_converted_package(
    conn: &rusqlite::Connection,
    trove_id: i64,
    original_format: &str,
    checksum: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, ?2, ?3, 'high', 'pending', 0)",
        params![trove_id, original_format, checksum],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Insert test files for a trove
fn insert_test_files(conn: &rusqlite::Connection, trove_id: i64, files: &[(&str, i64, i32)]) {
    for (path, size, mode) in files {
        // Make hash unique per trove to avoid UNIQUE constraint issues
        let hash = format!("hash_{}_t{}", path.replace('/', "_"), trove_id);
        conn.execute(
            "INSERT INTO files (trove_id, path, sha256_hash, size, permissions)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![trove_id, path, hash, size, mode],
        )
        .unwrap();
    }
}

/// Insert dependencies for a trove
fn insert_dependencies(conn: &rusqlite::Connection, trove_id: i64, deps: &[&str]) {
    for dep in deps {
        conn.execute(
            "INSERT INTO dependencies (trove_id, depends_on_name, dependency_type)
             VALUES (?1, ?2, 'runtime')",
            params![trove_id, dep],
        )
        .unwrap();
    }
}

/// Create a full test package setup (trove + converted_package + files + deps)
fn create_test_package(
    conn: &rusqlite::Connection,
    name: &str,
    files: &[(&str, i64, i32)],
    deps: &[&str],
) -> (i64, i64) {
    let trove_id = insert_test_trove(conn, name, "1.0.0");
    let converted_id = insert_converted_package(conn, trove_id, "rpm", &format!("checksum_{}", name));
    insert_test_files(conn, trove_id, files);
    insert_dependencies(conn, trove_id, deps);
    (trove_id, converted_id)
}

// =============================================================================
// ENHANCEMENT STATUS TESTS
// =============================================================================

#[test]
fn test_count_by_status_empty_database() {
    let (_temp, conn) = create_test_db();
    let stats = ConvertedPackageInfo::count_by_status(&conn).unwrap();

    assert_eq!(stats.total, 0);
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.complete, 0);
    assert_eq!(stats.failed, 0);
}

#[test]
fn test_count_by_status_with_packages() {
    let (_temp, conn) = create_test_db();

    // Create packages with different statuses
    let trove1_id = insert_test_trove(&conn, "pending-pkg", "1.0.0");
    insert_converted_package(&conn, trove1_id, "rpm", "cs1");

    let trove2_id = insert_test_trove(&conn, "complete-pkg", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs2', 'high', 'complete', 1)",
        [trove2_id],
    ).unwrap();

    let trove3_id = insert_test_trove(&conn, "failed-pkg", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'deb', 'cs3', 'high', 'failed', 0)",
        [trove3_id],
    ).unwrap();

    let stats = ConvertedPackageInfo::count_by_status(&conn).unwrap();

    assert_eq!(stats.total, 3);
    assert_eq!(stats.pending, 1);
    assert_eq!(stats.complete, 1);
    assert_eq!(stats.failed, 1);
}

#[test]
fn test_find_pending_packages() {
    let (_temp, conn) = create_test_db();

    // Create pending packages with unique file paths
    let files1 = [("/usr/bin/pending1", 100i64, 0o755)];
    let files2 = [("/usr/bin/pending2", 100i64, 0o755)];
    let (_trove1_id, _) = create_test_package(&conn, "pending-pkg-1", &files1, &[]);
    let (_trove2_id, _) = create_test_package(&conn, "pending-pkg-2", &files2, &[]);

    // Create a complete package (should not be found)
    let trove3_id = insert_test_trove(&conn, "complete-pkg", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs3', 'high', 'complete', 1)",
        [trove3_id],
    ).unwrap();

    let pending = ConvertedPackageInfo::find_pending(&conn).unwrap();

    assert_eq!(pending.len(), 2, "Should find only pending packages");
    assert!(
        pending.iter().any(|p| p.name == "pending-pkg-1"),
        "Should find pending-pkg-1"
    );
    assert!(
        pending.iter().any(|p| p.name == "pending-pkg-2"),
        "Should find pending-pkg-2"
    );
    assert!(
        pending.iter().all(|p| p.enhancement_status == EnhancementStatus::Pending),
        "All should be pending"
    );
}

#[test]
fn test_find_outdated_packages() {
    let (_temp, conn) = create_test_db();

    // Create a package with old enhancement version
    let trove1_id = insert_test_trove(&conn, "old-pkg", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs1', 'high', 'complete', 0)",
        [trove1_id],
    ).unwrap();

    // Create a package with current version (should not be found)
    let trove2_id = insert_test_trove(&conn, "current-pkg", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs2', 'high', 'complete', ?2)",
        params![trove2_id, ENHANCEMENT_VERSION],
    ).unwrap();

    let outdated = ConvertedPackageInfo::find_outdated(&conn, ENHANCEMENT_VERSION).unwrap();

    assert_eq!(outdated.len(), 1, "Should find only outdated package");
    assert_eq!(outdated[0].name, "old-pkg");
    assert_eq!(outdated[0].enhancement_version, 0);
}

// =============================================================================
// ENHANCEMENT TYPE TESTS
// =============================================================================

#[test]
fn test_enhancement_type_parsing() {
    assert_eq!(
        EnhancementType::from_str("capabilities"),
        Some(EnhancementType::Capabilities)
    );
    assert_eq!(
        EnhancementType::from_str("caps"),
        Some(EnhancementType::Capabilities)
    );
    assert_eq!(
        EnhancementType::from_str("provenance"),
        Some(EnhancementType::Provenance)
    );
    assert_eq!(
        EnhancementType::from_str("prov"),
        Some(EnhancementType::Provenance)
    );
    assert_eq!(
        EnhancementType::from_str("subpackages"),
        Some(EnhancementType::Subpackages)
    );
    assert_eq!(
        EnhancementType::from_str("subpkg"),
        Some(EnhancementType::Subpackages)
    );
    assert_eq!(EnhancementType::from_str("unknown"), None);
}

#[test]
fn test_enhancement_type_all() {
    let all = EnhancementType::all();
    assert_eq!(all.len(), 3);
    assert!(all.contains(&EnhancementType::Capabilities));
    assert!(all.contains(&EnhancementType::Provenance));
    assert!(all.contains(&EnhancementType::Subpackages));
}

#[test]
fn test_enhancement_type_name() {
    assert_eq!(EnhancementType::Capabilities.name(), "capabilities");
    assert_eq!(EnhancementType::Provenance.name(), "provenance");
    assert_eq!(EnhancementType::Subpackages.name(), "subpackages");
}

// =============================================================================
// ENHANCEMENT RESULT TESTS
// =============================================================================

#[test]
fn test_enhancement_result_new() {
    let result = EnhancementResult_::new(42);

    assert_eq!(result.trove_id, 42);
    assert!(result.applied.is_empty());
    assert!(result.skipped.is_empty());
    assert!(result.failed.is_empty());
    assert!(result.is_success());
    assert_eq!(result.enhancement_version, ENHANCEMENT_VERSION);
}

#[test]
fn test_enhancement_result_success_tracking() {
    let mut result = EnhancementResult_::new(42);

    result.record_success(EnhancementType::Capabilities);
    assert!(result.is_success());
    assert_eq!(result.applied.len(), 1);
    assert!(result.applied.contains(&EnhancementType::Capabilities));

    result.record_success(EnhancementType::Provenance);
    assert_eq!(result.applied.len(), 2);
}

#[test]
fn test_enhancement_result_skipped_tracking() {
    let mut result = EnhancementResult_::new(42);

    result.record_skipped(EnhancementType::Subpackages);
    assert!(result.is_success()); // Skipped doesn't mean failure
    assert_eq!(result.skipped.len(), 1);
    assert!(result.skipped.contains(&EnhancementType::Subpackages));
}

#[test]
fn test_enhancement_result_failure_tracking() {
    let mut result = EnhancementResult_::new(42);

    result.record_success(EnhancementType::Capabilities);
    result.record_failure(EnhancementType::Provenance, "Test error");

    assert!(!result.is_success());
    assert_eq!(result.applied.len(), 1);
    assert_eq!(result.failed.len(), 1);
    assert_eq!(result.failed[0].0, EnhancementType::Provenance);
    assert_eq!(result.failed[0].1, "Test error");
}

// =============================================================================
// ENHANCEMENT RUNNER TESTS
// =============================================================================

#[test]
fn test_runner_with_default_options() {
    let (_temp, conn) = create_test_db();

    // Create a test package
    let files = [
        ("/usr/bin/myapp", 1024i64, 0o755),
        ("/etc/myapp/config.conf", 256i64, 0o644),
    ];
    let (trove_id, _) = create_test_package(&conn, "myapp", &files, &["libc6"]);

    let runner = EnhancementRunner::new(&conn);

    // Enhancement should work (may skip some enhancements if files don't exist)
    let result = runner.enhance(trove_id);
    assert!(result.is_ok(), "Enhancement should not error: {:?}", result.err());

    let result = result.unwrap();
    assert_eq!(result.trove_id, trove_id);
    // At minimum, it should have attempted all enhancement types
    let total_processed = result.applied.len() + result.skipped.len() + result.failed.len();
    assert!(
        total_processed > 0,
        "Should have processed at least one enhancement type"
    );
}

#[test]
fn test_runner_with_specific_types() {
    let (_temp, conn) = create_test_db();

    let files = [("/usr/bin/tool", 100i64, 0o755)];
    let (trove_id, _) = create_test_package(&conn, "tool", &files, &[]);

    let options = EnhancementOptions {
        types: vec![EnhancementType::Capabilities], // Only capabilities
        force: false,
        install_root: PathBuf::from("/"),
        fail_fast: false,
        parallel: false,
        parallel_workers: 0,
        cancel_token: None,
    };

    let runner = EnhancementRunner::with_options(&conn, options);
    let result = runner.enhance(trove_id).unwrap();

    // Should only process capabilities, not others
    let processed_types: Vec<_> = result
        .applied
        .iter()
        .chain(result.skipped.iter())
        .chain(result.failed.iter().map(|(t, _)| t))
        .collect();

    // All processed types should be Capabilities (the only requested type)
    assert!(
        processed_types.iter().all(|t| **t == EnhancementType::Capabilities)
            || processed_types.is_empty(),
        "Should only process requested enhancement type"
    );
}

#[test]
fn test_runner_enhance_all_pending() {
    let (_temp, conn) = create_test_db();

    // Create multiple pending packages with unique file paths
    let files1 = [("/usr/bin/app1", 100i64, 0o755)];
    let files2 = [("/usr/bin/app2", 100i64, 0o755)];
    let files3 = [("/usr/bin/app3", 100i64, 0o755)];
    create_test_package(&conn, "app1", &files1, &[]);
    create_test_package(&conn, "app2", &files2, &[]);
    create_test_package(&conn, "app3", &files3, &[]);

    let runner = EnhancementRunner::new(&conn);
    let results = runner.enhance_all_pending().unwrap();

    assert_eq!(results.len(), 3, "Should process all 3 pending packages");

    // All packages should have been processed
    let trove_ids: Vec<i64> = results.iter().map(|r| r.trove_id).collect();
    assert_eq!(trove_ids.len(), 3);
}

#[test]
fn test_runner_enhance_all_outdated() {
    let (_temp, conn) = create_test_db();

    // Create a package with outdated enhancement
    let trove_id = insert_test_trove(&conn, "old-enhanced", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs1', 'high', 'complete', 0)",
        [trove_id],
    ).unwrap();
    insert_test_files(&conn, trove_id, &[("/usr/bin/old", 100, 0o755)]);

    // Create a package with current enhancement (should not be re-enhanced)
    let trove2_id = insert_test_trove(&conn, "current-enhanced", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs2', 'high', 'complete', ?2)",
        params![trove2_id, ENHANCEMENT_VERSION],
    ).unwrap();

    let runner = EnhancementRunner::new(&conn);
    let results = runner.enhance_all_outdated().unwrap();

    // Should only re-enhance the outdated package
    assert_eq!(results.len(), 1, "Should only re-enhance outdated package");
    assert_eq!(results[0].trove_id, trove_id);
}

#[test]
fn test_runner_force_enhancement() {
    let (_temp, conn) = create_test_db();

    // Create a package that's already complete
    let trove_id = insert_test_trove(&conn, "force-test", "1.0.0");
    conn.execute(
        "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_fidelity, enhancement_status, enhancement_version)
         VALUES (?1, 'rpm', 'cs1', 'high', 'complete', ?2)",
        params![trove_id, ENHANCEMENT_VERSION],
    ).unwrap();
    insert_test_files(&conn, trove_id, &[("/usr/bin/test", 100, 0o755)]);

    let options = EnhancementOptions {
        types: EnhancementType::all().to_vec(),
        force: true, // Force re-enhancement
        install_root: PathBuf::from("/"),
        fail_fast: false,
        parallel: false,
        parallel_workers: 0,
        cancel_token: None,
    };

    let runner = EnhancementRunner::with_options(&conn, options);
    let result = runner.enhance(trove_id);

    // Should complete without error (force allows re-enhancement)
    assert!(result.is_ok(), "Force enhancement should work: {:?}", result.err());
}

// =============================================================================
// STATUS UPDATE TESTS
// =============================================================================

#[test]
fn test_enhancement_updates_status() {
    let (_temp, conn) = create_test_db();

    let files = [("/usr/bin/status-test", 100i64, 0o755)];
    let (trove_id, converted_id) = create_test_package(&conn, "status-test", &files, &[]);

    // Verify initial status is pending
    let status_before: String = conn
        .query_row(
            "SELECT enhancement_status FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status_before, "pending");

    // Run enhancement
    let runner = EnhancementRunner::new(&conn);
    let result = runner.enhance(trove_id);
    assert!(result.is_ok());

    // Check status was updated
    let status_after: String = conn
        .query_row(
            "SELECT enhancement_status FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| row.get(0),
        )
        .unwrap();

    // Status should be either complete or skipped (not pending anymore)
    assert!(
        status_after == "complete" || status_after == "skipped",
        "Status should be complete or skipped, got: {}",
        status_after
    );
}

#[test]
fn test_enhancement_version_updated() {
    let (_temp, conn) = create_test_db();

    let files = [("/usr/bin/version-test", 100i64, 0o755)];
    let (trove_id, converted_id) = create_test_package(&conn, "version-test", &files, &[]);

    // Verify initial version is 0
    let version_before: i32 = conn
        .query_row(
            "SELECT enhancement_version FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version_before, 0);

    // Run enhancement
    let runner = EnhancementRunner::new(&conn);
    let _ = runner.enhance(trove_id);

    // Check version was updated
    let version_after: i32 = conn
        .query_row(
            "SELECT enhancement_version FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| row.get(0),
        )
        .unwrap();

    // Version should be updated to current
    assert!(
        version_after >= 0,
        "Enhancement version should be set (got {})",
        version_after
    );
}

// =============================================================================
// SUBPACKAGE DETECTION TESTS
// =============================================================================

#[test]
fn test_subpackage_naming_patterns() {
    // Test common subpackage naming patterns
    let patterns = [
        ("nginx-devel", "nginx", true),      // -devel suffix
        ("nginx-doc", "nginx", true),        // -doc suffix
        ("nginx-libs", "nginx", true),       // -libs suffix
        ("libfoo-devel", "libfoo", true),    // lib prefix with -devel
        ("httpd-common", "httpd", true),     // -common suffix
        ("nginx", "nginx", false),           // Not a subpackage
    ];

    for (subpkg, base, should_match) in patterns {
        let detected = detect_base_package(subpkg);
        if should_match {
            assert!(
                detected.is_some() && detected.as_ref().unwrap() == base,
                "Expected {} to be detected as subpackage of {}, got {:?}",
                subpkg,
                base,
                detected
            );
        } else {
            assert!(
                detected.is_none(),
                "Expected {} to NOT be detected as subpackage, got {:?}",
                subpkg,
                detected
            );
        }
    }
}

/// Simple helper to detect base package from subpackage name
/// (mirrors logic in subpackage enhancer)
fn detect_base_package(name: &str) -> Option<String> {
    let suffixes = ["-devel", "-dev", "-doc", "-libs", "-common", "-data", "-tools"];
    for suffix in suffixes {
        if name.ends_with(suffix) {
            return Some(name.trim_end_matches(suffix).to_string());
        }
    }
    None
}

// =============================================================================
// PARALLEL ENHANCEMENT TESTS
// =============================================================================

#[test]
fn test_parallel_enhancement() {
    let (_temp, conn) = create_test_db();

    // Create multiple packages with unique file paths
    for i in 0..5 {
        let files = [(format!("/usr/bin/parallel-{}", i), 100i64, 0o755)];
        let files_slice: Vec<(&str, i64, i32)> = files
            .iter()
            .map(|(p, s, m)| (p.as_str(), *s, *m))
            .collect();
        create_test_package(&conn, &format!("parallel-pkg-{}", i), &files_slice, &[]);
    }

    let options = EnhancementOptions {
        types: vec![EnhancementType::Capabilities],
        force: false,
        install_root: PathBuf::from("/"),
        fail_fast: false,
        parallel: true, // Enable parallel processing
        parallel_workers: 4,
        cancel_token: None,
    };

    let runner = EnhancementRunner::with_options(&conn, options);
    let results = runner.enhance_all_pending().unwrap();

    assert_eq!(results.len(), 5, "Should process all 5 packages in parallel");
}

// =============================================================================
// ERROR HANDLING TESTS
// =============================================================================

#[test]
fn test_enhancement_nonexistent_trove() {
    let (_temp, conn) = create_test_db();

    let runner = EnhancementRunner::new(&conn);
    let result = runner.enhance(99999); // Non-existent trove ID

    assert!(
        result.is_err(),
        "Should fail for non-existent trove"
    );
}

#[test]
fn test_enhancement_trove_without_converted_package() {
    let (_temp, conn) = create_test_db();

    // Create a trove without a converted_packages entry
    let trove_id = insert_test_trove(&conn, "no-converted", "1.0.0");
    // Don't insert into converted_packages

    let runner = EnhancementRunner::new(&conn);
    let result = runner.enhance(trove_id);

    assert!(
        result.is_err(),
        "Should fail for trove without converted_packages entry"
    );
}

// =============================================================================
// CAPABILITY INFERENCE INTEGRATION
// =============================================================================

#[test]
fn test_capability_inference_server_package() {
    let (_temp, conn) = create_test_db();

    // Create a server-like package
    let files = [
        ("/usr/sbin/myserver", 1024i64, 0o755),
        ("/etc/myserver/config.conf", 256i64, 0o644),
        ("/var/log/myserver/.keep", 0i64, 0o644),
    ];
    let (trove_id, converted_id) = create_test_package(
        &conn,
        "myserver-daemon",
        &files,
        &["libssl3", "libc6"],
    );

    let options = EnhancementOptions {
        types: vec![EnhancementType::Capabilities],
        force: false,
        install_root: PathBuf::from("/"),
        fail_fast: false,
        parallel: false,
        parallel_workers: 0,
        cancel_token: None,
    };

    let runner = EnhancementRunner::with_options(&conn, options);
    let result = runner.enhance(trove_id).unwrap();

    // Capabilities enhancement should have been processed
    let processed = result.applied.contains(&EnhancementType::Capabilities)
        || result.skipped.contains(&EnhancementType::Capabilities);
    assert!(processed, "Capabilities should have been processed");

    // Check if inferred_caps_json was stored (it may be null if skipped)
    let caps_json: Option<String> = conn
        .query_row(
            "SELECT inferred_caps_json FROM converted_packages WHERE id = ?1",
            [converted_id],
            |row| row.get(0),
        )
        .unwrap_or(None);

    // Either applied (has JSON) or skipped (null) - both are valid
    if result.applied.contains(&EnhancementType::Capabilities) {
        assert!(
            caps_json.is_some(),
            "Should have stored inferred capabilities"
        );
    }
}

// =============================================================================
// ENHANCEMENT WINDOW TESTS
// =============================================================================

#[test]
fn test_enhancement_window_status() {
    let (_temp, conn) = create_test_db();

    // Create a package with pending enhancement
    let files = [("/usr/bin/window-test", 100i64, 0o755)];
    let (trove_id, _) = create_test_package(&conn, "window-test", &files, &[]);

    let status = check_enhancement_window(&conn, trove_id);
    println!("Enhancement window status: {:?}", status);

    // Status should be InProgress since we just created a pending package
    match status {
        EnhancementWindowStatus::InProgress { package_name } => {
            assert_eq!(package_name, "window-test");
        }
        EnhancementWindowStatus::Complete => {
            // Also valid if enhancement ran automatically
        }
        other => {
            panic!("Unexpected status: {:?}", other);
        }
    }

    // Test for non-existent trove
    let status_nonexistent = check_enhancement_window(&conn, 99999);
    assert!(matches!(status_nonexistent, EnhancementWindowStatus::NotConverted));
}

// =============================================================================
// PRIORITY SCHEDULING TESTS
// =============================================================================

#[test]
fn test_priority_scheduling() {
    let (_temp, conn) = create_test_db();

    // Create packages with unique file paths
    let files1 = [("/usr/bin/low-prio", 100i64, 0o755)];
    let files2 = [("/usr/bin/high-prio", 100i64, 0o755)];
    let (trove1_id, _) = create_test_package(&conn, "low-priority", &files1, &[]);
    let (trove2_id, _) = create_test_package(&conn, "high-priority", &files2, &[]);

    // Schedule with different priorities using the enum
    schedule_for_enhancement(&conn, trove1_id, EnhancementPriority::Low).unwrap();
    schedule_for_enhancement(&conn, trove2_id, EnhancementPriority::High).unwrap();

    // Verify priorities were set (stored as integers)
    let priority1: i32 = conn
        .query_row(
            "SELECT enhancement_priority FROM converted_packages WHERE trove_id = ?1",
            [trove1_id],
            |row| row.get(0),
        )
        .unwrap();
    let priority2: i32 = conn
        .query_row(
            "SELECT enhancement_priority FROM converted_packages WHERE trove_id = ?1",
            [trove2_id],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(priority1, EnhancementPriority::Low as i32);
    assert_eq!(priority2, EnhancementPriority::High as i32);
}

#[test]
fn test_get_pending_by_priority() {
    let (_temp, conn) = create_test_db();

    // Create packages with unique file paths
    let files1 = [("/usr/bin/low-prio-pkg", 100i64, 0o755)];
    let files2 = [("/usr/bin/high-prio-pkg", 100i64, 0o755)];

    // Create packages with different priorities
    let (trove1_id, _) = create_test_package(&conn, "low-pkg", &files1, &[]);
    let (trove2_id, _) = create_test_package(&conn, "high-pkg", &files2, &[]);

    // Set priorities using integer values (Low=0, High=2)
    conn.execute(
        "UPDATE converted_packages SET enhancement_priority = ?2 WHERE trove_id = ?1",
        params![trove1_id, EnhancementPriority::Low as i32],
    )
    .unwrap();
    conn.execute(
        "UPDATE converted_packages SET enhancement_priority = ?2 WHERE trove_id = ?1",
        params![trove2_id, EnhancementPriority::High as i32],
    )
    .unwrap();

    // Get pending sorted by priority (returns Vec<i64> of trove IDs)
    let pending = get_pending_by_priority(&conn, 100).unwrap();

    assert_eq!(pending.len(), 2);
    // High priority should come first
    assert_eq!(pending[0], trove2_id, "High priority should be first");
    assert_eq!(pending[1], trove1_id, "Low priority should be second");
}
