// apps/remi/src/server/handlers/search.rs
//! Search endpoints for the Remi package index
//!
//! Provides full-text search and autocomplete suggestions powered by the
//! Tantivy search engine. Returns typed 503 unless the committed projection is
//! bound to the exact active signed public universe.

use crate::server::ServerState;
use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::HandlerResult;
use super::public_read::{self, PublicUniverseUnavailableReason};

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

/// Extract the configured search engine. Each blocking query performs the
/// authority comparison while holding the index-swap read lock.
async fn get_search_engine(
    state: &Arc<RwLock<ServerState>>,
) -> HandlerResult<Arc<crate::server::SearchEngine>> {
    let guard = state.read().await;
    let Some(engine) = &guard.search_engine else {
        return Err(public_read::unavailable_response(
            PublicUniverseUnavailableReason::SearchIndexUnavailable,
            None,
        )
        .into());
    };
    Ok(Arc::clone(engine))
}

fn search_error_response(error: crate::server::search::PublicSearchError) -> Response {
    let reason = match error {
        crate::server::search::PublicSearchError::Unavailable => {
            PublicUniverseUnavailableReason::SearchIndexUnavailable
        }
        crate::server::search::PublicSearchError::RevisionMismatch => {
            PublicUniverseUnavailableReason::SearchIndexRevisionMismatch
        }
        crate::server::search::PublicSearchError::Query(error) => {
            tracing::error!(%error, "public search query failed");
            PublicUniverseUnavailableReason::AuthorityUnavailable
        }
    };
    public_read::unavailable_response(reason, None)
}

/// GET /v1/search?q=nginx&distro=fedora&limit=20
///
/// Full-text package search. Searches package names and descriptions.
/// Returns typed 503 if the search projection is unavailable or stale.
pub async fn search_packages(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let context = match public_read::context(&state).await {
        Ok(context) => context,
        Err(response) => return response.into_response(),
    };
    let profile = if let Some(distro) = params.distro.as_deref() {
        if let Err(response) = super::validate_supported_distro_route(distro) {
            return response;
        }
        match context.profile_for_route(distro) {
            Ok(profile) => Some(profile),
            Err(response) => return response.into_response(),
        }
    } else {
        None
    };
    let search_engine = match get_search_engine(&state).await {
        Ok(engine) => engine,
        Err(response) => return response.into_response(),
    };

    let query = params.q.unwrap_or_default();
    if query.is_empty() {
        return public_read::stamp(
            (
                StatusCode::BAD_REQUEST,
                Json(SearchResponse {
                    results: Vec::new(),
                    total: 0,
                    query: String::new(),
                }),
            )
                .into_response(),
            &context.universe,
            profile.as_ref(),
        );
    }

    let limit = params.limit.unwrap_or(20).min(100);
    let distro = params.distro.as_deref();

    // Run search on blocking thread since Tantivy is synchronous
    let query_clone = query.clone();
    let distro_owned = distro.map(String::from);
    let expected_universe = context.universe.identity().clone();
    let results = tokio::task::spawn_blocking(move || {
        search_engine.search_public_universe(
            &expected_universe,
            &query_clone,
            distro_owned.as_deref(),
            limit,
        )
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

            public_read::stamp(
                (
                    StatusCode::OK,
                    [(header::CACHE_CONTROL, "public, max-age=30")],
                    Json(response),
                )
                    .into_response(),
                &context.universe,
                profile.as_ref(),
            )
        }
        Ok(Err(error)) => search_error_response(error),
        Err(e) => {
            tracing::error!("Search task panicked: {}", e);
            public_read::unavailable_response(
                PublicUniverseUnavailableReason::AuthorityUnavailable,
                None,
            )
        }
    }
}

/// GET /v1/suggest?prefix=ngi&limit=10
///
/// Autocomplete suggestions based on package name prefix.
/// Returns typed 503 if the search projection is unavailable or stale.
pub async fn suggest_packages(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(params): Query<SuggestQuery>,
) -> Response {
    let context = match public_read::context(&state).await {
        Ok(context) => context,
        Err(response) => return response.into_response(),
    };
    let search_engine = match get_search_engine(&state).await {
        Ok(engine) => engine,
        Err(response) => return response.into_response(),
    };

    let prefix = params.prefix.unwrap_or_default();
    if prefix.is_empty() {
        return public_read::stamp(
            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "public, max-age=30")],
                Json(SuggestResponse {
                    suggestions: Vec::new(),
                    prefix: String::new(),
                }),
            )
                .into_response(),
            &context.universe,
            None,
        );
    }

    let limit = params.limit.unwrap_or(10).min(50);

    let prefix_clone = prefix.clone();
    let expected_universe = context.universe.identity().clone();
    let results = tokio::task::spawn_blocking(move || {
        search_engine.suggest_public_universe(&expected_universe, &prefix_clone, limit)
    })
    .await;

    match results {
        Ok(Ok(suggestions)) => {
            let response = SuggestResponse {
                suggestions,
                prefix,
            };

            public_read::stamp(
                (
                    StatusCode::OK,
                    [(header::CACHE_CONTROL, "public, max-age=30")],
                    Json(response),
                )
                    .into_response(),
                &context.universe,
                None,
            )
        }
        Ok(Err(error)) => search_error_response(error),
        Err(e) => {
            tracing::error!("Suggest task panicked: {}", e);
            public_read::unavailable_response(
                PublicUniverseUnavailableReason::AuthorityUnavailable,
                None,
            )
        }
    }
}
