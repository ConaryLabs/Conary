// apps/remi/src/server/handlers/tuf/tests.rs

use super::*;
use axum::extract::{Path, State};
use conary_core::db::models::Repository;
use conary_core::db::schema;
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

fn remi_empty_db_state() -> (tempfile::TempDir, PathBuf, Arc<RwLock<ServerState>>) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("remi-test.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
    }

    let config = crate::server::ServerConfig {
        db_path,
        chunk_dir: temp.path().join("chunks"),
        cache_dir: temp.path().join("cache"),
        ..Default::default()
    };
    std::fs::create_dir_all(&config.chunk_dir).unwrap();
    std::fs::create_dir_all(&config.cache_dir).unwrap();

    let state = Arc::new(RwLock::new(
        crate::server::ServerState::new(config).expect("test server state"),
    ));
    let cache_dir = temp.path().join("cache");
    (temp, cache_dir, state)
}

#[tokio::test]
async fn tuf_metadata_rejects_unsupported_distro_before_db_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = get_timestamp(State(state), Path("debian".to_string())).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tuf_refresh_rejects_unsupported_distro_before_key_config_lookup() {
    let (_temp, _cache_dir, state) = remi_empty_db_state();
    let response = refresh_timestamp(State(state), Path("debian".to_string())).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(
        !body.contains("repository_keys_dir"),
        "route validation must happen before release_publish.repository_keys_dir lookup: {body}"
    );
}

fn insert_tuf_repo(conn: &Connection, name: &str) -> i64 {
    let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
    repo.tuf_enabled = true;
    repo.insert(conn).unwrap()
}

fn insert_non_tuf_repo(conn: &Connection, name: &str) -> i64 {
    let mut repo = Repository::new(name.to_string(), "https://example.com".to_string());
    repo.insert(conn).unwrap()
}

fn insert_tuf_root(conn: &Connection, repo_id: i64, version: i64, metadata: &str) {
    conn.execute(
        "INSERT INTO tuf_roots (repository_id, version, signed_metadata, spec_version, expires_at, thresholds_json, role_keys_json)
         VALUES (?1, ?2, ?3, '1.0.31', '2099-01-01T00:00:00Z', '{}', '{}')",
        params![repo_id, version, metadata],
    )
    .unwrap();
}

fn insert_tuf_metadata(conn: &Connection, repo_id: i64, role: &str, metadata: &str) {
    conn.execute(
        "INSERT INTO tuf_metadata (repository_id, role, version, metadata_hash, signed_metadata, expires_at)
         VALUES (?1, ?2, 1, 'sha256:test', ?3, '2099-01-01T00:00:00Z')",
        params![repo_id, role, metadata],
    )
    .unwrap();
}

// --- query_tuf_role_metadata tests ---

#[test]
fn test_timestamp_metadata_found() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    let metadata = r#"{"signed":{"_type":"timestamp","version":1}}"#;
    insert_tuf_metadata(&conn, repo_id, "timestamp", metadata);

    let result = query_tuf_role_metadata(temp_file.path(), "fedora", "timestamp").unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), metadata);
}

#[test]
fn test_snapshot_metadata_found() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    let metadata = r#"{"signed":{"_type":"snapshot","version":1}}"#;
    insert_tuf_metadata(&conn, repo_id, "snapshot", metadata);

    let result = query_tuf_role_metadata(temp_file.path(), "fedora", "snapshot").unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), metadata);
}

#[test]
fn test_targets_metadata_found() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    let metadata = r#"{"signed":{"_type":"targets","version":1}}"#;
    insert_tuf_metadata(&conn, repo_id, "targets", metadata);

    let result = query_tuf_role_metadata(temp_file.path(), "fedora", "targets").unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), metadata);
}

#[test]
fn test_metadata_not_found_unknown_distro() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_metadata(
        &conn,
        repo_id,
        "timestamp",
        r#"{"signed":{"_type":"timestamp"}}"#,
    );

    let result = query_tuf_role_metadata(temp_file.path(), "gentoo", "timestamp").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_metadata_not_found_unknown_role() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_metadata(
        &conn,
        repo_id,
        "timestamp",
        r#"{"signed":{"_type":"timestamp"}}"#,
    );

    let result = query_tuf_role_metadata(temp_file.path(), "fedora", "nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_metadata_not_found_empty_db() {
    let (temp_file, _conn) = create_test_db();
    let result = query_tuf_role_metadata(temp_file.path(), "fedora", "timestamp").unwrap();
    assert!(result.is_none());
}

