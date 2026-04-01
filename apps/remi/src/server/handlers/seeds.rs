// apps/remi/src/server/handlers/seeds.rs
//! Seed registry endpoints
//!
//! Stores and retrieves bootstrap seed metadata and images.
//! Seeds are pre-built EROFS images that serve as Layer 0 for the CAS-layered
//! bootstrap pipeline. The image is stored in CAS; metadata is indexed in `seeds`.
//!
//! Endpoints:
//! - PUT  /v1/seeds/:seed_id        -- publish a seed (requires bearer token)
//! - GET  /v1/seeds/:seed_id        -- fetch seed metadata TOML (public)
//! - GET  /v1/seeds/:seed_id/image  -- stream seed image from CAS (public)
//! - GET  /v1/seeds?target=         -- list seeds filtered by target triple (public)
//! - GET  /v1/seeds/latest?target=  -- fetch most-recent seed for a target (public)

use crate::server::ServerState;
use crate::server::handlers::{
    cas_object_path, is_valid_path_param, open_handler_db, require_admin_token,
};
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// ────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────

/// Validate that a seed ID contains only URL-safe characters.
fn is_valid_seed_id(id: &str) -> bool {
    is_valid_path_param(id)
}

// ────────────────────────────────────────────────────────────
// Request / response types
// ────────────────────────────────────────────────────────────

/// Metadata fields parsed from the PUT body TOML.
///
/// Mirrors `conary_core::derivation::seed::SeedMetadata` but lives here to
/// avoid a server -> core dependency on the derivation module's internal types.
#[derive(Debug, Deserialize)]
struct SeedMetadataBody {
    seed_id: String,
    source: String,
    #[allow(dead_code)]
    origin_url: Option<String>,
    builder: Option<String>,
    packages: Option<Vec<String>>,
    target_triple: String,
    verified_by: Option<Vec<String>>,
    image_cas_hash: String,
}

/// Per-seed summary item returned by list / latest endpoints.
#[derive(Debug, Serialize)]
pub struct SeedListItem {
    pub seed_id: String,
    pub target_triple: String,
    pub source: String,
    pub builder: Option<String>,
    pub package_count: usize,
    pub verified_by_count: usize,
    pub created_at: String,
}

/// Query parameters for list / latest endpoints.
#[derive(Debug, Deserialize)]
pub struct SeedListQuery {
    target: Option<String>,
}

/// Raw row returned by the `get_seed` DB query.
type SeedRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
);

// ────────────────────────────────────────────────────────────
// PUT /v1/seeds/:seed_id
// ────────────────────────────────────────────────────────────

