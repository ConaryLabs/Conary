// src/server/handlers/federation.rs
//! Federation-related server endpoints

use crate::server::ServerState;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Serialize)]
struct DirectoryPeer {
    node_id: String,
    endpoint: String,
    tier: String,
}

/// GET /v1/federation/directory
///
/// Returns a JSON list of known peers (enabled only).
pub async fn directory(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> impl IntoResponse {
    let db_path = { state.read().await.config.db_path.clone() };

    let result = tokio::task::spawn_blocking(move || -> crate::error::Result<Vec<DirectoryPeer>> {
        let conn = crate::db::open(&db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, endpoint, tier FROM federation_peers
             WHERE is_enabled = 1
             ORDER BY tier, endpoint",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(DirectoryPeer {
                node_id: row.get(0)?,
                endpoint: row.get(1)?,
                tier: row.get(2)?,
            })
        })?;

        let mut peers = Vec::new();
        for row in rows {
            peers.push(row?);
        }

        Ok(peers)
    })
    .await;

    match result {
        Ok(Ok(peers)) => Json(peers).into_response(),
        Ok(Err(err)) => {
            tracing::error!("Failed to query federation directory: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load federation directory",
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!("Federation directory task failed: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load federation directory",
            )
                .into_response()
        }
    }
}
