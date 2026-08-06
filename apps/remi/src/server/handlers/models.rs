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
use conary_core::model::remote::{
    CollectionData, CollectionSignature, PublishedCollectionEnvelope,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(test)]
use conary_core::model::remote::CollectionMemberData;
#[cfg(test)]
use std::collections::BTreeMap;

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
) -> Result<Option<CollectionSignature>, anyhow::Error> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let conn = Connection::open(db_path)?;

    let result: Option<(Vec<u8>, String)> = conn
        .query_row(
            "SELECT rc.signature, rc.signer_key_id
             FROM remote_collections rc
             WHERE rc.name = ?1
               AND rc.signature IS NOT NULL
               AND rc.signer_key_id IS NOT NULL
             ORDER BY rc.fetched_at DESC LIMIT 1",
            [name],
            |row| {
                let sig: Vec<u8> = row.get(0)?;
                let key_id: String = row.get(1)?;
                Ok((sig, key_id))
            },
        )
        .optional()?;

    Ok(result.map(|(signature, key_id)| CollectionSignature {
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

/// Authenticated external-admin wrapper for model publication.
pub async fn put_model_authenticated(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    Query(params): Query<PutModelParams>,
    scopes: Option<axum::Extension<crate::server::auth::TokenScopes>>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(error) = super::admin::check_scope(&scopes, crate::server::auth::Scope::Admin) {
        return error;
    }
    put_model(State(state), Path(name), Query(params), body).await
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
    if let Err(error) = super::validate_name(&name) {
        return error;
    }
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
        Ok(Err(StoreError::InvalidSigningAuthority(e))) => {
            let msg = format!("Invalid signing authority: {e}");
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        Ok(Err(StoreError::SignatureVerificationFailed)) => (
            StatusCode::BAD_REQUEST,
            "Collection signature verification failed",
        )
            .into_response(),
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
    #[error("invalid signing authority: {0}")]
    InvalidSigningAuthority(String),
    #[error("collection signature verification failed")]
    SignatureVerificationFailed,
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
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let envelope: PublishedCollectionEnvelope =
        serde_json::from_slice(body).map_err(|e| StoreError::InvalidJson(e.to_string()))?;
    let data = envelope.collection;
    let signature = BASE64
        .decode(&envelope.signature)
        .map_err(|error| StoreError::InvalidSigningAuthority(error.to_string()))?;
    let public_key = hex::decode(&envelope.public_key)
        .map_err(|error| StoreError::InvalidSigningAuthority(error.to_string()))?;
    if public_key.len() != 32 {
        return Err(StoreError::InvalidSigningAuthority(format!(
            "Ed25519 public key has {} bytes; expected 32",
            public_key.len()
        )));
    }
    if !conary_core::model::signing::verify_collection(&data, &signature, &public_key)
        .map_err(|error| StoreError::InvalidSigningAuthority(error.to_string()))?
    {
        return Err(StoreError::SignatureVerificationFailed);
    }
    let signer_key_id = hex::encode(&public_key[..8]);

    // Validate: name in URL must match name in body
    if data.name != url_name {
        return Err(StoreError::NameMismatch {
            url_name: url_name.to_string(),
            body_name: data.name.clone(),
        });
    }

    conary_core::model::remote::validate_collection_data(&data, url_name)
        .map_err(|error| StoreError::InvalidJson(error.to_string()))?;
    let computed = conary_core::model::remote::collection_content_hash(&data)
        .map_err(|error| StoreError::InvalidJson(error.to_string()))?;
    if computed != data.content_hash {
        return Err(StoreError::HashMismatch {
            expected: data.content_hash.clone(),
            computed,
        });
    }

    let mut conn = Connection::open(db_path).map_err(|e| StoreError::Database(e.to_string()))?;
    let transaction = conn
        .transaction()
        .map_err(|error| StoreError::Database(error.to_string()))?;

    // Check if collection already exists
    let existing = Trove::find_by_name(&transaction, url_name)
        .map_err(|e| StoreError::Database(e.to_string()))?;
    let existing_collections: Vec<_> = existing
        .iter()
        .filter(|trove| trove.trove_type == TroveType::Collection)
        .collect();

    if !existing_collections.is_empty() {
        if !force {
            return Err(StoreError::AlreadyExists(url_name.to_string()));
        }

        // Force mode replaces every stale version atomically.
        for collection in existing_collections {
            let collection_id = collection
                .id
                .ok_or_else(|| StoreError::Database("Collection has no ID".into()))?;
            CollectionMember::delete_all_for_collection(&transaction, collection_id)
                .map_err(|e| StoreError::Database(e.to_string()))?;
            Trove::delete(&transaction, collection_id)
                .map_err(|e| StoreError::Database(e.to_string()))?;
        }
    }

    // Create collection trove
    let mut trove = Trove::new(
        data.name.clone(),
        data.version.clone(),
        TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let collection_id = trove
        .insert(&transaction)
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
            .insert(&transaction)
            .map_err(|e| StoreError::Database(e.to_string()))?;
    }

    // Persist exactly the signed wire format so GET and signature verification
    // consume the same bytes and authority.
    let stored = data.clone();

    let data_json = serde_json::to_string(&stored)
        .map_err(|e| StoreError::Database(format!("serialize wire format: {e}")))?;

    // Upsert into remote_collections. Use '' sentinel for label (not NULL)
    // to match the normalized label convention from chunk 1 fixes.
    transaction
        .execute(
            "INSERT INTO remote_collections (
             name, label, version, content_hash, data_json, expires_at, signature, signer_key_id
         )
         VALUES (?1, '', ?2, ?3, ?4, datetime('now', '+10 years'), ?5, ?6)
         ON CONFLICT(name, label) DO UPDATE SET
             version = excluded.version,
             content_hash = excluded.content_hash,
             data_json = excluded.data_json,
             fetched_at = datetime('now'),
             expires_at = excluded.expires_at,
             signature = excluded.signature,
             signer_key_id = excluded.signer_key_id",
            rusqlite::params![
                stored.name,
                stored.version,
                stored.content_hash,
                data_json,
                signature,
                signer_key_id
            ],
        )
        .map_err(|e| StoreError::Database(e.to_string()))?;

    transaction
        .commit()
        .map_err(|error| StoreError::Database(error.to_string()))?;

    Ok(PutModelResponse {
        name: stored.name,
        version: stored.version,
        members: member_count,
    })
}

/// Build CollectionData for a named collection from the database.
///
/// Loads the stored wire format, which is the current authority for published
/// collection semantics including includes, pins, and exclusions.
fn build_collection_data(
    db_path: &std::path::Path,
    name: &str,
) -> Result<Option<CollectionData>, anyhow::Error> {
    let conn = Connection::open(db_path)?;

    let stored: Option<String> = conn
        .query_row(
            "SELECT data_json FROM remote_collections
             WHERE name = ?1 AND label = ''
             ORDER BY fetched_at DESC LIMIT 1",
            [name],
            |row| row.get(0),
        )
        .optional()?;

    stored
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| anyhow::anyhow!("invalid stored collection {name}: {error}"))
        })
        .transpose()
}

