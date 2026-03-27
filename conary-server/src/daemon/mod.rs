// conary-server/src/daemon/mod.rs

//! Conary Daemon (conaryd) - REST API for package operations
//!
//! The daemon provides:
//! - Exclusive ownership of the transaction lock
//! - REST API for package operations (install, remove, update)
//! - SSE event streaming for progress updates
//! - Background automation (security updates, orphan cleanup)
//!
//! # Architecture
//!
//! The daemon is the "Guardian of State" - it holds the exclusive write lock
//! for package operations. The CLI checks for a running daemon and forwards
//! commands via the Unix socket.
//!
//! ```text
//! CLI (thin client)                     conaryd
//!      │                                   │
//!      ├─ POST /v1/transactions ──────────►│
//!      │                                   │ holds SystemLock
//!      │◄── Location: /v1/transactions/X ──┤
//!      │                                   │
//!      ├─ GET /v1/transactions/X/stream ──►│
//!      │                                   │
//!      │◄─────── SSE progress events ──────┤
//! ```
//!
//! # Module Structure
//!
//! - `lock` - System-wide exclusive lock
//! - `handlers` - HTTP request handlers
//! - `socket` - Unix socket listener (TODO)
//! - `auth` - Peer credentials and PolicyKit (TODO)
//! - `events` - SSE event streaming (TODO)
//! - `client` - Client for CLI forwarding (TODO)

pub mod auth;
pub mod client;
pub mod enhance;
pub mod jobs;
pub mod lock;
pub mod routes;
pub mod socket;
pub mod systemd;

use conary_core::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::broadcast;

pub use auth::{Action, AuditEntry, AuditLogger, AuthChecker, PeerCredentials, Permission};
pub use client::{DaemonClient, should_forward_to_daemon, try_connect};
pub use enhance::{
    EnhanceJobResult, EnhanceJobSpec, EnhancedPackageResult, enhancement_background_worker,
    execute_enhance_job,
};
pub use jobs::{DaemonJob, JobPriority, OperationQueue, QueuedJob};
pub use lock::SystemLock;
pub use systemd::{
    IdleTracker, SystemdManager, WatchdogTask, is_socket_activated, listen_fds, listen_fds_count,
    notify_ready, notify_status, notify_stopping, notify_watchdog,
};

/// Daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Path to Unix socket (default: /run/conary/conaryd.sock)
    pub socket_path: PathBuf,
    /// Socket file mode (default: 0o660)
    pub socket_mode: u32,
    /// Socket group (default: wheel/sudo)
    pub socket_group: Option<String>,
    /// Enable TCP listener (default: false)
    pub enable_tcp: bool,
    /// TCP bind address (default: 127.0.0.1:7890)
    pub tcp_bind: Option<String>,
    /// Database path
    pub db_path: PathBuf,
    /// Root filesystem path (usually "/")
    pub root: PathBuf,
    /// Path to daemon lock file
    pub lock_path: PathBuf,
    /// Maximum concurrent read operations (writes are always serialized)
    pub max_concurrent_reads: usize,
    /// Enable automation scheduler
    pub enable_automation: bool,
    /// Require PolicyKit for non-root users
    pub require_polkit: bool,
    /// Exit after idle timeout (for socket activation)
    pub idle_timeout_secs: Option<u64>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/conary/conaryd.sock"),
            socket_mode: 0o660,
            socket_group: None, // Will try wheel, then sudo
            enable_tcp: false,
            tcp_bind: Some("127.0.0.1:7890".to_string()),
            db_path: PathBuf::from("/var/lib/conary/conary.db"),
            root: PathBuf::from("/"),
            lock_path: PathBuf::from(SystemLock::DEFAULT_PATH),
            max_concurrent_reads: 8,
            enable_automation: true,
            require_polkit: true,
            idle_timeout_secs: None,
        }
    }
}

impl DaemonConfig {
    /// Create a new configuration with a custom database path
    pub fn with_db_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.db_path = path.into();
        self
    }

    /// Set the socket path
    pub fn with_socket_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.socket_path = path.into();
        self
    }

    /// Enable or disable TCP listener
    pub fn with_tcp(mut self, enable: bool, bind: Option<String>) -> Self {
        self.enable_tcp = enable;
        self.tcp_bind = bind;
        self
    }

    /// Set idle timeout for socket activation
    pub fn with_idle_timeout(mut self, secs: u64) -> Self {
        self.idle_timeout_secs = Some(secs);
        self
    }
}

