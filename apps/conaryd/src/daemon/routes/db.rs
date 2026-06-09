// apps/conaryd/src/daemon/routes/db.rs
//! Blocking database query plumbing for daemon routes.

use super::errors::{ApiError, internal_api_error, internal_error_with};
use super::types::SharedState;

/// Run a blocking database query on a background thread
///
/// Handles the common pattern of cloning state, spawning a blocking task,
/// opening a database connection, and mapping errors consistently.
#[allow(clippy::result_large_err)]
pub(super) async fn run_db_query<T: Send + 'static>(
    state: &SharedState,
    f: impl FnOnce(&rusqlite::Connection) -> conary_core::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state
            .open_db()
            .map_err(|e| internal_error_with("Failed to open daemon database", e))?;
        f(&conn).map_err(|e| internal_error_with("Daemon database query failed", e))
    })
    .await
    .map_err(|e| internal_api_error("Daemon database task join failed", e))?
    .map_err(|e| ApiError(Box::new(e)))
}
