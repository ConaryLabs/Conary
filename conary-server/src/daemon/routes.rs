// conary-server/src/daemon/routes.rs

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
use conary_core::db::models::{Changeset, DependencyEntry, Trove};
use axum::{
    Router,
    extract::{Extension, Path, Query, Request, State},
    http::{Method, StatusCode},
    middleware,
    response::{
        IntoResponse, Json, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

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
    pub pid: u32,
    pub uptime_secs: u64,
}

/// Version information response
#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub version: &'static str,
    pub api_version: &'static str,
    pub schema_version: i32,
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

fn action_for_job_kind(kind: crate::daemon::JobKind) -> Action {
    match kind {
        crate::daemon::JobKind::Install => Action::Install,
        crate::daemon::JobKind::Remove => Action::Remove,
        crate::daemon::JobKind::Update => Action::Update,
        crate::daemon::JobKind::DryRun => Action::Query,
        crate::daemon::JobKind::Rollback => Action::Rollback,
        crate::daemon::JobKind::Verify => Action::Verify,
        crate::daemon::JobKind::GarbageCollect => Action::GarbageCollect,
        // Enhance is a background admin operation; map to GarbageCollect
        // (requires root/admin privilege) since there is no dedicated
        // Action::Enhance variant.
        crate::daemon::JobKind::Enhance => Action::GarbageCollect,
    }
}

/// Run a blocking database query on a background thread
///
/// Handles the common pattern of cloning state, spawning a blocking task,
/// opening a database connection, and mapping errors consistently.
async fn run_db_query<T: Send + 'static>(
    state: &SharedState,
    f: impl FnOnce(&rusqlite::Connection) -> conary_core::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state
            .open_db()
            .map_err(|e| DaemonError::internal(&format!("Database error: {}", e)))?;
        f(&conn).map_err(|e| DaemonError::internal(&format!("Database error: {}", e)))
    })
    .await
    .map_err(|e| {
        ApiError(Box::new(DaemonError::internal(&format!(
            "Task join error: {}",
            e
        ))))
    })?
    .map_err(|e| ApiError(Box::new(e)))
}

