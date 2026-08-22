// apps/remi/src/server/handlers/oci/tests.rs
use super::*;
use crate::server::catalog_authority::test_support::{
    ActiveCatalogFixture, package as catalog_package,
};
use crate::server::conversion::test_support::seed_repository_conversion_source;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};

fn insert_converted_package(
    conn: &Connection,
    distro: &str,
    profile_revision_sha256: &str,
    name: &str,
    version: &str,
    chunks: &[String],
) {
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro).unwrap();
    let transport = crate::server::conversion::test_support::test_transport(chunks);
    let mut pkg = ConvertedPackage::new_repository(
        source_profile.id().to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        format!("sha256:test-{}-{}", name, version),
        &transport,
        4096,
        format!("sha256:content-{}-{}", name, version),
        format!("/data/{}-{}.ccs", name, version),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_conversion_source(conn, &mut pkg);
    pkg.insert_with_conversion_pin(conn, 1).unwrap();
}

fn insert_stale_converted_package(
    conn: &Connection,
    distro: &str,
    profile_revision_sha256: &str,
    name: &str,
    version: &str,
    chunks: &[String],
) {
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro).unwrap();
    let transport = crate::server::conversion::test_support::test_transport(chunks);
    let mut pkg = ConvertedPackage::new_repository(
        source_profile.id().to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        format!("sha256:test-{}-{}", name, version),
        &transport,
        4096,
        format!("sha256:content-{}-{}", name, version),
        format!("/data/{}-{}.ccs", name, version),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    pkg.conversion_version = CONVERSION_VERSION - 1;
    pkg.insert(conn).unwrap();
}

const OCI_TEST_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

