// apps/conaryd/src/daemon/routes.rs

//! Axum router configuration for conaryd
//!
//! Defines all HTTP routes for the daemon REST API:
//! - `/health` - Health check endpoint
//! - `/v1/version` - API version info
//! - `/v1/transactions` - Transaction operations
//! - `/v1/packages` - Package queries and operations
//! - `/v1/events` - SSE event stream

use crate::daemon::auth::{Action, AuthChecker, PeerCredentials};
use crate::daemon::{DaemonError, DaemonEvent, DaemonJob, DaemonState, JobStatus};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    extract::{Extension, Path, Query, Request, State},
    http::StatusCode,
    middleware,
    response::{
        IntoResponse, Json, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use conary_core::db::models::{Changeset, DependencyEntry, Trove};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

mod events;
mod query;
mod system;
mod transactions;

const DAEMON_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DAEMON_SSE_CONNECTIONS: u64 = 64;
const INTERNAL_ERROR_DETAIL: &str = "An internal daemon error occurred";

/// Shared daemon state type
pub type SharedState = Arc<DaemonState>;

/// RAII guard that decrements the SSE connection counter on drop
struct SseConnectionGuard {
    metrics: Arc<DaemonState>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        self.metrics
            .metrics
            .sse_connections
            .fetch_sub(1, Ordering::Relaxed);
    }
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_secs: u64,
}

/// Version information response
#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub version: &'static str,
    pub api_version: &'static str,
    pub build_date: Option<&'static str>,
    pub git_commit: Option<&'static str>,
}

/// Error response wrapper for RFC 7807 format
pub struct ApiError(Box<DaemonError>);

impl From<DaemonError> for ApiError {
    fn from(err: DaemonError) -> Self {
        ApiError(Box::new(err))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let body = Json(&*self.0);

        (status, [("content-type", "application/problem+json")], body).into_response()
    }
}

/// Result type for API handlers
pub type ApiResult<T> = Result<T, ApiError>;

fn not_found_error(resource: &str, identifier: &str) -> ApiError {
    ApiError(Box::new(DaemonError::not_found(&format!(
        "{} '{}'",
        resource, identifier
    ))))
}

fn bad_request_error(message: &str) -> ApiError {
    ApiError(Box::new(DaemonError::bad_request(message)))
}

fn not_implemented_error(detail: &str) -> ApiError {
    ApiError(Box::new(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        detail,
    )))
}

fn internal_error(message: &str) -> DaemonError {
    tracing::error!("{message}");
    DaemonError::internal(INTERNAL_ERROR_DETAIL)
}

fn internal_error_with(context: &str, error: impl Display) -> DaemonError {
    tracing::error!(error = %error, "{context}");
    DaemonError::internal(INTERNAL_ERROR_DETAIL)
}

fn internal_api_error(context: &str, error: impl Display) -> ApiError {
    ApiError(Box::new(internal_error_with(context, error)))
}

fn acquire_sse_connection(state: &SharedState) -> Result<SseConnectionGuard, ApiError> {
    let result = state.metrics.sse_connections.fetch_update(
        Ordering::AcqRel,
        Ordering::Relaxed,
        |current| {
            if current < MAX_DAEMON_SSE_CONNECTIONS {
                Some(current + 1)
            } else {
                None
            }
        },
    );

    if result.is_err() {
        return Err(ApiError(Box::new(DaemonError::new(
            "too_many_connections",
            "Too Many Connections",
            503,
            "Too many concurrent SSE connections",
        ))));
    }

    Ok(SseConnectionGuard {
        metrics: state.clone(),
    })
}

fn action_for_job_kind(kind: crate::daemon::JobKind) -> Action {
    match kind {
        crate::daemon::JobKind::Install => Action::Install,
        crate::daemon::JobKind::Remove => Action::Remove,
        crate::daemon::JobKind::Update => Action::Update,
        crate::daemon::JobKind::DryRun => Action::Query,
        crate::daemon::JobKind::Rollback => Action::Rollback,
        crate::daemon::JobKind::Verify => Action::Verify,
        crate::daemon::JobKind::GarbageCollect => Action::GarbageCollect,
        crate::daemon::JobKind::Enhance => Action::Enhance,
    }
}