#[cfg(test)]
fn insert_invalid_stored_collection(conn: &Connection, name: &str) {
    conn.execute(
        "INSERT INTO remote_collections (
            name, label, content_hash, data_json, expires_at
        ) VALUES (?1, '', 'sha256:invalid', '{', '2099-12-31T23:59:59Z')",
        [name],
    )
    .unwrap();
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
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use conary_core::db::schema;
    use conary_core::model::signing::sign_collection;
    use ed25519_dalek::SigningKey;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        (temp_file, conn)
    }

    fn signed_body(mut data: CollectionData) -> Vec<u8> {
        if data.content_hash.is_empty() {
            let mut hash_input = data.clone();
            hash_input.content_hash.clear();
            data.content_hash = conary_core::hash::sha256_prefixed(
                &conary_core::json::canonical_json(&hash_input).unwrap(),
            );
        }
        let key = SigningKey::from_bytes(&[11; 32]);
        let envelope = PublishedCollectionEnvelope {
            signature: BASE64.encode(sign_collection(&data, &key).unwrap()),
            public_key: hex::encode(key.verifying_key().to_bytes()),
            collection: data,
        };
        serde_json::to_vec(&envelope).unwrap()
    }

    #[test]
    fn test_unpublished_collection_is_not_reconstructed() {
        let (temp_file, conn) = create_test_db();

        // Create a collection
        let mut trove = Trove::new(
            "group-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
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

        let data = build_collection_data(temp_file.path(), "group-base").unwrap();
        assert!(data.is_none());
    }

    #[test]
    fn test_invalid_stored_collection_fails_closed() {
        let (temp_file, conn) = create_test_db();
        insert_invalid_stored_collection(&conn, "group-invalid");

        let error = build_collection_data(temp_file.path(), "group-invalid").unwrap_err();
        assert!(error.to_string().contains("invalid stored collection"));
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

        let body = signed_body(data);

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

        let envelope: PublishedCollectionEnvelope = serde_json::from_slice(&body).unwrap();
        let stored_signature = query_signature(temp_file.path(), "group-web")
            .unwrap()
            .unwrap();
        assert_eq!(stored_signature.signature, envelope.signature);
        assert_eq!(stored_signature.key_id, envelope.public_key[..16]);
    }

    #[test]
    fn test_put_model_conflict() {
        let (temp_file, conn) = create_test_db();

        // Pre-create a collection
        let mut trove = Trove::new(
            "group-existing".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
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

        let body = signed_body(data);

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

        let body = signed_body(data);

        // URL name doesn't match body name
        let result = store_collection(temp_file.path(), "group-b", &body, false);
        assert!(matches!(result, Err(StoreError::NameMismatch { .. })));
    }

    #[test]
    fn test_put_model_rejects_unsigned_collection_body() {
        let (temp_file, _conn) = create_test_db();
        let data = CollectionData {
            name: "group-unsigned".to_string(),
            version: "1.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: "sha256:untrusted".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let body = serde_json::to_vec(&data).unwrap();

        let result = store_collection(temp_file.path(), "group-unsigned", &body, false);

        assert!(matches!(result, Err(StoreError::InvalidJson(_))));
    }

    #[test]
    fn test_put_model_rejects_signature_from_a_different_key() {
        let (temp_file, _conn) = create_test_db();
        let data = CollectionData {
            name: "group-wrong-key".to_string(),
            version: "1.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: String::new(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let body = signed_body(data);
        let mut envelope: PublishedCollectionEnvelope = serde_json::from_slice(&body).unwrap();
        envelope.public_key =
            hex::encode(SigningKey::from_bytes(&[12; 32]).verifying_key().to_bytes());

        let result = store_collection(
            temp_file.path(),
            "group-wrong-key",
            &serde_json::to_vec(&envelope).unwrap(),
            false,
        );

        assert!(matches!(
            result,
            Err(StoreError::SignatureVerificationFailed)
        ));
    }

    #[test]
    fn test_put_model_rejects_signed_content_hash_lie() {
        let (temp_file, _conn) = create_test_db();
        let key = SigningKey::from_bytes(&[11; 32]);
        let data = CollectionData {
            name: "group-bad-hash".to_string(),
            version: "1.0.0".to_string(),
            members: vec![],
            includes: vec![],
            pins: BTreeMap::new(),
            exclude: vec![],
            content_hash: "sha256:not-the-content".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let envelope = PublishedCollectionEnvelope {
            signature: BASE64.encode(sign_collection(&data, &key).unwrap()),
            public_key: hex::encode(key.verifying_key().to_bytes()),
            collection: data,
        };

        let result = store_collection(
            temp_file.path(),
            "group-bad-hash",
            &serde_json::to_vec(&envelope).unwrap(),
            false,
        );

        assert!(matches!(result, Err(StoreError::HashMismatch { .. })));
    }

    #[test]
    fn test_list_models() {
        let (temp_file, conn) = create_test_db();

        // Create two collections
        let mut t1 = Trove::new(
            "group-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let id1 = t1.insert(&conn).unwrap();

        let mut m = CollectionMember::new(id1, "nginx".to_string());
        m.insert(&conn).unwrap();

        let mut t2 = Trove::new(
            "group-dev".to_string(),
            "2.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        t2.description = Some("Dev tools".to_string());
        t2.insert(&conn).unwrap();

        // Also create a non-collection trove (should not appear)
        let mut pkg = Trove::new(
            "nginx".to_string(),
            "1.24.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
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
