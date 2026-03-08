// conary-server/src/server/handlers/mod.rs
//! HTTP request handlers for the Remi server

pub mod admin;
pub mod canonical;
pub mod chunks;
pub mod openapi;
pub mod detail;
pub mod federation;
pub mod index;
pub mod jobs;
pub mod models;
pub mod oci;
pub mod packages;
pub mod recipes;
pub mod search;
pub mod self_update;
pub mod sparse;
pub mod tuf;

use conary_core::db::models::Repository;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;

/// Format bytes as human-readable string (e.g., "1.50 KB", "700.00 GB")
pub(crate) fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

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

/// Validate distro name and both path parameters (distro + name) in one call.
/// Returns a 400 response on validation failure.
#[allow(clippy::result_large_err)]
pub fn validate_distro_and_name(distro: &str, name: &str) -> Result<(), Response> {
    validate_name(distro)?;
    validate_name(name)?;
    if !SUPPORTED_DISTROS.contains(&distro) {
        return Err((StatusCode::BAD_REQUEST, "Unknown distribution").into_response());
    }
    Ok(())
}

/// Run a blocking database closure via `spawn_blocking` and flatten the nested Result.
///
/// Handles the triple-match boilerplate (`Ok(Ok(..))`, `Ok(Err(..))`, `Err(..)`) that
/// appears in every handler that calls `spawn_blocking` for SQLite queries.
#[allow(clippy::result_large_err)]
pub async fn run_blocking<T, F>(context: &str, f: F) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => {
            tracing::error!("Database error in {context}: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response())
        }
        Err(e) => {
            tracing::error!("Task panicked in {context}: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response())
        }
    }
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
        if repo.name.contains(distro) {
            matched.push(repo.clone());
        }
    }

    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_bytes_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn test_human_bytes_kb() {
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1536), "1.50 KB");
    }

    #[test]
    fn test_human_bytes_mb() {
        assert_eq!(human_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn test_human_bytes_gb() {
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(human_bytes(700 * 1024 * 1024 * 1024), "700.00 GB");
    }

    #[test]
    fn test_human_bytes_tb() {
        assert_eq!(human_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }
}