/// Check authorization for a mutating action.
///
/// Extracts `PeerCredentials` from the request extension (injected per-connection
/// in `run_daemon`). Unix socket connections get credentials via `SO_PEERCRED`;
/// TCP connections have no credentials and are restricted to read-only access.
///
/// Returns `Ok(())` if the action is authorized, or an `ApiError` with 403 Forbidden.
fn require_auth(
    checker: &AuthChecker,
    creds: &Option<PeerCredentials>,
    action: Action,
) -> Result<(), ApiError> {
    match creds {
        Some(creds) => {
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

/// Auth gate middleware for defense-in-depth
///
/// Rejects POST/PUT/DELETE requests without valid credentials at the router level.
/// Uses `Action::Install` (requires root/admin/polkit) so that a new mutating
/// endpoint missing its own `require_auth()` call is still protected.
/// Individual handlers still check their specific action permissions.
async fn auth_gate_middleware(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, ApiError> {
    if request.method() == Method::POST
        || request.method() == Method::PUT
        || request.method() == Method::DELETE
    {
        require_auth(&state.auth_checker, &creds, Action::Install)?;
    }
    Ok(next.run(request).await)
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

/// Extract idempotency key from request headers
fn get_idempotency_key(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Build the main router
pub fn build_router(state: SharedState) -> Router {
    Router::new()
        // Health check (no auth required)
        .route("/health", get(health_handler))
        // API v1
        .nest("/v1", build_v1_router(state.clone()))
        .with_state(state)
}

/// Build the v1 API router
fn build_v1_router(state: SharedState) -> Router<SharedState> {
    Router::new()
        // Version info
        .route("/version", get(version_handler))
        // Metrics (Prometheus format)
        .route("/metrics", get(metrics_handler))
        // Transactions
        .route("/transactions", get(list_transactions_handler))
        .route("/transactions", post(create_transaction_handler))
        // dry-run must come before :id to avoid the wildcard capturing "dry-run"
        .route("/transactions/dry-run", post(dry_run_handler))
        .route("/transactions/:id", get(get_transaction_handler))
        .route("/transactions/:id", delete(cancel_transaction_handler))
        .route("/transactions/:id/stream", get(transaction_stream_handler))
        // Package convenience endpoints
        .route("/packages", get(list_packages_handler))
        .route("/packages/:name", get(get_package_handler))
        .route("/packages/:name/files", get(get_package_files_handler))
        .route("/packages/install", post(install_packages_handler))
        .route("/packages/remove", post(remove_packages_handler))
        .route("/packages/update", post(update_packages_handler))
        // Search
        .route("/search", get(search_handler))
        // Dependencies
        .route("/depends/:name", get(depends_handler))
        .route("/rdepends/:name", get(rdepends_handler))
        // History
        .route("/history", get(history_handler))
        // System operations
        .route("/system/states", get(list_states_handler))
        .route("/system/rollback", post(rollback_handler))
        .route("/system/verify", post(verify_handler))
        .route("/system/gc", post(gc_handler))
        // Global event stream
        .route("/events", get(events_handler))
        // Defense-in-depth: reject mutating requests without credentials
        .layer(middleware::from_fn_with_state(
            state,
            auth_gate_middleware,
        ))
}

// =============================================================================
// Health & Version Handlers
// =============================================================================

/// Health check endpoint
///
/// GET /health
///
/// Returns health status. Used by systemd watchdog and load balancers.
async fn health_handler(State(state): State<SharedState>) -> Json<HealthResponse> {
    let uptime_secs = state.uptime_secs();

    Json(HealthResponse {
        status: "healthy",
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
        uptime_secs,
    })
}

/// Version information endpoint
///
/// GET /v1/version
///
/// Returns detailed version information.
async fn version_handler(State(_state): State<SharedState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION"),
        api_version: "1.0",
        schema_version: conary_core::db::schema::SCHEMA_VERSION,
        build_date: option_env!("BUILD_DATE"),
        git_commit: option_env!("GIT_COMMIT"),
    })
}

/// Metrics endpoint (Prometheus format)
///
/// GET /v1/metrics
async fn metrics_handler(State(state): State<SharedState>) -> String {
    let m = &state.metrics;

    format!(
        r#"# HELP conary_jobs_total Total jobs processed
# TYPE conary_jobs_total counter
conary_jobs_total {}

# HELP conary_jobs_running Currently running jobs
# TYPE conary_jobs_running gauge
conary_jobs_running {}

# HELP conary_jobs_completed Jobs completed successfully
# TYPE conary_jobs_completed counter
conary_jobs_completed {}

# HELP conary_jobs_failed Jobs that failed
# TYPE conary_jobs_failed counter
conary_jobs_failed {}

# HELP conary_jobs_cancelled Jobs that were cancelled
# TYPE conary_jobs_cancelled counter
conary_jobs_cancelled {}

# HELP conary_sse_connections Active SSE connections
# TYPE conary_sse_connections gauge
conary_sse_connections {}
"#,
        m.jobs_total.load(Ordering::Relaxed),
        m.jobs_running.load(Ordering::Relaxed),
        m.jobs_completed.load(Ordering::Relaxed),
        m.jobs_failed.load(Ordering::Relaxed),
        m.jobs_cancelled.load(Ordering::Relaxed),
        m.sse_connections.load(Ordering::Relaxed),
    )
}

// =============================================================================
// Transaction Handlers (Stubs)
// =============================================================================

/// List transactions
///
/// GET /v1/transactions?status=queued|running|completed|failed|cancelled&limit=N
///
/// Lists transactions (jobs) with optional filtering by status.
async fn list_transactions_handler(
    State(state): State<SharedState>,
    Query(params): Query<TransactionListQuery>,
) -> ApiResult<Json<Vec<TransactionSummary>>> {
    let limit = params.limit.map(|n| n.min(1000));
    let jobs = run_db_query(&state, move |conn| {
        let status_filter = params.status.as_deref().and_then(|s| match s {
            "queued" => Some(JobStatus::Queued),
            "running" => Some(JobStatus::Running),
            "completed" => Some(JobStatus::Completed),
            "failed" => Some(JobStatus::Failed),
            "cancelled" => Some(JobStatus::Cancelled),
            _ => None,
        });

        match status_filter {
            Some(status) => DaemonJob::list_by_status(conn, status),
            None => DaemonJob::list_all(conn, limit),
        }
    })
    .await?;

    let summaries: Vec<TransactionSummary> = jobs.iter().map(TransactionSummary::from).collect();
    Ok(Json(summaries))
}

/// Create a new transaction
///
/// POST /v1/transactions
///
/// Creates a new transaction with one or more operations (install, remove, update).
/// The transaction is queued and executed asynchronously.
///
/// Request headers:
/// - `X-Idempotency-Key`: Optional client-provided key for deduplication
///
/// Returns:
/// - 202 Accepted with Location header pointing to the transaction resource
/// - 409 Conflict if an idempotency key was provided and a job with that key already exists
async fn create_transaction_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CreateTransactionRequest>,
) -> ApiResult<(
    StatusCode,
    [(axum::http::header::HeaderName, String); 1],
    Json<CreateTransactionResponse>,
)> {
    require_auth(
        &state.auth_checker,
        &creds,
        action_for_job_kind(determine_job_kind(&request.operations)),
    )?;

    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

    // Get idempotency key from headers
    let idempotency_key = get_idempotency_key(&headers);

    // Check for existing job with same idempotency key
    if let Some(ref key) = idempotency_key {
        let key_clone = key.clone();
        let existing = run_db_query(&state, move |conn| {
            DaemonJob::find_by_idempotency_key(conn, &key_clone)
        })
        .await?;

        if let Some(existing_job) = existing {
            // Return the existing job's info
            let location = format!("/v1/transactions/{}", existing_job.id);
            let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);

            let response = CreateTransactionResponse {
                job_id: existing_job.id,
                status: existing_job.status.as_str().to_string(),
                queue_position,
                location: location.clone(),
            };

            return Ok((
                StatusCode::OK,
                [(axum::http::header::LOCATION, location)],
                Json(response),
            ));
        }
    }

    // Determine job kind from operations
    let job_kind = determine_job_kind(&request.operations);

    // Create the job
    let spec = serde_json::to_value(&request.operations).map_err(|e| {
        ApiError(Box::new(DaemonError::internal(&format!(
            "Serialization error: {}",
            e
        ))))
    })?;

    let mut job = DaemonJob::new(job_kind, spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }

    let job_id = job.id.clone();

    // Insert into database
    let insert_job = job.clone();
    run_db_query(&state, move |conn| insert_job.insert(conn)).await?;

    // Enqueue the job
    let _cancel_token = state
        .queue
        .enqueue(job, crate::daemon::JobPriority::Normal)
        .await;

    // Get queue position
    let queue_position = state.queue.position(&job_id).await.unwrap_or(0);

    // Emit event
    state.emit(DaemonEvent::JobQueued {
        job_id: job_id.clone(),
        position: queue_position,
    });

    // Increment metrics
    state.metrics.jobs_total.fetch_add(1, Ordering::Relaxed);

    // Build response
    let location = format!("/v1/transactions/{}", job_id);
    let response = CreateTransactionResponse {
        job_id,
        status: "queued".to_string(),
        queue_position,
        location: location.clone(),
    };

    Ok((
        StatusCode::ACCEPTED,
        [(axum::http::header::LOCATION, location)],
        Json(response),
    ))
}

/// Determine the job kind from operations
fn determine_job_kind(operations: &[TransactionOperation]) -> crate::daemon::JobKind {
    use crate::daemon::JobKind;

    // If all operations are the same type, use that type
    // Otherwise, use Install as the default (mixed operations)
    let mut has_install = false;
    let mut has_remove = false;
    let mut has_update = false;

    for op in operations {
        match op {
            TransactionOperation::Install { .. } => has_install = true,
            TransactionOperation::Remove { .. } => has_remove = true,
            TransactionOperation::Update { .. } => has_update = true,
        }
    }

    match (has_install, has_remove, has_update) {
        (true, false, false) => JobKind::Install,
        (false, true, false) => JobKind::Remove,
        (false, false, true) => JobKind::Update,
        _ => JobKind::Install, // Mixed operations default to Install
    }
}

/// Get transaction details
///
/// GET /v1/transactions/:id
///
/// Returns full details of a transaction including its spec, result, and error.
async fn get_transaction_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TransactionDetails>> {
    // First check if it's in the queue
    let queue_position = state.queue.position(&id).await;

    let job_id = id.clone();
    let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &job_id)).await?;

    let job = job.ok_or_else(|| not_found_error("transaction", &id))?;
    Ok(Json(TransactionDetails::from_job(&job, queue_position)))
}