/// Unique job identifier
pub type JobId = String;

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Job is waiting in queue
    Queued,
    /// Job is currently executing
    Running,
    /// Job completed successfully
    Completed,
    /// Job failed with an error
    Failed,
    /// Job was cancelled
    Cancelled,
}

impl JobStatus {
    /// Return the lowercase string representation (matches serde)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Job kind (type of operation)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Install packages
    Install,
    /// Remove packages
    Remove,
    /// Update packages
    Update,
    /// Dry run (plan without executing)
    DryRun,
    /// System rollback
    Rollback,
    /// System verification
    Verify,
    /// Garbage collection
    GarbageCollect,
    /// Enhance converted packages (background capability inference)
    Enhance,
}

impl JobKind {
    /// Return the snake_case string representation (matches serde)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
            Self::Update => "update",
            Self::DryRun => "dry_run",
            Self::Rollback => "rollback",
            Self::Verify => "verify",
            Self::GarbageCollect => "garbage_collect",
            Self::Enhance => "enhance",
        }
    }
}

/// Error response format (RFC 7807)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonError {
    /// Error type URI
    #[serde(rename = "type")]
    pub error_type: String,
    /// Human-readable title
    pub title: String,
    /// HTTP status code
    pub status: u16,
    /// Detailed description
    pub detail: String,
    /// Instance URI (the request that caused the error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Additional error-specific data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

impl DaemonError {
    /// Create a new daemon error
    pub fn new(error_type: &str, title: &str, status: u16, detail: &str) -> Self {
        Self {
            error_type: format!("urn:conary:error:{}", error_type),
            title: title.to_string(),
            status,
            detail: detail.to_string(),
            instance: None,
            extensions: None,
        }
    }

    /// Not found error
    pub fn not_found(resource: &str) -> Self {
        Self::new(
            "not_found",
            "Not Found",
            404,
            &format!("{} not found", resource),
        )
    }

    /// Conflict error
    pub fn conflict(detail: &str) -> Self {
        Self::new("conflict", "Conflict", 409, detail)
    }

    /// Internal error
    pub fn internal(detail: &str) -> Self {
        Self::new("internal", "Internal Error", 500, detail)
    }

    /// Cancelled error
    pub fn cancelled() -> Self {
        Self::new(
            "cancelled",
            "Operation Cancelled",
            499,
            "The operation was cancelled",
        )
    }

    /// Bad request error
    pub fn bad_request(detail: &str) -> Self {
        Self::new("bad_request", "Bad Request", 400, detail)
    }

    /// Unauthorized error
    pub fn unauthorized(detail: &str) -> Self {
        Self::new("unauthorized", "Unauthorized", 401, detail)
    }

    /// Forbidden error
    pub fn forbidden(detail: &str) -> Self {
        Self::new("forbidden", "Forbidden", 403, detail)
    }

    /// Set the instance URI
    pub fn with_instance(mut self, instance: String) -> Self {
        self.instance = Some(instance);
        self
    }

    /// Add extensions
    pub fn with_extensions(mut self, extensions: serde_json::Value) -> Self {
        self.extensions = Some(extensions);
        self
    }
}

