// apps/remi/src/server/handlers/oci/tests.rs
use super::*;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
use conary_core::db::schema;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::migrate(&conn).unwrap();
    (temp_file, conn)
}

fn insert_converted_package(
    conn: &Connection,
    distro: &str,
    name: &str,
    version: &str,
    chunks: &[String],
) {
    let mut pkg = ConvertedPackage::new_server(
        distro.to_string(),
        name.to_string(),
        version.to_string(),
        "rpm".to_string(),
        format!("sha256:test-{}-{}", name, version),
        chunks,
        4096,
        format!("sha256:content-{}-{}", name, version),
        format!("/data/{}-{}.ccs", name, version),
    );
    pkg.insert(conn).unwrap();
}

fn insert_stale_converted_package(
    conn: &Connection,
    distro: &str,
    name: &str,
    version: &str,
    chunks: &[String],
) {
    let mut pkg = ConvertedPackage::new_server(
        distro.to_string(),
        name.to_string(),
        version.to_string(),
        "rpm".to_string(),
        format!("sha256:test-{}-{}", name, version),
        chunks,
        4096,
        format!("sha256:content-{}-{}", name, version),
        format!("/data/{}-{}.ccs", name, version),
    );
    pkg.conversion_version = CONVERSION_VERSION - 1;
    pkg.insert(conn).unwrap();
}

fn insert_private_review_converted_package(
    conn: &Connection,
    distro: &str,
    name: &str,
    version: &str,
    chunks: &[String],
) {
    let mut pkg = ConvertedPackage::new_server(
        distro.to_string(),
        name.to_string(),
        version.to_string(),
        "rpm".to_string(),
        format!("sha256:test-{}-{}", name, version),
        chunks,
        4096,
        format!("sha256:content-{}-{}", name, version),
        format!("/data/{}-{}.ccs", name, version),
    );
    pkg.set_scriptlet_metadata(&ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    })
    .unwrap();
    pkg.insert(conn).unwrap();
}

const OCI_TEST_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn private_review_summary() -> ScriptletBundleSummary {
    ScriptletBundleSummary {
        publication_status: "private-review".to_string(),
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        ..Default::default()
    }
}

async fn oci_blob_state_with_db(
    hash: &str,
    rows: Vec<ScriptletBundleSummary>,
) -> (Arc<RwLock<crate::server::ServerState>>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = Connection::open(&db_path).unwrap();
    schema::migrate(&conn).unwrap();

    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    let chunk_path = crate::server::handlers::cas_object_path(&chunk_dir, hash);
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, b"blob bytes").unwrap();

    for (index, summary) in rows.into_iter().enumerate() {
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            format!("pkg-{index}"),
            "1.0".to_string(),
            "rpm".to_string(),
            format!("sha256:source-{index}"),
            &[hash.to_string()],
            10,
            format!("sha256:content-{index}"),
            format!("/tmp/pkg-{index}.ccs"),
        );
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();
    }

    let config = crate::server::ServerConfig {
        db_path,
        chunk_dir,
        cache_dir,
        enable_bloom_filter: false,
        upstream_url: None,
        ..Default::default()
    };
    std::fs::create_dir_all(&config.cache_dir).unwrap();
    let state = crate::server::ServerState::new(config).expect("test server state");
    (Arc::new(RwLock::new(state)), temp)
}

#[test]
fn test_parse_oci_name_namespaced() {
    let (distro, pkg) = parse_oci_name("conary/fedora/nginx").unwrap();
    assert_eq!(distro, "fedora");
    assert_eq!(pkg, "nginx");
}

#[test]
fn test_parse_oci_name_bare() {
    let (distro, pkg) = parse_oci_name("fedora/nginx").unwrap();
    assert_eq!(distro, "fedora");
    assert_eq!(pkg, "nginx");
}

#[test]
fn test_parse_oci_name_invalid() {
    assert!(parse_oci_name("nginx").is_none());
    assert!(parse_oci_name("").is_none());
    assert!(parse_oci_name("/nginx").is_none());
    assert!(parse_oci_name("fedora/").is_none());
    // Nested packages not supported
    assert!(parse_oci_name("fedora/nginx/extra").is_none());
}