/// Cancel a transaction
///
/// DELETE /v1/transactions/:id
///
/// Cancels a queued or running transaction.
/// - If queued: removes from queue and marks as cancelled
/// - If running: sets cancel token (operation will stop at next checkpoint)
async fn cancel_transaction_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    require_auth(&state.auth_checker, &creds, Action::CancelJob)?;

    let job_id = id.clone();

    // First check if the job exists
    let find_id = job_id.clone();
    let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &find_id))
        .await?
        .ok_or_else(|| not_found_error("transaction", &id))?;

    // Check if already completed/cancelled/failed
    match job.status {
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
            return Err(ApiError(Box::new(DaemonError::conflict(&format!(
                "Transaction '{}' is already {}",
                id,
                job.status.as_str()
            )))));
        }
        _ => {}
    }

    // Try to cancel
    let cancelled = state.cancel_job(&job_id).await;

    if cancelled || job.status == JobStatus::Queued {
        // Update database
        let update_id = job_id.clone();
        let updated = run_db_query(&state, move |conn| {
            DaemonJob::update_status(conn, &update_id, JobStatus::Cancelled)
        })
        .await?;

        if updated {
            state.emit(DaemonEvent::JobCancelled { job_id });
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(not_found_error("transaction", &id))
        }
    } else {
        Err(ApiError(Box::new(DaemonError::conflict(&format!(
            "Cannot cancel transaction '{}' - it may already be completing",
            id
        )))))
    }
}