/// Publish a bootstrap seed.
///
/// The request body must be TOML containing at minimum `seed_id`,
/// `target_triple`, `source`, and `image_cas_hash`. The image itself
/// must already exist in the CAS before calling this endpoint.
///
/// Returns 201 Created on success.
///
/// NOTE: Auth is checked inline via `require_admin_token` rather than the admin
/// router's middleware so that GET on the same path stays public. This means
/// the admin rate limiters (governor-based) do not apply here.
// TODO: Move write endpoints to the admin router to get rate limiting for free.
pub async fn put_seed(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(seed_id): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if !is_valid_seed_id(&seed_id) {
        return (StatusCode::BAD_REQUEST, "Invalid seed ID format").into_response();
    }

    let (db_path, _chunk_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.chunk_dir.clone())
    };

    if let Some(err) = require_admin_token(&headers, &db_path).await {
        return err;
    }

    // Read body (4 MiB limit for metadata TOML)
    let body_bytes = match axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to read PUT /v1/seeds body: {e}");
            return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response();
        }
    };

    if body_bytes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Request body must not be empty").into_response();
    }

    let body_str = match std::str::from_utf8(&body_bytes) {
        Ok(s) => s,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Request body must be valid UTF-8").into_response();
        }
    };

    let meta: SeedMetadataBody = match toml::from_str(body_str) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Invalid seed metadata TOML for {seed_id}: {e}");
            return (StatusCode::BAD_REQUEST, "Invalid seed metadata TOML").into_response();
        }
    };

    if meta.seed_id != seed_id {
        return (
            StatusCode::BAD_REQUEST,
            "seed_id in body does not match URL path",
        )
            .into_response();
    }

    if meta.target_triple.is_empty() {
        return (StatusCode::BAD_REQUEST, "target_triple must not be empty").into_response();
    }

    if meta.image_cas_hash.is_empty() {
        return (StatusCode::BAD_REQUEST, "image_cas_hash must not be empty").into_response();
    }

    let packages_json = serde_json::to_string(&meta.packages.unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_owned());
    let verified_by_json = serde_json::to_string(&meta.verified_by.unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_owned());

    match tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let conn = open_handler_db(&db_path)?;
        conn.execute(
            "INSERT INTO seeds (seed_id, target_triple, source, builder, packages_json, verified_by_json, image_cas_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(seed_id) DO UPDATE SET
                 target_triple    = excluded.target_triple,
                 source           = excluded.source,
                 builder          = excluded.builder,
                 packages_json    = excluded.packages_json,
                 verified_by_json = excluded.verified_by_json,
                 image_cas_hash   = excluded.image_cas_hash",
            rusqlite::params![
                meta.seed_id,
                meta.target_triple,
                meta.source,
                meta.builder,
                packages_json,
                verified_by_json,
                meta.image_cas_hash,
            ],
        )?;
        Ok(())
    })
    .await
    {
        Ok(Ok(())) => {
            tracing::info!(seed_id = %seed_id, "Seed registered");
            Response::builder()
                .status(StatusCode::CREATED)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(format!("Registered seed {seed_id}")))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(e)) => {
            tracing::error!("DB error inserting seed {seed_id}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked inserting seed {seed_id}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// GET /v1/seeds/:seed_id
// ────────────────────────────────────────────────────────────

/// Retrieve seed metadata as TOML.
///
/// Returns:
/// - 200 OK with `Content-Type: application/toml`
/// - 404 Not Found if the seed ID is unknown
pub async fn get_seed(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(seed_id): Path<String>,
) -> Response {
    if !is_valid_seed_id(&seed_id) {
        return (StatusCode::BAD_REQUEST, "Invalid seed ID format").into_response();
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let row = match tokio::task::spawn_blocking({
        let id = seed_id.clone();
        move || -> anyhow::Result<Option<SeedRow>> {
            let conn = open_handler_db(&db_path)?;
            let result = conn.query_row(
                "SELECT seed_id, target_triple, builder, source, packages_json, verified_by_json, image_cas_hash
                 FROM seeds WHERE seed_id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            );
            match result {
                Ok(row) => Ok(Some(row)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
    })
    .await
    {
        Ok(Ok(Some(row))) => row,
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND, "Seed not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error looking up seed {seed_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Task panicked looking up seed {seed_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    let (id, target_triple, builder, source, packages_json, verified_by_json, image_cas_hash) = row;

    // Reconstruct a TOML representation from stored fields.
    let packages: Vec<String> = serde_json::from_str(&packages_json).unwrap_or_default();
    let verified_by: Vec<String> = serde_json::from_str(&verified_by_json).unwrap_or_default();

    // Build a TOML-serializable value using toml::Table.
    let mut table = toml::Table::new();
    table.insert("seed_id".to_owned(), toml::Value::String(id));
    table.insert(
        "target_triple".to_owned(),
        toml::Value::String(target_triple),
    );
    table.insert("source".to_owned(), toml::Value::String(source));
    table.insert(
        "image_cas_hash".to_owned(),
        toml::Value::String(image_cas_hash),
    );
    if let Some(b) = builder {
        table.insert("builder".to_owned(), toml::Value::String(b));
    }
    table.insert(
        "packages".to_owned(),
        toml::Value::Array(packages.into_iter().map(toml::Value::String).collect()),
    );
    table.insert(
        "verified_by".to_owned(),
        toml::Value::Array(verified_by.into_iter().map(toml::Value::String).collect()),
    );

    let toml_str = match toml::to_string_pretty(&table) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize seed {seed_id} to TOML: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Serialization error").into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/toml")
        .header(header::CONTENT_LENGTH, toml_str.len())
        .body(Body::from(toml_str))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ────────────────────────────────────────────────────────────
// GET /v1/seeds/:seed_id/image
// ────────────────────────────────────────────────────────────

/// Stream the seed EROFS image from CAS.
///
/// Returns:
/// - 200 OK with `Content-Type: application/octet-stream`
/// - 404 Not Found if the seed or its image is unknown
pub async fn get_seed_image(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(seed_id): Path<String>,
) -> Response {
    if !is_valid_seed_id(&seed_id) {
        return (StatusCode::BAD_REQUEST, "Invalid seed ID format").into_response();
    }

    let (db_path, chunk_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.chunk_dir.clone())
    };

    // Look up the image_cas_hash for this seed.
    let cas_hash = match tokio::task::spawn_blocking({
        let id = seed_id.clone();
        move || -> anyhow::Result<Option<String>> {
            let conn = open_handler_db(&db_path)?;
            let result = conn.query_row(
                "SELECT image_cas_hash FROM seeds WHERE seed_id = ?1",
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
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND, "Seed not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("DB error looking up seed image {seed_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        Err(e) => {
            tracing::error!("Task panicked looking up seed image {seed_id}: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response();
        }
    };

    let object_path = cas_object_path(&chunk_dir, &cas_hash);

    // Stream the file instead of reading it all into memory
    let file = match tokio::fs::File::open(&object_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("seeds row exists for {seed_id} but CAS object {cas_hash} missing");
            return (StatusCode::NOT_FOUND, "Seed image not found in CAS").into_response();
        }
        Err(e) => {
            tracing::error!("Failed to open seed image CAS object {cas_hash}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read seed image",
            )
                .into_response();
        }
    };

    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to stat seed image CAS object {cas_hash}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read seed image",
            )
                .into_response();
        }
    };

    let stream = tokio_util::io::ReaderStream::new(file);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, metadata.len())
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ────────────────────────────────────────────────────────────
// GET /v1/seeds?target=x86_64
// ────────────────────────────────────────────────────────────

/// List seeds, optionally filtered by `target_triple` query param.
///
/// Returns a JSON array of seed summary objects.
pub async fn list_seeds(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SeedListQuery>,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let target = params.target.clone();

    match tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SeedListItem>> {
        let conn = open_handler_db(&db_path)?;
        query_seeds(&conn, target.as_deref(), None)
    })
    .await
    {
        Ok(Ok(items)) => {
            let json = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_owned());
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(e)) => {
            tracing::error!("DB error listing seeds: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked listing seeds: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// GET /v1/seeds/latest?target=x86_64
// ────────────────────────────────────────────────────────────

/// Return the most-recent seed for the given target triple.
///
/// Returns a single JSON object, or 404 if no seeds exist for the target.
pub async fn get_latest_seed(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SeedListQuery>,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let target = params.target.clone();

    match tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SeedListItem>> {
        let conn = open_handler_db(&db_path)?;
        query_seeds(&conn, target.as_deref(), Some(1))
    })
    .await
    {
        Ok(Ok(mut items)) => {
            if let Some(item) = items.pop() {
                let json = match serde_json::to_string(&item) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!("Failed to serialize latest seed: {e}");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Serialization error")
                            .into_response();
                    }
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            } else {
                (StatusCode::NOT_FOUND, "No seeds found for target").into_response()
            }
        }
        Ok(Err(e)) => {
            tracing::error!("DB error fetching latest seed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked fetching latest seed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// Shared DB query helper
// ────────────────────────────────────────────────────────────

/// Query the `seeds` table, optionally filtering by target triple and limiting rows.
///
/// Results are ordered by `created_at DESC` so the newest seed comes first.
/// The `limit` is applied in SQL when provided.
fn query_seeds(
    conn: &rusqlite::Connection,
    target: Option<&str>,
    limit: Option<u32>,
) -> anyhow::Result<Vec<SeedListItem>> {
    // Build the SQL with an optional LIMIT clause. The limit value comes from
    // internal callers only (never user input), so formatting it into the SQL
    // string is safe.
    let sql = match (target.is_some(), limit) {
        (true, Some(lim)) => format!(
            "SELECT seed_id, target_triple, source, builder, packages_json, verified_by_json, created_at
             FROM seeds WHERE target_triple = ?1 ORDER BY created_at DESC LIMIT {lim}"
        ),
        (true, None) => {
            "SELECT seed_id, target_triple, source, builder, packages_json, verified_by_json, created_at
             FROM seeds WHERE target_triple = ?1 ORDER BY created_at DESC"
                .to_owned()
        }
        (false, Some(lim)) => format!(
            "SELECT seed_id, target_triple, source, builder, packages_json, verified_by_json, created_at
             FROM seeds ORDER BY created_at DESC LIMIT {lim}"
        ),
        (false, None) => {
            "SELECT seed_id, target_triple, source, builder, packages_json, verified_by_json, created_at
             FROM seeds ORDER BY created_at DESC"
                .to_owned()
        }
    };

    let mut stmt = conn.prepare(&sql)?;

    let mut rows = if let Some(t) = target {
        stmt.query(rusqlite::params![t])?
    } else {
        stmt.query([])?
    };

    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let packages_json: String = row.get(4)?;
        let verified_by_json: String = row.get(5)?;
        let packages: Vec<String> = serde_json::from_str(&packages_json).unwrap_or_default();
        let verified_by: Vec<String> = serde_json::from_str(&verified_by_json).unwrap_or_default();

        items.push(SeedListItem {
            seed_id: row.get(0)?,
            target_triple: row.get(1)?,
            source: row.get(2)?,
            builder: row.get(3)?,
            package_count: packages.len(),
            verified_by_count: verified_by.len(),
            created_at: row.get(6)?,
        });
    }

    Ok(items)
}

// ────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    fn valid_seed_ids() {
        assert!(is_valid_seed_id(
            "abc1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcd"
        ));
        assert!(is_valid_seed_id("seed-x86_64"));
        assert!(is_valid_seed_id("s.1"));
    }

    #[test]
    fn invalid_seed_ids() {
        assert!(!is_valid_seed_id(""));
        assert!(!is_valid_seed_id("has/slash"));
        assert!(!is_valid_seed_id("has space"));
        assert!(!is_valid_seed_id(&"x".repeat(129)));
    }

    #[test]
    fn list_seeds_query() {
        let (conn, _tmp) = make_test_db();

        // Insert two seeds with different targets.
        conn.execute(
            "INSERT INTO seeds (seed_id, target_triple, source, builder, packages_json, verified_by_json, image_cas_hash)
             VALUES ('seed-x86', 'x86_64-conary-linux-gnu', 'community', 'conary 0.9.0',
                     '[\"gcc\",\"glibc\"]', '[\"sig:abc\"]', 'deadbeef01')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO seeds (seed_id, target_triple, source, builder, packages_json, verified_by_json, image_cas_hash)
             VALUES ('seed-arm', 'aarch64-conary-linux-gnu', 'selfbuilt', NULL,
                     '[\"gcc\"]', '[]', 'deadbeef02')",
            [],
        )
        .unwrap();

        // Query for x86_64 only — should return 1 row.
        let x86_items = query_seeds(&conn, Some("x86_64-conary-linux-gnu"), None).unwrap();
        assert_eq!(x86_items.len(), 1, "expected 1 x86_64 seed");
        assert_eq!(x86_items[0].seed_id, "seed-x86");
        assert_eq!(x86_items[0].package_count, 2);
        assert_eq!(x86_items[0].verified_by_count, 1);

        // Query for aarch64 only — should return 1 row.
        let arm_items = query_seeds(&conn, Some("aarch64-conary-linux-gnu"), None).unwrap();
        assert_eq!(arm_items.len(), 1, "expected 1 aarch64 seed");
        assert_eq!(arm_items[0].seed_id, "seed-arm");
        assert_eq!(arm_items[0].package_count, 1);
        assert_eq!(arm_items[0].verified_by_count, 0);

        // Query for all — should return 2 rows.
        let all_items = query_seeds(&conn, None, None).unwrap();
        assert_eq!(all_items.len(), 2, "expected 2 seeds total");

        // Query with limit 1 — should return exactly 1 row.
        let limited = query_seeds(&conn, None, Some(1)).unwrap();
        assert_eq!(limited.len(), 1, "expected 1 seed with limit=1");
    }

    #[test]
    fn cas_path_layout() {
        let dir = std::path::PathBuf::from("/chunks");
        let hash = "aabbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890";
        let path = cas_object_path(&dir, hash);
        assert_eq!(
            path,
            std::path::PathBuf::from(
                "/chunks/objects/aa/bbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890"
            )
        );
    }
}
