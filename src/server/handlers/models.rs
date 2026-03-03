// src/server/handlers/models.rs

//! Model collection endpoints for the Remi server
//!
//! Serves published collections as JSON for remote model include resolution.
//! Clients fetch these via `GET /v1/models/:name` to resolve `[include]`
//! directives in their system.toml files.

use crate::db::models::{CollectionMember, Trove, TroveType};
use crate::model::remote::{CollectionData, CollectionMemberData};
use crate::server::ServerState;
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    let state = state.read().await;
    let db_path = &state.config.db_path;

    match build_collection_data(db_path, &name) {
        Ok(Some(data)) => {
            let json = match super::serialize_json(&data, &format!("collection '{name}'")) {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Collection not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to build collection '{}': {}", name, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build collection data",
            )
                .into_response()
        }
    }
}

/// GET /v1/models
///
/// Lists all published collections (name, version, member count).
pub async fn list_models(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Response {
    let state = state.read().await;
    let db_path = &state.config.db_path;

    match build_collection_list(db_path) {
        Ok(entries) => {
            let json = match super::serialize_json(&entries, "collection list") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Err(e) => {
            tracing::error!("Failed to list collections: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list collections",
            )
                .into_response()
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
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let state = state.read().await;
    let db_path = &state.config.db_path;

    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to open database: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // Query for signature in remote_collections
    let result: Result<Option<(Vec<u8>, String)>, _> = conn
        .query_row(
            "SELECT rc.signature, rc.signer_key_id
             FROM remote_collections rc
             WHERE rc.name = ?1 AND rc.signature IS NOT NULL
             ORDER BY rc.fetched_at DESC LIMIT 1",
            [&name],
            |row| {
                let sig: Vec<u8> = row.get(0)?;
                let key_id: String = row.get(1)?;
                Ok((sig, key_id))
            },
        )
        .optional();

    match result {
        Ok(Some((signature, key_id))) => {
            let response = SignatureResponse {
                signature: BASE64.encode(&signature),
                key_id,
            };
            let json = match super::serialize_json(&response, "signature") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 300)
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No signature found for collection").into_response(),
        Err(e) => {
            tracing::error!("Failed to query signature for '{}': {}", name, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
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

    let state = state.read().await;
    let db_path = &state.config.db_path;

    match store_collection(db_path, &name, &body, params.force) {
        Ok(response) => {
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
        Err(StoreError::NameMismatch { url_name, body_name }) => {
            let msg = format!(
                "Name mismatch: URL has '{}' but body has '{}'",
                url_name, body_name
            );
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Err(StoreError::HashMismatch { expected, computed }) => {
            let msg = format!(
                "Content hash mismatch: body claims '{}' but computed '{}'",
                expected, computed
            );
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Err(StoreError::AlreadyExists(name)) => {
            let msg = format!("Collection '{}' already exists (use ?force=true to overwrite)", name);
            (StatusCode::CONFLICT, msg).into_response()
        }
        Err(StoreError::InvalidJson(e)) => {
            let msg = format!("Invalid JSON: {}", e);
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Err(StoreError::Database(e)) => {
            tracing::error!("Failed to store collection '{}': {}", name, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        }
    }
}

/// Errors from store_collection
#[derive(Debug)]
enum StoreError {
    NameMismatch { url_name: String, body_name: String },
    HashMismatch { expected: String, computed: String },
    AlreadyExists(String),
    InvalidJson(String),
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
    let data: CollectionData = serde_json::from_slice(body)
        .map_err(|e| StoreError::InvalidJson(e.to_string()))?;

    // Validate: name in URL must match name in body
    if data.name != url_name {
        return Err(StoreError::NameMismatch {
            url_name: url_name.to_string(),
            body_name: data.name.clone(),
        });
    }

    // Verify content hash if present
    if !data.content_hash.is_empty() {
        let computed = crate::hash::sha256_prefixed(body);
        if computed != data.content_hash {
            return Err(StoreError::HashMismatch {
                expected: data.content_hash.clone(),
                computed,
            });
        }
    }

    let conn = Connection::open(db_path)
        .map_err(|e| StoreError::Database(e.to_string()))?;

    // Check if collection already exists
    let existing = Trove::find_by_name(&conn, url_name)
        .map_err(|e| StoreError::Database(e.to_string()))?;
    let existing_collection = existing.iter().find(|t| t.trove_type == TroveType::Collection);

    if let Some(coll) = existing_collection {
        if !force {
            return Err(StoreError::AlreadyExists(url_name.to_string()));
        }

        // Force mode: delete old collection and its members
        let coll_id = coll.id.ok_or_else(|| StoreError::Database("Collection has no ID".into()))?;
        CollectionMember::delete_all_for_collection(&conn, coll_id)
            .map_err(|e| StoreError::Database(e.to_string()))?;
        Trove::delete(&conn, coll_id)
            .map_err(|e| StoreError::Database(e.to_string()))?;
    }

    // Create collection trove
    let mut trove = Trove::new(
        data.name.clone(),
        data.version.clone(),
        TroveType::Collection,
    );
    let collection_id = trove.insert(&conn)
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
        member.insert(&conn)
            .map_err(|e| StoreError::Database(e.to_string()))?;
    }

    Ok(PutModelResponse {
        name: data.name,
        version: data.version,
        members: member_count,
    })
}

/// Build CollectionData for a named collection from the database
fn build_collection_data(
    db_path: &std::path::Path,
    name: &str,
) -> Result<Option<CollectionData>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find the collection trove
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

    // Compute content hash over the serialized member data
    let members_json = serde_json::to_string(&member_data)?;
    let content_hash = crate::hash::sha256_prefixed(members_json.as_bytes());

    let now = chrono::Utc::now().to_rfc3339();

    Ok(Some(CollectionData {
        name: name.to_string(),
        version: coll.version.clone(),
        members: member_data,
        includes: Vec::new(),
        pins: HashMap::new(),
        exclude: Vec::new(),
        content_hash,
        published_at: coll.installed_at.unwrap_or(now),
    }))
}

/// Build a listing of all collections
fn build_collection_list(
    db_path: &std::path::Path,
) -> Result<Vec<CollectionEntry>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    // Find all collection troves
    let mut stmt = conn.prepare(
        "SELECT id, name, version, description FROM troves WHERE type = 'collection' ORDER BY name",
    )?;

    let entries: Vec<CollectionEntry> = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let version: String = row.get(2)?;
            let description: Option<String> = row.get(3)?;

            // Count members (may fail for individual rows, so return 0 on error)
            let member_count: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM collection_members WHERE collection_id = ?1",
                    [id],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0) as usize;

            Ok(CollectionEntry {
                name,
                version,
                member_count,
                description,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
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
            pins: HashMap::new(),
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
            pins: HashMap::new(),
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
            pins: HashMap::new(),
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
