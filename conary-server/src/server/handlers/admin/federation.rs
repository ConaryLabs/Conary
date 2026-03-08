// conary-server/src/server/handlers/admin/federation.rs
//! Federation peer and config management handlers

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use rusqlite::OptionalExtension;

use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::ServerState;

use super::{check_scope, validate_path_param};

/// Response body for federation peer endpoints.
#[derive(Debug, Serialize)]
pub struct PeerResponse {
    pub id: String,
    pub endpoint: String,
    pub node_name: Option<String>,
    pub tier: String,
    pub first_seen: String,
    pub last_seen: String,
    pub latency_ms: f64,
    pub success_count: i64,
    pub failure_count: i64,
    pub consecutive_failures: i64,
    pub is_enabled: bool,
}

/// Request body for adding a federation peer.
#[derive(Debug, Deserialize)]
pub struct AddPeerRequest {
    pub endpoint: String,
    pub tier: Option<String>,
    pub node_name: Option<String>,
}

/// Health status for a peer.
#[derive(Debug, Serialize)]
pub struct PeerHealthStatus {
    pub success_rate: f64,
    pub total_requests: i64,
    pub status: String,
}

/// Detailed health response for a single peer.
#[derive(Debug, Serialize)]
pub struct PeerHealthResponse {
    pub peer: PeerResponse,
    pub health: PeerHealthStatus,
}

