// apps/remi/src/server/handlers/tuf.rs

//! TUF metadata HTTP handlers for the Remi server
//!
//! Serves TUF metadata files for repository trust verification:
//! - timestamp.json (frequently updated, short-lived)
//! - snapshot.json (pins all metadata versions)
//! - targets.json (maps packages to hashes)
//! - root.json (trust anchor, rarely changes)
//! - {version}.root.json (versioned roots for key rotation)

use crate::server::ServerState;
use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::signing::SigningKeyPair;
use conary_core::trust::{
    MetaFile, Signed, TUF_SPEC_VERSION, TimestampMetadata, sign_tuf_metadata,
};
use rusqlite::OptionalExtension;
use rusqlite::params;
use std::collections::BTreeMap;
use std::path::Path as StdPath;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use super::open_handler_db;

/// GET /v1/{distro}/tuf/timestamp.json
pub async fn get_timestamp(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "timestamp".to_string()).await
}

/// GET /v1/{distro}/tuf/snapshot.json
pub async fn get_snapshot(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "snapshot".to_string()).await
}

/// GET /v1/{distro}/tuf/targets.json
pub async fn get_targets(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "targets".to_string()).await
}

/// GET /v1/{distro}/tuf/root.json (latest version)
pub async fn get_root(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || query_latest_root(&db_path, &distro)).await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch TUF root: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /v1/{distro}/tuf/{version}.root.json (specific version for key rotation)
pub async fn get_versioned_root(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, version_str)): Path<(String, String)>,
) -> Response {
    // Parse version from "{version}.root" pattern
    let version: i64 = match version_str
        .strip_suffix(".root")
        .and_then(|v| v.parse().ok())
    {
        Some(v) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result =
        tokio::task::spawn_blocking(move || query_versioned_root(&db_path, &distro, version)).await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch versioned TUF root: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct TimestampRefreshResult {
    pub status: String,
    pub role: String,
    pub distro: String,
    pub version: u64,
}

/// POST /v1/admin/tuf/{distro}/refresh-timestamp (admin endpoint)
pub async fn refresh_timestamp(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    match refresh_timestamp_for_distro(&state, &distro).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(error) => {
            warn!("Failed to refresh TUF timestamp for {distro}: {error:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "code": "TIMESTAMP_REFRESH_FAILED",
                })),
            )
                .into_response()
        }
    }
}

pub async fn refresh_timestamp_for_distro(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
) -> Result<TimestampRefreshResult> {
    let (db_path, keys_dir) = {
        let guard = state.read().await;
        let keys_dir = guard
            .config
            .release_publish
            .repository_keys_dir
            .clone()
            .context("release_publish.repository_keys_dir is not configured")?;
        (guard.config.db_path.clone(), keys_dir)
    };
    let distro = distro.to_string();

    tokio::task::spawn_blocking(move || {
        refresh_timestamp_for_distro_blocking(&db_path, &keys_dir, &distro)
    })
    .await
    .context("refresh timestamp blocking task failed")?
}

/// Helper: Get TUF metadata by role from the database
async fn get_tuf_metadata(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    role: String,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result =
        tokio::task::spawn_blocking(move || query_tuf_role_metadata(&db_path, &distro, &role))
            .await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch TUF metadata: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- Database query functions (run on blocking threads) ---

fn query_latest_root(db_path: &std::path::Path, distro: &str) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tr.signed_metadata FROM tuf_roots tr
             JOIN repositories r ON tr.repository_id = r.id
             WHERE r.name = ?1
             ORDER BY tr.version DESC LIMIT 1",
            params![distro],
            |row| row.get(0),
        )
        .optional()?)
}

fn query_versioned_root(
    db_path: &std::path::Path,
    distro: &str,
    version: i64,
) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tr.signed_metadata FROM tuf_roots tr
             JOIN repositories r ON tr.repository_id = r.id
             WHERE r.name = ?1 AND tr.version = ?2",
            params![distro, version],
            |row| row.get(0),
        )
        .optional()?)
}

