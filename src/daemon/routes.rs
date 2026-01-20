// src/daemon/routes.rs

//! Axum router configuration for conaryd
//!
//! Defines all HTTP routes for the daemon REST API:
//! - `/health` - Health check endpoint
//! - `/v1/version` - API version info
//! - `/v1/transactions` - Transaction operations
//! - `/v1/packages` - Package queries and operations
//! - `/v1/events` - SSE event stream

use crate::daemon::{DaemonError, DaemonEvent, DaemonJob, DaemonState, JobStatus};
use crate::db::models::{Changeset, DependencyEntry, Trove};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
    routing::{delete, get, post},
    Router,
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// Shared daemon state type
pub type SharedState = Arc<DaemonState>;

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
pub struct ApiError(DaemonError);

impl From<DaemonError> for ApiError {
    fn from(err: DaemonError) -> Self {
        ApiError(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let body = Json(&self.0);

        (
            status,
            [("content-type", "application/problem+json")],
            body,
        )
            .into_response()
    }
}

/// Result type for API handlers
pub type ApiResult<T> = Result<T, ApiError>;

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
        let kind = serde_json::to_string(&job.kind)
            .unwrap_or_else(|_| "unknown".to_string())
            .trim_matches('"')
            .to_string();
        let status = serde_json::to_string(&job.status)
            .unwrap_or_else(|_| "unknown".to_string())
            .trim_matches('"')
            .to_string();

        Self {
            id: job.id.clone(),
            kind,
            status,
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
        let kind = serde_json::to_string(&job.kind)
            .unwrap_or_else(|_| "unknown".to_string())
            .trim_matches('"')
            .to_string();
        let status = serde_json::to_string(&job.status)
            .unwrap_or_else(|_| "unknown".to_string())
            .trim_matches('"')
            .to_string();

        Self {
            id: job.id.clone(),
            idempotency_key: job.idempotency_key.clone(),
            kind,
            status,
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
        .nest("/v1", build_v1_router())
        .with_state(state)
}

/// Build the v1 API router
fn build_v1_router() -> Router<SharedState> {
    Router::new()
        // Version info
        .route("/version", get(version_handler))
        // Metrics (Prometheus format)
        .route("/metrics", get(metrics_handler))
        // Transactions
        .route("/transactions", get(list_transactions_handler))
        .route("/transactions", post(create_transaction_handler))
        .route("/transactions/:id", get(get_transaction_handler))
        .route("/transactions/:id", delete(cancel_transaction_handler))
        .route("/transactions/:id/stream", get(transaction_stream_handler))
        .route("/transactions/dry-run", post(dry_run_handler))
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
}

// =============================================================================
// Health & Version Handlers
// =============================================================================

/// Health check endpoint
///
/// GET /health
///
/// Returns health status. Used by systemd watchdog and load balancers.
async fn health_handler(State(_state): State<SharedState>) -> Json<HealthResponse> {
    // Calculate uptime (approximation - would need to store start time in state)
    let uptime_secs = 0; // TODO: Track actual uptime

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
        schema_version: crate::db::schema::SCHEMA_VERSION,
        build_date: option_env!("BUILD_DATE"),
        git_commit: option_env!("GIT_COMMIT"),
    })
}

