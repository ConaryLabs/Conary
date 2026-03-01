// src/server/handlers/search.rs
//! Search endpoints for the Remi package index
//!
//! Provides full-text search and autocomplete suggestions powered by the
//! Tantivy search engine. Returns 503 if no search engine is configured.

use crate::server::ServerState;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Query parameters for the search endpoint
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Search query string
    pub q: Option<String>,
    /// Optional distribution filter
    pub distro: Option<String>,
    /// Maximum results to return (default 20, max 100)
    pub limit: Option<usize>,
}

/// Search response
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<crate::server::search::SearchResult>,
    pub total: usize,
    pub query: String,
}

/// Query parameters for the suggest endpoint
#[derive(Debug, Deserialize)]
pub struct SuggestQuery {
    /// Prefix to autocomplete
    pub prefix: Option<String>,
    /// Maximum suggestions to return (default 10, max 50)
    pub limit: Option<usize>,
}

/// Suggest response
#[derive(Debug, Serialize)]
pub struct SuggestResponse {
    pub suggestions: Vec<String>,
    pub prefix: String,
}

/// GET /v1/search?q=nginx&distro=fedora&limit=20
///
/// Full-text package search. Searches package names and descriptions.
/// Returns 503 if the search engine is not available.
pub async fn search_packages(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let state_guard = state.read().await;

    let search_engine = match &state_guard.search_engine {
        Some(engine) => Arc::clone(engine),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Search engine not available",
            )
                .into_response();
        }
    };
    drop(state_guard);

    let query = params.q.unwrap_or_default();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(SearchResponse {
                results: Vec::new(),
                total: 0,
                query: String::new(),
            }),
        )
            .into_response();
    }

    let limit = params.limit.unwrap_or(20).min(100);
    let distro = params.distro.as_deref();

    // Run search on blocking thread since Tantivy is synchronous
    let query_clone = query.clone();
    let distro_owned = distro.map(String::from);
    let results =
        tokio::task::spawn_blocking(move || {
            search_engine.search(&query_clone, distro_owned.as_deref(), limit)
        })
        .await;

    match results {
        Ok(Ok(results)) => {
            let total = results.len();
            let response = SearchResponse {
                results,
                total,
                query,
            };

            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "public, max-age=30")],
                Json(response),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Search error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Search failed").into_response()
        }
        Err(e) => {
            tracing::error!("Search task panicked: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Search failed").into_response()
        }
    }
}

/// GET /v1/suggest?prefix=ngi&limit=10
///
/// Autocomplete suggestions based on package name prefix.
/// Returns 503 if the search engine is not available.
pub async fn suggest_packages(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SuggestQuery>,
) -> Response {
    let state_guard = state.read().await;

    let search_engine = match &state_guard.search_engine {
        Some(engine) => Arc::clone(engine),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Search engine not available",
            )
                .into_response();
        }
    };
    drop(state_guard);

    let prefix = params.prefix.unwrap_or_default();
    if prefix.is_empty() {
        return (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=30")],
            Json(SuggestResponse {
                suggestions: Vec::new(),
                prefix: String::new(),
            }),
        )
            .into_response();
    }

    let limit = params.limit.unwrap_or(10).min(50);

    let prefix_clone = prefix.clone();
    let results =
        tokio::task::spawn_blocking(move || search_engine.suggest(&prefix_clone, limit)).await;

    match results {
        Ok(Ok(suggestions)) => {
            let response = SuggestResponse {
                suggestions,
                prefix,
            };

            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "public, max-age=30")],
                Json(response),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Suggest error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Suggest failed").into_response()
        }
        Err(e) => {
            tracing::error!("Suggest task panicked: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Suggest failed").into_response()
        }
    }
}
