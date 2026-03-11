// conary-server/src/server/handlers/admin/federation.rs
//! Federation peer and config management handlers

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use rusqlite::OptionalExtension;

use crate::server::ServerState;
use crate::server::admin_service::{self, AddPeerInput, ServiceError};
use crate::server::auth::{Scope, TokenScopes, json_error};

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

impl From<conary_core::db::models::federation_peer::FederationPeer> for PeerResponse {
    fn from(p: conary_core::db::models::federation_peer::FederationPeer) -> Self {
        Self {
            id: p.id,
            endpoint: p.endpoint,
            node_name: p.node_name,
            tier: p.tier,
            first_seen: p.first_seen,
            last_seen: p.last_seen,
            latency_ms: p.latency_ms as f64,
            success_count: p.success_count,
            failure_count: p.failure_count,
            consecutive_failures: p.consecutive_failures,
            is_enabled: p.is_enabled,
        }
    }
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

    match admin_service::list_peers(&state).await {
        Ok(peers) => {
            let response: Vec<PeerResponse> = peers.into_iter().map(PeerResponse::from).collect();
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to list federation peers: {e}");
            json_error(500, "Failed to list peers", "INTERNAL_ERROR")
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

    let input = AddPeerInput {
        endpoint: body.endpoint,
        tier: body.tier,
        node_name: body.node_name,
    };

    match admin_service::add_peer(&state, input).await {
        Ok((peer_id, peer)) => {
            let guard = state.read().await;
            guard.publish_event(
                "federation.peer_added",
                serde_json::json!({"id": &peer_id, "endpoint": &peer.endpoint}),
            );
            drop(guard);

            (StatusCode::CREATED, Json(PeerResponse::from(peer))).into_response()
        }
        Err(ServiceError::BadRequest(msg)) => json_error(400, &msg, "BAD_REQUEST"),
        Err(ServiceError::Conflict(msg)) => json_error(409, &msg, "DUPLICATE_PEER"),
        Err(e) => {
            tracing::error!("Failed to add peer: {e}");
            json_error(500, "Failed to add peer", "INTERNAL_ERROR")
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

    match admin_service::delete_peer(&state, &id).await {
        Ok(true) => {
            let guard = state.read().await;
            guard.publish_event("federation.peer_removed", serde_json::json!({"id": &id}));
            drop(guard);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => json_error(404, "Peer not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to delete peer {id}: {e}");
            json_error(500, "Failed to delete peer", "INTERNAL_ERROR")
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

    match admin_service::get_peer(&state, &id).await {
        Ok(Some(model_peer)) => {
            let peer = PeerResponse::from(model_peer);
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
        Ok(None) => json_error(404, "Peer not found", "NOT_FOUND"),
        Err(e) => {
            tracing::error!("Failed to get peer health: {e}");
            json_error(500, "Failed to get peer health", "INTERNAL_ERROR")
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
    use tower::ServiceExt;

    use super::super::test_helpers::{rebuild_app, test_app};

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
        let peer_id = body["id"]
            .as_str()
            .expect("id should be a string")
            .to_string();

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
                    .uri(format!("/v1/admin/federation/peers/{peer_id}"))
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
                    .uri(format!("/v1/admin/federation/peers/{peer_id}/health"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