/// Transaction event stream (SSE)
///
/// GET /v1/transactions/:id/stream
///
/// Streams events for a specific transaction/job using Server-Sent Events.
/// Only events related to the specified job are included.
///
/// The stream will:
/// - Send a "connected" event immediately with job info
/// - Stream job-specific progress events
/// - End when the job completes, fails, or is cancelled
async fn transaction_stream_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let job_id = id.clone();

    // First verify the job exists
    let check_id = job_id.clone();
    let exists = run_db_query(&state, move |conn| {
        Ok(DaemonJob::find_by_id(conn, &check_id)?.is_some())
    })
    .await?;

    if !exists {
        return Err(not_found_error("transaction", &id));
    }

    // Track SSE connection (guard decrements on drop when stream ends)
    state
        .metrics
        .sse_connections
        .fetch_add(1, Ordering::Relaxed);
    let _guard = SseConnectionGuard {
        metrics: state.clone(),
    };

    // Subscribe to the event broadcast channel
    let rx = state.subscribe();
    let filter_job_id = job_id.clone();

    // Create a stream that filters to only this job's events
    let event_stream = BroadcastStream::new(rx).filter_map(move |result| {
        let job_id = filter_job_id.clone();
        match result {
            Ok(event) => {
                if event.job_id() != Some(job_id.as_str()) {
                    return None;
                }
                match serde_json::to_string(&event) {
                    Ok(json) => Some(Ok(Event::default()
                        .event(event.event_type_name())
                        .data(json))),
                    Err(_) => None,
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("SSE client (job {}) lagged {} events", job_id, n);
                Some(Ok(Event::default()
                    .event("warning")
                    .data(format!(r#"{{"lagged": {}}}"#, n))))
            }
        }
    });

    // Prepend a "connected" event with job info
    let connected_data = serde_json::json!({
        "status": "connected",
        "job_id": job_id
    });
    let connected_event = stream::once(async move {
        Ok(Event::default()
            .event("connected")
            .data(connected_data.to_string()))
    });

    // Move guard into the stream so it lives as long as the stream does
    let guard_stream = futures::stream::once(async move {
        let _guard = _guard;
        // This stream item is never yielded; it just keeps the guard alive
        futures::future::pending::<Result<Event, Infallible>>().await
    });

    // Create the final stream
    let stream = connected_event.chain(event_stream).chain(guard_stream);

    // Return SSE response with keepalive
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    ))
}

/// Dry-run a transaction
///
/// POST /v1/transactions/dry-run
async fn dry_run_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Json(request): Json<CreateTransactionRequest>,
) -> ApiResult<Json<DryRunResponse>> {
    require_auth(
        &state.auth_checker,
        &creds,
        action_for_job_kind(determine_job_kind(&request.operations)),
    )?;

    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

    // Extract package names from operations (placeholder implementation)
    let mut install = Vec::new();
    let mut remove = Vec::new();
    let mut update = Vec::new();

    for op in &request.operations {
        match op {
            TransactionOperation::Install { packages, .. } => {
                install.extend(packages.iter().cloned());
            }
            TransactionOperation::Remove { packages, .. } => {
                remove.extend(packages.iter().cloned());
            }
            TransactionOperation::Update { packages, .. } => {
                update.extend(packages.iter().cloned());
            }
        }
    }

    let total_affected = install.len() + remove.len() + update.len();

    let response = DryRunResponse {
        operations: request.operations,
        summary: DryRunSummary {
            install,
            remove,
            update,
            total_affected,
        },
    };

    Ok(Json(response))
}

// =============================================================================
// Package Handlers
// =============================================================================

/// List installed packages
///
/// GET /v1/packages
async fn list_packages_handler(
    State(state): State<SharedState>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let troves = run_db_query(&state, Trove::list_all).await?;
    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

/// Get package details
///
/// GET /v1/packages/:name
async fn get_package_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<PackageDetails>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        Ok(trove.map(|t| {
            let deps = if let Some(id) = t.id {
                DependencyEntry::find_by_trove(conn, id).unwrap_or_default()
            } else {
                vec![]
            };
            (t, deps)
        }))
    })
    .await?;

    let (trove, deps) = result.ok_or_else(|| not_found_error("package", &name))?;
    let details = PackageDetails {
        name: trove.name,
        version: trove.version,
        package_type: trove.trove_type.as_str().to_string(),
        architecture: trove.architecture,
        description: trove.description,
        installed_at: trove.installed_at,
        install_source: trove.install_source.as_str().to_string(),
        install_reason: trove.install_reason.as_str().to_string(),
        selection_reason: trove.selection_reason,
        flavor: trove.flavor_spec,
        pinned: trove.pinned,
        dependencies: deps.iter().map(DependencyInfo::from).collect(),
    };
    Ok(Json(details))
}

