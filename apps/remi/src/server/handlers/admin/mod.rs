// apps/remi/src/server/handlers/admin/mod.rs
//! Handlers for the external admin API

mod artifacts;
mod audit;
mod chunk_gc;
mod events;
mod federation;
mod repos;
pub mod test_data;
mod tokens;

pub use artifacts::*;
pub use audit::*;
pub use chunk_gc::*;
pub use events::*;
pub use federation::*;
pub use repos::*;
pub use tokens::*;

use axum::{
    extract::{Path, Request, State},
    response::Response,
};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};

pub async fn upload_release_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    request: Request,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_supported_admin_distro_route(&distro) {
        return err;
    }

    crate::server::release_publish::handle_release_upload(state, distro, request).await
}

pub(crate) fn validate_supported_admin_distro_route(distro: &str) -> Option<Response> {
    if let Some(err) = validate_path_param(distro, "distro") {
        return Some(err);
    }
    if conary_core::repository::supported_profiles::route_by_slug(distro).is_none() {
        return Some(json_error(
            400,
            "Unknown distribution",
            "UNKNOWN_DISTRIBUTION",
        ));
    }
    None
}

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
            conary_core::db::schema::ensure_current(&conn).unwrap();
        }

        let config = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir: tmp.path().join("chunks"),
            cache_dir: tmp.path().join("cache"),
            release_publish: test_release_publish(tmp.path()),
            ..Default::default()
        };
        std::fs::create_dir_all(&config.chunk_dir).unwrap();
        std::fs::create_dir_all(&config.cache_dir).unwrap();

        let mut server_state = crate::server::ServerState::new(config).expect("test server state");
        // ServerState::with_options points test_db_path at the production
        // /conary/test-data.db; every fixture-backed handler must stay inside
        // the tempdir.
        server_state.test_db_path = Some(
            tmp.path()
                .join("test-data.db")
                .to_string_lossy()
                .into_owned(),
        );
        let state = Arc::new(RwLock::new(server_state));

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
            release_publish: test_release_publish(db_path.parent().unwrap()),
            ..Default::default()
        };
        let mut server_state = crate::server::ServerState::new(config).expect("test server state");
        server_state.test_db_path = Some(
            db_path
                .parent()
                .unwrap()
                .join("test-data.db")
                .to_string_lossy()
                .into_owned(),
        );
        let state = Arc::new(tokio::sync::RwLock::new(server_state));
        crate::server::routes::create_external_admin_router(state, None)
    }

    fn test_release_publish(
        root: &std::path::Path,
    ) -> crate::server::config::ReleasePublishSection {
        use std::os::unix::fs::PermissionsExt;

        let keys_dir = root.join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();
        std::fs::set_permissions(&keys_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        for profile in ["fedora-44", "ubuntu-26.04"] {
            let distro_dir = keys_dir.join(profile);
            std::fs::create_dir_all(&distro_dir).unwrap();
            std::fs::set_permissions(&distro_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let private = distro_dir.join("targets.private");
            let public = distro_dir.join("targets.public");
            if !private.exists() {
                conary_core::ccs::signing::SigningKeyPair::generate()
                    .with_key_id("targets")
                    .save_to_files(&private, &public)
                    .unwrap();
            }
        }
        crate::server::config::ReleasePublishSection {
            repository_keys_dir: Some(keys_dir),
            trusted_build_attestation_signers: Vec::new(),
        }
    }
}