/// Events emitted by the daemon and broadcast over SSE streams.
///
/// Every event is serialized with a `"type"` discriminant field (snake_case)
/// so clients can dispatch on `event.type` without knowing the full schema.
///
/// Job lifecycle events (`JobQueued` → `JobStarted` → `JobCompleted` /
/// `JobFailed` / `JobCancelled`) follow a predictable state machine. Progress
/// and phase events may appear zero or more times in between.
///
/// Enhancement events are independent of the job lifecycle and may be emitted
/// concurrently by the background enhancement worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// A job was accepted and placed in the operation queue.
    JobQueued {
        /// Unique identifier for the job.
        job_id: JobId,
        /// Zero-based position in the queue (0 means next to run).
        position: usize,
    },
    /// A job left the queue and began executing.
    JobStarted {
        /// Unique identifier for the job.
        job_id: JobId,
    },
    /// The job moved to a new named execution phase (e.g. "resolving", "downloading").
    JobPhase {
        /// Unique identifier for the job.
        job_id: JobId,
        /// Human-readable phase name.
        phase: String,
    },
    /// Incremental progress update within the current phase.
    JobProgress {
        /// Unique identifier for the job.
        job_id: JobId,
        /// Number of units completed so far.
        current: u64,
        /// Total units to complete (may be 0 if unknown).
        total: u64,
        /// Human-readable progress message.
        message: String,
    },
    /// A job finished successfully.
    JobCompleted {
        /// Unique identifier for the job.
        job_id: JobId,
        /// Wall-clock elapsed time in milliseconds.
        duration_ms: u64,
    },
    /// A job terminated with an error.
    JobFailed {
        /// Unique identifier for the job.
        job_id: JobId,
        /// Structured RFC 7807 error describing what went wrong.
        error: DaemonError,
    },
    /// A job was cancelled before or during execution.
    JobCancelled {
        /// Unique identifier for the job.
        job_id: JobId,
    },
    /// A package was successfully installed to the system.
    PackageInstalled {
        /// Package name (e.g. "nginx").
        name: String,
        /// Installed version string.
        version: String,
    },
    /// A package was successfully removed from the system.
    PackageRemoved {
        /// Package name (e.g. "nginx").
        name: String,
        /// Removed version string.
        version: String,
    },
    /// A new system state snapshot (generation) was committed to the database.
    StateCreated {
        /// Generation number of the newly created state.
        state_number: i64,
    },
    /// The automation scheduler completed a check cycle.
    AutomationCheckComplete {
        /// Number of deferred actions (e.g. security updates) waiting to run.
        pending_actions: usize,
    },
    /// Background enhancement of a converted package has started.
    EnhancementStarted {
        /// Database trove ID of the package being enhanced.
        trove_id: i64,
        /// Human-readable package name.
        package_name: String,
    },
    /// Incremental progress from the enhancement pipeline.
    EnhancementProgress {
        /// Database trove ID of the package being enhanced.
        trove_id: i64,
        /// Human-readable package name.
        package_name: String,
        /// Number of packages processed so far in the current batch.
        current: u32,
        /// Total packages to process in the current batch.
        total: u32,
        /// Current enhancement phase (e.g. "analyzing", "inferring").
        phase: String,
    },
    /// Enhancement finished successfully for a package.
    EnhancementCompleted {
        /// Database trove ID of the package that was enhanced.
        trove_id: i64,
        /// Human-readable package name.
        package_name: String,
        /// Whether capability metadata was inferred during enhancement.
        capabilities_inferred: bool,
    },
    /// Enhancement failed for a package.
    EnhancementFailed {
        /// Database trove ID of the package that failed enhancement.
        trove_id: i64,
        /// Human-readable package name.
        package_name: String,
        /// Error description from the enhancement pipeline.
        error: String,
    },
}

impl DaemonEvent {
    pub fn job_id(&self) -> Option<&str> {
        match self {
            Self::JobQueued { job_id, .. }
            | Self::JobStarted { job_id, .. }
            | Self::JobPhase { job_id, .. }
            | Self::JobProgress { job_id, .. }
            | Self::JobCompleted { job_id, .. }
            | Self::JobFailed { job_id, .. }
            | Self::JobCancelled { job_id, .. } => Some(job_id),
            _ => None,
        }
    }

    /// Return the SSE event type name for this event
    pub fn event_type_name(&self) -> &'static str {
        match self {
            Self::JobQueued { .. } => "job_queued",
            Self::JobStarted { .. } => "job_started",
            Self::JobPhase { .. } => "job_phase",
            Self::JobProgress { .. } => "job_progress",
            Self::JobCompleted { .. } => "job_completed",
            Self::JobFailed { .. } => "job_failed",
            Self::JobCancelled { .. } => "job_cancelled",
            Self::PackageInstalled { .. } => "package_installed",
            Self::PackageRemoved { .. } => "package_removed",
            Self::StateCreated { .. } => "state_created",
            Self::AutomationCheckComplete { .. } => "automation_check",
            Self::EnhancementStarted { .. } => "enhancement_started",
            Self::EnhancementProgress { .. } => "enhancement_progress",
            Self::EnhancementCompleted { .. } => "enhancement_completed",
            Self::EnhancementFailed { .. } => "enhancement_failed",
        }
    }
}

