// conary/tests/features/derived_packages.rs

use super::*;

// =============================================================================
// DERIVED PACKAGE TESTS
// =============================================================================

/// Test derived package creation and basic operations
#[test]
fn test_derived_package_creation() {
    use conary_core::db::models::{DerivedPackage, DerivedStatus, VersionPolicy};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create a derived package from nginx
    let mut derived = DerivedPackage::new("nginx-custom".to_string(), "nginx".to_string());
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
    assert_eq!(
        found.version_policy,
        VersionPolicy::Suffix("+custom".to_string())
    );

    // List all derived packages
    let all = DerivedPackage::list_all(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "nginx-custom");
}

/// Test derived package patches
#[test]
fn test_derived_package_patches() {
    use conary_core::db::models::{DerivedPackage, DerivedPatch};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new("nginx-patched".to_string(), "nginx".to_string());
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
    use conary_core::db::models::{DerivedOverride, DerivedPackage};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new("nginx-config".to_string(), "nginx".to_string());
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
    let mut override2 =
        DerivedOverride::new_remove(derived_id, "/etc/nginx/sites-enabled/default".to_string());
    override2.insert(&conn).unwrap();

    // Retrieve overrides
    let overrides = DerivedOverride::find_by_derived(&conn, derived_id).unwrap();
    assert_eq!(overrides.len(), 2);

    // Check replacement
    let replacement = overrides
        .iter()
        .find(|o| o.target_path == "/etc/nginx/nginx.conf")
        .unwrap();
    assert!(!replacement.is_removal());
    assert_eq!(
        replacement.source_hash.as_deref(),
        Some("custom_config_hash")
    );
    assert_eq!(replacement.permissions, Some(0o644));

    // Check removal
    let removal = overrides
        .iter()
        .find(|o| o.target_path == "/etc/nginx/sites-enabled/default")
        .unwrap();
    assert!(removal.is_removal());
    assert!(removal.source_hash.is_none());

    // Test find by path
    let found = DerivedOverride::find_by_path(&conn, derived_id, "/etc/nginx/nginx.conf").unwrap();
    assert!(found.is_some());
}

/// Test derived package status transitions
#[test]
fn test_derived_package_status() {
    use conary_core::db::models::{DerivedPackage, DerivedStatus, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    // Create derived package
    let mut derived = DerivedPackage::new("nginx-status-test".to_string(), "nginx".to_string());
    let _derived_id = derived.insert(&conn).unwrap();

    // Initial status is Pending
    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test")
        .unwrap()
        .unwrap();
    assert_eq!(found.status, DerivedStatus::Pending);

    // Mark as built (need to create a trove first)
    let mut built_trove = Trove::new(
        "nginx-status-test".to_string(),
        "1.24.0+custom".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = built_trove.insert(&conn).unwrap();

    // Re-fetch as mutable to call mark_built
    let mut found = DerivedPackage::find_by_name(&conn, "nginx-status-test")
        .unwrap()
        .unwrap();
    found.mark_built(&conn, trove_id).unwrap();

    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test")
        .unwrap()
        .unwrap();
    assert_eq!(found.status, DerivedStatus::Built);
    assert_eq!(found.built_trove_id, Some(trove_id));

    // Mark parent as stale
    DerivedPackage::mark_stale(&conn, "nginx").unwrap();
    let found = DerivedPackage::find_by_name(&conn, "nginx-status-test")
        .unwrap()
        .unwrap();
    assert_eq!(found.status, DerivedStatus::Stale);

    // Find stale packages
    let stale = DerivedPackage::find_by_status(&conn, DerivedStatus::Stale).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].name, "nginx-status-test");
}

