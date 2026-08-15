// apps/remi/src/server/handlers/chunks/tests.rs
use super::*;
use crate::server::conversion::test_support::seed_repository_conversion_source;

use conary_core::db::models::{CONVERSION_VERSION, ChunkAccess, ConvertedPackage};

const TEST_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn durable_authority_failure_has_stable_typed_http_contract() {
    let response = durable_chunk_unavailable(TEST_HASH);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get("x-conary-error").unwrap(),
        "durable-chunk-unavailable"
    );
}

async fn chunk_state_with_db(
    hash: &str,
    stale_rows: Vec<bool>,
) -> (Arc<RwLock<crate::server::ServerState>>, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    let chunk_path = crate::server::handlers::cas_object_path(&chunk_dir, hash);
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, b"chunk bytes").unwrap();

    for (index, stale) in stale_rows.into_iter().enumerate() {
        let transport =
            crate::server::conversion::test_support::test_transport(&[hash.to_string()]);
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            format!("pkg-{index}"),
            "1.0".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            format!("sha256:source-{index}"),
            &transport,
            11,
            format!("sha256:content-{index}"),
            format!("/tmp/pkg-{index}.ccs"),
            conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        if stale {
            converted.conversion_version = CONVERSION_VERSION - 1;
        } else {
            seed_repository_conversion_source(&conn, &mut converted);
        }
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
fn test_parse_range_header_closed_range() {
    // bytes=0-499 (first 500 bytes)
    let result = parse_range_header("bytes=0-499", 1000);
    assert_eq!(result, Some((0, 499)));

    // bytes=500-999 (last 500 bytes)
    let result = parse_range_header("bytes=500-999", 1000);
    assert_eq!(result, Some((500, 999)));

    // bytes=0-0 (first byte only)
    let result = parse_range_header("bytes=0-0", 1000);
    assert_eq!(result, Some((0, 0)));
}

#[test]
fn test_parse_range_header_open_ended() {
    // bytes=500- (from byte 500 to end)
    let result = parse_range_header("bytes=500-", 1000);
    assert_eq!(result, Some((500, 999)));

    // bytes=0- (entire file)
    let result = parse_range_header("bytes=0-", 1000);
    assert_eq!(result, Some((0, 999)));
}

#[test]
fn test_parse_range_header_suffix() {
    // bytes=-500 (last 500 bytes)
    let result = parse_range_header("bytes=-500", 1000);
    assert_eq!(result, Some((500, 999)));

    // bytes=-1 (last byte)
    let result = parse_range_header("bytes=-1", 1000);
    assert_eq!(result, Some((999, 999)));

    // bytes=-1000 (entire file via suffix)
    let result = parse_range_header("bytes=-1000", 1000);
    assert_eq!(result, Some((0, 999)));
}

#[test]
fn test_parse_range_header_clamp_to_file_size() {
    // bytes=0-9999 should clamp end to file size - 1
    let result = parse_range_header("bytes=0-9999", 1000);
    assert_eq!(result, Some((0, 999)));
}

#[test]
fn test_parse_range_header_invalid() {
    // No bytes= prefix
    assert!(parse_range_header("0-499", 1000).is_none());

    // Start > end
    assert!(parse_range_header("bytes=500-100", 1000).is_none());

    // Start >= file_size
    assert!(parse_range_header("bytes=1000-", 1000).is_none());
    assert!(parse_range_header("bytes=1001-", 1000).is_none());

    // Suffix larger than file
    assert!(parse_range_header("bytes=-1001", 1000).is_none());

    // Zero suffix
    assert!(parse_range_header("bytes=-0", 1000).is_none());

    // Multiple ranges (not supported)
    assert!(parse_range_header("bytes=0-100,200-300", 1000).is_none());

    // Invalid format
    assert!(parse_range_header("bytes=abc-def", 1000).is_none());
    assert!(parse_range_header("bytes=", 1000).is_none());
    assert!(parse_range_header("bytes=-", 1000).is_none());
}

#[test]
fn test_is_valid_hash() {
    // Valid 64-char hex hash
    assert!(is_valid_hash(
        "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
    ));

    // Wrong length
    assert!(!is_valid_hash("abcdef"));
    assert!(!is_valid_hash(""));

    // Invalid characters
    assert!(!is_valid_hash(
        "ghijklmnopqrstuv1234567890abcdef1234567890abcdef1234567890abcd"
    ));
    assert!(!is_valid_hash(
        "ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF12345678901"
    )); // too long

    // Uppercase hex rejected (CAS uses lowercase paths)
    assert!(!is_valid_hash(
        "ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890"
    ));
}

#[test]
fn test_max_range_size_constant() {
    // MAX_RANGE_SIZE should be 64 MB
    assert_eq!(MAX_RANGE_SIZE, 64 * 1024 * 1024);

    // A range that exceeds MAX_RANGE_SIZE would be rejected at the handler
    // level. Verify the constant is reasonable (> 0, < 1 GB).
    const { assert!(MAX_RANGE_SIZE > 0) };
    const { assert!(MAX_RANGE_SIZE < 1024 * 1024 * 1024) };
}

#[test]
fn test_extract_hash_from_path() {
    use std::path::Path;

    let path = Path::new("/chunks/objects/ab/cdef1234");
    assert_eq!(extract_hash_from_path(path), Some("abcdef1234".to_string()));

    let path = Path::new("/ab/cdef");
    assert_eq!(extract_hash_from_path(path), Some("abcdef".to_string()));
}

#[tokio::test]
async fn get_chunk_returns_not_found_for_stale_conversion_only_hash() {
    let (state, _temp) = chunk_state_with_db(TEST_HASH, vec![true]).await;

    let response = get_chunk(State(state), Path(TEST_HASH.to_string()), HeaderMap::new()).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_chunk_returns_not_found_for_stale_conversion_only_hash() {
    let (state, _temp) = chunk_state_with_db(TEST_HASH, vec![true]).await;

    let response = head_chunk(State(state), Path(TEST_HASH.to_string())).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_chunk_allows_hash_shared_with_current_conversion_row() {
    let (state, _temp) = chunk_state_with_db(TEST_HASH, vec![true, false]).await;

    let response = get_chunk(State(state), Path(TEST_HASH.to_string()), HeaderMap::new()).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn head_chunk_allows_hash_shared_with_current_conversion_row() {
    let (state, _temp) = chunk_state_with_db(TEST_HASH, vec![true, false]).await;

    let response = head_chunk(State(state), Path(TEST_HASH.to_string())).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_chunk_hides_unreferenced_protected_local_cache_hash() {
    let (state, _temp) = chunk_state_with_db(TEST_HASH, Vec::new()).await;
    {
        let state_guard = state.read().await;
        let conn = crate::server::open_runtime_db(&state_guard.config.db_path).unwrap();
        let mut chunk = ChunkAccess::new(TEST_HASH.to_string(), 11);
        chunk.protected = true;
        chunk.upsert(&conn).unwrap();
    }

    let response = get_chunk(State(state), Path(TEST_HASH.to_string()), HeaderMap::new()).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
