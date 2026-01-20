// src/daemon/mod.rs

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
pub mod jobs;
pub mod lock;
pub mod routes;
pub mod socket;
pub mod systemd;

use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::broadcast;

pub use auth::{
    Action, AuditEntry, AuditLogger, AuthChecker, PeerCredentials, Permission,
};
pub use client::{DaemonClient, should_forward_to_daemon, try_connect};
pub use jobs::{DaemonJob, JobPriority, OperationQueue, QueuedJob};
pub use lock::SystemLock;
pub use systemd::{
    is_socket_activated, listen_fds, listen_fds_count,
    notify_ready, notify_status, notify_stopping, notify_watchdog,
    IdleTracker, SystemdManager, WatchdogTask,
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
}

/// A job in the queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier
    pub id: JobId,
    /// Client-provided idempotency key (for deduplication)
    pub idempotency_key: Option<String>,
    /// Type of operation
    pub kind: JobKind,
    /// Current status
    pub status: JobStatus,
    /// Job specification (serialized JSON)
    pub spec: serde_json::Value,
    /// Result (if completed)
    pub result: Option<serde_json::Value>,
    /// Error details (if failed)
    pub error: Option<DaemonError>,
    /// UID of requesting user
    pub requested_by_uid: Option<u32>,
    /// Client information (peer creds, socket path)
    pub client_info: Option<String>,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Start timestamp (when execution began)
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Completion timestamp
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
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
        Self::new("not_found", "Not Found", 404, &format!("{} not found", resource))
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
        Self::new("cancelled", "Operation Cancelled", 499, "The operation was cancelled")
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

/// Events emitted by the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Job was queued
    JobQueued {
        job_id: JobId,
        position: usize,
    },
    /// Job execution started
    JobStarted {
        job_id: JobId,
    },
    /// Job phase changed
    JobPhase {
        job_id: JobId,
        phase: String,
    },
    /// Job progress update
    JobProgress {
        job_id: JobId,
        current: u64,
        total: u64,
        message: String,
    },
    /// Job completed successfully
    JobCompleted {
        job_id: JobId,
        duration_ms: u64,
    },
    /// Job failed
    JobFailed {
        job_id: JobId,
        error: DaemonError,
    },
    /// Job was cancelled
    JobCancelled {
        job_id: JobId,
    },
    /// Package was installed
    PackageInstalled {
        name: String,
        version: String,
    },
    /// Package was removed
    PackageRemoved {
        name: String,
        version: String,
    },
    /// System state snapshot created
    StateCreated {
        state_number: i64,
    },
    /// Automation check complete
    AutomationCheckComplete {
        pending_actions: usize,
    },
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
}

impl DaemonState {
    /// Open a database connection
    ///
    /// Creates a new connection to the database. This should be called
    /// from within `spawn_blocking` for async handlers.
    pub fn open_db(&self) -> crate::Result<rusqlite::Connection> {
        rusqlite::Connection::open(&self.db_path)
            .map_err(|e| crate::Error::IoError(format!("Failed to open database: {}", e)))
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

        Self {
            config,
            system_lock,
            queue: OperationQueue::new(),
            event_tx,
            metrics: DaemonMetrics::default(),
            db_path,
        }
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
pub fn get_daemon_pid() -> Option<u32> {
    if is_daemon_running() {
        SystemLock::holder_pid(SystemLock::DEFAULT_PATH)
    } else {
        None
    }
}

/// Run the daemon
///
/// This is the main entry point for the daemon. It:
/// 1. Acquires the system lock
/// 2. Binds to Unix and optionally TCP sockets
/// 3. Sets up the Axum router
/// 4. Runs until shutdown signal
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
            log::info!("Socket activation detected, {} FDs passed", systemd::listen_fds_count());
        }
    }

    // Acquire system lock
    let system_lock = SystemLock::try_acquire(&config.lock_path)?
        .ok_or_else(|| crate::Error::IoError(
            "Another daemon instance is already running".to_string()
        ))?;

    // Write our PID
    system_lock.write_pid()?;
    log::info!("Daemon PID: {}", std::process::id());

    // Create daemon state
    let state = Arc::new(DaemonState::new(config.clone(), system_lock));

    // Build router
    let app = routes::build_router(state.clone());

    // Create socket manager
    let socket_config = socket::SocketConfig {
        unix_path: config.socket_path.clone(),
        unix_mode: config.socket_mode,
        unix_group: config.socket_group.clone(),
        enable_tcp: config.enable_tcp,
        tcp_bind: config.tcp_bind.clone(),
    };

    let mut socket_manager = socket::SocketManager::new(socket_config);
    socket_manager.bind().await?;

    // Notify systemd we're ready
    systemd_manager.notify_ready(Some("conaryd ready for connections"));
    log::info!("Notified systemd: READY");

    // Get Unix listener
    let unix_listener = socket_manager.take_unix_listener()
        .expect("Unix listener should be bound");

    log::info!("Daemon ready, accepting connections");

    // Track active connections for idle timeout
    let active_connections = Arc::new(AtomicU64::new(0));

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

                        // Increment connection counter
                        let conns = active_connections.clone();
                        conns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        let app = app.clone();
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
