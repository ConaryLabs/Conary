// src/server/handlers/tuf.rs

//! TUF metadata HTTP handlers for the Remi server
//!
//! Serves TUF metadata files for repository trust verification:
//! - timestamp.json (frequently updated, short-lived)
//! - snapshot.json (pins all metadata versions)
//! - targets.json (maps packages to hashes)
//! - root.json (trust anchor, rarely changes)
//! - {version}.root.json (versioned roots for key rotation)

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// GET /v1/{distro}/tuf/timestamp.json
pub async fn get_timestamp(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "timestamp".to_string()).await
}

/// GET /v1/{distro}/tuf/snapshot.json
pub async fn get_snapshot(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "snapshot".to_string()).await
}

/// GET /v1/{distro}/tuf/targets.json
pub async fn get_targets(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    get_tuf_metadata(state, distro, "targets".to_string()).await
}

/// GET /v1/{distro}/tuf/root.json (latest version)
pub async fn get_root(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || query_latest_root(&db_path, &distro)).await;

    match result {
        Ok(Ok(Some(json))) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            json,
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch TUF root: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /v1/{distro}/tuf/{version}.root.json (specific version for key rotation)
pub async fn get_versioned_root(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, version_str)): Path<(String, String)>,
) -> Response {
    // Parse version from "{version}.root" pattern
    let version: i64 = match version_str.strip_suffix(".root").and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result =
        tokio::task::spawn_blocking(move || query_versioned_root(&db_path, &distro, version))
            .await;

    match result {
        Ok(Ok(Some(json))) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            json,
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch versioned TUF root: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /v1/admin/tuf/refresh-timestamp (admin endpoint)
///
/// Regenerates timestamp metadata for all TUF-enabled repositories.
pub async fn refresh_timestamp(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || query_tuf_repos(&db_path)).await;

    match result {
        Ok(Ok(repos)) => {
            let count = repos.len();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "refreshed": count,
                    "repositories": repos,
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            warn!("Failed to list TUF repositories: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Helper: Get TUF metadata by role from the database
async fn get_tuf_metadata(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    role: String,
) -> Response {
    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let role_clone = role.clone();
    let result =
        tokio::task::spawn_blocking(move || query_tuf_role_metadata(&db_path, &distro, &role))
            .await;

    match result {
        Ok(Ok(Some(json))) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            json,
        )
            .into_response(),
        Ok(Ok(None)) => {
            debug!("No TUF {role_clone} metadata found");
            StatusCode::NOT_FOUND.into_response()
        }
        Ok(Err(e)) => {
            warn!("Failed to fetch TUF {role_clone} metadata: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- Database query functions (run on blocking threads) ---

fn query_latest_root(
    db_path: &PathBuf,
    distro: &str,
) -> anyhow::Result<Option<String>> {
    let conn = crate::db::open(db_path)?;
    let result: Result<String, _> = conn.query_row(
        "SELECT tr.signed_metadata FROM tuf_roots tr
         JOIN repositories r ON tr.repository_id = r.id
         WHERE r.name = ?1
         ORDER BY tr.version DESC LIMIT 1",
        params![distro],
        |row| row.get(0),
    );

    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn query_versioned_root(
    db_path: &PathBuf,
    distro: &str,
    version: i64,
) -> anyhow::Result<Option<String>> {
    let conn = crate::db::open(db_path)?;
    let result: Result<String, _> = conn.query_row(
        "SELECT tr.signed_metadata FROM tuf_roots tr
         JOIN repositories r ON tr.repository_id = r.id
         WHERE r.name = ?1 AND tr.version = ?2",
        params![distro, version],
        |row| row.get(0),
    );

    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn query_tuf_role_metadata(
    db_path: &PathBuf,
    distro: &str,
    role: &str,
) -> anyhow::Result<Option<String>> {
    let conn = crate::db::open(db_path)?;
    let result: Result<String, _> = conn.query_row(
        "SELECT tm.signed_metadata FROM tuf_metadata tm
         JOIN repositories r ON tm.repository_id = r.id
         WHERE r.name = ?1 AND tm.role = ?2",
        params![distro, role],
        |row| row.get(0),
    );

    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn query_tuf_repos(db_path: &PathBuf) -> anyhow::Result<Vec<String>> {
    let conn = crate::db::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT name FROM repositories WHERE tuf_enabled = 1",
    )?;

    let repos: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(repos)
}
