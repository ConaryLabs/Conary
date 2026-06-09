// apps/conaryd/src/daemon/routes/test_support.rs
//! Shared test helpers for daemon route tests.

use super::router::build_router;
use super::types::SharedState;
use crate::daemon::auth::PeerCredentials;
use crate::daemon::{DaemonConfig, DaemonState, SystemLock};
use axum::{Extension, Router, response::Response};
use http_body_util::BodyExt;
use std::sync::Arc;

/// Create a DaemonState backed by a temporary database for testing.
///
/// Returns the shared state and the temp directory (must be held alive
/// for the duration of the test to prevent cleanup).
pub(super) fn create_test_state() -> (SharedState, tempfile::TempDir) {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let lock_path = temp_dir.path().join("daemon.lock");

    // Initialize the database with the full schema
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    drop(conn);

    let config = DaemonConfig {
        db_path,
        lock_path: lock_path.clone(),
        ..Default::default()
    };

    let system_lock = SystemLock::try_acquire(&lock_path)
        .unwrap()
        .expect("Failed to acquire test lock");

    let state = Arc::new(DaemonState::new(config, system_lock));
    (state, temp_dir)
}

pub(super) fn create_test_state_with_db_path(
    db_path: std::path::PathBuf,
) -> (SharedState, tempfile::TempDir) {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let lock_path = temp_dir.path().join("daemon.lock");
    let config = DaemonConfig {
        db_path,
        lock_path: lock_path.clone(),
        ..Default::default()
    };

    let system_lock = SystemLock::try_acquire(&lock_path)
        .unwrap()
        .expect("Failed to acquire test lock");

    let state = Arc::new(DaemonState::new(config, system_lock));
    (state, temp_dir)
}

pub(super) fn current_process_creds() -> Option<PeerCredentials> {
    Some(PeerCredentials {
        pid: std::process::id(),
        uid: nix::unistd::geteuid().as_raw(),
        gid: nix::unistd::getegid().as_raw(),
    })
}

/// Build a test router with peer credentials injected as a layer.
///
/// The daemon normally injects credentials per-connection in run_daemon.
/// For tests we add them as a global layer so all requests have them.
pub(super) fn test_router(state: SharedState, creds: Option<PeerCredentials>) -> Router {
    build_router(state).layer(Extension(creds))
}

/// Extract the response body as bytes.
pub(super) async fn body_bytes(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

/// Extract the response body as a JSON value.
pub(super) async fn body_json(response: Response) -> serde_json::Value {
    let bytes = body_bytes(response).await;
    serde_json::from_slice(&bytes).unwrap()
}
