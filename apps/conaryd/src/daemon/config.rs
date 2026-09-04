// apps/conaryd/src/daemon/config.rs

//! Daemon runtime configuration and canonical defaults.

use super::SystemLock;
use std::path::PathBuf;

/// Daemon configuration.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Path to Unix socket (default: /run/conary/conaryd.sock)
    pub socket_path: PathBuf,
    /// Socket file mode (default: 0o660)
    pub socket_mode: u32,
    /// Exact Unix group authorized to connect and perform daemon operations
    pub socket_group: Option<String>,
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
    /// Exit after idle timeout (for socket activation)
    pub idle_timeout_secs: Option<u64>,
}

impl DaemonConfig {
    pub const DEFAULT_SOCKET_PATH: &'static str = "/run/conary/conaryd.sock";
    pub const DEFAULT_SOCKET_MODE: u32 = 0o660;
    pub const DEFAULT_DB_PATH: &'static str = "/var/lib/conary/conary.db";

    pub fn default_socket_path() -> PathBuf {
        PathBuf::from(Self::DEFAULT_SOCKET_PATH)
    }

    pub fn default_db_path() -> PathBuf {
        PathBuf::from(Self::DEFAULT_DB_PATH)
    }

    /// Create a new configuration with a custom database path.
    pub fn with_db_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.db_path = path.into();
        self
    }

    /// Set the socket path.
    pub fn with_socket_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.socket_path = path.into();
        self
    }

    /// Set idle timeout for socket activation.
    pub fn with_idle_timeout(mut self, secs: u64) -> Self {
        self.idle_timeout_secs = Some(secs);
        self
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: Self::default_socket_path(),
            socket_mode: Self::DEFAULT_SOCKET_MODE,
            socket_group: None,
            db_path: Self::default_db_path(),
            root: PathBuf::from("/"),
            lock_path: PathBuf::from(SystemLock::DEFAULT_PATH),
            max_concurrent_reads: 8,
            enable_automation: true,
            idle_timeout_secs: None,
        }
    }
}
