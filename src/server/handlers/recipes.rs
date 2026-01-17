// src/server/handlers/recipes.rs
//! Recipe build handlers for the Remi server

use crate::server::ServerState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

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

/// Build a package from a recipe
///
/// POST /v1/recipes/build
pub async fn build_recipe(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<RecipeBuildRequest>,
) -> Response {
    info!("Recipe build request: {}", request.recipe_url);

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

    // Sanitize filename
    let safe_name = name.replace(['/', '\\', '\0'], "_");
    let safe_version = version.replace(['/', '\\', '\0'], "_");
    let filename = format!("{}-{}.ccs", safe_name, safe_version);
    let ccs_path = cache_dir.join("packages").join(&filename);

    if !ccs_path.exists() {
        let error = serde_json::json!({
            "error": "not_found",
            "message": format!("Package {}-{} not found", name, version),
        });
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }

    // Read and serve the file
    match tokio::fs::read(&ccs_path).await {
        Ok(data) => {
            let headers = [
                ("Content-Type", "application/octet-stream"),
                (
                    "Content-Disposition",
                    &format!("attachment; filename=\"{}\"", filename),
                ),
            ];
            (StatusCode::OK, headers, data).into_response()
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