// --- query_latest_root tests ---

#[test]
fn test_latest_root_found() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );
    insert_tuf_root(
        &conn,
        repo_id,
        2,
        r#"{"signed":{"_type":"root","version":2}}"#,
    );

    let result = query_latest_root(temp_file.path(), "fedora").unwrap();
    assert!(result.is_some());
    // Should return the latest version (version 2)
    let metadata = result.unwrap();
    assert!(metadata.contains("\"version\":2"));
}

#[test]
fn test_latest_root_single_version() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "arch");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );

    let result = query_latest_root(temp_file.path(), "arch").unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().contains("\"version\":1"));
}

#[test]
fn test_latest_root_not_found() {
    let (temp_file, _conn) = create_test_db();
    let result = query_latest_root(temp_file.path(), "fedora").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_latest_root_wrong_distro() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );

    let result = query_latest_root(temp_file.path(), "arch").unwrap();
    assert!(result.is_none());
}

// --- query_versioned_root tests ---

#[test]
fn test_versioned_root_found() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );
    insert_tuf_root(
        &conn,
        repo_id,
        2,
        r#"{"signed":{"_type":"root","version":2}}"#,
    );

    let result = query_versioned_root(temp_file.path(), "fedora", 1).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().contains("\"version\":1"));

    let result2 = query_versioned_root(temp_file.path(), "fedora", 2).unwrap();
    assert!(result2.is_some());
    assert!(result2.unwrap().contains("\"version\":2"));
}

#[test]
fn test_versioned_root_not_found_wrong_version() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );

    let result = query_versioned_root(temp_file.path(), "fedora", 99).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_versioned_root_not_found_wrong_distro() {
    let (temp_file, conn) = create_test_db();
    let repo_id = insert_tuf_repo(&conn, "fedora");
    insert_tuf_root(
        &conn,
        repo_id,
        1,
        r#"{"signed":{"_type":"root","version":1}}"#,
    );

    let result = query_versioned_root(temp_file.path(), "arch", 1).unwrap();
    assert!(result.is_none());
}

// --- query_tuf_repos tests ---

#[test]
fn test_tuf_repos_lists_enabled() {
    let (temp_file, conn) = create_test_db();
    insert_tuf_repo(&conn, "fedora");
    insert_tuf_repo(&conn, "arch");
    insert_non_tuf_repo(&conn, "debian-nontuf");

    let repos = query_tuf_repos(temp_file.path()).unwrap();
    assert_eq!(repos.len(), 2);
    assert!(repos.contains(&"fedora".to_string()));
    assert!(repos.contains(&"arch".to_string()));
    assert!(!repos.contains(&"debian-nontuf".to_string()));
}

#[test]
fn test_tuf_repos_empty() {
    let (temp_file, _conn) = create_test_db();
    let repos = query_tuf_repos(temp_file.path()).unwrap();
    assert!(repos.is_empty());
}

#[test]
fn test_tuf_repos_no_enabled() {
    let (temp_file, conn) = create_test_db();
    insert_non_tuf_repo(&conn, "fedora");
    insert_non_tuf_repo(&conn, "arch");

    let repos = query_tuf_repos(temp_file.path()).unwrap();
    assert!(repos.is_empty());
}

// --- metadata isolation between distros ---