/// Get package files
///
/// GET /v1/packages/:name/files
async fn get_package_files_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<String>>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        match trove {
            Some(t) => {
                let trove_id =
                    t.id.ok_or_else(|| conary_core::Error::NotFound("Package has no ID".to_string()))?;
                let mut stmt =
                    conn.prepare("SELECT path FROM files WHERE trove_id = ?1 ORDER BY path")?;
                let files: Vec<String> = stmt
                    .query_map([trove_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Some(files))
            }
            None => Ok(None),
        }
    })
    .await?;

    result
        .map(Json)
        .ok_or_else(|| not_found_error("package", &name))
}

type TransactionResult = ApiResult<(
    StatusCode,
    [(axum::http::header::HeaderName, String); 1],
    Json<CreateTransactionResponse>,
)>;

async fn forward_package_operation(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    request: PackageOperationRequest,
    require_packages: bool,
    to_operation: impl FnOnce(PackageOperationRequest) -> TransactionOperation,
) -> TransactionResult {
    if require_packages && request.packages.is_empty() {
        return Err(bad_request_error("At least one package name is required"));
    }

    let tx_request = CreateTransactionRequest {
        operations: vec![to_operation(request)],
    };

    create_transaction_handler(state, creds, headers, Json(tx_request)).await
}

/// POST /v1/packages/install
async fn install_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, true, |r| {
        TransactionOperation::Install {
            packages: r.packages,
            allow_downgrade: r.options.allow_downgrade,
            skip_deps: r.options.skip_deps,
        }
    })
    .await
}

/// POST /v1/packages/remove
async fn remove_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, true, |r| {
        TransactionOperation::Remove {
            packages: r.packages,
            cascade: r.options.cascade,
            remove_orphans: r.options.remove_orphans,
        }
    })
    .await
}

/// POST /v1/packages/update (empty packages = update all)
async fn update_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, false, |r| {
        TransactionOperation::Update {
            packages: r.packages,
            security_only: r.options.security_only,
        }
    })
    .await
}

// =============================================================================
// Query Handlers
// =============================================================================