/// Daemon state (shared across handlers)
pub struct DaemonState {
    /// Configuration
    pub config: DaemonConfig,
    /// System lock (held for daemon lifetime)
    pub system_lock: SystemLock,
    /// Operation queue for managing job execution
    pub queue: OperationQueue,
    /// Event broadcast channel
    pub event_tx: broadcast::Sender<DaemonEvent>,
    /// Metrics
    pub metrics: DaemonMetrics,
    /// Database connection pool (path for on-demand connections)
    db_path: PathBuf,
    /// When the daemon started (for uptime tracking)
    start_time: std::time::Instant,
    /// Pre-built auth checker (respects config.require_polkit)
    pub auth_checker: auth::AuthChecker,
}

impl DaemonState {
    /// Open a database connection with WAL mode and proper pragmas
    ///
    /// Uses `open_fast` to set WAL journal mode, busy_timeout, and foreign_keys.
    /// This should be called from within `spawn_blocking` for async handlers.
    pub fn open_db(&self) -> conary_core::Result<rusqlite::Connection> {
        conary_core::db::open_fast(&self.db_path)
    }
}

/// Daemon metrics
#[derive(Debug, Default)]
pub struct DaemonMetrics {
    /// Total jobs processed
    pub jobs_total: std::sync::atomic::AtomicU64,
    /// Jobs currently running
    pub jobs_running: std::sync::atomic::AtomicU64,
    /// Jobs completed successfully
    pub jobs_completed: std::sync::atomic::AtomicU64,
    /// Jobs failed
    pub jobs_failed: std::sync::atomic::AtomicU64,
    /// Jobs cancelled
    pub jobs_cancelled: std::sync::atomic::AtomicU64,
    /// Active SSE connections
    pub sse_connections: std::sync::atomic::AtomicU64,
}

impl DaemonState {
    /// Create a new daemon state
    ///
    /// # Arguments
    /// * `config` - Daemon configuration
    /// * `system_lock` - Pre-acquired system lock
    pub fn new(config: DaemonConfig, system_lock: SystemLock) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        let db_path = config.db_path.clone();

        // Create AuthChecker once, respecting the config's require_polkit setting.
        // This avoids per-request getgrnam syscalls and ensures disable_polkit is
        // actually wired through.
        let auth_checker = if config.require_polkit {
            auth::AuthChecker::new()
        } else {
            auth::AuthChecker::new().disable_polkit()
        };

        Self {
            config,
            system_lock,
            queue: OperationQueue::new(),
            event_tx,
            metrics: DaemonMetrics::default(),
            db_path,
            start_time: std::time::Instant::now(),
            auth_checker,
        }
    }

    /// Get daemon uptime in seconds
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Broadcast an event to all subscribers
    pub fn emit(&self, event: DaemonEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.event_tx.send(event);
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.event_tx.subscribe()
    }

    /// Request cancellation of a job
    ///
    /// This will either remove the job from the queue (if queued) or
    /// set the cancel token (if running).
    pub async fn cancel_job(&self, job_id: &str) -> bool {
        self.queue.cancel(job_id).await
    }

    /// Get the cancel token for a job
    pub async fn get_cancel_token(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.queue.get_cancel_token(job_id).await
    }
}

/// Check if the daemon is running
pub fn is_daemon_running() -> bool {
    SystemLock::is_held(SystemLock::DEFAULT_PATH)
}

/// Get the PID of the running daemon (if any)
///
/// Returns `Some(pid)` only when the lock is held AND the .pid file exists.
/// `holder_pid` already returns `None` if the file is missing, and
/// `is_daemon_running` would redundantly open the lock file just to
/// probe it, so we skip that check.
pub fn get_daemon_pid() -> Option<u32> {
    SystemLock::holder_pid(SystemLock::DEFAULT_PATH).filter(|_| is_daemon_running())
}