async fn oci_blob_state_with_db(
    hash: &str,
    stale_rows: Vec<bool>,
) -> (
    Arc<RwLock<crate::server::ServerState>>,
    tempfile::TempDir,
    ActiveCatalogFixture,
) {
    let temp = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        stale_rows
            .iter()
            .enumerate()
            .map(|(index, _)| {
                catalog_package(
                    "fedora-44",
                    &format!("pkg-{index}"),
                    "1.0",
                    "1",
                    Some("x86_64"),
                    10,
                    &format!("source-{index}"),
                )
            })
            .collect(),
    );
    let db_path = catalogs.db_path().to_path_buf();
    let conn = catalogs.connection();

    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    let chunk_path = crate::server::handlers::cas_object_path(&chunk_dir, hash);
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, b"blob bytes").unwrap();

    for (index, stale) in stale_rows.into_iter().enumerate() {
        let transport =
            crate::server::conversion::test_support::test_transport(&[hash.to_string()]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            profile_revision_sha256.clone(),
            format!("pkg-{index}"),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            format!("sha256:source-{index}"),
            &transport,
            10,
            format!("sha256:content-{index}"),
            format!("/tmp/pkg-{index}.ccs"),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        if stale {
            converted.conversion_version = CONVERSION_VERSION - 1;
        } else {
            seed_repository_conversion_source(&conn, &mut converted);
        }
        if stale {
            converted.insert(&conn).unwrap();
        } else {
            converted.insert_with_conversion_pin(&conn, 1).unwrap();
        }
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
    let mut state = state;
    state.catalog_authority = catalogs.authority().clone();
    (Arc::new(RwLock::new(state)), temp, catalogs)
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
    let catalogs = ActiveCatalogFixture::new();
    let fedora_revision = catalogs.activate(
        "fedora-44",
        1,
        vec![
            catalog_package(
                "fedora-44",
                "nginx",
                "1.24.0-1.fc44",
                "1",
                Some("x86_64"),
                4096,
                "nginx-1",
            ),
            catalog_package(
                "fedora-44",
                "nginx",
                "1.25.0-1.fc44",
                "1",
                Some("x86_64"),
                4096,
                "nginx-2",
            ),
        ],
    );
    let arch_revision = catalogs.activate(
        "arch",
        2,
        vec![catalog_package(
            "arch",
            "nginx",
            "1.25.0-1",
            "1",
            Some("x86_64"),
            4096,
            "nginx-3",
        )],
    );
    let conn = catalogs.connection();

    insert_converted_package(
        &conn,
        "fedora",
        &fedora_revision,
        "nginx",
        "1.24.0-1.fc44",
        &["chunk1".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        &fedora_revision,
        "nginx",
        "1.25.0-1.fc44",
        &["chunk2".to_string()],
    );
    insert_converted_package(
        &conn,
        "arch",
        &arch_revision,
        "nginx",
        "1.25.0-1",
        &["chunk3".to_string()],
    );

    let tags =
        build_tags_list(catalogs.authority(), catalogs.db_path(), "fedora", "nginx").unwrap();
    assert_eq!(tags, vec!["1.24.0-1.fc44", "1.25.0-1.fc44"]);

    let tags = build_tags_list(catalogs.authority(), catalogs.db_path(), "arch", "nginx").unwrap();
    assert_eq!(tags, vec!["1.25.0-1"]);

    let tags = build_tags_list(
        catalogs.authority(),
        catalogs.db_path(),
        "fedora",
        "nonexistent",
    )
    .unwrap();
    assert!(tags.is_empty());
}

#[test]
fn oci_tags_ignore_stale_converted_rows() {
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate(
        "fedora-44",
        1,
        vec![
            catalog_package(
                "fedora-44",
                "nginx",
                "1.24.0-1.fc44",
                "1",
                Some("x86_64"),
                4096,
                "stale",
            ),
            catalog_package(
                "fedora-44",
                "nginx",
                "1.25.0-1.fc44",
                "1",
                Some("x86_64"),
                4096,
                "current",
            ),
        ],
    );
    let conn = catalogs.connection();

    insert_stale_converted_package(
        &conn,
        "fedora",
        &revision,
        "nginx",
        "1.24.0-1.fc44",
        &["stale-chunk".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        &revision,
        "nginx",
        "1.25.0-1.fc44",
        &["current-chunk".to_string()],
    );

    let tags =
        build_tags_list(catalogs.authority(), catalogs.db_path(), "fedora", "nginx").unwrap();
    assert_eq!(tags, vec!["1.25.0-1.fc44"]);
}

#[test]
fn test_build_catalog() {
    let catalogs = ActiveCatalogFixture::new();
    let fedora_revision = catalogs.activate(
        "fedora-44",
        1,
        vec![
            catalog_package(
                "fedora-44",
                "nginx",
                "1.24.0",
                "1",
                Some("x86_64"),
                4096,
                "nginx",
            ),
            catalog_package(
                "fedora-44",
                "curl",
                "8.5.0",
                "1",
                Some("x86_64"),
                4096,
                "curl",
            ),
        ],
    );
    let arch_revision = catalogs.activate(
        "arch",
        2,
        vec![catalog_package(
            "arch",
            "nginx",
            "1.25.0",
            "1",
            Some("x86_64"),
            4096,
            "arch-nginx",
        )],
    );
    let conn = catalogs.connection();

    insert_converted_package(
        &conn,
        "fedora",
        &fedora_revision,
        "nginx",
        "1.24.0",
        &["chunk1".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        &fedora_revision,
        "curl",
        "8.5.0",
        &["chunk2".to_string()],
    );
    insert_converted_package(
        &conn,
        "arch",
        &arch_revision,
        "nginx",
        "1.25.0",
        &["chunk3".to_string()],
    );

    let catalog = build_catalog(catalogs.authority(), catalogs.db_path()).unwrap();
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
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate(
        "fedora-44",
        1,
        vec![
            catalog_package(
                "fedora-44",
                "stale-only",
                "1.0.0",
                "1",
                Some("x86_64"),
                4096,
                "stale",
            ),
            catalog_package(
                "fedora-44",
                "current",
                "1.0.0",
                "1",
                Some("x86_64"),
                4096,
                "current",
            ),
        ],
    );
    let conn = catalogs.connection();

    insert_stale_converted_package(
        &conn,
        "fedora",
        &revision,
        "stale-only",
        "1.0.0",
        &["stale-chunk".to_string()],
    );
    insert_converted_package(
        &conn,
        "fedora",
        &revision,
        "current",
        "1.0.0",
        &["current-chunk".to_string()],
    );

    let catalog = build_catalog(catalogs.authority(), catalogs.db_path()).unwrap();
    assert_eq!(catalog.repositories, vec!["conary/fedora/current"]);
}

#[test]
fn test_build_catalog_empty() {
    let catalogs = ActiveCatalogFixture::new();
    let catalog = build_catalog(catalogs.authority(), catalogs.db_path()).unwrap();
    assert!(catalog.repositories.is_empty());
}

#[test]
fn test_build_manifest() {
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "nginx",
            "1.24.0",
            "1",
            Some("x86_64"),
            4096,
            "nginx",
        )],
    );
    let conn = catalogs.connection();

    let chunks = vec!["aabbccdd".to_string(), "eeff0011".to_string()];
    insert_converted_package(&conn, "fedora", &revision, "nginx", "1.24.0", &chunks);

    // Create a temporary chunk cache directory
    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        catalogs.db_path().to_path_buf(),
    );
    for (hash, bytes) in [
        ("aabbccdd", b"first".as_slice()),
        ("eeff0011", b"second".as_slice()),
    ] {
        let path = chunk_cache.chunk_path(hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    let result = build_manifest(
        catalogs.authority(),
        catalogs.db_path(),
        "fedora",
        "nginx",
        "1.24.0",
        &chunk_cache,
    )
    .unwrap();

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
    assert_eq!(layers[0]["size"], 5);
    assert_eq!(layers[1]["size"], 6);

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
    let catalogs = ActiveCatalogFixture::new();
    catalogs.activate("fedora-44", 1, Vec::new());
    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        catalogs.db_path().to_path_buf(),
    );

    let result = build_manifest(
        catalogs.authority(),
        catalogs.db_path(),
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
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "nginx",
            "1.24.0",
            "1",
            Some("x86_64"),
            4096,
            "nginx",
        )],
    );
    let conn = catalogs.connection();
    insert_stale_converted_package(
        &conn,
        "fedora",
        &revision,
        "nginx",
        "1.24.0",
        &["stale-chunk".to_string()],
    );

    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        catalogs.db_path().to_path_buf(),
    );

    let result = build_manifest(
        catalogs.authority(),
        catalogs.db_path(),
        "fedora",
        "nginx",
        "1.24.0",
        &chunk_cache,
    )
    .unwrap();
    assert!(result.is_none());
}