/// Search installed packages
///
/// GET /v1/search?q=<query>
///
/// Searches installed packages by name pattern (supports % and _ wildcards).
async fn search_handler(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let query = params.q.unwrap_or_default();

    let troves = run_db_query(&state, move |conn| {
        let pattern = if query.is_empty() {
            "%".to_string()
        } else {
            format!("%{}%", query)
        };

        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, \
             installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, \
             selection_reason, label_id \
             FROM troves WHERE name LIKE ?1 ORDER BY name, version",
        )?;

        let troves: Vec<Trove> = stmt
            .query_map([pattern], Trove::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(troves)
    })
    .await?;

    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

/// Get package dependencies
///
/// GET /v1/depends/:name
///
/// Returns all dependencies of the specified package.
async fn depends_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<DependencyInfo>>> {
    let pkg_name = name.clone();

    let result = run_db_query(&state, move |conn| {
        let trove = Trove::find_one_by_name(conn, &pkg_name)?;
        Ok(trove.map(|t| {
            if let Some(id) = t.id {
                DependencyEntry::find_by_trove(conn, id).unwrap_or_default()
            } else {
                vec![]
            }
        }))
    })
    .await?;

    let deps = result.ok_or_else(|| not_found_error("package", &name))?;
    Ok(Json(deps.iter().map(DependencyInfo::from).collect()))
}

/// Get reverse dependencies
///
/// GET /v1/rdepends/:name
///
/// Returns all packages that depend on the specified package.
async fn rdepends_handler(
    State(state): State<SharedState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let pkg_name = name.clone();

    let troves = run_db_query(&state, move |conn| {
        let dep_entries = DependencyEntry::find_dependents(conn, &pkg_name)?;
        let mut troves = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for dep in dep_entries {
            if !seen_ids.contains(&dep.trove_id)
                && let Some(trove) = Trove::find_by_id(conn, dep.trove_id)?
            {
                seen_ids.insert(dep.trove_id);
                troves.push(trove);
            }
        }
        Ok(troves)
    })
    .await?;

    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

/// Get transaction history
///
/// GET /v1/history
///
/// Returns the history of all changesets (transactions).
async fn history_handler(State(state): State<SharedState>) -> ApiResult<Json<Vec<HistoryEntry>>> {
    let changesets = run_db_query(&state, Changeset::list_all).await?;
    let history: Vec<HistoryEntry> = changesets.iter().map(HistoryEntry::from).collect();
    Ok(Json(history))
}

// =============================================================================
// System Handlers (Stubs)
// =============================================================================

/// List system states
///
/// GET /v1/system/states
async fn list_states_handler(State(_state): State<SharedState>) -> ApiResult<Json<Vec<()>>> {
    Ok(Json(vec![]))
}

/// POST /v1/system/rollback
async fn rollback_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    require_auth(&state.auth_checker, &creds, Action::Rollback)?;
    Err(not_implemented_error("Rollback not yet implemented"))
}

/// POST /v1/system/verify
async fn verify_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_auth(&state.auth_checker, &creds, Action::Verify)?;
    Err(not_implemented_error(
        "System verification not yet implemented",
    ))
}

/// POST /v1/system/gc
async fn gc_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_auth(&state.auth_checker, &creds, Action::GarbageCollect)?;
    Err(not_implemented_error(
        "Garbage collection not yet implemented",
    ))
}