#[test]
fn test_metadata_isolated_between_distros() {
    let (temp_file, conn) = create_test_db();
    let fedora_id = insert_tuf_repo(&conn, "fedora");
    let arch_id = insert_tuf_repo(&conn, "arch");

    insert_tuf_metadata(&conn, fedora_id, "timestamp", r#"{"distro":"fedora"}"#);
    insert_tuf_metadata(&conn, arch_id, "timestamp", r#"{"distro":"arch"}"#);

    let fedora_ts = query_tuf_role_metadata(temp_file.path(), "fedora", "timestamp")
        .unwrap()
        .unwrap();
    assert!(fedora_ts.contains("fedora"));

    let arch_ts = query_tuf_role_metadata(temp_file.path(), "arch", "timestamp")
        .unwrap()
        .unwrap();
    assert!(arch_ts.contains("arch"));
}

// --- root versions isolated between distros ---

#[test]
fn test_root_versions_isolated_between_distros() {
    let (temp_file, conn) = create_test_db();
    let fedora_id = insert_tuf_repo(&conn, "fedora");
    let arch_id = insert_tuf_repo(&conn, "arch");

    insert_tuf_root(&conn, fedora_id, 1, r#"{"distro":"fedora","version":1}"#);
    insert_tuf_root(&conn, fedora_id, 2, r#"{"distro":"fedora","version":2}"#);
    insert_tuf_root(&conn, arch_id, 1, r#"{"distro":"arch","version":1}"#);

    // Fedora latest should be version 2
    let fedora_latest = query_latest_root(temp_file.path(), "fedora")
        .unwrap()
        .unwrap();
    assert!(fedora_latest.contains("\"version\":2"));

    // Arch latest should be version 1
    let arch_latest = query_latest_root(temp_file.path(), "arch")
        .unwrap()
        .unwrap();
    assert!(arch_latest.contains("\"distro\":\"arch\""));

    // Arch version 2 should not exist
    let arch_v2 = query_versioned_root(temp_file.path(), "arch", 2).unwrap();
    assert!(arch_v2.is_none());
}

#[tokio::test]
async fn remi_tuf_refresh_timestamp_returns_signed_monotonic_metadata() {
    let fixture = TimestampRefreshFixture::new("fedora", true);
    let first = call_refresh_timestamp_for_tests(&fixture, "fedora").await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_json = response_json_for_tests(first).await;

    assert_eq!(first_json["role"], "timestamp");
    assert_eq!(first_json["distro"], "fedora");
    assert!(first_json["version"].as_u64().unwrap() > 0);

    let second = call_refresh_timestamp_for_tests(&fixture, "fedora").await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_json = response_json_for_tests(second).await;

    assert!(second_json["version"].as_u64().unwrap() > first_json["version"].as_u64().unwrap());
}

#[tokio::test]
async fn remi_tuf_refresh_timestamp_route_is_distro_scoped() {
    let fixture = TimestampRefreshFixture::new("fedora", true);
    let first = call_refresh_timestamp_for_tests(&fixture, "fedora").await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_json = response_json_for_tests(first).await;

    assert_eq!(first_json["role"], "timestamp");
    assert_eq!(first_json["distro"], "fedora");
    assert!(first_json["version"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn remi_tuf_refresh_timestamp_fails_closed_without_role_key() {
    let fixture = TimestampRefreshFixture::new("fedora", false);
    let response = call_refresh_timestamp_for_tests(&fixture, "fedora").await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

struct TimestampRefreshFixture {
    _temp: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl TimestampRefreshFixture {
    fn new(distro: &str, write_timestamp_key: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("remi.db");
        let keys_dir = temp.path().join("keys");
        let source_profile =
            conary_core::repository::supported_profiles::profile_for_remi_route(distro)
                .unwrap()
                .id();
        let distro_key_dir = keys_dir.join(source_profile);
        std::fs::create_dir_all(&distro_key_dir).unwrap();
        std::fs::set_permissions(&keys_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&distro_key_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        let repo_id = insert_tuf_repo(&conn, distro);
        insert_tuf_metadata(&conn, repo_id, "snapshot", &snapshot_metadata_for_tests());
        drop(conn);

        if write_timestamp_key {
            let key =
                conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("timestamp");
            crate::server::signing_authority::save_fixture_key_pair(
                &key,
                &distro_key_dir.join("timestamp.private"),
                &distro_key_dir.join("timestamp.public"),
            )
            .unwrap();
        }

        let release_publish = crate::server::config::ReleasePublishSection {
            repository_keys_dir: Some(keys_dir),
            trusted_build_attestation_signers: Vec::new(),
        };
        let config = crate::server::ServerConfig {
            db_path,
            chunk_dir: temp.path().join("chunks"),
            cache_dir: temp.path().join("cache"),
            release_publish,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));

        Self { _temp: temp, state }
    }
}

async fn call_refresh_timestamp_for_tests(
    fixture: &TimestampRefreshFixture,
    distro: &str,
) -> Response {
    refresh_timestamp(State(fixture.state.clone()), Path(distro.to_string())).await
}

async fn response_json_for_tests(response: Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn snapshot_metadata_for_tests() -> String {
    let snapshot = conary_core::trust::Signed {
        signed: conary_core::trust::SnapshotMetadata {
            type_field: "snapshot".to_string(),
            spec_version: conary_core::trust::TUF_SPEC_VERSION.to_string(),
            version: 1,
            expires: chrono::Utc::now() + chrono::Duration::days(7),
            meta: std::collections::BTreeMap::new(),
        },
        signatures: Vec::new(),
    };
    serde_json::to_string(&snapshot).unwrap()
}
