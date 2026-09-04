// apps/remi/src/server/handlers/tuf.rs

//! TUF metadata HTTP handlers for the Remi server
//!
//! Serves TUF metadata files for repository trust verification:
//! - timestamp.json (frequently updated, short-lived)
//! - snapshot.json (pins all metadata versions)
//! - targets.json (maps packages to hashes)
//! - root.json (trust anchor, rarely changes)
//! - {version}.root.json (versioned roots for key rotation)

use crate::server::ServerState;
use crate::server::signing_authority::{RepositorySigningRole, load_role_key};
use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::trust::{
    MetaFile, Signed, TUF_SPEC_VERSION, TimestampMetadata, sign_tuf_metadata,
};
use rusqlite::OptionalExtension;
use rusqlite::params;
use std::collections::BTreeMap;
use std::path::Path as StdPath;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use super::open_handler_db;

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
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result = tokio::task::spawn_blocking(move || query_latest_root(&db_path, &distro)).await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
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
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    // Parse version from "{version}.root" pattern
    let version: i64 = match version_str
        .strip_suffix(".root")
        .and_then(|v| v.parse().ok())
    {
        Some(v) => v,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result =
        tokio::task::spawn_blocking(move || query_versioned_root(&db_path, &distro, version)).await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
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

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct TimestampRefreshResult {
    pub status: String,
    pub role: String,
    pub distro: String,
    pub version: u64,
}

/// POST /v1/admin/tuf/{distro}/refresh-timestamp (admin endpoint)
pub async fn refresh_timestamp(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    match refresh_timestamp_for_distro(&state, &distro).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(error) => {
            warn!("Failed to refresh TUF timestamp for {distro}: {error:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "code": "TIMESTAMP_REFRESH_FAILED",
                })),
            )
                .into_response()
        }
    }
}

pub async fn refresh_timestamp_for_distro(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
) -> Result<TimestampRefreshResult> {
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .with_context(|| {
                format!("release route {distro} does not map to exactly one repository profile")
            })?
            .id()
            .to_string();
    let (db_path, keys_dir) = {
        let guard = state.read().await;
        let keys_dir = guard
            .config
            .release_publish
            .repository_keys_dir
            .clone()
            .context("release_publish.repository_keys_dir is not configured")?;
        (guard.config.db_path.clone(), keys_dir)
    };
    let distro = distro.to_string();

    tokio::task::spawn_blocking(move || {
        refresh_timestamp_for_distro_blocking(&db_path, &keys_dir, &distro, &source_profile)
    })
    .await
    .context("refresh timestamp blocking task failed")?
}

/// Helper: Get TUF metadata by role from the database
async fn get_tuf_metadata(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    role: String,
) -> Response {
    if let Err(e) = super::validate_supported_distro_route(&distro) {
        return e;
    }

    let db_path = {
        let guard = state.read().await;
        guard.config.db_path.clone()
    };

    let result =
        tokio::task::spawn_blocking(move || query_tuf_role_metadata(&db_path, &distro, &role))
            .await;

    match result {
        Ok(Ok(Some(json))) => {
            (StatusCode::OK, [("content-type", "application/json")], json).into_response()
        }
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => {
            warn!("Failed to fetch TUF metadata: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            warn!("Blocking task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- Database query functions (run on blocking threads) ---

fn query_latest_root(db_path: &std::path::Path, distro: &str) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tr.signed_metadata FROM tuf_roots tr
             JOIN repositories r ON tr.repository_id = r.id
             WHERE r.name = ?1
             ORDER BY tr.version DESC LIMIT 1",
            params![distro],
            |row| row.get(0),
        )
        .optional()?)
}

fn query_versioned_root(
    db_path: &std::path::Path,
    distro: &str,
    version: i64,
) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tr.signed_metadata FROM tuf_roots tr
             JOIN repositories r ON tr.repository_id = r.id
             WHERE r.name = ?1 AND tr.version = ?2",
            params![distro, version],
            |row| row.get(0),
        )
        .optional()?)
}