#[test]
fn oci_tags_catalog_and_manifest_ignore_stale_rows() {
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "stale-only",
            "1.0",
            "1",
            Some("x86_64"),
            4096,
            "stale",
        )],
    );
    let conn = catalogs.connection();
    insert_stale_converted_package(
        &conn,
        "fedora",
        &revision,
        "stale-only",
        "1.0",
        &["stale-chunk".to_string()],
    );

    let chunk_dir = tempfile::tempdir().unwrap();
    let chunk_cache = crate::server::ChunkCache::new(
        chunk_dir.path().to_path_buf(),
        1024 * 1024 * 1024,
        catalogs.db_path().to_path_buf(),
    );

    let tags = build_tags_list(
        catalogs.authority(),
        catalogs.db_path(),
        "fedora",
        "stale-only",
    )
    .unwrap();
    let catalog = build_catalog(catalogs.authority(), catalogs.db_path()).unwrap();
    let manifest = build_manifest(
        catalogs.authority(),
        catalogs.db_path(),
        "fedora",
        "stale-only",
        "1.0",
        &chunk_cache,
    )
    .unwrap();

    assert!(tags.is_empty());
    assert!(catalog.repositories.is_empty());
    assert!(manifest.is_none());
}

#[tokio::test]
async fn oci_blob_returns_not_found_for_stale_conversion_only_hash() {
    let (state, _temp, _catalogs) = oci_blob_state_with_db(OCI_TEST_HASH, vec![true]).await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = get_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oci_head_blob_returns_not_found_for_stale_conversion_only_hash() {
    let (state, _temp, _catalogs) = oci_blob_state_with_db(OCI_TEST_HASH, vec![true]).await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = head_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oci_blob_allows_hash_shared_with_current_conversion_row() {
    let (state, _temp, _catalogs) = oci_blob_state_with_db(OCI_TEST_HASH, vec![true, false]).await;
    let digest = format!("sha256:{OCI_TEST_HASH}");

    let response = get_blob_inner(state, "conary/fedora/pkg", &digest).await;

    assert_eq!(response.status(), StatusCode::OK);
}
