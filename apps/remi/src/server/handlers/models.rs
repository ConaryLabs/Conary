// apps/remi/src/server/handlers/models.rs

//! Model collection endpoints for the Remi server
//!
//! Serves published collections as JSON for remote model include resolution.
//! Clients fetch these via `GET /v1/models/:name` to resolve `[include]`
//! directives in their system.toml files.

use crate::server::ServerState;
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::db::models::{CollectionMember, Trove, TroveType};
use conary_core::model::remote::{CollectionData, CollectionMemberData};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Summary entry for the collection listing endpoint
#[derive(Serialize)]
pub struct CollectionEntry {
    pub name: String,
    pub version: String,
    pub member_count: usize,
    pub description: Option<String>,
}

/// GET /v1/models/:name
///
/// Returns a published collection as JSON (CollectionData wire format).
/// Used by clients resolving remote includes in system.toml files.
pub async fn get_model(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Response {
    // Validate path parameter against traversal and injection
    if let Err(e) = super::validate_name(&name) {
        return e;
    }

    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || build_collection_data(&db_path, &name)).await;

    match result {
        Ok(Ok(Some(data))) => {
            let json = match super::serialize_json(&data, "collection") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "Collection not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to build collection: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build collection data",
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in get_model: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// GET /v1/models
///
/// Lists all published collections (name, version, member count).
pub async fn list_models(State(state): State<Arc<RwLock<ServerState>>>) -> Response {
    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || build_collection_list(&db_path)).await;

    match result {
        Ok(Ok(entries)) => {
            let json = match super::serialize_json(&entries, "collection list") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to list collections: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list collections",
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in list_models: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Response body for signature endpoint
#[derive(Serialize)]
struct SignatureResponse {
    signature: String,
    key_id: String,
}

/// GET /v1/models/:name/signature
///
/// Returns the Ed25519 signature and signer key ID for a published collection.
/// Signature is base64-encoded; key_id is hex-encoded (first 8 bytes of public key).
pub async fn get_model_signature(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Response {
    // Validate path parameter against traversal and injection
    if let Err(e) = super::validate_name(&name) {
        return e;
    }

    let db_path = state.read().await.config.db_path.clone();

    let result = tokio::task::spawn_blocking(move || query_signature(&db_path, &name)).await;

    match result {
        Ok(Ok(Some(response))) => {
            let json = match super::serialize_json(&response, "signature") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(Ok(None)) => {
            (StatusCode::NOT_FOUND, "No signature found for collection").into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to query signature: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in get_model_signature: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Query the signature for a named collection from the database
fn query_signature(
    db_path: &std::path::Path,
    name: &str,
) -> Result<Option<SignatureResponse>, anyhow::Error> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let conn = Connection::open(db_path)?;

    let result: Option<(Vec<u8>, String)> = conn
        .query_row(
            "SELECT rc.signature, rc.signer_key_id
             FROM remote_collections rc
             WHERE rc.name = ?1 AND rc.signature IS NOT NULL
             ORDER BY rc.fetched_at DESC LIMIT 1",
            [name],
            |row| {
                let sig: Vec<u8> = row.get(0)?;
                let key_id: String = row.get(1)?;
                Ok((sig, key_id))
            },
        )
        .optional()?;

    Ok(result.map(|(signature, key_id)| SignatureResponse {
        signature: BASE64.encode(&signature),
        key_id,
    }))
}

/// Query parameters for PUT model endpoint
#[derive(Deserialize)]
pub struct PutModelParams {
    /// If true, overwrite existing collection
    #[serde(default)]
    pub force: bool,
}

/// Response body for successful model creation
#[derive(Serialize)]
struct PutModelResponse {
    name: String,
    version: String,
    members: usize,
}

/// Maximum body size for PUT model endpoint (1 MB)
const MAX_MODEL_BODY_SIZE: usize = 1_048_576;

/// PUT /v1/admin/models/:name
///
/// Creates a new collection from a published model. Returns 409 Conflict
/// if the collection already exists (unless `?force=true` is set).
pub async fn put_model(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    Query(params): Query<PutModelParams>,
    body: axum::body::Bytes,
) -> Response {
    if body.len() > MAX_MODEL_BODY_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Request body too large ({} bytes, max {} bytes)",
                body.len(),
                MAX_MODEL_BODY_SIZE
            ),
        )
            .into_response();
    }

    let db_path = state.read().await.config.db_path.clone();
    let force = params.force;

    let result =
        tokio::task::spawn_blocking(move || store_collection(&db_path, &name, &body, force)).await;

    match result {
        Ok(Ok(response)) => {
            let json = match super::serialize_json(&response, "put model response") {
                Ok(j) => j,
                Err(e) => return e,
            };
            Response::builder()
                .status(StatusCode::CREATED)
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(json))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Ok(Err(StoreError::NameMismatch {
            url_name,
            body_name,
        })) => {
            let msg = format!(
                "Name mismatch: URL has '{}' but body has '{}'",
                url_name, body_name
            );
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Ok(Err(StoreError::HashMismatch { expected, computed })) => {
            let msg = format!(
                "Content hash mismatch: body claims '{}' but computed '{}'",
                expected, computed
            );
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Ok(Err(StoreError::AlreadyExists(name))) => {
            let msg = format!(
                "Collection '{}' already exists (use ?force=true to overwrite)",
                name
            );
            (StatusCode::CONFLICT, msg).into_response()
        }
        Ok(Err(StoreError::InvalidJson(e))) => {
            let msg = format!("Invalid JSON: {}", e);
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Ok(Err(StoreError::Database(e))) => {
            tracing::error!("Failed to store collection: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
        Err(e) => {
            tracing::error!("Task panicked in put_model: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Errors from store_collection
#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("URL name '{url_name}' does not match body name '{body_name}'")]
    NameMismatch { url_name: String, body_name: String },
    #[error("content hash mismatch: expected {expected}, computed {computed}")]
    HashMismatch { expected: String, computed: String },
    #[error("collection already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("database error: {0}")]
    Database(String),
}

/// Store a collection in the database from PUT body bytes
fn store_collection(
    db_path: &std::path::Path,
    url_name: &str,
    body: &[u8],
    force: bool,
) -> Result<PutModelResponse, StoreError> {
    // Deserialize body as CollectionData
    let data: CollectionData =
        serde_json::from_slice(body).map_err(|e| StoreError::InvalidJson(e.to_string()))?;

    // Validate: name in URL must match name in body
    if data.name != url_name {
        return Err(StoreError::NameMismatch {
            url_name: url_name.to_string(),
            body_name: data.name.clone(),
        });
    }

    // Verify content hash if present.
    // The protocol computes the hash over JSON with content_hash blanked to avoid
    // a chicken-and-egg problem (matching conary-core/src/model/remote.rs).
    if !data.content_hash.is_empty() {
        let mut verification_data = data.clone();
        verification_data.content_hash = String::new();
        let verification_json = serde_json::to_vec(&verification_data)
            .map_err(|e| StoreError::InvalidJson(format!("re-serialize for hash: {e}")))?;
        let computed = conary_core::hash::sha256_prefixed(&verification_json);
        if computed != data.content_hash {
            return Err(StoreError::HashMismatch {
                expected: data.content_hash.clone(),
                computed,
            });
        }
    }

    let conn = Connection::open(db_path).map_err(|e| StoreError::Database(e.to_string()))?;

    // Check if collection already exists
    let existing =
        Trove::find_by_name(&conn, url_name).map_err(|e| StoreError::Database(e.to_string()))?;
    let existing_collection = existing
        .iter()
        .find(|t| t.trove_type == TroveType::Collection);

    if let Some(coll) = existing_collection {
        if !force {
            return Err(StoreError::AlreadyExists(url_name.to_string()));
        }

        // Force mode: delete old collection and its members
        let coll_id = coll
            .id
            .ok_or_else(|| StoreError::Database("Collection has no ID".into()))?;
        CollectionMember::delete_all_for_collection(&conn, coll_id)
            .map_err(|e| StoreError::Database(e.to_string()))?;
        Trove::delete(&conn, coll_id).map_err(|e| StoreError::Database(e.to_string()))?;
    }

    // Create collection trove
    let mut trove = Trove::new(
        data.name.clone(),
        data.version.clone(),
        TroveType::Collection,
    );
    let collection_id = trove
        .insert(&conn)
        .map_err(|e| StoreError::Database(e.to_string()))?;

    // Add members
    let member_count = data.members.len();
    for m in &data.members {
        let mut member = CollectionMember::new(collection_id, m.name.clone());
        if let Some(ref v) = m.version_constraint {
            member = member.with_version(v.clone());
        }
        if m.is_optional {
            member = member.optional();
        }
        member
            .insert(&conn)
            .map_err(|e| StoreError::Database(e.to_string()))?;
    }

    // Persist the full CollectionData wire format (including includes/pins/exclude)
    // in remote_collections so GET can serve it back without reconstruction.
    // Compute content_hash over the canonical form (content_hash blanked).
    // Fill published_at BEFORE computing content_hash so the stored payload
    // matches the hash. Core verification hashes the full CollectionData with
    // only content_hash blanked, not published_at.
    let mut stored = data.clone();
    if stored.published_at.is_empty() {
        stored.published_at = chrono::Utc::now().to_rfc3339();
    }

    let mut canonical = stored.clone();
    canonical.content_hash = String::new();
    let canonical_json = serde_json::to_vec(&canonical)
        .map_err(|e| StoreError::Database(format!("serialize for hash: {e}")))?;
    stored.content_hash = conary_core::hash::sha256_prefixed(&canonical_json);

    let data_json = serde_json::to_string(&stored)
        .map_err(|e| StoreError::Database(format!("serialize wire format: {e}")))?;

    // Upsert into remote_collections. Use '' sentinel for label (not NULL)
    // to match the normalized label convention from chunk 1 fixes.
    conn.execute(
        "INSERT INTO remote_collections (name, label, version, content_hash, data_json, expires_at)
         VALUES (?1, '', ?2, ?3, ?4, datetime('now', '+10 years'))
         ON CONFLICT(name, label) DO UPDATE SET
             version = excluded.version,
             content_hash = excluded.content_hash,
             data_json = excluded.data_json,
             fetched_at = datetime('now'),
             expires_at = excluded.expires_at,
             signature = NULL,
             signer_key_id = NULL",
        rusqlite::params![stored.name, stored.version, stored.content_hash, data_json],
    )
    .map_err(|e| StoreError::Database(e.to_string()))?;

    Ok(PutModelResponse {
        name: stored.name,
        version: stored.version,
        members: member_count,
    })
}

/// Build CollectionData for a named collection from the database.
///
/// Checks `remote_collections` first for a stored wire format (which preserves
/// includes/pins/exclude from the original PUT). Falls back to reconstructing
/// from troves + collection_members for legacy data.
fn build_collection_data(
    db_path: &std::path::Path,
    name: &str,
) -> Result<Option<CollectionData>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Try the stored wire format first (preserves includes/pins/exclude).
    // Use '' sentinel for label (matching the normalized convention).
    // Also check NULL for backwards compatibility with pre-v58 data.
    let stored: Option<String> = conn
        .query_row(
            "SELECT data_json FROM remote_collections
             WHERE name = ?1 AND (label = '' OR label IS NULL)
             ORDER BY fetched_at DESC LIMIT 1",
            [name],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(json) = stored {
        if let Ok(data) = serde_json::from_str::<CollectionData>(&json) {
            return Ok(Some(data));
        }
        // If deserialization fails, fall through to reconstruction
        tracing::warn!(
            "Stored wire format for collection '{}' is invalid, reconstructing",
            name
        );
    }

    // Fallback: reconstruct from troves + collection_members (legacy path)
    let troves = Trove::find_by_name(&conn, name)?;
    let collection = troves
        .into_iter()
        .find(|t| t.trove_type == TroveType::Collection);

    let coll = match collection {
        Some(c) => c,
        None => return Ok(None),
    };

    let coll_id = coll
        .id
        .ok_or_else(|| anyhow::anyhow!("Collection has no ID"))?;

    // Get members
    let members = CollectionMember::find_by_collection(&conn, coll_id)?;

    let member_data: Vec<CollectionMemberData> = members
        .iter()
        .map(|m| CollectionMemberData {
            name: m.member_name.clone(),
            version_constraint: m.member_version.clone(),
            is_optional: m.is_optional,
        })
        .collect();

    // Compute content hash using the canonical method (content_hash blanked)
    let mut reconstructed = CollectionData {
        name: name.to_string(),
        version: coll.version.clone(),
        members: member_data,
        includes: Vec::new(),
        pins: BTreeMap::new(),
        exclude: Vec::new(),
        content_hash: String::new(),
        published_at: coll
            .installed_at
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    };
    let canonical_json = serde_json::to_vec(&reconstructed)?;
    reconstructed.content_hash = conary_core::hash::sha256_prefixed(&canonical_json);

    Ok(Some(reconstructed))
}

/// Build a listing of all collections with member counts in a single query
fn build_collection_list(db_path: &std::path::Path) -> Result<Vec<CollectionEntry>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT t.name, t.version, t.description, COUNT(cm.id) AS member_count
         FROM troves t
         LEFT JOIN collection_members cm ON cm.collection_id = t.id
         WHERE t.type = 'collection'
         GROUP BY t.id
         ORDER BY t.name",
    )?;

    let entries: Vec<CollectionEntry> = stmt
        .query_map([], |row| {
            Ok(CollectionEntry {
                name: row.get(0)?,
                version: row.get(1)?,
                description: row.get(2)?,
                member_count: row.get::<_, i64>(3)? as usize,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_get_model_returns_collection() {
        let (temp_file, conn) = create_test_db();

        // Create a collection
        let mut trove = Trove::new(
            "group-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
        );
        trove.description = Some("Base server collection".to_string());
        let coll_id = trove.insert(&conn).unwrap();

        // Add members
        let mut m1 = CollectionMember::new(coll_id, "nginx".to_string());
        m1.insert(&conn).unwrap();

        let mut m2 = CollectionMember::new(coll_id, "redis".to_string())
            .with_version("7.0.*".to_string())
            .optional();
        m2.insert(&conn).unwrap();

        // Build collection data
        let data = build_collection_data(temp_file.path(), "group-base")
            .unwrap()
            .unwrap();

        assert_eq!(data.name, "group-base");
        assert_eq!(data.version, "1.0.0");
        assert_eq!(data.members.len(), 2);
        assert!(data.content_hash.starts_with("sha256:"));

        // Check member details
        let nginx = data.members.iter().find(|m| m.name == "nginx").unwrap();
        assert!(!nginx.is_optional);
        assert!(nginx.version_constraint.is_none());

        let redis = data.members.iter().find(|m| m.name == "redis").unwrap();
        assert!(redis.is_optional);
        assert_eq!(redis.version_constraint, Some("7.0.*".to_string()));
    }

    #[test]
    fn test_get_model_not_found() {
        let (temp_file, _conn) = create_test_db();

        let result = build_collection_data(temp_file.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_put_model_creates_collection() {
        let (temp_file, _conn) = create_test_db();

        let data = CollectionData {
            name: "group-web".to_string(),
            version: "1.0.0".to_string(),
            members: vec![
                CollectionMemberData {
                    name: "nginx".to_string(),
                    version_constraint: Some("1.24.*".to_string()),
                    is_optional: false,
                },
                CollectionMemberData {
                    name: "redis".to_string(),
                    version_constraint: None,
                    is_optional: true,
                },
            ],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: String::new(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let body = serde_json::to_vec(&data).unwrap();

        // PUT should create the collection
        let result = store_collection(temp_file.path(), "group-web", &body, false).unwrap();
        assert_eq!(result.name, "group-web");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(result.members, 2);

        // Verify it can be retrieved via build_collection_data
        let fetched = build_collection_data(temp_file.path(), "group-web")
            .unwrap()
            .unwrap();
        assert_eq!(fetched.name, "group-web");
        assert_eq!(fetched.members.len(), 2);

        let nginx = fetched.members.iter().find(|m| m.name == "nginx").unwrap();
        assert_eq!(nginx.version_constraint, Some("1.24.*".to_string()));
        assert!(!nginx.is_optional);

        let redis = fetched.members.iter().find(|m| m.name == "redis").unwrap();
        assert!(redis.is_optional);
    }

    #[test]
    fn test_put_model_conflict() {
        let (temp_file, conn) = create_test_db();

        // Pre-create a collection
        let mut trove = Trove::new(
            "group-existing".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
        );
        trove.insert(&conn).unwrap();

        let data = CollectionData {
            name: "group-existing".to_string(),
            version: "2.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: String::new(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let body = serde_json::to_vec(&data).unwrap();

        // PUT without force should return AlreadyExists
        let result = store_collection(temp_file.path(), "group-existing", &body, false);
        assert!(matches!(result, Err(StoreError::AlreadyExists(_))));

        // PUT with force should succeed
        let result = store_collection(temp_file.path(), "group-existing", &body, true).unwrap();
        assert_eq!(result.name, "group-existing");
        assert_eq!(result.version, "2.0.0");
    }

    #[test]
    fn test_put_model_name_mismatch() {
        let (temp_file, _conn) = create_test_db();

        let data = CollectionData {
            name: "group-a".to_string(),
            version: "1.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: String::new(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let body = serde_json::to_vec(&data).unwrap();

        // URL name doesn't match body name
        let result = store_collection(temp_file.path(), "group-b", &body, false);
        assert!(matches!(result, Err(StoreError::NameMismatch { .. })));
    }

    #[test]
    fn test_list_models() {
        let (temp_file, conn) = create_test_db();

        // Create two collections
        let mut t1 = Trove::new(
            "group-base".to_string(),
            "1.0".to_string(),
            TroveType::Collection,
        );
        let id1 = t1.insert(&conn).unwrap();

        let mut m = CollectionMember::new(id1, "nginx".to_string());
        m.insert(&conn).unwrap();

        let mut t2 = Trove::new(
            "group-dev".to_string(),
            "2.0".to_string(),
            TroveType::Collection,
        );
        t2.description = Some("Dev tools".to_string());
        t2.insert(&conn).unwrap();

        // Also create a non-collection trove (should not appear)
        let mut pkg = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
        );
        pkg.insert(&conn).unwrap();

        let entries = build_collection_list(temp_file.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "group-base");
        assert_eq!(entries[0].member_count, 1);
        assert_eq!(entries[1].name, "group-dev");
        assert_eq!(entries[1].description, Some("Dev tools".to_string()));
    }
}
