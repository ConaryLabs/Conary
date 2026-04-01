// apps/remi/src/server/handlers/profiles.rs
//! Profile publishing endpoints
//!
//! Stores and retrieves build profiles keyed by content-addressed SHA-256 hash.
//! The profile TOML is stored as a raw CAS object.
//!
//! Endpoints:
//! - GET /v1/profiles/:profile_hash -- fetch profile TOML (public)
//! - PUT /v1/profiles/:profile_hash -- publish profile (requires bearer token)

use crate::server::ServerState;
use crate::server::handlers::{cas_object_path, is_valid_hex_hash, require_admin_token};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tokio::sync::RwLock;

// ────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────

/// Validate that a profile hash contains only hex characters and is exactly 64 chars.
fn is_valid_profile_hash(hash: &str) -> bool {
    is_valid_hex_hash(hash)
}

// ────────────────────────────────────────────────────────────
// GET /v1/profiles/:profile_hash
// ────────────────────────────────────────────────────────────

/// Retrieve a build profile TOML by its SHA-256 hash.
///
/// Returns:
/// - 200 OK with `Content-Type: application/toml` and the raw TOML body
/// - 404 Not Found if the hash is unknown
pub async fn get_profile(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(profile_hash): Path<String>,
) -> Response {
    if !is_valid_profile_hash(&profile_hash) {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid profile hash format (expected 64 hex chars)",
        )
            .into_response();
    }

    let chunk_dir = {
        let guard = state.read().await;
        guard.config.chunk_dir.clone()
    };

    let object_path = cas_object_path(&chunk_dir, &profile_hash);

    match tokio::fs::read(&object_path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/toml")
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "Profile not found").into_response()
        }
        Err(e) => {
            tracing::error!("Failed to read profile CAS object {profile_hash}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read profile").into_response()
        }
    }
}

// ────────────────────────────────────────────────────────────
// PUT /v1/profiles/:profile_hash
// ────────────────────────────────────────────────────────────

/// Publish a build profile TOML.
///
/// Steps:
/// 1. Validate bearer token (admin scope required)
/// 2. Read body bytes
/// 3. Compute SHA-256; verify it matches the `profile_hash` URL parameter
/// 4. Write bytes into CAS at `<chunk_dir>/objects/<2>/<62>`
///
/// Returns 201 Created on success.
///
/// NOTE: Auth is checked inline via `require_admin_token` rather than the admin
/// router's middleware so that GET on the same path stays public. This means
/// the admin rate limiters (governor-based) do not apply here.
// TODO: Move write endpoints to the admin router to get rate limiting for free.
pub async fn put_profile(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(profile_hash): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if !is_valid_profile_hash(&profile_hash) {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid profile hash format (expected 64 hex chars)",
        )
            .into_response();
    }

    let (db_path, chunk_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.chunk_dir.clone())
    };

    // Auth check (inline, so GET on the same path stays public)
    if let Some(err) = require_admin_token(&headers, &db_path).await {
        return err;
    }

    // Read body (cap at 4 MiB -- profiles are small TOML files)
    let body_bytes = match axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to read PUT /v1/profiles body: {e}");
            return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response();
        }
    };

    if body_bytes.is_empty() {
        return (StatusCode::BAD_REQUEST, "Request body must not be empty").into_response();
    }

    // Compute SHA-256 and verify it matches the URL parameter
    let computed_hash = conary_core::hash::sha256(&body_bytes);

    if computed_hash != profile_hash.to_ascii_lowercase() {
        return (
            StatusCode::BAD_REQUEST,
            "Hash mismatch: body SHA-256 does not match profile_hash in URL",
        )
            .into_response();
    }

    // Write to CAS
    let object_path = cas_object_path(&chunk_dir, &computed_hash);
    if let Some(parent) = object_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        tracing::error!("Failed to create CAS directory {}: {e}", parent.display());
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write to CAS").into_response();
    }

    // Write is idempotent: if the file already exists, skip
    if !object_path.exists()
        && let Err(e) = tokio::fs::write(&object_path, &body_bytes).await
    {
        tracing::error!("Failed to write profile CAS object {computed_hash}: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to write to CAS").into_response();
    }

    tracing::info!(profile_hash = %computed_hash, "Profile published");

    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(format!("Published profile {computed_hash}")))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_hashes() {
        // Exactly 64 lowercase hex chars
        assert!(is_valid_profile_hash(
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"
        ));
        // Uppercase hex is also valid (hex digits include A-F)
        assert!(is_valid_profile_hash(
            "AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899"
        ));
    }

    #[test]
    fn invalid_hashes() {
        // Too short
        assert!(!is_valid_profile_hash("abc"));
        // Too long
        assert!(!is_valid_profile_hash(
            "aabbccddeeff00112233445566778899aabbccddeeff001122334455667788990"
        ));
        // Non-hex character
        assert!(!is_valid_profile_hash(
            "aabbccddeeff00112233445566778899aabbccddeeff0011223344556677889g"
        ));
        // Empty
        assert!(!is_valid_profile_hash(""));
    }

    #[test]
    fn cas_path_layout() {
        let dir = std::path::PathBuf::from("/chunks");
        let hash = "aabbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890";
        let path = cas_object_path(&dir, hash);
        assert_eq!(
            path,
            std::path::PathBuf::from(
                "/chunks/objects/aa/bbcc1234567890aabbcc1234567890aabbcc1234567890aabbcc1234567890"
            )
        );
    }
}