/// GET /v1/admin/federation/peers
///
/// List all federation peers with their health status. Requires "federation:read".
pub async fn list_peers(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationRead) {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, endpoint, node_name, tier, first_seen, last_seen,
                    latency_ms, success_count, failure_count, consecutive_failures, is_enabled
             FROM federation_peers
             ORDER BY tier, endpoint",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(PeerResponse {
                id: row.get(0)?,
                endpoint: row.get(1)?,
                node_name: row.get(2)?,
                tier: row.get(3)?,
                first_seen: row.get::<_, String>(4)?,
                last_seen: row.get::<_, String>(5)?,
                latency_ms: row.get::<_, i64>(6)? as f64,
                success_count: row.get(7)?,
                failure_count: row.get(8)?,
                consecutive_failures: row.get(9)?,
                is_enabled: row.get::<_, i64>(10)? != 0,
            })
        })?;

        Ok::<_, conary_core::Error>(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .await;

    match result {
        Ok(Ok(peers)) => Json(peers).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to list federation peers: {}", e);
            json_error(500, "Failed to list peers", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error listing peers: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// POST /v1/admin/federation/peers
///
/// Add a new federation peer. Requires "federation:write".
pub async fn add_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<AddPeerRequest>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationWrite) {
        return err;
    }

    let endpoint = body.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return json_error(400, "Endpoint must not be empty", "INVALID_ENDPOINT");
    }

    // Validate URL
    if url::Url::parse(&endpoint).is_err() {
        return json_error(400, "Invalid endpoint URL", "INVALID_URL");
    }

    let tier = body.tier.unwrap_or_else(|| "leaf".to_string());
    if !["leaf", "cell_hub", "region_hub"].contains(&tier.as_str()) {
        return json_error(
            400,
            "Tier must be one of: leaf, cell_hub, region_hub",
            "INVALID_TIER",
        );
    }

    let node_name = body.node_name;
    let peer_id = conary_core::hash::sha256(endpoint.as_bytes());
    let now = chrono::Utc::now().to_rfc3339();

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let peer_id_clone = peer_id.clone();
    let endpoint_clone = endpoint.clone();
    let tier_clone = tier.clone();
    let node_name_clone = node_name.clone();
    let now_clone = now.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        conn.execute(
            "INSERT INTO federation_peers (id, endpoint, node_name, tier, first_seen, last_seen,
             latency_ms, success_count, failure_count, consecutive_failures, is_enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 0, 0, 1)",
            rusqlite::params![peer_id_clone, endpoint_clone, node_name_clone, tier_clone, now_clone, now_clone],
        )?;
        Ok::<_, conary_core::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.peer_added",
                serde_json::json!({"id": &peer_id, "endpoint": &endpoint}),
            );
            drop(guard);

            let response = PeerResponse {
                id: peer_id,
                endpoint,
                node_name,
                tier,
                first_seen: now.clone(),
                last_seen: now,
                latency_ms: 0.0,
                success_count: 0,
                failure_count: 0,
                consecutive_failures: 0,
                is_enabled: true,
            };
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE constraint") {
                json_error(409, "Peer with this endpoint already exists", "DUPLICATE_PEER")
            } else {
                tracing::error!("Failed to add peer: {}", e);
                json_error(500, "Failed to add peer", "DB_ERROR")
            }
        }
        Err(e) => {
            tracing::error!("Task join error adding peer: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// DELETE /v1/admin/federation/peers/:id
///
/// Remove a federation peer. Requires "federation:write".
pub async fn delete_peer(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationWrite) {
        return err;
    }
    if let Some(err) = validate_path_param(&id, "peer id") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let id_clone = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let affected = conn.execute(
            "DELETE FROM federation_peers WHERE id = ?1",
            rusqlite::params![id_clone],
        )?;
        Ok::<_, conary_core::Error>(affected)
    })
    .await;

    match result {
        Ok(Ok(0)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Ok(_)) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.peer_removed",
                serde_json::json!({"id": &id}),
            );
            drop(guard);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to delete peer {}: {}", id, e);
            json_error(500, "Failed to delete peer", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error deleting peer: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/peers/:id/health
///
/// Get detailed health for a specific peer. Requires "federation:read".
pub async fn peer_health(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationRead) {
        return err;
    }
    if let Some(err) = validate_path_param(&id, "peer id") {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let peer = conn
            .query_row(
                "SELECT id, endpoint, node_name, tier, first_seen, last_seen,
                        latency_ms, success_count, failure_count, consecutive_failures, is_enabled
                 FROM federation_peers WHERE id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok(PeerResponse {
                        id: row.get(0)?,
                        endpoint: row.get(1)?,
                        node_name: row.get(2)?,
                        tier: row.get(3)?,
                        first_seen: row.get::<_, String>(4)?,
                        last_seen: row.get::<_, String>(5)?,
                        latency_ms: row.get::<_, i64>(6)? as f64,
                        success_count: row.get(7)?,
                        failure_count: row.get(8)?,
                        consecutive_failures: row.get(9)?,
                        is_enabled: row.get::<_, i64>(10)? != 0,
                    })
                },
            )
            .optional()?;
        Ok::<_, conary_core::Error>(peer)
    })
    .await;

    match result {
        Ok(Ok(Some(peer))) => {
            let total = peer.success_count + peer.failure_count;
            let success_rate = if total > 0 {
                peer.success_count as f64 / total as f64
            } else {
                1.0
            };
            let status = if peer.consecutive_failures > 5 {
                "unhealthy"
            } else if peer.consecutive_failures > 0 {
                "degraded"
            } else {
                "healthy"
            };

            let health = PeerHealthStatus {
                success_rate,
                total_requests: total,
                status: status.to_string(),
            };
            Json(PeerHealthResponse { peer, health }).into_response()
        }
        Ok(Ok(None)) => json_error(404, "Peer not found", "NOT_FOUND"),
        Ok(Err(e)) => {
            tracing::error!("Failed to get peer health: {}", e);
            json_error(500, "Failed to get peer health", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting peer health: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// GET /v1/admin/federation/config
///
/// Get the current federation configuration. Requires "federation:read".
/// Returns defaults if no config has been stored yet.
pub async fn get_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationRead) {
        return err;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        let json_str: Option<String> = conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'federation_config'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None); // Table may not exist -- treat as missing

        let config: crate::federation::FederationConfig = match json_str {
            Some(s) => serde_json::from_str(&s).unwrap_or_default(),
            None => crate::federation::FederationConfig::default(),
        };
        Ok::<_, conary_core::Error>(config)
    })
    .await;

    match result {
        Ok(Ok(config)) => Json(config).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to get federation config: {}", e);
            json_error(500, "Failed to get federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error getting federation config: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

/// PUT /v1/admin/federation/config
///
/// Update the federation configuration. Requires "federation:write".
/// The request body must be a valid `FederationConfig` JSON object.
pub async fn update_federation_config(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::FederationWrite) {
        return err;
    }

    // Validate by deserializing into FederationConfig
    let config: crate::federation::FederationConfig = match serde_json::from_value(body.clone()) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                400,
                &format!("Invalid federation config: {e}"),
                "INVALID_CONFIG",
            );
        }
    };

    let json_str = serde_json::to_string(&config).unwrap_or_default();

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open_fast(&db_path)?;
        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('federation_config', ?1)",
            rusqlite::params![json_str],
        )?;
        Ok::<_, conary_core::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.config_updated",
                serde_json::json!({"enabled": config.enabled}),
            );
            drop(guard);
            Json(config).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to update federation config: {}", e);
            json_error(500, "Failed to update federation config", "DB_ERROR")
        }
        Err(e) => {
            tracing::error!("Task join error updating federation config: {}", e);
            json_error(500, "Internal error", "INTERNAL_ERROR")
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn rebuild_app(db_path: &std::path::Path) -> axum::Router {
        let mut config = crate::server::ServerConfig::default();
        config.db_path = db_path.to_path_buf();
        config.chunk_dir = db_path.parent().unwrap().join("chunks");
        config.cache_dir = db_path.parent().unwrap().join("cache");
        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        crate::server::routes::create_external_admin_router(state, None)
    }

    async fn test_app() -> (axum::Router, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            conary_core::db::schema::migrate(&conn).unwrap();
        }

        let mut config = crate::server::ServerConfig::default();
        config.db_path = db_path.clone();
        config.chunk_dir = tmp.path().join("chunks");
        config.cache_dir = tmp.path().join("cache");
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let state = Arc::new(RwLock::new(crate::server::ServerState::new(config)));
        let app = crate::server::routes::create_external_admin_router(state, None);

        let test_token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(test_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
                .unwrap();
        }

        std::mem::forget(tmp);
        (app, db_path)
    }

    #[tokio::test]
    async fn test_federation_peer_lifecycle() {
        let (app, db_path) = test_app().await;
        let token = "test-admin-token-12345";

        // Add a peer
        let add_body = serde_json::json!({
            "endpoint": "https://peer1.example.com:7891",
            "tier": "leaf",
            "node_name": "peer1"
        });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(add_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["endpoint"], "https://peer1.example.com:7891");
        assert_eq!(body["tier"], "leaf");
        assert_eq!(body["is_enabled"], true);
        let peer_id = body["id"].as_str().expect("id should be a string").to_string();

        // List peers and verify it appears
        let app2 = rebuild_app(&db_path);
        let resp = app2
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/admin/federation/peers")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let peers = body.as_array().expect("should be an array");
        assert!(peers.iter().any(|p| p["id"] == peer_id));

        // Delete the peer
        let app3 = rebuild_app(&db_path);
        let resp = app3
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/admin/federation/peers/{peer_id}"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify it is gone (health check returns 404)
        let app4 = rebuild_app(&db_path);
        let resp = app4
            .oneshot(
                axum::http::Request::builder()
                    .uri(&format!("/v1/admin/federation/peers/{peer_id}/health"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
