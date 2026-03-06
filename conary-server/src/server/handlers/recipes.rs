// conary-server/src/server/handlers/recipes.rs
//! Recipe build handlers for the Remi server

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use url::Url;

/// Request body for recipe build
#[derive(Debug, Deserialize)]
pub struct RecipeBuildRequest {
    /// URL to the recipe file
    pub recipe_url: String,
}

/// Response for successful recipe build
#[derive(Debug, Serialize)]
pub struct RecipeBuildResponse {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Download URL for the built package
    pub download_url: String,
    /// Chunk hashes
    pub chunks: Vec<String>,
    /// Total size
    pub size: u64,
}

/// Validate a URL to prevent SSRF attacks
///
/// Rejects non-HTTPS URLs and URLs targeting private/loopback IP ranges.
fn validate_url(url_str: &str) -> std::result::Result<(), String> {
    let parsed = Url::parse(url_str).map_err(|e| format!("Invalid URL: {e}"))?;

    if parsed.scheme() != "https" {
        return Err(format!(
            "Only https:// URLs are allowed, got {}://",
            parsed.scheme()
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return Err("URLs targeting localhost are not allowed".to_string());
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!(
                "URLs targeting private IP ranges are not allowed: {ip}"
            ));
        }
    }

    Ok(())
}

/// Check if an IP address is in a private/reserved range
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// Build a package from a recipe
///
/// POST /v1/recipes/build
pub async fn build_recipe(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<RecipeBuildRequest>,
) -> Response {
    info!("Recipe build request: {}", request.recipe_url);

    // Validate URL to prevent SSRF
    if let Err(e) = validate_url(&request.recipe_url) {
        let error = serde_json::json!({
            "error": "invalid_url",
            "message": e,
        });
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    }

    let state_read = state.read().await;
    let result = state_read
        .conversion_service
        .build_from_recipe(&request.recipe_url)
        .await;

    match result {
        Ok(conversion_result) => {
            let download_url = format!(
                "/v1/recipes/{}/{}/download",
                conversion_result.name, conversion_result.version
            );

            let response = RecipeBuildResponse {
                name: conversion_result.name,
                version: conversion_result.version,
                download_url,
                chunks: conversion_result.chunk_hashes,
                size: conversion_result.total_size,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": "build_failed",
                "message": format!("{}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// Download a recipe-built package
///
/// GET /v1/recipes/:name/:version/download
pub async fn download_recipe_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((name, version)): Path<(String, String)>,
) -> Response {
    let state_read = state.read().await;
    let cache_dir = &state_read.config.cache_dir;

    // Sanitize filename: strip path separators, null bytes, and ".." sequences
    let safe_name = name.replace(['/', '\\', '\0'], "_").replace("..", "_");
    let safe_version = version.replace(['/', '\\', '\0'], "_").replace("..", "_");
    let filename = format!("{}-{}.ccs", safe_name, safe_version);
    let ccs_path = cache_dir.join("packages").join(&filename);

    if !ccs_path.exists() {
        let error = serde_json::json!({
            "error": "not_found",
            "message": format!("Package {}-{} not found", name, version),
        });
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }

    // Stream the file instead of reading it all into memory
    match tokio::fs::File::open(&ccs_path).await {
        Ok(file) => {
            let metadata = match file.metadata().await {
                Ok(m) => m,
                Err(e) => {
                    let error = serde_json::json!({
                        "error": "read_failed",
                        "message": format!("Failed to read package metadata: {}", e),
                    });
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
                }
            };

            let stream = tokio_util::io::ReaderStream::new(file);
            let body = axum::body::Body::from_stream(stream);

            axum::http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", metadata.len())
                .header(
                    "Content-Disposition",
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": "read_failed",
                "message": format!("Failed to read package: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}