/// Background job executor loop.
///
/// Continuously dequeues jobs from the operation queue, executes them,
/// updates DB status/result/error, and emits lifecycle events. Without
/// this loop, jobs accepted by the API would remain queued forever.
async fn job_executor_loop(state: Arc<DaemonState>) {
    use std::sync::atomic::Ordering;

    log::info!("Job executor started");

    loop {
        // Dequeue the next job (non-blocking check)
        let queued = state.queue.dequeue().await;

        let Some(queued) = queued else {
            // No pending jobs -- sleep briefly before polling again
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        };

        let job = queued.job;
        let cancel_token = queued.cancel_token;
        let job_id = job.id.clone();
        let job_kind = job.kind;

        log::info!("Executing job {} (kind: {})", job_id, job_kind.as_str());

        // Mark as current and update DB status to Running
        state.queue.set_current(Some(job_id.clone())).await;
        let start_time = std::time::Instant::now();

        let update_id = job_id.clone();
        let db_state = state.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            let conn = db_state.open_db().map_err(|e| format!("open_db: {e}"))?;
            DaemonJob::update_status(&conn, &update_id, JobStatus::Running)
                .map_err(|e| format!("update_status Running: {e}"))
        })
        .await
        {
            tracing::error!("Failed to persist Running status for job {job_id}: {e}");
        }

        state.metrics.jobs_running.fetch_add(1, Ordering::Relaxed);
        state.emit(DaemonEvent::JobStarted {
            job_id: job_id.clone(),
        });

        // Execute the job based on its kind
        let result: std::result::Result<Option<serde_json::Value>, String> = match job_kind {
            JobKind::Enhance => {
                let spec: enhance::EnhanceJobSpec =
                    serde_json::from_value(job.spec.clone()).unwrap_or_default();
                match enhance::execute_enhance_job(state.clone(), spec, cancel_token).await {
                    Ok(r) => Ok(serde_json::to_value(r).ok()),
                    Err(e) => Err(e.to_string()),
                }
            }
            // TODO: Implement Install, Remove, Update, Rollback, Verify,
            // GarbageCollect, and DryRun job execution.  These are currently
            // rejected at the API boundary (routes.rs) but this fallback
            // provides defense-in-depth.
            _ => Err(format!(
                "Job kind '{}' execution not yet implemented",
                job_kind.as_str()
            )),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        state.metrics.jobs_running.fetch_sub(1, Ordering::Relaxed);

        // Persist result and emit terminal event
        let final_id = job_id.clone();
        let db_state = state.clone();
        match result {
            Ok(result_value) => {
                let log_id = final_id.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    let conn = db_state.open_db().map_err(|e| format!("open_db: {e}"))?;
                    if let Some(ref val) = result_value {
                        DaemonJob::set_result(&conn, &final_id, val)
                            .map_err(|e| format!("set_result: {e}"))?;
                    }
                    DaemonJob::update_status(&conn, &final_id, JobStatus::Completed)
                        .map_err(|e| format!("update_status Completed: {e}"))
                })
                .await
                {
                    tracing::error!(
                        "Failed to persist Completed status for job {log_id}: {e}"
                    );
                }

                state.metrics.jobs_completed.fetch_add(1, Ordering::Relaxed);
                state.emit(DaemonEvent::JobCompleted {
                    job_id: job_id.clone(),
                    duration_ms,
                });
                log::info!("Job {} completed in {}ms", job_id, duration_ms);
            }
            Err(error_msg) => {
                let daemon_error = DaemonError::internal(&error_msg);
                let err_for_db = daemon_error.clone();
                let log_id = final_id.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    let conn = db_state.open_db().map_err(|e| format!("open_db: {e}"))?;
                    DaemonJob::set_error(&conn, &final_id, &err_for_db)
                        .map_err(|e| format!("set_error: {e}"))?;
                    DaemonJob::update_status(&conn, &final_id, JobStatus::Failed)
                        .map_err(|e| format!("update_status Failed: {e}"))
                })
                .await
                {
                    tracing::error!(
                        "Failed to persist Failed status for job {log_id}: {e}"
                    );
                }

                state.metrics.jobs_failed.fetch_add(1, Ordering::Relaxed);
                state.emit(DaemonEvent::JobFailed {
                    job_id: job_id.clone(),
                    error: daemon_error,
                });
                log::error!("Job {} failed: {}", job_id, error_msg);
            }
        }

        // Clear current job and clean up cancel token
        state.queue.set_current(None).await;
        state.queue.remove_token(&job_id).await;
    }
}