fn query_tuf_role_metadata(
    db_path: &std::path::Path,
    distro: &str,
    role: &str,
) -> anyhow::Result<Option<String>> {
    let conn = open_handler_db(db_path)?;
    Ok(conn
        .query_row(
            "SELECT tm.signed_metadata FROM tuf_metadata tm
             JOIN repositories r ON tm.repository_id = r.id
             WHERE r.name = ?1 AND tm.role = ?2",
            params![distro, role],
            |row| row.get(0),
        )
        .optional()?)
}

fn refresh_timestamp_for_distro_blocking(
    db_path: &StdPath,
    keys_dir: &StdPath,
    distro: &str,
    source_profile: &str,
) -> Result<TimestampRefreshResult> {
    let conn = open_handler_db(db_path)?;
    refresh_timestamp_for_distro_in_conn(&conn, keys_dir, distro, source_profile)
}

pub(crate) fn refresh_timestamp_for_distro_in_conn(
    conn: &rusqlite::Connection,
    keys_dir: &StdPath,
    distro: &str,
    source_profile: &str,
) -> Result<TimestampRefreshResult> {
    let timestamp_key = load_role_key(keys_dir, source_profile, RepositorySigningRole::Timestamp)?;
    let repo_id: i64 = conn
        .query_row(
            "SELECT id FROM repositories WHERE name = ?1 AND tuf_enabled = 1",
            params![distro],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("TUF repository not found for distro {distro}"))?;

    let Some((snapshot_version, snapshot_json)) = query_snapshot_for_timestamp(conn, repo_id)?
    else {
        bail!("snapshot metadata is missing for distro {distro}");
    };
    let previous_version: Option<i64> = conn
        .query_row(
            "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = 'timestamp'",
            params![repo_id],
            |row| row.get(0),
        )
        .optional()?;
    let version = previous_version.unwrap_or(0) + 1;
    let snapshot_bytes = snapshot_json.as_bytes();
    let mut hashes = BTreeMap::new();
    hashes.insert(
        "sha256".to_string(),
        conary_core::hash::sha256(snapshot_bytes),
    );
    let mut meta = BTreeMap::new();
    meta.insert(
        "snapshot.json".to_string(),
        MetaFile {
            version: snapshot_version as u64,
            length: Some(snapshot_bytes.len() as u64),
            hashes: Some(hashes),
        },
    );
    let timestamp = TimestampMetadata {
        type_field: "timestamp".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: version as u64,
        expires: chrono::Utc::now() + chrono::Duration::days(1),
        meta,
    };
    let signed = Signed {
        signatures: vec![
            sign_tuf_metadata(&timestamp_key, &timestamp).map_err(anyhow::Error::from)?,
        ],
        signed: timestamp,
    };
    let signed_json = serde_json::to_string(&signed)?;
    let metadata_hash = conary_core::hash::sha256(signed_json.as_bytes());

    conn.execute(
        "INSERT OR REPLACE INTO tuf_metadata
         (repository_id, role, version, metadata_hash, signed_metadata, expires_at)
         VALUES (?1, 'timestamp', ?2, ?3, ?4, ?5)",
        params![
            repo_id,
            version,
            metadata_hash,
            signed_json,
            signed.signed.expires.to_rfc3339(),
        ],
    )?;

    Ok(TimestampRefreshResult {
        status: "ok".to_string(),
        role: "timestamp".to_string(),
        distro: distro.to_string(),
        version: version as u64,
    })
}

fn query_snapshot_for_timestamp(
    conn: &rusqlite::Connection,
    repo_id: i64,
) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT version, signed_metadata FROM tuf_metadata
             WHERE repository_id = ?1 AND role = 'snapshot'",
            params![repo_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
fn query_tuf_repos(db_path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let conn = conary_core::db::open(db_path)?;
    let mut stmt = conn.prepare("SELECT name FROM repositories WHERE tuf_enabled = 1")?;

    let repos: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(repos)
}

#[cfg(test)]
mod tests;
