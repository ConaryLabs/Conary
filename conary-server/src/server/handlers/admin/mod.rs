// conary-server/src/server/handlers/admin/mod.rs
//! Handlers for the external admin API

mod artifacts;
mod audit;
mod ci;
mod events;
mod federation;
mod packages;
mod repos;
pub mod test_data;
mod tokens;

pub use artifacts::*;
pub use audit::*;
pub use ci::*;
pub use events::*;
pub use federation::*;
pub use packages::*;
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
///
/// Returns `None` on **success** (caller is authorized) and `Some(error_response)`
/// on **failure**. This "inverted Option" convention means callers use:
/// ```ignore
/// if let Some(err) = check_scope(&scopes, Scope::Admin) {
///     return err;
/// }
/// ```
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

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Build an axum app backed by a temporary database with one pre-seeded
    /// admin token (`test-admin-token-12345`, scopes = `admin`).
    ///
    /// Returns the router and the database path so callers can inspect DB
    /// state if needed. The `tempfile::TempDir` is leaked intentionally --
    /// tests are short-lived and the OS reclaims the directory on process
    /// exit.
    pub async fn test_app() -> (axum::Router, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        // Initialize DB with full schema
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            conary_core::db::schema::migrate(&conn).unwrap();
        }

        let config = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir: tmp.path().join("chunks"),
            cache_dir: tmp.path().join("cache"),
            ..Default::default()
        };
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));

        // Build the external admin router (includes auth middleware)
        let app = crate::server::routes::create_external_admin_router(state, None);

        // Seed a bootstrap token for tests
        let test_token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(test_token);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin")
                .unwrap();
        }

        // Leak the TempDir so it outlives the test (cleaned up at process exit)
        std::mem::forget(tmp);

        (app, db_path)
    }

    /// Helper to rebuild a fresh router against an existing database path.
    ///
    /// `oneshot()` consumes the router, so tests that need to make multiple
    /// sequential requests use this to create a fresh router each time.
    pub fn rebuild_app(db_path: &std::path::Path) -> axum::Router {
        let config = crate::server::ServerConfig {
            db_path: db_path.to_path_buf(),
            chunk_dir: db_path.parent().unwrap().join("chunks"),
            cache_dir: db_path.parent().unwrap().join("cache"),
            ..Default::default()
        };
        let state = Arc::new(tokio::sync::RwLock::new(
            crate::server::ServerState::new(config).expect("test server state"),
        ));
        crate::server::routes::create_external_admin_router(state, None)
    }
}