/// Run the daemon
///
/// This is the main entry point for the daemon. It:
/// 1. Acquires the system lock
/// 2. Binds to Unix and optionally TCP sockets
/// 3. Sets up the Axum router
/// 4. Runs the job executor loop
/// 5. Runs until shutdown signal
///
/// # Arguments
/// * `config` - Daemon configuration
///
/// # Returns
/// * `Result<()>` - Ok if daemon shut down cleanly
pub async fn run_daemon(config: DaemonConfig) -> Result<()> {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use hyper_util::service::TowerToHyperService;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    log::info!("Starting conaryd version {}", env!("CARGO_PKG_VERSION"));

    // Create systemd manager
    let idle_timeout = config.idle_timeout_secs.map(Duration::from_secs);
    let mut systemd_manager = SystemdManager::new(idle_timeout);

    if systemd_manager.is_systemd() {
        log::info!("Running under systemd supervision");
        if systemd::is_socket_activated() {
            log::info!(
                "Socket activation detected, {} FDs passed",
                systemd::listen_fds_count()
            );
        }
    }

    // Acquire system lock
    let system_lock = SystemLock::try_acquire(&config.lock_path)?.ok_or_else(|| {
        conary_core::Error::IoError("Another daemon instance is already running".to_string())
    })?;

    // Write our PID
    system_lock.write_pid()?;
    log::info!("Daemon PID: {}", std::process::id());

    // Create daemon state
    let state = Arc::new(DaemonState::new(config.clone(), system_lock));

    // Re-enqueue any jobs that were left in 'queued' state from a previous
    // daemon run (e.g. after a crash or SIGKILL).  Jobs that were 'running'
    // are reset to 'queued' first, since we cannot resume mid-execution.
    // (Gemini fix: re-enqueue stuck jobs on startup)
    {
        let db_path = state.config.db_path.clone();
        match tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open_fast(&db_path)?;
            // Reset any jobs stuck in 'running' (interrupted by previous crash)
            conn.execute(
                "UPDATE daemon_jobs SET status = 'queued', started_at = NULL
                 WHERE status = 'running'",
                [],
            )?;
            jobs::DaemonJob::list_by_status(&conn, JobStatus::Queued, None)
        })
        .await
        {
            Ok(Ok(queued_jobs)) => {
                if !queued_jobs.is_empty() {
                    log::info!(
                        "Re-enqueueing {} job(s) left over from previous daemon run",
                        queued_jobs.len()
                    );
                    for job in queued_jobs {
                        state.queue.enqueue(job, jobs::JobPriority::Normal).await;
                    }
                }
            }
            Ok(Err(e)) => {
                log::warn!("Failed to query stuck jobs on startup: {}", e);
            }
            Err(e) => {
                log::warn!("Startup job scan task panicked: {}", e);
            }
        }
    }

    // Build router
    let app = routes::build_router(state.clone());

    // Acquire a Unix listener -- either from systemd socket activation or
    // by binding a fresh socket ourselves.
    let (_socket_manager, unix_listener) = if systemd::is_socket_activated() {
        let fds = systemd::listen_fds();
        if fds.is_empty() {
            return Err(conary_core::Error::IoError(
                "Socket activation detected but no file descriptors received".to_string(),
            ));
        }

        // Adopt the first passed FD (FD 3) as our Unix listener.
        use std::os::unix::io::FromRawFd;
        let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fds[0]) };
        std_listener.set_nonblocking(true).map_err(|e| {
            conary_core::Error::IoError(format!(
                "Failed to set socket-activated FD non-blocking: {e}"
            ))
        })?;
        let listener = tokio::net::UnixListener::from_std(std_listener).map_err(|e| {
            conary_core::Error::IoError(format!("Failed to adopt socket-activated listener: {e}"))
        })?;
        log::info!("Adopted systemd socket-activated listener (FD {})", fds[0]);
        (None, listener)
    } else {
        let socket_config = socket::SocketConfig {
            unix_path: config.socket_path.clone(),
            unix_mode: config.socket_mode,
            unix_group: config.socket_group.clone(),
            enable_tcp: config.enable_tcp,
            tcp_bind: config.tcp_bind.clone(),
        };

        let mut mgr = socket::SocketManager::new(socket_config);
        mgr.bind().await?;

        let listener = mgr.take_unix_listener().ok_or_else(|| {
            conary_core::Error::IoError(
                "Unix listener not bound - check socket configuration".to_string(),
            )
        })?;
        (Some(mgr), listener)
    };

    // Notify systemd we're ready
    systemd_manager.notify_ready(Some("conaryd ready for connections"));
    log::info!("Notified systemd: READY");

    log::info!("Daemon ready, accepting connections");

    // Track active connections for idle timeout
    let active_connections = Arc::new(AtomicU64::new(0));

    // Spawn the job executor task that drains the operation queue
    let executor_state = state.clone();
    let _executor_handle = tokio::spawn(async move {
        job_executor_loop(executor_state).await;
    });

    // Setup shutdown signal
    let shutdown = tokio::signal::ctrl_c();

    // Calculate tick interval for watchdog and idle timeout
    let tick_interval = systemd_manager.tick_interval();

    // Accept connections with watchdog and idle timeout support
    tokio::select! {
        // Main accept loop
        _ = async {
            loop {
                // Use timeout on accept for periodic housekeeping
                match tokio::time::timeout(tick_interval, unix_listener.accept()).await {
                    Ok(Ok((stream, _addr))) => {
                        // Record activity for idle tracking
                        systemd_manager.activity();

                        // Extract peer credentials from Unix socket (SO_PEERCRED)
                        let peer_creds = socket::get_peer_credentials(&stream);

                        // Increment connection counter
                        let conns = active_connections.clone();
                        conns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        // Add peer credentials as a request extension for this connection
                        let app = app.clone().layer(axum::Extension(peer_creds));
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            // Convert tower service to hyper service
                            let service = TowerToHyperService::new(app);
                            if let Err(err) = http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                log::warn!("Error serving connection: {:?}", err);
                            }
                            // Decrement connection counter
                            conns.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        });
                    }
                    Ok(Err(e)) => {
                        log::error!("Failed to accept connection: {}", e);
                    }
                    Err(_) => {
                        // Timeout - perform housekeeping
                        // Send watchdog ping if due
                        systemd_manager.watchdog_tick();

                        // Check idle timeout
                        if systemd_manager.is_idle_expired() {
                            let conn_count = active_connections.load(std::sync::atomic::Ordering::Relaxed);
                            if conn_count == 0 {
                                log::info!("Idle timeout expired, shutting down");
                                break;
                            }
                        }
                    }
                }
            }
        } => {}
        // Shutdown signal
        _ = shutdown => {
            log::info!("Received shutdown signal");
        }
    }

    // Notify systemd we're stopping
    systemd_manager.notify_stopping();
    log::info!("Daemon shutting down");

    // Graceful drain: wait for in-flight connections to finish (max 10s)
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let conn_count = active_connections.load(std::sync::atomic::Ordering::Relaxed);
        if conn_count == 0 {
            log::info!("All connections drained");
            break;
        }
        if tokio::time::Instant::now() >= drain_deadline {
            log::warn!(
                "Drain timeout: {} connections still active, forcing shutdown",
                conn_count
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Cleanup handled by Drop implementations
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();

        assert_eq!(
            config.socket_path,
            PathBuf::from("/run/conary/conaryd.sock")
        );
        assert_eq!(config.socket_mode, 0o660);
        assert!(!config.enable_tcp);
    }

    #[test]
    fn test_daemon_error_serialization() {
        let error = DaemonError::not_found("package nginx");
        let json = serde_json::to_string(&error).unwrap();

        assert!(json.contains("not_found"));
        assert!(json.contains("nginx"));
    }

    #[test]
    fn test_daemon_event_serialization() {
        let event = DaemonEvent::JobProgress {
            job_id: "test-123".to_string(),
            current: 50,
            total: 100,
            message: "Installing nginx".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();

        assert!(json.contains("job_progress"));
        assert!(json.contains("test-123"));
        assert!(json.contains("Installing nginx"));
    }

    #[test]
    fn test_job_status() {
        let status = JobStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, JobStatus::Running);
    }
}