/// Run a blocking database query on a background thread
///
/// Handles the common pattern of cloning state, spawning a blocking task,
/// opening a database connection, and mapping errors consistently.
#[allow(clippy::result_large_err)]
async fn run_db_query<T: Send + 'static>(
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

/// Check authorization for a mutating action.
///
/// Extracts `PeerCredentials` from the request extension (injected per-connection
/// in `run_daemon`). Unix socket connections get credentials via `SO_PEERCRED`.
///
/// Returns `Ok(())` if the action is authorized, or an `ApiError` with 403 Forbidden.
fn require_auth(
    checker: &AuthChecker,
    creds: &Option<PeerCredentials>,
    action: Action,
) -> Result<(), ApiError> {
    match creds {
        Some(creds) => {
            if !creds.matches_current_process_identity() {
                tracing::warn!(
                    uid = creds.uid,
                    gid = creds.gid,
                    pid = creds.pid,
                    action = ?action,
                    "Authorization denied: stale peer credentials"
                );
                return Err(ApiError(Box::new(DaemonError::forbidden(
                    "Peer credentials are no longer valid for the current process",
                ))));
            }

            if checker.is_allowed(creds, action) {
                Ok(())
            } else {
                tracing::warn!(
                    uid = creds.uid,
                    gid = creds.gid,
                    pid = creds.pid,
                    action = ?action,
                    "Authorization denied"
                );
                Err(ApiError(Box::new(DaemonError::forbidden(&format!(
                    "User (uid={}) is not authorized for {:?}",
                    creds.uid, action
                )))))
            }
        }
        None => {
            // No peer credentials (TCP connection) - deny mutating actions
            tracing::warn!(action = ?action, "Mutating request denied: no peer credentials (TCP connection)");
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Mutating operations require a Unix socket connection with peer credentials",
            ))))
        }
    }
}

/// Require that a daemon API request comes from root or the daemon's own UID.
fn require_socket_identity(creds: &Option<PeerCredentials>) -> Result<(), ApiError> {
    let daemon_uid = nix::unistd::geteuid().as_raw();

    match creds {
        Some(creds) if !creds.matches_current_process_identity() => {
            tracing::warn!(
                uid = creds.uid,
                gid = creds.gid,
                pid = creds.pid,
                daemon_uid,
                "Daemon API request denied: peer credentials no longer match live process identity"
            );
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Daemon API requires live peer credentials from the current process",
            ))))
        }
        Some(creds) if creds.matches_daemon_identity(daemon_uid) => Ok(()),
        Some(creds) => {
            tracing::warn!(
                uid = creds.uid,
                gid = creds.gid,
                pid = creds.pid,
                daemon_uid,
                "Daemon API request denied: peer does not match daemon identity"
            );
            Err(ApiError(Box::new(DaemonError::forbidden(&format!(
                "Daemon API requires root or daemon uid {}; got uid={}",
                daemon_uid, creds.uid
            )))))
        }
        None => {
            tracing::warn!("Daemon API request denied: no peer credentials");
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Daemon API requires a Unix socket connection with peer credentials",
            ))))
        }
    }
}

