// src/server/handlers/index.rs
//! Repository index endpoints - metadata serving

use crate::server::ServerState;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Repository metadata response
#[derive(Serialize)]
pub struct RepositoryMetadata {
    /// Repository identifier
    pub id: String,
    /// Distribution name
    pub distro: String,
    /// Last sync timestamp (ISO 8601)
    pub last_sync: Option<String>,
    /// Number of packages available
    pub package_count: usize,
    /// Number of packages already converted to CCS
    pub converted_count: usize,
    /// List of available packages (names only for index)
    pub packages: Vec<PackageEntry>,
}

#[derive(Serialize)]
pub struct PackageEntry {
    pub name: String,
    pub version: String,
    /// Whether this package has been converted to CCS
    pub converted: bool,
}

/// GET /v1/:distro/metadata
///
/// Returns repository metadata index. Cached by Cloudflare.
pub async fn get_metadata(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    let state = state.read().await;
    let db_path = &state.config.db_path;

    // Query repository metadata
    match build_metadata(db_path, &distro) {
        Ok(metadata) => {
            // Cache for 5 minutes (Cloudflare will cache this)
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::from(
                    serde_json::to_string(&metadata).unwrap(),
                ))
                .unwrap()
        }
        Err(e) => {
            tracing::error!("Failed to build metadata for {}: {}", distro, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build metadata").into_response()
        }
    }
}

/// Build repository metadata from database
fn build_metadata(
    db_path: &std::path::Path,
    distro: &str,
) -> Result<RepositoryMetadata, anyhow::Error> {
    // TODO: Query repository_packages and converted_packages tables
    // For now, return stub data
    let _ = db_path;

    Ok(RepositoryMetadata {
        id: format!("conary-{}", distro),
        distro: distro.to_string(),
        last_sync: None,
        package_count: 0,
        converted_count: 0,
        packages: vec![],
    })
}

/// GET /v1/:distro/metadata.sig
///
/// Returns GPG signature for repository metadata.
pub async fn get_metadata_sig(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
) -> Response {
    // Validate distro
    if !["arch", "fedora", "ubuntu", "debian"].contains(&distro.as_str()) {
        return (StatusCode::BAD_REQUEST, "Unknown distribution").into_response();
    }

    let state = state.read().await;
    let sig_path = state
        .config
        .chunk_dir
        .parent()
        .unwrap_or(&state.config.chunk_dir)
        .join("repo")
        .join(&distro)
        .join("metadata.json.sig");

    if !sig_path.exists() {
        return (StatusCode::NOT_FOUND, "Signature not found").into_response();
    }

    match tokio::fs::read(&sig_path).await {
        Ok(sig) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/pgp-signature")
            .header(header::CACHE_CONTROL, "public, max-age=300")
            .body(axum::body::Body::from(sig))
            .unwrap(),
        Err(e) => {
            tracing::error!("Failed to read signature: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read signature").into_response()
        }
    }
}
