// conary-server/src/server/handlers/federation.rs
//! Federation-related server endpoints

use crate::server::ServerState;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::run_blocking;

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
) -> Result<Response, Response> {
    let db_path = state.read().await.config.db_path.clone();

    let peers = run_blocking("federation directory", move || {
        let conn = conary_core::db::open(&db_path)?;
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

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .await?;

    Ok(Json(peers).into_response())
}