/// Metrics endpoint (Prometheus format)
///
/// GET /v1/metrics
async fn metrics_handler(State(state): State<SharedState>) -> String {
    use std::sync::atomic::Ordering;

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
    let state = state.clone();

    let result: Result<Vec<DaemonJob>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        let jobs = match params.status.as_deref() {
            Some("queued") => DaemonJob::list_by_status(&conn, JobStatus::Queued)?,
            Some("running") => DaemonJob::list_by_status(&conn, JobStatus::Running)?,
            Some("completed") => DaemonJob::list_by_status(&conn, JobStatus::Completed)?,
            Some("failed") => DaemonJob::list_by_status(&conn, JobStatus::Failed)?,
            Some("cancelled") => DaemonJob::list_by_status(&conn, JobStatus::Cancelled)?,
            _ => DaemonJob::list_all(&conn, params.limit)?,
        };

        Ok(jobs)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let jobs = result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;
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
    headers: axum::http::HeaderMap,
    Json(request): Json<CreateTransactionRequest>,
) -> ApiResult<(StatusCode, [(axum::http::header::HeaderName, String); 1], Json<CreateTransactionResponse>)> {
    // Validate request
    if request.operations.is_empty() {
        return Err(ApiError(DaemonError::bad_request("At least one operation is required")));
    }

    // Get idempotency key from headers
    let idempotency_key = get_idempotency_key(&headers);

    // Check for existing job with same idempotency key
    if let Some(ref key) = idempotency_key {
        let state_clone = state.clone();
        let key_clone = key.clone();

        let existing: Result<Option<DaemonJob>, crate::Error> = tokio::task::spawn_blocking(move || {
            let conn = state_clone.open_db()?;
            DaemonJob::find_by_idempotency_key(&conn, &key_clone)
        })
        .await
        .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

        if let Ok(Some(existing_job)) = existing {
            // Return the existing job's info
            let location = format!("/v1/transactions/{}", existing_job.id);
            let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);

            let status = serde_json::to_string(&existing_job.status)
                .unwrap_or_else(|_| "unknown".to_string())
                .trim_matches('"')
                .to_string();

            let response = CreateTransactionResponse {
                job_id: existing_job.id,
                status,
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
    let spec = serde_json::to_value(&request.operations)
        .map_err(|e| ApiError(DaemonError::internal(&format!("Serialization error: {}", e))))?;

    let mut job = DaemonJob::new(job_kind, spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }

    let job_id = job.id.clone();

    // Insert into database
    let state_clone = state.clone();
    let insert_job = job.clone();
    let insert_result: Result<(), crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state_clone.open_db()?;
        insert_job.insert(&conn)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    insert_result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;

    // Enqueue the job
    let _cancel_token = state.queue.enqueue(job, crate::daemon::JobPriority::Normal).await;

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
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<Json<TransactionDetails>> {
    let state_clone = state.clone();
    let job_id = id.clone();

    // First check if it's in the queue
    let queue_position = state.queue.position(&id).await;

    let result: Result<Option<DaemonJob>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state_clone.open_db()?;
        DaemonJob::find_by_id(&conn, &job_id)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    match result {
        Ok(Some(job)) => {
            let details = TransactionDetails::from_job(&job, queue_position);
            Ok(Json(details))
        }
        Ok(None) => {
            Err(ApiError(DaemonError::not_found(&format!("transaction '{}'", id))))
        }
        Err(e) => Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
    }
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
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<StatusCode> {
    let state_clone = state.clone();
    let job_id = id.clone();

    // First check if the job exists
    let job_result: Result<Option<DaemonJob>, crate::Error> = tokio::task::spawn_blocking({
        let state = state_clone.clone();
        let id = job_id.clone();
        move || {
            let conn = state.open_db()?;
            DaemonJob::find_by_id(&conn, &id)
        }
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let job = match job_result {
        Ok(Some(j)) => j,
        Ok(None) => return Err(ApiError(DaemonError::not_found(&format!("transaction '{}'", id)))),
        Err(e) => return Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
    };

    // Check if already completed/cancelled/failed
    match job.status {
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
            return Err(ApiError(DaemonError::conflict(
                &format!("Transaction '{}' is already {}", id,
                    match job.status {
                        JobStatus::Completed => "completed",
                        JobStatus::Failed => "failed",
                        JobStatus::Cancelled => "cancelled",
                        _ => "finished",
                    }
                )
            )));
        }
        _ => {}
    }

    // Try to cancel
    let cancelled = state.cancel_job(&job_id).await;

    if cancelled || job.status == JobStatus::Queued {
        // Update database
        let update_result: Result<bool, crate::Error> = tokio::task::spawn_blocking({
            let state = state_clone.clone();
            let id = job_id.clone();
            move || {
                let conn = state.open_db()?;
                DaemonJob::update_status(&conn, &id, JobStatus::Cancelled)
            }
        })
        .await
        .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

        match update_result {
            Ok(true) => {
                // Emit cancellation event
                state.emit(DaemonEvent::JobCancelled { job_id });
                Ok(StatusCode::NO_CONTENT)
            }
            Ok(false) => Err(ApiError(DaemonError::not_found(&format!("transaction '{}'", id)))),
            Err(e) => Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
        }
    } else {
        Err(ApiError(DaemonError::conflict(
            &format!("Cannot cancel transaction '{}' - it may already be completing", id)
        )))
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
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let job_id = id.clone();

    // First verify the job exists
    let state_clone = state.clone();
    let check_id = job_id.clone();
    let job_exists: Result<bool, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state_clone.open_db()?;
        Ok(DaemonJob::find_by_id(&conn, &check_id)?.is_some())
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let exists = job_exists.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;

    if !exists {
        return Err(ApiError(DaemonError::not_found(&format!("transaction '{}'", id))));
    }

    // Track SSE connection
    state.metrics.sse_connections.fetch_add(1, Ordering::Relaxed);

    // Subscribe to the event broadcast channel
    let rx = state.subscribe();
    let filter_job_id = job_id.clone();

    // Create a stream that filters to only this job's events
    let event_stream = BroadcastStream::new(rx)
        .filter_map(move |result| {
            let job_id = filter_job_id.clone();
            // Filter out lagged messages and non-matching job events
            match result {
                Ok(event) => {
                    // Check if this event is for our job
                    let event_job_id = match &event {
                        DaemonEvent::JobQueued { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobStarted { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobPhase { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobProgress { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobCompleted { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobFailed { job_id, .. } => Some(job_id.as_str()),
                        DaemonEvent::JobCancelled { job_id, .. } => Some(job_id.as_str()),
                        // Package and system events are not job-specific
                        _ => None,
                    };

                    // Only include events for this job
                    if event_job_id != Some(job_id.as_str()) {
                        return None;
                    }

                    // Serialize the event to JSON
                    match serde_json::to_string(&event) {
                        Ok(json) => {
                            let event_type = match &event {
                                DaemonEvent::JobQueued { .. } => "job_queued",
                                DaemonEvent::JobStarted { .. } => "job_started",
                                DaemonEvent::JobPhase { .. } => "job_phase",
                                DaemonEvent::JobProgress { .. } => "job_progress",
                                DaemonEvent::JobCompleted { .. } => "job_completed",
                                DaemonEvent::JobFailed { .. } => "job_failed",
                                DaemonEvent::JobCancelled { .. } => "job_cancelled",
                                _ => "event",
                            };
                            Some(Ok(Event::default().event(event_type).data(json)))
                        }
                        Err(_) => None,
                    }
                }
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    log::warn!("SSE client (job {}) lagged {} events", job_id, n);
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

    // Create the final stream
    let stream = connected_event.chain(event_stream);

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
    State(_state): State<SharedState>,
) -> ApiResult<Json<serde_json::Value>> {
    // TODO: Implement
    Err(ApiError(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        "Dry-run not yet implemented",
    )))
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
    let state = state.clone();

    let result: Result<Vec<Trove>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;
        let troves = Trove::list_all(&conn)?;
        Ok(troves)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let troves = result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;
    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

/// Get package details
///
/// GET /v1/packages/:name
async fn get_package_handler(
    State(state): State<SharedState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<Json<PackageDetails>> {
    let state = state.clone();
    let pkg_name = name.clone();

    let result: Result<(Trove, Vec<DependencyEntry>), crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        // Find the package
        let trove = Trove::find_one_by_name(&conn, &pkg_name)?
            .ok_or_else(|| crate::Error::NotFound(format!("Package '{}' not found", pkg_name)))?;

        // Get its dependencies
        let deps = if let Some(id) = trove.id {
            DependencyEntry::find_by_trove(&conn, id)?
        } else {
            vec![]
        };

        Ok((trove, deps))
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    match result {
        Ok((trove, deps)) => {
            let dep_infos: Vec<DependencyInfo> = deps.iter().map(DependencyInfo::from).collect();
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
                dependencies: dep_infos,
            };
            Ok(Json(details))
        }
        Err(crate::Error::NotFound(_)) => {
            Err(ApiError(DaemonError::not_found(&format!("package '{}'", name))))
        }
        Err(e) => Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
    }
}

/// Get package files
///
/// GET /v1/packages/:name/files
async fn get_package_files_handler(
    State(state): State<SharedState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<Json<Vec<String>>> {
    let state = state.clone();
    let pkg_name = name.clone();

    let result: Result<Vec<String>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        // Find the package
        let trove = Trove::find_one_by_name(&conn, &pkg_name)?
            .ok_or_else(|| crate::Error::NotFound(format!("Package '{}' not found", pkg_name)))?;

        // Get its files
        let trove_id = trove.id.ok_or_else(|| {
            crate::Error::NotFound("Package has no ID".to_string())
        })?;

        let mut stmt = conn.prepare(
            "SELECT path FROM files WHERE trove_id = ?1 ORDER BY path"
        )?;

        let files: Vec<String> = stmt
            .query_map([trove_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(files)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    match result {
        Ok(files) => Ok(Json(files)),
        Err(crate::Error::NotFound(_)) => {
            Err(ApiError(DaemonError::not_found(&format!("package '{}'", name))))
        }
        Err(e) => Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
    }
}

/// Install packages
///
/// POST /v1/packages/install
///
/// Convenience endpoint that creates an install transaction.
/// Equivalent to POST /v1/transactions with an install operation.
async fn install_packages_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> ApiResult<(StatusCode, [(axum::http::header::HeaderName, String); 1], Json<CreateTransactionResponse>)> {
    // Validate request
    if request.packages.is_empty() {
        return Err(ApiError(DaemonError::bad_request("At least one package name is required")));
    }

    // Convert to transaction request
    let tx_request = CreateTransactionRequest {
        operations: vec![TransactionOperation::Install {
            packages: request.packages,
            allow_downgrade: request.options.allow_downgrade,
            skip_deps: request.options.skip_deps,
        }],
    };

    // Forward to transaction handler
    create_transaction_handler(State(state), headers, Json(tx_request)).await
}

/// Remove packages
///
/// POST /v1/packages/remove
///
/// Convenience endpoint that creates a remove transaction.
/// Equivalent to POST /v1/transactions with a remove operation.
async fn remove_packages_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> ApiResult<(StatusCode, [(axum::http::header::HeaderName, String); 1], Json<CreateTransactionResponse>)> {
    // Validate request
    if request.packages.is_empty() {
        return Err(ApiError(DaemonError::bad_request("At least one package name is required")));
    }

    // Convert to transaction request
    let tx_request = CreateTransactionRequest {
        operations: vec![TransactionOperation::Remove {
            packages: request.packages,
            cascade: request.options.cascade,
            remove_orphans: request.options.remove_orphans,
        }],
    };

    // Forward to transaction handler
    create_transaction_handler(State(state), headers, Json(tx_request)).await
}

/// Update packages
///
/// POST /v1/packages/update
///
/// Convenience endpoint that creates an update transaction.
/// Equivalent to POST /v1/transactions with an update operation.
async fn update_packages_handler(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> ApiResult<(StatusCode, [(axum::http::header::HeaderName, String); 1], Json<CreateTransactionResponse>)> {
    // Convert to transaction request
    // Note: empty packages list means "update all"
    let tx_request = CreateTransactionRequest {
        operations: vec![TransactionOperation::Update {
            packages: request.packages,
            security_only: request.options.security_only,
        }],
    };

    // Forward to transaction handler
    create_transaction_handler(State(state), headers, Json(tx_request)).await
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
    let state = state.clone();
    let query = params.q.unwrap_or_default();

    let result: Result<Vec<Trove>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        // Search by name pattern
        let pattern = if query.is_empty() {
            "%".to_string()
        } else {
            format!("%{}%", query)
        };

        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, \
             installed_by_changeset_id, install_source, install_reason, flavor_spec, pinned, \
             selection_reason, label_id \
             FROM troves WHERE name LIKE ?1 ORDER BY name, version"
        )?;

        let troves: Vec<Trove> = stmt
            .query_map([pattern], Trove::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(troves)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let troves = result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;
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
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<Json<Vec<DependencyInfo>>> {
    let state = state.clone();
    let pkg_name = name.clone();

    let result: Result<Vec<DependencyEntry>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        // Find the package
        let trove = Trove::find_one_by_name(&conn, &pkg_name)?
            .ok_or_else(|| crate::Error::NotFound(format!("Package '{}' not found", pkg_name)))?;

        // Get its dependencies
        let deps = if let Some(id) = trove.id {
            DependencyEntry::find_by_trove(&conn, id)?
        } else {
            vec![]
        };

        Ok(deps)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    match result {
        Ok(deps) => {
            let dep_info: Vec<DependencyInfo> = deps.iter().map(DependencyInfo::from).collect();
            Ok(Json(dep_info))
        }
        Err(crate::Error::NotFound(_)) => {
            Err(ApiError(DaemonError::not_found(&format!("package '{}'", name))))
        }
        Err(e) => Err(ApiError(DaemonError::internal(&format!("Database error: {}", e)))),
    }
}

/// Get reverse dependencies
///
/// GET /v1/rdepends/:name
///
/// Returns all packages that depend on the specified package.
async fn rdepends_handler(
    State(state): State<SharedState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> ApiResult<Json<Vec<PackageSummary>>> {
    let state = state.clone();
    let pkg_name = name.clone();

    let result: Result<Vec<Trove>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;

        // Find all dependency entries that reference this package
        let dep_entries = DependencyEntry::find_dependents(&conn, &pkg_name)?;

        // Get the troves that have these dependencies
        let mut troves = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for dep in dep_entries {
            if !seen_ids.contains(&dep.trove_id) {
                if let Some(trove) = Trove::find_by_id(&conn, dep.trove_id)? {
                    seen_ids.insert(dep.trove_id);
                    troves.push(trove);
                }
            }
        }

        Ok(troves)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let troves = result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;
    let packages: Vec<PackageSummary> = troves.iter().map(PackageSummary::from).collect();
    Ok(Json(packages))
}

/// Get transaction history
///
/// GET /v1/history
///
/// Returns the history of all changesets (transactions).
async fn history_handler(
    State(state): State<SharedState>,
) -> ApiResult<Json<Vec<HistoryEntry>>> {
    let state = state.clone();

    let result: Result<Vec<Changeset>, crate::Error> = tokio::task::spawn_blocking(move || {
        let conn = state.open_db()?;
        let changesets = Changeset::list_all(&conn)?;
        Ok(changesets)
    })
    .await
    .map_err(|e| ApiError(DaemonError::internal(&format!("Task join error: {}", e))))?;

    let changesets = result.map_err(|e| ApiError(DaemonError::internal(&format!("Database error: {}", e))))?;
    let history: Vec<HistoryEntry> = changesets.iter().map(HistoryEntry::from).collect();
    Ok(Json(history))
}

// =============================================================================
// System Handlers (Stubs)
// =============================================================================

/// List system states
///
/// GET /v1/system/states
async fn list_states_handler(
    State(_state): State<SharedState>,
) -> ApiResult<Json<Vec<()>>> {
    Ok(Json(vec![]))
}

/// Rollback to a previous state
///
/// POST /v1/system/rollback
async fn rollback_handler(
    State(_state): State<SharedState>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    Err(ApiError(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        "Rollback not yet implemented",
    )))
}

/// Verify system integrity
///
/// POST /v1/system/verify
async fn verify_handler(
    State(_state): State<SharedState>,
) -> ApiResult<Json<serde_json::Value>> {
    Err(ApiError(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        "System verification not yet implemented",
    )))
}

/// Garbage collect unused data
///
/// POST /v1/system/gc
async fn gc_handler(
    State(_state): State<SharedState>,
) -> ApiResult<Json<serde_json::Value>> {
    Err(ApiError(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        "Garbage collection not yet implemented",
    )))
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
    // Track SSE connection
    state.metrics.sse_connections.fetch_add(1, Ordering::Relaxed);

    // Subscribe to the event broadcast channel
    let rx = state.subscribe();

    // Create a stream from the broadcast receiver
    let event_stream = BroadcastStream::new(rx)
        .filter_map(|result| {
            // Filter out lagged messages and convert to SSE events
            match result {
                Ok(event) => {
                    // Serialize the event to JSON
                    match serde_json::to_string(&event) {
                        Ok(json) => {
                            // Get the event type name for the SSE event field
                            let event_type = match &event {
                                DaemonEvent::JobQueued { .. } => "job_queued",
                                DaemonEvent::JobStarted { .. } => "job_started",
                                DaemonEvent::JobPhase { .. } => "job_phase",
                                DaemonEvent::JobProgress { .. } => "job_progress",
                                DaemonEvent::JobCompleted { .. } => "job_completed",
                                DaemonEvent::JobFailed { .. } => "job_failed",
                                DaemonEvent::JobCancelled { .. } => "job_cancelled",
                                DaemonEvent::PackageInstalled { .. } => "package_installed",
                                DaemonEvent::PackageRemoved { .. } => "package_removed",
                                DaemonEvent::StateCreated { .. } => "state_created",
                                DaemonEvent::AutomationCheckComplete { .. } => "automation_check",
                            };
                            Some(Ok(Event::default().event(event_type).data(json)))
                        }
                        Err(_) => None,
                    }
                }
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    // Client fell behind, send a warning event
                    log::warn!("SSE client lagged {} events", n);
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

    // Create the final stream
    let stream = connected_event.chain(event_stream);

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
}
