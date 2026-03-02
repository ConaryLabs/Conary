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
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rusqlite::Connection;
use serde::Serialize;
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
            let json = match serde_json::to_string(&data) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("Failed to serialize collection '{}': {}", name, e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Serialization error",
                    )
                        .into_response();
                }
            };

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::from(json))
                .unwrap()
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
            let json = serde_json::to_string(&entries).unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::from(json))
                .unwrap()
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
    let content_hash = format!("sha256:{}", crate::hash::sha256(members_json.as_bytes()));

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
