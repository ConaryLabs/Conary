// apps/remi/src/server/handlers/derivations.rs
//! Derivation cache endpoints
//!
//! Stores and retrieves pre-built derivation outputs keyed by a content-addressed
//! derivation ID. The manifest (OutputManifest TOML) is stored as a raw CAS object;
//! metadata (package_name, package_version) is indexed in `derivation_cache`.
//!
//! Endpoints:
//! - GET  /v1/derivations/:derivation_id   -- fetch manifest TOML (public)
//! - HEAD /v1/derivations/:derivation_id   -- check existence (public)
//! - PUT  /v1/derivations/:derivation_id   -- publish manifest (requires bearer token)
//! - POST /v1/derivations/probe            -- batch existence check (public)

use crate::server::ServerState;
use crate::server::handlers::{
    cas_object_path, is_valid_path_param, open_handler_db, require_admin_token,
};
use axum::{
    Json,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────

/// Validate that a derivation ID contains only URL-safe characters.
///
/// Derivation IDs are typically SHA-256 hex strings, but we allow any
/// alphanumeric + `[._-]` string up to 128 characters to be future-proof.
fn is_valid_derivation_id(id: &str) -> bool {
    is_valid_path_param(id)
}

// ────────────────────────────────────────────────────────────
// Structs for PUT body parsing
// ────────────────────────────────────────────────────────────

/// Minimal subset of the OutputManifest TOML we need to extract for indexing.
#[derive(Deserialize)]
struct ManifestFields {
    package_name: String,
    package_version: String,
}

// ────────────────────────────────────────────────────────────
// GET /v1/derivations/:derivation_id
// ────────────────────────────────────────────────────────────

/// Retrieve the manifest TOML for a cached derivation output.
///
/// Returns:
/// - 200 OK with `Content-Type: application/toml` and the raw TOML body
/// - 404 Not Found if the derivation ID is unknown
pub async fn get_derivation(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(derivation_id): Path<String>,
) -> Response {
    if !is_valid_derivation_id(&derivation_id) {
        return (StatusCode::BAD_REQUEST, "Invalid derivation ID format").into_response();
    }

    let (db_path, chunk_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.chunk_dir.clone())
    };

    // Query the DB for the CAS hash
    let cas_hash = match tokio::task::spawn_blocking({
        let id = derivation_id.clone();
        move || -> anyhow::Result<Option<String>> {
            let conn = open_handler_db(&db_path)?;
            let result = conn.query_row(
                "SELECT manifest_cas_hash FROM derivation_cache WHERE derivation_id = ?1",
                rusqlite::params![id],
                |row| row.get::<_, String>(0),
            );
            match result {
                Ok(hash) => Ok(Some(hash)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
    })
    .await
    {
        Ok(Ok(Some(hash))) => hash,
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND, "Derivation not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error looking up derivation {derivation_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Task panicked looking up derivation {derivation_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    // Read the manifest bytes from CAS
    let object_path = cas_object_path(&chunk_dir, &cas_hash);
    match tokio::fs::read(&object_path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/toml")
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(
                "derivation_cache row exists for {derivation_id} but CAS object {cas_hash} missing"
            );
            (StatusCode::NOT_FOUND, "Manifest not found in CAS").into_response()
        }
        Err(e) => {
            tracing::error!("Failed to read CAS object {cas_hash}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read manifest").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// HEAD /v1/derivations/:derivation_id
// ────────────────────────────────────────────────────────────

/// Check existence of a cached derivation output without reading data.
///
/// Returns:
/// - 204 No Content if the derivation ID exists in the cache
/// - 404 Not Found if it does not
pub async fn head_derivation(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(derivation_id): Path<String>,
) -> Response {
    if !is_valid_derivation_id(&derivation_id) {
        return (StatusCode::BAD_REQUEST, "Invalid derivation ID format").into_response();
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    match tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        let conn = open_handler_db(&db_path)?;
        let result = conn.query_row(
            "SELECT 1 FROM derivation_cache WHERE derivation_id = ?1",
            rusqlite::params![derivation_id],
            |_| Ok(()),
        );
        match result {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    })
    .await
    {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error in HEAD derivation: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in HEAD derivation: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// PUT /v1/derivations/:derivation_id
// ────────────────────────────────────────────────────────────

/// Publish a derivation output manifest.
///
/// The request body must be the raw TOML bytes of an OutputManifest, which
/// must contain at least `package_name` and `package_version` fields.
///
/// Steps:
/// 1. Validate bearer token (admin scope required)
/// 2. Read body bytes; SHA-256 hash them for the CAS key
/// 3. Parse TOML to extract `package_name` / `package_version`
/// 4. Write bytes into CAS at `<chunk_dir>/objects/<2>/<62>`
/// 5. Upsert row into `derivation_cache`
///
/// Returns 201 Created on success.
///
/// NOTE: Auth is checked inline via `require_admin_token` rather than the admin
/// router's middleware so that GET/HEAD on the same path stay public. This means
/// the admin rate limiters (governor-based) do not apply here.
// TODO: Move write endpoints to the admin router to get rate limiting for free.
pub async fn put_derivation(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(derivation_id): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if !is_valid_derivation_id(&derivation_id) {
        return (StatusCode::BAD_REQUEST, "Invalid derivation ID format").into_response();
    }

    let (db_path, chunk_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.chunk_dir.clone())
    };

    // Auth check (inline, so GET/HEAD on same path stay public)
    if let Some(err) = require_admin_token(&headers, &db_path).await {
        return err;
    }

    // Read body
    let body_bytes = match axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to read PUT /v1/derivations body: {e}");
            return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response();
        }
    };

    if body_bytes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Request body must not be empty").into_response();
    }

    // Parse TOML to extract required fields
    let body_str = match std::str::from_utf8(&body_bytes) {
        Ok(s) => s,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Request body must be valid UTF-8").into_response();
        }
    };
    let manifest: ManifestFields = match toml::from_str(body_str) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Invalid manifest TOML for {derivation_id}: {e}");
            return (
                StatusCode::BAD_REQUEST,
                "Invalid manifest TOML: missing package_name or package_version",
            )
                .into_response();
        }
    };

    if manifest.package_name.is_empty() || manifest.package_version.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "package_name and package_version must not be empty",
        )
            .into_response();
    }

    // Compute CAS hash
    let cas_hash = conary_core::hash::sha256(&body_bytes);

    // Write to CAS
    let object_path = cas_object_path(&chunk_dir, &cas_hash);
    if let Some(parent) = object_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        tracing::error!("Failed to create CAS directory {}: {e}", parent.display());
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write to CAS").into_response();
    }

    // Write is idempotent: if the file already exists with the same content, skip
    if !object_path.exists()
        && let Err(e) = tokio::fs::write(&object_path, &body_bytes).await
    {
        tracing::error!("Failed to write CAS object {cas_hash}: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write to CAS").into_response();
    }

    // Upsert into derivation_cache
    let package_name = manifest.package_name.clone();
    let package_version = manifest.package_version.clone();
    let id = derivation_id.clone();
    let hash = cas_hash.clone();

    match tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let conn = open_handler_db(&db_path)?;
        conn.execute(
            "INSERT INTO derivation_cache (derivation_id, manifest_cas_hash, package_name, package_version)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(derivation_id) DO UPDATE SET
                 manifest_cas_hash = excluded.manifest_cas_hash,
                 package_name      = excluded.package_name,
                 package_version   = excluded.package_version",
            rusqlite::params![id, hash, package_name, package_version],
        )?;
        Ok(())
    })
    .await
    {
        Ok(Ok(())) => {
            tracing::info!(
                derivation_id = %derivation_id,
                package = %manifest.package_name,
                version = %manifest.package_version,
                cas_hash = %cas_hash,
                "Derivation cached"
            );
            Response::builder()
                .status(StatusCode::CREATED)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(format!(
                    "Cached derivation {derivation_id} (cas: {cas_hash})"
                )))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(e)) => {
            tracing::error!("DB error inserting derivation {derivation_id}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked inserting derivation {derivation_id}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// POST /v1/derivations/probe
// ────────────────────────────────────────────────────────────

/// Batch existence check for derivation IDs.
///
/// Accepts a JSON array of derivation ID strings and returns a JSON object
/// mapping each ID to a boolean indicating whether it exists in the cache.
///
/// Example request:  `["abc123", "def456"]`
/// Example response: `{"abc123": true, "def456": false}`
pub async fn probe_derivations(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(ids): Json<Vec<String>>,
) -> Response {
    if ids.is_empty() {
        let empty: HashMap<String, bool> = HashMap::new();
        return Json(empty).into_response();
    }

    // Validate all IDs up front
    for id in &ids {
        if !is_valid_derivation_id(id) {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid derivation ID: {id}"),
            )
                .into_response();
        }
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    match tokio::task::spawn_blocking(move || -> anyhow::Result<HashMap<String, bool>> {
        let conn = open_handler_db(&db_path)?;

        // Build a result map, defaulting everything to false
        let mut result: HashMap<String, bool> = ids.iter().cloned().map(|id| (id, false)).collect();

        // Query for IDs that actually exist
        // SQLite doesn't support binding a dynamic list natively so we use a
        // temporary in-memory approach: query one-by-one for simplicity and
        // correctness. For the expected batch sizes (< 1000), this is fast enough.
        let mut stmt =
            conn.prepare("SELECT derivation_id FROM derivation_cache WHERE derivation_id = ?1")?;

        for (id, exists) in result.iter_mut() {
            let found = stmt.query_row(rusqlite::params![id], |_| Ok(())).is_ok();
            *exists = found;
        }

        Ok(result)
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error in probe_derivations: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in probe_derivations: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── unit tests for pure helpers ──────────────────────────

    #[test]
    fn valid_derivation_ids() {
        assert!(is_valid_derivation_id(
            "abc1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcd"
        ));
        assert!(is_valid_derivation_id("short-id"));
        assert!(is_valid_derivation_id("pkg.version_1-0"));
    }

    #[test]
    fn invalid_derivation_ids() {
        assert!(!is_valid_derivation_id(""));
        assert!(!is_valid_derivation_id("has/slash"));
        assert!(!is_valid_derivation_id("has space"));
        assert!(!is_valid_derivation_id(&"x".repeat(129)));
    }

    #[test]
    fn cas_path_layout() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/chunks");
        let hash = "aabbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890";
        let path = cas_object_path(&dir, hash);
        assert_eq!(
            path,
            PathBuf::from(
                "/chunks/objects/aa/bbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890"
            )
        );
    }

    // ── integration-style DB logic tests ────────────────────

    /// Build a temporary in-memory-like DB with the full schema.
    fn make_test_db() -> (rusqlite::Connection, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();
        (conn, tmp)
    }

    #[test]
    fn probe_logic_works() {
        let (conn, _tmp) = make_test_db();

        // Insert one row
        conn.execute(
            "INSERT INTO derivation_cache (derivation_id, manifest_cas_hash, package_name, package_version)
             VALUES ('drv-exists', 'cafebabe', 'mypkg', '1.0.0')",
            [],
        )
        .unwrap();

        // Verify present
        let found: bool = conn
            .query_row(
                "SELECT 1 FROM derivation_cache WHERE derivation_id = ?1",
                rusqlite::params!["drv-exists"],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(found, "drv-exists should be present");

        // Verify absent
        let not_found = conn
            .query_row(
                "SELECT 1 FROM derivation_cache WHERE derivation_id = ?1",
                rusqlite::params!["drv-missing"],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(!not_found, "drv-missing should not be present");
    }

    #[test]
    fn upsert_derivation_cache() {
        let (conn, _tmp) = make_test_db();

        // Initial insert
        conn.execute(
            "INSERT INTO derivation_cache (derivation_id, manifest_cas_hash, package_name, package_version)
             VALUES ('drv-upsert', 'hash1', 'pkg', '1.0.0')",
            [],
        )
        .unwrap();

        // Upsert with updated hash
        conn.execute(
            "INSERT INTO derivation_cache (derivation_id, manifest_cas_hash, package_name, package_version)
             VALUES ('drv-upsert', 'hash2', 'pkg', '1.0.1')
             ON CONFLICT(derivation_id) DO UPDATE SET
                 manifest_cas_hash = excluded.manifest_cas_hash,
                 package_name      = excluded.package_name,
                 package_version   = excluded.package_version",
            [],
        )
        .unwrap();

        let (hash, version): (String, String) = conn
            .query_row(
                "SELECT manifest_cas_hash, package_version FROM derivation_cache WHERE derivation_id = 'drv-upsert'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(hash, "hash2");
        assert_eq!(version, "1.0.1");
    }
}
