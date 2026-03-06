// conary-server/src/server/handlers/mod.rs
//! HTTP request handlers for the Remi server

pub mod canonical;
pub mod chunks;
pub mod detail;
pub mod federation;
pub mod index;
pub mod jobs;
pub mod models;
pub mod oci;
pub mod packages;
pub mod recipes;
pub mod search;
pub mod sparse;
pub mod tuf;

use conary_core::db::models::Repository;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;

/// Supported distribution names for validation
pub const SUPPORTED_DISTROS: &[&str] = &["arch", "fedora", "ubuntu", "debian"];

/// Validate a package or distro name: no path traversal, no null bytes, reasonable length
#[allow(clippy::result_large_err)]
pub fn validate_name(name: &str) -> Result<(), Response> {
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Name must not be empty").into_response());
    }
    if name.len() > 256 {
        return Err((StatusCode::BAD_REQUEST, "Name too long (max 256 chars)").into_response());
    }
    if name.contains('/') || name.contains("..") || name.contains('\0') {
        return Err((StatusCode::BAD_REQUEST, "Name contains invalid characters").into_response());
    }
    Ok(())
}

/// Serialize a value to JSON, returning a proper error response on failure
#[allow(clippy::result_large_err)]
pub fn serialize_json<T: serde::Serialize>(value: &T, context: &str) -> Result<String, Response> {
    serde_json::to_string(value).map_err(|e| {
        tracing::error!("Failed to serialize {}: {}", context, e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Serialization error").into_response()
    })
}

/// Build a JSON response with cache headers
pub fn json_response(json: String, cache_max_age: u32) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::CACHE_CONTROL,
            format!("public, max-age={cache_max_age}"),
        )
        .body(axum::body::Body::from(json))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Find a repository configured for the given distro
///
/// Tries `default_strategy_distro` first, then falls back to name matching.
/// Returns the first match only (used by conversion endpoints).
pub fn find_repository_for_distro(
    conn: &Connection,
    distro: &str,
) -> Result<Option<Repository>, anyhow::Error> {
    let all = find_repositories_for_distro(conn, distro)?;
    Ok(all.into_iter().next())
}

/// Find all repositories configured for the given distro
///
/// Returns repos with matching `default_strategy_distro` first,
/// then any with matching names. Used by the metadata endpoint to
/// aggregate packages across all repos for a distro (e.g. arch-core + arch-extra).
pub fn find_repositories_for_distro(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<Repository>, anyhow::Error> {
    let repos = Repository::list_enabled(conn)?;
    let mut matched = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // First pass: exact match on default_strategy_distro
    for repo in &repos {
        if repo.default_strategy_distro.as_deref() == Some(distro) {
            if let Some(id) = repo.id {
                seen_ids.insert(id);
            }
            matched.push(repo.clone());
        }
    }

    // Second pass: name-based matching (skip already matched)
    for repo in &repos {
        if let Some(id) = repo.id
            && seen_ids.contains(&id)
        {
            continue;
        }
        if repo.name.starts_with(distro) || repo.name.contains(distro) {
            matched.push(repo.clone());
        }
    }

    Ok(matched)
}
