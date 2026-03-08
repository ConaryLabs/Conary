// conary-server/src/server/handlers/admin/mod.rs
//! Handlers for the external admin API

mod audit;
mod ci;
mod events;
mod federation;
mod repos;
mod tokens;

pub use audit::*;
pub use ci::*;
pub use events::*;
pub use federation::*;
pub use repos::*;
pub use tokens::*;

use axum::response::Response;

use crate::server::auth::{Scope, TokenScopes, json_error};

/// Validate a path parameter against a safe pattern.
///
/// Rejects values containing slashes, `..`, null bytes, or characters
/// outside `[a-zA-Z0-9._-]`. Returns a 400 Bad Request response on failure.
pub(crate) fn validate_path_param(value: &str, param_name: &str) -> Option<Response> {
    if value.is_empty()
        || value.contains('/')
        || value.contains("..")
        || value.contains('\0')
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        Some(json_error(
            400,
            &format!("Invalid {param_name}: must match [a-zA-Z0-9._-]+"),
            "INVALID_PARAMETER",
        ))
    } else {
        None
    }
}

/// Check that the caller has the required scope, returning an error response if not.
pub(crate) fn check_scope(
    scopes: &Option<axum::Extension<TokenScopes>>,
    required: Scope,
) -> Option<Response> {
    match scopes {
        Some(axum::Extension(s)) if s.has_scope(required) => None,
        Some(_) => Some(json_error(403, "Insufficient scope", "INSUFFICIENT_SCOPE")),
        None => Some(json_error(401, "Not authenticated", "UNAUTHORIZED")),
    }
}