/// Test that derived builds persist concrete artifact metadata
#[test]
fn test_derive_build_records_installable_artifact_metadata() {
    use conary_core::db::models::{DerivedOverride, DerivedPackage, VersionPolicy};
    use conary_core::derived::{build_from_definition, persist_build_artifact};
    use conary_core::filesystem::CasStore;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let objects_dir = conary_core::db::paths::objects_dir(&db_path);
    let cas = CasStore::new(&objects_dir).unwrap();

    let mut derived = DerivedPackage::new("nginx-custom".to_string(), "nginx".to_string());
    derived.version_policy = VersionPolicy::Suffix("+custom".to_string());
    let derived_id = derived.insert(&conn).unwrap();

    let override_content = b"user nginx;\nworker_processes auto;\n".to_vec();
    let override_hash = cas.store(&override_content).unwrap();

    let mut override_entry = DerivedOverride::new_replace(
        derived_id,
        "/etc/nginx/nginx.conf".to_string(),
        override_hash.clone(),
    );
    override_entry.permissions = Some(0o640);
    override_entry.insert(&conn).unwrap();

    let build_result = build_from_definition(&conn, &derived, &cas).unwrap();
    let build_meta = persist_build_artifact(&conn, &mut derived, &build_result, &cas).unwrap();

    assert_eq!(build_meta.version, "1.24.0+custom");
    assert_eq!(build_meta.parent_version, "1.24.0");
    assert_eq!(
        build_meta.artifact_path,
        format!("cas://{}", build_meta.artifact_hash)
    );
    assert!(cas.exists(&build_meta.artifact_hash));

    let stored = DerivedPackage::find_by_name(&conn, "nginx-custom")
        .unwrap()
        .unwrap();
    assert_eq!(stored.status.as_str(), "built");
    assert_eq!(stored.last_built_version.as_deref(), Some("1.24.0+custom"));
    assert_eq!(stored.last_built_parent_version.as_deref(), Some("1.24.0"));
    assert_eq!(
        stored.build_artifact_hash.as_deref(),
        Some(build_meta.artifact_hash.as_str())
    );
    assert_eq!(
        stored.build_artifact_path.as_deref(),
        Some(build_meta.artifact_path.as_str())
    );

    let artifact_json = cas.retrieve(&build_meta.artifact_hash).unwrap();
    let artifact: serde_json::Value = serde_json::from_slice(&artifact_json).unwrap();
    assert_eq!(artifact["name"], "nginx-custom");
    assert_eq!(artifact["version"], "1.24.0+custom");
    assert_eq!(artifact["parent_name"], "nginx");
    assert_eq!(artifact["parent_version"], "1.24.0");
    assert_eq!(
        artifact["files"]["/etc/nginx/nginx.conf"]["hash"],
        override_hash
    );
}

/// Test stale propagation helper only fires on real parent version changes
#[test]
fn test_mark_stale_if_parent_version_changes_only_marks_real_upgrades() {
    use conary_core::db::models::{DerivedOverride, DerivedPackage, DerivedStatus, VersionPolicy};
    use conary_core::derived::{build_from_definition, persist_build_artifact};
    use conary_core::filesystem::CasStore;

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let conn = db::open(&db_path).unwrap();

    let objects_dir = conary_core::db::paths::objects_dir(&db_path);
    let cas = CasStore::new(&objects_dir).unwrap();

    let mut derived = DerivedPackage::new("nginx-stale-check".to_string(), "nginx".to_string());
    derived.version_policy = VersionPolicy::Suffix("+stale".to_string());
    let derived_id = derived.insert(&conn).unwrap();

    let override_content = b"worker_processes 2;\n".to_vec();
    let override_hash = cas.store(&override_content).unwrap();

    let mut override_entry = DerivedOverride::new_replace(
        derived_id,
        "/etc/nginx/nginx.conf".to_string(),
        override_hash,
    );
    override_entry.insert(&conn).unwrap();

    let build_result = build_from_definition(&conn, &derived, &cas).unwrap();
    persist_build_artifact(&conn, &mut derived, &build_result, &cas).unwrap();

    assert_eq!(
        DerivedPackage::mark_stale_if_parent_changed(&conn, "nginx", None, "1.25.0").unwrap(),
        0
    );
    assert_eq!(
        DerivedPackage::find_by_name(&conn, "nginx-stale-check")
            .unwrap()
            .unwrap()
            .status,
        DerivedStatus::Built
    );

    assert_eq!(
        DerivedPackage::mark_stale_if_parent_changed(&conn, "nginx", Some("1.24.0"), "1.24.0")
            .unwrap(),
        0
    );
    assert_eq!(
        DerivedPackage::find_by_name(&conn, "nginx-stale-check")
            .unwrap()
            .unwrap()
            .status,
        DerivedStatus::Built
    );

    assert_eq!(
        DerivedPackage::mark_stale_if_parent_changed(&conn, "nginx", Some("1.24.0"), "1.25.0")
            .unwrap(),
        1
    );
    assert_eq!(
        DerivedPackage::find_by_name(&conn, "nginx-stale-check")
            .unwrap()
            .unwrap()
            .status,
        DerivedStatus::Stale
    );
}

/// Test version policy computation
#[test]
fn test_derived_version_policy() {
    use conary_core::db::models::VersionPolicy;

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