/// Global event stream (SSE)
///
/// GET /v1/events
///
/// Streams all daemon events in real-time using Server-Sent Events.
/// Events include job progress, package changes, and system state updates.
///
/// The stream will:
/// - Send a "connected" event immediately on connection
/// - Send keepalive comments every 30 seconds
/// - Stream all daemon events until the client disconnects
async fn events_handler(
    State(state): State<SharedState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Track SSE connection (guard decrements on drop when stream ends)
    state
        .metrics
        .sse_connections
        .fetch_add(1, Ordering::Relaxed);
    let _guard = SseConnectionGuard {
        metrics: state.clone(),
    };

    // Subscribe to the event broadcast channel
    let rx = state.subscribe();

    // Create a stream from the broadcast receiver
    let event_stream = BroadcastStream::new(rx).filter_map(|result| {
        // Filter out lagged messages and convert to SSE events
        match result {
            Ok(event) => {
                // Serialize the event to JSON
                match serde_json::to_string(&event) {
                    Ok(json) => Some(Ok(Event::default()
                        .event(event.event_type_name())
                        .data(json))),
                    Err(_) => None,
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                // Client fell behind, send a warning event
                tracing::warn!("SSE client lagged {} events", n);
                Some(Ok(Event::default()
                    .event("warning")
                    .data(format!(r#"{{"lagged": {}}}"#, n))))
            }
        }
    });

    // Prepend a "connected" event
    let connected_event = stream::once(async {
        Ok(Event::default()
            .event("connected")
            .data(r#"{"status": "connected"}"#))
    });

    // Move guard into the stream so it lives as long as the stream does
    let guard_stream = futures::stream::once(async move {
        let _guard = _guard;
        futures::future::pending::<Result<Event, Infallible>>().await
    });

    // Create the final stream
    let stream = connected_event.chain(event_stream).chain(guard_stream);

    // Return SSE response with keepalive
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    )
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
            pid: 1234,
            uptime_secs: 100,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("1234"));
    }

    #[test]
    fn test_version_response_serialization() {
        let resp = VersionResponse {
            version: "0.2.0",
            api_version: "1.0",
            schema_version: 35,
            build_date: None,
            git_commit: None,
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("0.2.0"));
        assert!(json.contains("35"));
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
        assert!(json["pid"].is_number());
        assert!(json["uptime_secs"].is_number());
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
        assert!(json["schema_version"].is_number());
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
        // Root credentials required for mutating operations
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

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        // Should have a Location header
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.starts_with("/v1/transactions/"));

        let json = body_json(response).await;
        assert!(!json["job_id"].as_str().unwrap().is_empty());
        assert_eq!(json["status"], "queued");
        assert!(
            json["location"]
                .as_str()
                .unwrap()
                .starts_with("/v1/transactions/")
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

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["curl"]
                }
            ]
        });
        let body_str = serde_json::to_string(&body).unwrap();

        // First request creates the job
        let app1 = test_router(state.clone(), root_creds);
        let request1 = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str.clone()))
            .unwrap();

        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);
        let json1 = body_json(response1).await;
        let first_job_id = json1["job_id"].as_str().unwrap().to_string();

        // Second request with same key returns existing job (200 OK, not 202)
        let app2 = test_router(state, root_creds);
        let request2 = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str))
            .unwrap();

        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
        let json2 = body_json(response2).await;
        assert_eq!(json2["job_id"].as_str().unwrap(), first_job_id);
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

        // Create a transaction first
        let body = serde_json::json!({
            "operations": [
                {
                    "type": "remove",
                    "packages": ["vim"]
                }
            ]
        });

        let app1 = test_router(state.clone(), root_creds);
        let create_req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let create_resp = app1.oneshot(create_req).await.unwrap();
        assert_eq!(create_resp.status(), StatusCode::ACCEPTED);
        let create_json = body_json(create_resp).await;
        let job_id = create_json["job_id"].as_str().unwrap().to_string();

        // Now fetch the transaction details
        let app2 = test_router(state, root_creds);
        let get_req = axum::http::Request::builder()
            .uri(format!("/v1/transactions/{}", job_id))
            .body(Body::empty())
            .unwrap();

        let get_resp = app2.oneshot(get_req).await.unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);

        let details = body_json(get_resp).await;
        assert_eq!(details["id"].as_str().unwrap(), job_id);
        assert_eq!(details["kind"], "remove");
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

        // Create a transaction so there's something to list
        let body = serde_json::json!({
            "operations": [
                {
                    "type": "update",
                    "packages": ["bash"]
                }
            ]
        });

        let app1 = test_router(state.clone(), root_creds);
        let create_req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let create_resp = app1.oneshot(create_req).await.unwrap();
        assert_eq!(create_resp.status(), StatusCode::ACCEPTED);

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

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let json = body_json(response).await;
        assert!(!json["job_id"].as_str().unwrap().is_empty());
        assert_eq!(json["status"], "queued");
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

        // Update with empty packages means "update all" -- should be accepted
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

        assert_eq!(response.status(), StatusCode::ACCEPTED);
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

    // -- Auth gate middleware: GET passes without auth -------------------------

    #[tokio::test]
    async fn test_auth_gate_allows_get_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        // GET requests should pass through the middleware even without credentials
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "GET without credentials should pass through auth gate middleware"
        );
    }
}