#[test]
fn test_split_oci_segment() {
    let (name, reference) =
        split_oci_segment("conary/fedora/nginx/manifests/1.24.0", "/manifests/").unwrap();
    assert_eq!(name, "conary/fedora/nginx");
    assert_eq!(reference, "1.24.0");

    let (name, digest) = split_oci_segment("fedora/curl/blobs/sha256:abc123", "/blobs/").unwrap();
    assert_eq!(name, "fedora/curl");
    assert_eq!(digest, "sha256:abc123");
}

#[test]
fn test_split_oci_segment_missing() {
    assert!(split_oci_segment("foo/bar", "/manifests/").is_none());
    assert!(split_oci_segment("/manifests/ref", "/manifests/").is_none());
    assert!(split_oci_segment("foo/manifests/", "/manifests/").is_none());
}

#[test]
fn test_strip_digest_prefix() {
    // Valid: sha256 prefix with exactly 64 hex chars
    let valid_hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    assert_eq!(
        strip_digest_prefix(&format!("sha256:{}", valid_hash)),
        Some(valid_hash)
    );

    // Missing sha256: prefix
    assert_eq!(strip_digest_prefix(valid_hash), None);

    // Wrong algorithm prefix
    assert_eq!(strip_digest_prefix("sha512:abc"), None);

    // sha256: prefix but invalid hex (too short)
    assert_eq!(strip_digest_prefix("sha256:abc123"), None);

    // sha256: prefix but contains path traversal characters
    assert_eq!(
        strip_digest_prefix(
            "sha256:../../etc/passwd/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
        None
    );

    // sha256: prefix but contains non-hex chars (64 chars total)
    assert_eq!(
        strip_digest_prefix(
            "sha256:zzzzzz1234567890abcdef1234567890abcdef1234567890abcdef12345678"
        ),
        None
    );

    // sha256: prefix with uppercase (is_valid_hash accepts ascii hex which includes A-F)
    let upper_hash = "ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890";
    assert_eq!(
        strip_digest_prefix(&format!("sha256:{}", upper_hash)),
        Some(upper_hash)
    );
}

#[test]
fn test_build_tags_list() {
    let (temp_file, conn) = create_test_db();

    insert_converted_package(
        &conn,
        "fedora",
        "nginx",
        "1.24.0-1.fc44",
        &["chunk1".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        "nginx",
        "1.25.0-1.fc44",
        &["chunk2".to_string()],
    );
    insert_converted_package(&conn, "arch", "nginx", "1.25.0-1", &["chunk3".to_string()]);

    let tags = build_tags_list(temp_file.path(), "fedora", "nginx").unwrap();
    assert_eq!(tags, vec!["1.24.0-1.fc44", "1.25.0-1.fc44"]);

    let tags = build_tags_list(temp_file.path(), "arch", "nginx").unwrap();
    assert_eq!(tags, vec!["1.25.0-1"]);

    let tags = build_tags_list(temp_file.path(), "fedora", "nonexistent").unwrap();
    assert!(tags.is_empty());
}

#[test]
fn oci_tags_ignore_stale_converted_rows() {
    let (temp_file, conn) = create_test_db();

    insert_stale_converted_package(
        &conn,
        "fedora",
        "nginx",
        "1.24.0-1.fc44",
        &["stale-chunk".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        "nginx",
        "1.25.0-1.fc44",
        &["current-chunk".to_string()],
    );

    let tags = build_tags_list(temp_file.path(), "fedora", "nginx").unwrap();
    assert_eq!(tags, vec!["1.25.0-1.fc44"]);
}

#[test]
fn test_build_catalog() {
    let (temp_file, conn) = create_test_db();

    insert_converted_package(&conn, "fedora", "nginx", "1.24.0", &["chunk1".to_string()]);
    insert_converted_package(&conn, "fedora", "curl", "8.5.0", &["chunk2".to_string()]);
    insert_converted_package(&conn, "arch", "nginx", "1.25.0", &["chunk3".to_string()]);

    let catalog = build_catalog(temp_file.path()).unwrap();
    assert_eq!(
        catalog.repositories,
        vec![
            "conary/arch/nginx",
            "conary/fedora/curl",
            "conary/fedora/nginx",
        ]
    );
}

#[test]
fn oci_catalog_ignores_stale_converted_rows() {
    let (temp_file, conn) = create_test_db();

    insert_stale_converted_package(
        &conn,
        "fedora",
        "stale-only",
        "1.0.0",
        &["stale-chunk".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        "current",
        "1.0.0",
        &["current-chunk".to_string()],
    );

    let catalog = build_catalog(temp_file.path()).unwrap();
    assert_eq!(catalog.repositories, vec!["conary/fedora/current"]);
}

#[test]
fn test_build_catalog_empty() {
    let (temp_file, _conn) = create_test_db();
    let catalog = build_catalog(temp_file.path()).unwrap();
    assert!(catalog.repositories.is_empty());
}

#[test]
fn test_build_manifest() {
    let (temp_file, conn) = create_test_db();

    let chunks = vec!["aabbccdd".to_string(), "eeff0011".to_string()];
    insert_converted_package(&conn, "fedora", "nginx", "1.24.0", &chunks);

    // Create a temporary chunk cache directory
    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        30,
        temp_file.path().to_path_buf(),
    );

    let result =
        build_manifest(temp_file.path(), "fedora", "nginx", "1.24.0", &chunk_cache).unwrap();

    assert!(result.is_some());
    let (json, digest) = result.unwrap();

    // Verify it parses as valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["schemaVersion"], 2);
    assert_eq!(parsed["mediaType"], OCI_MANIFEST_MEDIA_TYPE);

    // Should have 2 layers (one per chunk)
    let layers = parsed["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0]["digest"], "sha256:aabbccdd");
    assert_eq!(layers[1]["digest"], "sha256:eeff0011");

    // Config should be present
    assert!(
        parsed["config"]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    // Digest should be sha256
    assert!(digest.starts_with("sha256:"));
}

#[test]
fn test_build_manifest_not_found() {
    let (temp_file, _conn) = create_test_db();
    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        30,
        temp_file.path().to_path_buf(),
    );

    let result = build_manifest(
        temp_file.path(),
        "fedora",
        "nonexistent",
        "1.0.0",
        &chunk_cache,
    )
    .unwrap();
    assert!(result.is_none());
}

#[test]
fn oci_manifest_ignores_stale_converted_rows() {
    let (temp_file, conn) = create_test_db();
    insert_stale_converted_package(
        &conn,
        "fedora",
        "nginx",
        "1.24.0",
        &["stale-chunk".to_string()],
    );

    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        30,
        temp_file.path().to_path_buf(),
    );

    let result =
        build_manifest(temp_file.path(), "fedora", "nginx", "1.24.0", &chunk_cache).unwrap();
    assert!(result.is_none());
}

#[test]
fn oci_tags_catalog_and_manifest_ignore_non_public_rows() {
    let (temp_file, conn) = create_test_db();
    insert_private_review_converted_package(
        &conn,
        "fedora",
        "private-only",
        "1.0",
        &["private-chunk".to_string()],
    );

    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        30,
        temp_file.path().to_path_buf(),
    );

    let tags = build_tags_list(temp_file.path(), "fedora", "private-only").unwrap();
    let catalog = build_catalog(temp_file.path()).unwrap();
    let manifest = build_manifest(
        temp_file.path(),
        "fedora",
        "private-only",
        "1.0",
        &chunk_cache,
    )
    .unwrap();

    assert!(tags.is_empty());
    assert!(catalog.repositories.is_empty());
    assert!(manifest.is_none());
}

#[tokio::test]
async fn oci_blob_returns_not_found_for_non_public_only_hash() {
    let (state, _temp) =
        oci_blob_state_with_db(OCI_TEST_HASH, vec![private_review_summary()]).await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = get_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oci_head_blob_returns_not_found_for_non_public_only_hash() {
    let (state, _temp) =
        oci_blob_state_with_db(OCI_TEST_HASH, vec![private_review_summary()]).await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = head_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oci_blob_allows_hash_shared_with_public_ready_row() {
    let (state, _temp) = oci_blob_state_with_db(
        OCI_TEST_HASH,
        vec![private_review_summary(), ScriptletBundleSummary::default()],
    )
    .await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = get_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::OK);
}