/// Auth gate middleware for defense-in-depth
///
/// Rejects all `/v1` daemon API requests unless the Unix socket peer is root or
/// the daemon's own service UID. Individual handlers still check their specific
/// action permissions.
async fn auth_gate_middleware(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, ApiError> {
    let _ = state;
    tracing::trace!(
        method = %request.method(),
        path = %request.uri().path(),
        "Checking daemon auth gate"
    );
    require_socket_identity(&creds)?;
    Ok(next.run(request).await)
}

fn job_visible_to_requester(
    creds: &Option<PeerCredentials>,
    requested_by_uid: Option<u32>,
) -> bool {
    match creds {
        Some(creds) if creds.is_root() => true,
        Some(creds) => requested_by_uid == Some(creds.uid),
        None => false,
    }
}

fn ensure_job_visible(
    creds: &Option<PeerCredentials>,
    job: &DaemonJob,
    requested_id: &str,
) -> Result<(), ApiError> {
    if job_visible_to_requester(creds, job.requested_by_uid) {
        Ok(())
    } else {
        tracing::warn!(
            requested_job_id = requested_id,
            requested_by_uid = job.requested_by_uid,
            caller_uid = creds.as_ref().map(|creds| creds.uid),
            "Daemon transaction access denied: job owned by a different user"
        );
        Err(not_found_error("transaction", requested_id))
    }
}

fn event_visible_to_requester(
    state: &SharedState,
    creds: &Option<PeerCredentials>,
    cache: &mut HashMap<String, bool>,
    event: &DaemonEvent,
) -> bool {
    if creds.as_ref().is_some_and(PeerCredentials::is_root) {
        return true;
    }

    let Some(job_id) = event.job_id() else {
        return false;
    };

    if let Some(visible) = cache.get(job_id) {
        return *visible;
    }

    let visible = state
        .open_db()
        .ok()
        .and_then(|conn| DaemonJob::find_by_id(&conn, job_id).ok().flatten())
        .is_some_and(|job| job_visible_to_requester(creds, job.requested_by_uid));
    cache.insert(job_id.to_string(), visible);
    visible
}

// =============================================================================
// Query Response Types
// =============================================================================

/// Package summary for list endpoints
#[derive(Debug, Serialize)]
pub struct PackageSummary {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub package_type: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub install_reason: String,
    pub pinned: bool,
}

impl From<&Trove> for PackageSummary {
    fn from(trove: &Trove) -> Self {
        Self {
            name: trove.name.clone(),
            version: trove.version.clone(),
            package_type: trove.trove_type.as_str().to_string(),
            architecture: trove.architecture.clone(),
            description: trove.description.clone(),
            installed_at: trove.installed_at.clone(),
            install_reason: trove.install_reason.as_str().to_string(),
            pinned: trove.pinned,
        }
    }
}

/// Package details response
#[derive(Debug, Serialize)]
pub struct PackageDetails {
    pub name: String,
    pub version: String,
    #[serde(rename = "type")]
    pub package_type: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub installed_at: Option<String>,
    pub install_source: String,
    pub install_reason: String,
    pub selection_reason: Option<String>,
    pub flavor: Option<String>,
    pub pinned: bool,
    pub dependencies: Vec<DependencyInfo>,
}

/// Dependency info for package details
#[derive(Debug, Serialize)]
pub struct DependencyInfo {
    pub name: String,
    pub kind: String,
    #[serde(rename = "type")]
    pub dependency_type: String,
    pub version_constraint: Option<String>,
}

impl From<&DependencyEntry> for DependencyInfo {
    fn from(dep: &DependencyEntry) -> Self {
        Self {
            name: dep.depends_on_name.clone(),
            kind: dep.kind.clone(),
            dependency_type: dep.dependency_type.clone(),
            version_constraint: dep.version_constraint.clone(),
        }
    }
}

/// Changeset history entry
#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub description: String,
    pub status: String,
    pub created_at: Option<String>,
    pub applied_at: Option<String>,
}

impl From<&Changeset> for HistoryEntry {
    fn from(cs: &Changeset) -> Self {
        Self {
            id: cs.id.unwrap_or(0),
            description: cs.description.clone(),
            status: cs.status.as_str().to_string(),
            created_at: cs.created_at.clone(),
            applied_at: cs.applied_at.clone(),
        }
    }
}

/// Search query parameters
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

// =============================================================================
// Transaction Response Types
// =============================================================================

