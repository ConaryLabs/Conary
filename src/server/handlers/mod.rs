// src/server/handlers/mod.rs
//! HTTP request handlers for the Remi server

pub mod chunks;
pub mod detail;
pub mod federation;
pub mod index;
pub mod jobs;
pub mod models;
pub mod packages;
pub mod recipes;
pub mod search;
pub mod sparse;
pub mod tuf;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

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