fn query_tuf_role_metadata(
    db_path: &std::path::Path,
    distro: &str,
    role: &str,
) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tm.signed_metadata FROM tuf_metadata tm
             JOIN repositories r ON tm.repository_id = r.id
             WHERE r.name = ?1 AND tm.role = ?2",
            params![distro, role],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn load_release_tuf_key(
    keys_dir: &StdPath,
    distro: &str,
    role: &str,
) -> Result<SigningKeyPair> {
    let path = keys_dir.join(distro).join(format!("{role}.private"));
    SigningKeyPair::load_from_file(&path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load Remi {role} TUF signing key {}", path.display()))
}

fn refresh_timestamp_for_distro_blocking(
    db_path: &StdPath,
    keys_dir: &StdPath,
    distro: &str,
) -> Result<TimestampRefreshResult> {
    let timestamp_key = load_release_tuf_key(keys_dir, distro, "timestamp")?;
    let conn = open_handler_db(db_path)?;
    let repo_id: i64 = conn
        .query_row(
            "SELECT id FROM repositories WHERE name = ?1 AND tuf_enabled = 1",
            params![distro],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("TUF repository not found for distro {distro}"))?;

    let Some((snapshot_version, snapshot_json)) = query_snapshot_for_timestamp(&conn, repo_id)?
    else {
        bail!("snapshot metadata is missing for distro {distro}");
    };
    let previous_version: Option<i64> = conn
        .query_row(
            "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = 'timestamp'",
            params![repo_id],
            |row| row.get(0),
        )
        .optional()?;
    let version = previous_version.unwrap_or(0) + 1;
    let snapshot_bytes = snapshot_json.as_bytes();
    let mut hashes = BTreeMap::new();
    hashes.insert(
        "sha256".to_string(),
        conary_core::hash::sha256(snapshot_bytes),
    );
    let mut meta = BTreeMap::new();
    meta.insert(
        "snapshot.json".to_string(),
        MetaFile {
            version: snapshot_version as u64,
            length: Some(snapshot_bytes.len() as u64),
            hashes: Some(hashes),
        },
    );
    let timestamp = TimestampMetadata {
        type_field: "timestamp".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: version as u64,
        expires: chrono::Utc::now() + chrono::Duration::days(1),
        meta,
    };
    let signed = Signed {
        signatures: vec![
            sign_tuf_metadata(&timestamp_key, &timestamp).map_err(anyhow::Error::from)?,
        ],
        signed: timestamp,
    };
    let signed_json = serde_json::to_string(&signed)?;
    let metadata_hash = conary_core::hash::sha256(signed_json.as_bytes());

    conn.execute(
        "INSERT OR REPLACE INTO tuf_metadata
         (repository_id, role, version, metadata_hash, signed_metadata, expires_at)
         VALUES (?1, 'timestamp', ?2, ?3, ?4, ?5)",
        params![
            repo_id,
            version,
            metadata_hash,
            signed_json,
            signed.signed.expires.to_rfc3339(),
        ],
    )?;

    Ok(TimestampRefreshResult {
        status: "ok".to_string(),
        role: "timestamp".to_string(),
        distro: distro.to_string(),
        version: version as u64,
    })
}

fn query_snapshot_for_timestamp(
    conn: &rusqlite::Connection,
    repo_id: i64,
) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT version, signed_metadata FROM tuf_metadata
             WHERE repository_id = ?1 AND role = 'snapshot'",
            params![repo_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
fn query_tuf_repos(db_path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let conn = conary_core::db::open(db_path)?;
    let mut stmt = conn.prepare("SELECT name FROM repositories WHERE tuf_enabled = 1")?;

    let repos: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(repos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::Repository;
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
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
        let fixture = TimestampRefreshFixture::new("test-distro", true);
        let first = call_refresh_timestamp_for_tests(&fixture, "test-distro").await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_json = response_json_for_tests(first).await;

        assert_eq!(first_json["role"], "timestamp");
        assert_eq!(first_json["distro"], "test-distro");
        assert!(first_json["version"].as_u64().unwrap() > 0);

        let second = call_refresh_timestamp_for_tests(&fixture, "test-distro").await;
        assert_eq!(second.status(), StatusCode::OK);
        let second_json = response_json_for_tests(second).await;

        assert!(second_json["version"].as_u64().unwrap() > first_json["version"].as_u64().unwrap());
    }

    #[tokio::test]
    async fn remi_tuf_refresh_timestamp_fails_closed_without_role_key() {
        let fixture = TimestampRefreshFixture::new("test-distro", false);
        let response = call_refresh_timestamp_for_tests(&fixture, "test-distro").await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    struct TimestampRefreshFixture {
        _temp: tempfile::TempDir,
        state: Arc<RwLock<ServerState>>,
    }

    impl TimestampRefreshFixture {
        fn new(distro: &str, write_timestamp_key: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let db_path = temp.path().join("remi.db");
            let keys_dir = temp.path().join("keys");
            let distro_key_dir = keys_dir.join(distro);
            std::fs::create_dir_all(&distro_key_dir).unwrap();

            let conn = Connection::open(&db_path).unwrap();
            conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
            schema::migrate(&conn).unwrap();
            let repo_id = insert_tuf_repo(&conn, distro);
            insert_tuf_metadata(&conn, repo_id, "snapshot", &snapshot_metadata_for_tests());
            drop(conn);

            if write_timestamp_key {
                let key =
                    conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("timestamp");
                key.save_to_files(
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
}
