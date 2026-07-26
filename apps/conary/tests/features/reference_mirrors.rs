// conary/tests/features/reference_mirrors.rs

use super::*;

// =============================================================================
// REFERENCE MIRROR TESTS
// =============================================================================

/// Test repository with content mirror (reference mirror pattern)
#[test]
fn test_reference_mirror_creation() {
    use conary_core::db::models::Repository;

    let (_dir, _path, conn) = common::create_test_db();

    // Create repository with separate metadata and content URLs
    let mut repo = Repository::with_content_mirror(
        "fedora-mirror".to_string(),
        "https://mirrors.fedoraproject.org/metalink".to_string(),
        "https://local-cache.example.com/fedora".to_string(),
    );

    repo.insert(&conn).unwrap();

    // Retrieve and verify
    let found = Repository::find_by_name(&conn, "fedora-mirror")
        .unwrap()
        .unwrap();
    assert_eq!(found.url, "https://mirrors.fedoraproject.org/metalink");
    assert_eq!(
        found.content_url,
        Some("https://local-cache.example.com/fedora".to_string())
    );

    // Effective content URL should be the content_url
    assert_eq!(
        found.effective_content_url(),
        "https://local-cache.example.com/fedora"
    );
}

/// Test repository without content mirror (standard pattern)
#[test]
fn test_repository_without_content_mirror() {
    use conary_core::db::models::Repository;

    let (_dir, _path, conn) = common::create_test_db();

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
    assert_eq!(
        found.effective_content_url(),
        "https://mirrors.fedoraproject.org/metalink"
    );
}

/// Test multiple repositories with different mirror configurations
#[test]
fn test_mixed_mirror_configurations() {
    use conary_core::db::models::Repository;

    let (_dir, _path, conn) = common::create_test_db();

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
    assert_eq!(
        repos[0].effective_content_url(),
        "https://updates.example.com"
    );
    assert_eq!(
        repos[1].effective_content_url(),
        "https://cdn.example.com/packages"
    );
}

/// Test repository update preserves content_url
#[test]
fn test_repository_update_content_mirror() {
    use conary_core::db::models::Repository;

    let (_dir, _path, conn) = common::create_test_db();

    // Create repo with content mirror
    let mut repo = Repository::with_content_mirror(
        "test-repo".to_string(),
        "https://old-metadata.example.com".to_string(),
        "https://old-cdn.example.com".to_string(),
    );
    repo.insert(&conn).unwrap();

    // Update URLs
    let mut found = Repository::find_by_name(&conn, "test-repo")
        .unwrap()
        .unwrap();
    found.url = "https://new-metadata.example.com".to_string();
    found.content_url = Some("https://new-cdn.example.com".to_string());
    found.update(&conn).unwrap();

    // Verify update
    let updated = Repository::find_by_name(&conn, "test-repo")
        .unwrap()
        .unwrap();
    assert_eq!(updated.url, "https://new-metadata.example.com");
    assert_eq!(
        updated.content_url,
        Some("https://new-cdn.example.com".to_string())
    );
}