/// Transaction (job) summary for list endpoints
#[derive(Debug, Serialize)]
pub struct TransactionSummary {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

impl From<&DaemonJob> for TransactionSummary {
    fn from(job: &DaemonJob) -> Self {
        Self {
            id: job.id.clone(),
            kind: job.kind.as_str().to_string(),
            status: job.status.as_str().to_string(),
            created_at: job.created_at.clone(),
            started_at: job.started_at.clone(),
            completed_at: job.completed_at.clone(),
        }
    }
}

/// Full transaction details
#[derive(Debug, Serialize)]
pub struct TransactionDetails {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub kind: String,
    pub status: String,
    pub spec: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<DaemonError>,
    pub requested_by_uid: Option<u32>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    /// Position in queue (if queued)
    pub queue_position: Option<usize>,
}

impl TransactionDetails {
    fn from_job(job: &DaemonJob, queue_position: Option<usize>) -> Self {
        Self {
            id: job.id.clone(),
            idempotency_key: job.idempotency_key.clone(),
            kind: job.kind.as_str().to_string(),
            status: job.status.as_str().to_string(),
            spec: job.spec.clone(),
            result: job.result.clone(),
            error: job.error.clone(),
            requested_by_uid: job.requested_by_uid,
            created_at: job.created_at.clone(),
            started_at: job.started_at.clone(),
            completed_at: job.completed_at.clone(),
            queue_position,
        }
    }
}

/// Transaction list query parameters
#[derive(Debug, Deserialize)]
pub struct TransactionListQuery {
    /// Filter by status
    pub status: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

// =============================================================================
// Request Types for Mutating Operations
// =============================================================================

/// A single operation in a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransactionOperation {
    /// Install packages
    Install {
        /// Package names or file paths to install
        packages: Vec<String>,
        /// Allow downgrades
        #[serde(default)]
        allow_downgrade: bool,
        /// Skip dependency resolution
        #[serde(default)]
        skip_deps: bool,
    },
    /// Remove packages
    Remove {
        /// Package names to remove
        packages: Vec<String>,
        /// Also remove packages that depend on these
        #[serde(default)]
        cascade: bool,
        /// Also remove orphaned dependencies
        #[serde(default)]
        remove_orphans: bool,
    },
    /// Update packages
    Update {
        /// Package names to update (empty = update all)
        packages: Vec<String>,
        /// Only apply security updates
        #[serde(default)]
        security_only: bool,
    },
}

/// Request body for creating a transaction
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTransactionRequest {
    /// Operations to perform (install, remove, update)
    pub operations: Vec<TransactionOperation>,
}

/// Convenience request for package operations
#[derive(Debug, Clone, Deserialize)]
pub struct PackageOperationRequest {
    /// Package names to operate on
    pub packages: Vec<String>,
    /// Additional options (varies by operation type)
    #[serde(default)]
    pub options: PackageOperationOptions,
}

/// Options for package operations
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackageOperationOptions {
    /// For install: allow downgrades
    #[serde(default)]
    pub allow_downgrade: bool,
    /// For install: skip dependency resolution
    #[serde(default)]
    pub skip_deps: bool,
    /// For remove: cascade to dependents
    #[serde(default)]
    pub cascade: bool,
    /// For remove: also remove orphaned dependencies
    #[serde(default)]
    pub remove_orphans: bool,
    /// For update: only apply security updates
    #[serde(default)]
    pub security_only: bool,
}

/// Response body for transaction creation
#[derive(Debug, Serialize)]
pub struct CreateTransactionResponse {
    /// Job ID
    pub job_id: String,
    /// Status
    pub status: String,
    /// Position in queue
    pub queue_position: usize,
    /// URL to check status
    pub location: String,
}

/// Response body for dry-run transaction
#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    /// Operations that would be performed
    pub operations: Vec<TransactionOperation>,
    /// Summary of changes (placeholder)
    pub summary: DryRunSummary,
}

/// Summary of changes in a dry-run
#[derive(Debug, Serialize)]
pub struct DryRunSummary {
    /// Packages that would be installed
    pub install: Vec<String>,
    /// Packages that would be removed
    pub remove: Vec<String>,
    /// Packages that would be updated
    pub update: Vec<String>,
    /// Total number of packages affected
    pub total_affected: usize,
}

/// Build the main router
pub fn build_router(state: SharedState) -> Router {
    system::root_router()
        .nest("/v1", build_v1_router(state.clone()))
        .with_state(state)
}

/// Build the v1 API router
fn build_v1_router(state: SharedState) -> Router<SharedState> {
    Router::new()
        .merge(system::v1_router())
        .merge(transactions::router())
        .merge(query::router())
        .merge(events::router())
        .layer(middleware::from_fn_with_state(state, auth_gate_middleware))
        .layer(DefaultBodyLimit::max(DAEMON_BODY_LIMIT_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_response() {
        let err = DaemonError::not_found("package nginx");
        let api_err = ApiError::from(err);

        // Just verify it can be converted to a response
        let _ = api_err.into_response();
    }

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            status: "healthy",
            version: "0.2.0",
            uptime_secs: 100,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("healthy"));
        assert!(!json.contains("pid"));
    }

    #[test]
    fn test_version_response_serialization() {
        let resp = VersionResponse {
            version: "0.2.0",
            api_version: "1.0",
            build_date: None,
            git_commit: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("0.2.0"));
        assert!(!json.contains("schema_version"));
    }

    #[test]
    fn test_require_auth_root_allowed() {
        let checker = AuthChecker::new();
        let creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        assert!(require_auth(&checker, &creds, Action::Install).is_ok());
        assert!(require_auth(&checker, &creds, Action::Remove).is_ok());
        assert!(require_auth(&checker, &creds, Action::Update).is_ok());
        assert!(require_auth(&checker, &creds, Action::Rollback).is_ok());
        assert!(require_auth(&checker, &creds, Action::GarbageCollect).is_ok());
        assert!(require_auth(&checker, &creds, Action::CancelJob).is_ok());
    }

    #[test]
    fn test_require_auth_admin_group_allowed() {
        let checker = AuthChecker::new();
        let creds = Some(PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 10, // wheel
        });
        assert!(require_auth(&checker, &creds, Action::Install).is_ok());
        assert!(require_auth(&checker, &creds, Action::Remove).is_ok());
    }

    #[test]
    fn test_require_auth_regular_user_denied() {
        let checker = AuthChecker::new();
        let creds = Some(PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        });
        assert!(require_auth(&checker, &creds, Action::Install).is_err());
        assert!(require_auth(&checker, &creds, Action::Remove).is_err());
        assert!(require_auth(&checker, &creds, Action::Update).is_err());
        assert!(require_auth(&checker, &creds, Action::Rollback).is_err());
    }

    #[test]
    fn test_require_auth_no_creds_denied() {
        let checker = AuthChecker::new();
        // TCP connection with no peer credentials
        let creds: Option<PeerCredentials> = None;
        assert!(require_auth(&checker, &creds, Action::Install).is_err());
        assert!(require_auth(&checker, &creds, Action::Remove).is_err());
    }

    // =========================================================================
    // Handler endpoint tests
    //
    // These use tower::ServiceExt::oneshot to send HTTP requests through the
    // full Axum router, testing actual handler logic against a temporary
    // database.
    // =========================================================================

    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Create a DaemonState backed by a temporary database for testing.
    ///
    /// Returns the shared state and the temp directory (must be held alive
    /// for the duration of the test to prevent cleanup).
    fn create_test_state() -> (SharedState, tempfile::TempDir) {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let lock_path = temp_dir.path().join("daemon.lock");

        // Initialize the database with the full schema
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        conary_core::db::schema::migrate(&conn).unwrap();
        drop(conn);

        let config = crate::daemon::DaemonConfig {
            db_path,
            lock_path: lock_path.clone(),
            ..Default::default()
        };

        let system_lock = crate::daemon::SystemLock::try_acquire(&lock_path)
            .unwrap()
            .expect("Failed to acquire test lock");

        let state = Arc::new(crate::daemon::DaemonState::new(config, system_lock));
        (state, temp_dir)
    }

    fn create_test_state_with_db_path(
        db_path: std::path::PathBuf,
    ) -> (SharedState, tempfile::TempDir) {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("daemon.lock");
        let config = crate::daemon::DaemonConfig {
            db_path,
            lock_path: lock_path.clone(),
            ..Default::default()
        };

        let system_lock = crate::daemon::SystemLock::try_acquire(&lock_path)
            .unwrap()
            .expect("Failed to acquire test lock");

        let state = Arc::new(crate::daemon::DaemonState::new(config, system_lock));
        (state, temp_dir)
    }

    fn current_process_creds() -> Option<PeerCredentials> {
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
    fn test_router(state: SharedState, creds: Option<PeerCredentials>) -> Router {
        build_router(state).layer(axum::Extension(creds))
    }

    /// Extract the response body as bytes.
    async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    /// Extract the response body as a JSON value.
    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = body_bytes(response).await;
        serde_json::from_slice(&bytes).unwrap()
    }

    // -- GET /health ----------------------------------------------------------

    #[tokio::test]
    async fn test_handler_health_returns_200() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert!(json["uptime_secs"].is_number());
        assert!(json.get("pid").is_none());
    }

    // -- GET /v1/version ------------------------------------------------------

    #[tokio::test]
    async fn test_handler_version_returns_info() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/version")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["api_version"], "1.0");
        assert!(json.get("schema_version").is_none());
    }

    #[tokio::test]
    async fn test_v1_router_rejects_request_bodies_over_2mb() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());
        let oversized = "1,".repeat(DAEMON_BODY_LIMIT_BYTES / 2 + 64);
        let body = format!(
            "{{\"batch_size\":10,\"trove_ids\":[{}],\"types\":[],\"force\":false}}",
            oversized
        );

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/enhance")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_internal_errors_are_sanitized_for_clients() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let bad_db_path = temp_dir.path().join("db-dir");
        std::fs::create_dir_all(&bad_db_path).unwrap();
        let (state, _guard) = create_test_state_with_db_path(bad_db_path.clone());
        let app = test_router(state, current_process_creds());

        let request = axum::http::Request::builder()
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(body.contains(INTERNAL_ERROR_DETAIL));
        assert!(!body.contains("Database error"));
        assert!(!body.contains(&bad_db_path.display().to_string()));
    }

    // -- GET /v1/metrics ------------------------------------------------------

    #[tokio::test]
    async fn test_handler_metrics_returns_prometheus_format() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/metrics")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let text = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(text.contains("conary_jobs_total"));
        assert!(text.contains("conary_jobs_running"));
        assert!(text.contains("conary_sse_connections"));
    }

    // -- GET /v1/packages (empty) ---------------------------------------------

    #[tokio::test]
    async fn test_handler_list_packages_empty_db() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // -- GET /v1/packages/:name (404) -----------------------------------------

    #[tokio::test]
    async fn test_handler_get_package_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/packages/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let json = body_json(response).await;
        assert_eq!(json["status"], 404);
        assert!(json["detail"].as_str().unwrap().contains("nonexistent-pkg"));
    }

    // -- GET /v1/packages/:name/files (404) -----------------------------------

    #[tokio::test]
    async fn test_handler_get_package_files_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/packages/nonexistent-pkg/files")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- GET /v1/search?q=pattern (empty results) -----------------------------

    #[tokio::test]
    async fn test_handler_search_empty_results() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/search?q=nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // -- GET /v1/search (no query param) --------------------------------------

    #[tokio::test]
    async fn test_handler_search_no_query_param() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/search")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should succeed with empty results (matches all with wildcard)
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
    }

    // -- GET /v1/transactions (empty) -----------------------------------------

    #[tokio::test]
    async fn test_handler_list_transactions_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/transactions")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // -- GET /v1/transactions/:id (404) ---------------------------------------

    #[tokio::test]
    async fn test_handler_get_transaction_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/transactions/nonexistent-job-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let json = body_json(response).await;
        assert_eq!(json["status"], 404);
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("nonexistent-job-id")
        );
    }

    // -- POST /v1/transactions (valid) ----------------------------------------

    #[tokio::test]
    async fn test_handler_create_transaction_valid() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        // Install/remove/update are not yet executable by the daemon --
        // they are rejected with 400 to prevent silently broken jobs.
        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                }
            ]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("not yet supported")
        );
    }

    // -- POST /v1/transactions (empty operations = 400) -----------------------

    #[tokio::test]
    async fn test_handler_create_transaction_empty_operations() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": []
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert_eq!(json["status"], 400);
        assert!(json["detail"].as_str().unwrap().contains("operation"));
    }

    // -- POST /v1/transactions (invalid JSON = 400) ---------------------------

    #[tokio::test]
    async fn test_handler_create_transaction_invalid_json() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from("not valid json"))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Axum returns 400 Bad Request for JSON deserialization failures
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- POST /v1/transactions (no auth = 403) --------------------------------

    #[tokio::test]
    async fn test_handler_create_transaction_forbidden() {
        let (state, _dir) = create_test_state();
        // No credentials (simulates TCP connection)
        let app = test_router(state, None);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                }
            ]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    // -- POST /v1/transactions with idempotency key ---------------------------

    #[tokio::test]
    async fn test_handler_create_transaction_idempotency() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });

        // Unsupported kinds are rejected before enqueueing, so both calls
        // should consistently return 400.
        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["curl"]
                }
            ]
        });
        let body_str = serde_json::to_string(&body).unwrap();

        let app1 = test_router(state.clone(), root_creds);
        let request1 = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str.clone()))
            .unwrap();

        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::BAD_REQUEST);

        let app2 = test_router(state, root_creds);
        let request2 = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str))
            .unwrap();

        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::BAD_REQUEST);
    }

    // -- GET /v1/transactions/:id (after creation) ----------------------------

    #[tokio::test]
    async fn test_handler_get_transaction_after_creation() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });

        // Insert a job directly (transaction API rejects unsupported kinds)
        let job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        );
        let job_id = job.id.clone();
        {
            let conn = state.open_db().unwrap();
            job.insert(&conn).unwrap();
        }

        let app = test_router(state, root_creds);
        let get_req = axum::http::Request::builder()
            .uri(format!("/v1/transactions/{}", job_id))
            .body(Body::empty())
            .unwrap();

        let get_resp = app.oneshot(get_req).await.unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);

        let details = body_json(get_resp).await;
        assert_eq!(details["id"].as_str().unwrap(), job_id);
        assert_eq!(details["kind"], "enhance");
        assert_eq!(details["status"], "queued");
    }

    // -- GET /v1/transactions?status=queued -----------------------------------

    #[tokio::test]
    async fn test_handler_list_transactions_with_status_filter() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });

        // Insert a job directly (transaction API rejects unsupported kinds)
        let job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        );
        {
            let conn = state.open_db().unwrap();
            job.insert(&conn).unwrap();
        }

        // List queued transactions
        let app2 = test_router(state.clone(), root_creds);
        let list_req = axum::http::Request::builder()
            .uri("/v1/transactions?status=queued")
            .body(Body::empty())
            .unwrap();

        let list_resp = app2.oneshot(list_req).await.unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);

        let json = body_json(list_resp).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["status"], "queued");

        // List completed (should be empty)
        let app3 = test_router(state, root_creds);
        let list_req2 = axum::http::Request::builder()
            .uri("/v1/transactions?status=completed")
            .body(Body::empty())
            .unwrap();

        let list_resp2 = app3.oneshot(list_req2).await.unwrap();
        assert_eq!(list_resp2.status(), StatusCode::OK);

        let json2 = body_json(list_resp2).await;
        assert!(json2.is_array());
        assert_eq!(json2.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_list_transactions_filters_by_requesting_uid() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let visible_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        )
        .with_uid(daemon_uid);
        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);

        {
            let conn = state.open_db().unwrap();
            visible_job.insert(&conn).unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .uri("/v1/transactions")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["id"], visible_job.id);
    }

    #[tokio::test]
    async fn test_handler_get_transaction_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .uri(format!("/v1/transactions/{}", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_transaction_stream_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .uri(format!("/v1/transactions/{}/stream", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- DELETE /v1/transactions/:id (404) ------------------------------------

    #[tokio::test]
    async fn test_handler_cancel_transaction_not_found() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let request = axum::http::Request::builder()
            .method("DELETE")
            .uri("/v1/transactions/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_cancel_transaction_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/v1/transactions/{}", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_events_rejects_when_sse_limit_reached() {
        let (state, _dir) = create_test_state();
        state.metrics.sse_connections.store(64, Ordering::Relaxed);
        let app = test_router(state, current_process_creds());

        let request = axum::http::Request::builder()
            .uri("/v1/events")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // -- POST /v1/packages/install (valid) ------------------------------------

    #[tokio::test]
    async fn test_handler_install_packages_creates_transaction() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        // Install kind is not yet supported -- rejected at API boundary
        let body = serde_json::json!({
            "packages": ["nginx", "curl"]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/packages/install")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("not yet supported")
        );
    }

    // -- POST /v1/packages/install (empty packages = 400) ---------------------

    #[tokio::test]
    async fn test_handler_install_packages_empty_list() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "packages": []
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/packages/install")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert_eq!(json["status"], 400);
        assert!(json["detail"].as_str().unwrap().contains("package"));
    }

    // -- POST /v1/packages/remove (empty packages = 400) ----------------------

    #[tokio::test]
    async fn test_handler_remove_packages_empty_list() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "packages": []
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/packages/remove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- POST /v1/packages/update (empty packages = allowed) ------------------

    #[tokio::test]
    async fn test_handler_update_packages_empty_list_allowed() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        // Update kind is not yet supported -- rejected at API boundary
        let body = serde_json::json!({
            "packages": []
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/packages/update")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- GET /v1/depends/:name (404) ------------------------------------------

    #[tokio::test]
    async fn test_handler_depends_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/depends/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- GET /v1/rdepends/:name (empty) ---------------------------------------

    #[tokio::test]
    async fn test_handler_rdepends_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/rdepends/nonexistent-pkg")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // rdepends returns empty array (not 404) when no dependents exist
        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // -- GET /v1/history (empty) ----------------------------------------------

    #[tokio::test]
    async fn test_handler_history_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/history")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // -- GET /v1/system/states (empty stub) -----------------------------------

    #[tokio::test]
    async fn test_handler_list_states_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/system/states")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
    }

    // -- POST /v1/system/rollback (501) ---------------------------------------

    #[tokio::test]
    async fn test_handler_rollback_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/system/rollback")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    // -- POST /v1/system/verify (501) -----------------------------------------

    #[tokio::test]
    async fn test_handler_verify_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/system/verify")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    // -- POST /v1/system/gc (501) ---------------------------------------------

    #[tokio::test]
    async fn test_handler_gc_not_implemented() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/system/gc")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], 501);
    }

    // -- POST /v1/system/* without auth (403) ---------------------------------

    #[tokio::test]
    async fn test_handler_system_endpoints_require_auth() {
        let (state, _dir) = create_test_state();
        // No credentials
        let app = test_router(state, None);

        for endpoint in &["/v1/system/rollback", "/v1/system/verify", "/v1/system/gc"] {
            let app_clone = app.clone();
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(*endpoint)
                .body(Body::empty())
                .unwrap();

            let response = app_clone.oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "Expected 403 for {}",
                endpoint
            );
        }
    }

    // -- POST /v1/transactions/dry-run ----------------------------------------

    #[tokio::test]
    async fn test_handler_dry_run_valid() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx", "curl"]
                },
                {
                    "type": "remove",
                    "packages": ["vim"]
                }
            ]
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions/dry-run")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(json["summary"]["install"].as_array().unwrap().len(), 2);
        assert_eq!(json["summary"]["remove"].as_array().unwrap().len(), 1);
        assert_eq!(json["summary"]["total_affected"], 3);
    }

    // -- POST /v1/transactions/dry-run (empty operations = 400) ---------------

    #[tokio::test]
    async fn test_handler_dry_run_empty_operations() {
        let (state, _dir) = create_test_state();
        let root_creds = Some(PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        });
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": []
        });

        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions/dry-run")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // -- Nonexistent route returns 404 ----------------------------------------

    #[tokio::test]
    async fn test_handler_nonexistent_route() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .uri("/v1/does-not-exist")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- Auth gate middleware: PUT without auth = 403 -------------------------

    #[tokio::test]
    async fn test_auth_gate_blocks_put_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .method("PUT")
            .uri("/v1/transactions/some-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "PUT without credentials should be blocked by auth gate middleware"
        );

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    // -- Auth gate middleware: DELETE without auth = 403 -----------------------

    #[tokio::test]
    async fn test_auth_gate_blocks_delete_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .method("DELETE")
            .uri("/v1/transactions/some-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "DELETE without credentials should be blocked by auth gate middleware"
        );

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    // -- Auth gate middleware: GET without auth = 403 -------------------------

    #[tokio::test]
    async fn test_auth_gate_blocks_get_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET without credentials should be blocked by auth gate middleware"
        );
    }

    #[tokio::test]
    async fn test_auth_gate_blocks_get_for_non_daemon_user() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let unauthorized_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };
        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: 2000,
                uid: unauthorized_uid,
                gid: unauthorized_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET from a non-root, non-daemon uid should be blocked"
        );
    }

    #[tokio::test]
    async fn test_auth_gate_revalidates_live_peer_identity() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: u32::MAX,
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET with stale peer credentials should be blocked"
        );
    }

    #[test]
    fn test_event_visibility_filters_by_requesting_uid() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let visible_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        )
        .with_uid(daemon_uid);
        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);

        {
            let conn = state.open_db().unwrap();
            visible_job.insert(&conn).unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let creds = Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        });
        let mut cache = HashMap::new();

        assert!(event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::JobStarted {
                job_id: visible_job.id.clone(),
            }
        ));
        assert!(!event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::JobStarted {
                job_id: hidden_job.id.clone(),
            }
        ));
        assert!(!event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::StateCreated { state_number: 99 }
        ));
    }
}
