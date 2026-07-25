// apps/conaryd/src/daemon/auth.rs

//! Authentication and authorization for the daemon
//!
//! Provides:
//! - Peer credential extraction (SO_PEERCRED)
//! - Permission checking (root, daemon identity, and configured socket group)
//! - Audit logging
//!
//! # Security Model
//!
//! The daemon enforces the following security model:
//!
//! - **Root users** (UID 0): Full access to all operations
//! - **Daemon identity**: Access to daemon-owned local API operations
//! - **Configured socket group**: Full access after primary or supplementary
//!   group membership is verified against the live peer process
//! - **Other users and non-Unix transports**: No daemon API access

use std::io;
use std::os::unix::net::UnixStream;

/// Peer credentials from a Unix socket connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    /// Process ID of the peer
    pub pid: u32,
    /// User ID of the peer
    pub uid: u32,
    /// Group ID of the peer
    pub gid: u32,
}

impl PeerCredentials {
    /// Extract peer credentials from a Unix stream
    ///
    /// Uses `SO_PEERCRED` socket option to get the UID/GID of the connected process.
    pub fn from_stream(stream: &UnixStream) -> io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        Self::from_raw_fd(stream.as_raw_fd())
    }

    /// Extract peer credentials from a raw file descriptor via `SO_PEERCRED`.
    ///
    /// This is the shared implementation used by both the blocking
    /// (`std::os::unix::net::UnixStream`) and async (`tokio::net::UnixStream`)
    /// code paths.
    #[cfg(target_os = "linux")]
    pub fn from_raw_fd(fd: std::os::unix::io::RawFd) -> io::Result<Self> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };

        if result == -1 {
            return Err(io::Error::last_os_error());
        }

        #[allow(clippy::unnecessary_cast)]
        Ok(PeerCredentials {
            pid: cred.pid as u32,
            uid: cred.uid as u32,
            gid: cred.gid as u32,
        })
    }

    /// Extract peer credentials from a raw file descriptor (unsupported platform stub).
    #[cfg(not(target_os = "linux"))]
    pub fn from_raw_fd(_fd: std::os::unix::io::RawFd) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Peer credentials not supported on this platform",
        ))
    }

    /// Check if the peer is running as root
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }

    /// Check if the peer matches the daemon's own identity.
    ///
    /// Daemon API access is reserved for root and the daemon service user even
    /// when the Unix socket is group-accessible.
    pub fn matches_daemon_identity(&self, daemon_uid: u32) -> bool {
        self.is_root() || self.uid == daemon_uid
    }

    /// Check that the live process behind this PID still matches the
    /// credentials captured when the socket was accepted.
    pub fn matches_current_process_identity(&self) -> bool {
        match Self::read_proc_status(self.pid) {
            Ok((proc_uid, _)) if proc_uid == self.uid => true,
            Ok((proc_uid, _)) => {
                tracing::warn!(
                    pid = self.pid,
                    expected_uid = self.uid,
                    current_uid = proc_uid,
                    "Daemon peer credential revalidation failed: PID identity changed"
                );
                false
            }
            Err(err) => {
                tracing::warn!(
                    pid = self.pid,
                    uid = self.uid,
                    error = %err,
                    "Daemon peer credential revalidation failed: PID no longer available"
                );
                false
            }
        }
    }

    /// Check if the peer's primary or supplementary groups contain any of the given GIDs
    ///
    /// Reads supplementary groups from `/proc/{pid}/status` with UID
    /// cross-validation against SO_PEERCRED to detect PID reuse races.
    pub(crate) fn has_any_gid(&self, gids: &[u32]) -> bool {
        if gids.is_empty() {
            return false;
        }

        if gids.contains(&self.gid) {
            return true;
        }

        let Ok((proc_uid, supplementary)) = Self::read_proc_status(self.pid) else {
            return false;
        };

        if proc_uid != self.uid {
            tracing::warn!(
                "PID reuse detected: SO_PEERCRED uid={} but /proc/{}/status uid={}. \
                 Denying supplementary group check.",
                self.uid,
                self.pid,
                proc_uid
            );
            return false;
        }

        supplementary.iter().any(|gid| gids.contains(gid))
    }

    /// Read UID and supplementary group IDs from `/proc/{pid}/status`
    ///
    /// Returns (real_uid, supplementary_groups). The UID is cross-validated
    /// against SO_PEERCRED to detect PID reuse races.
    fn read_proc_status(pid: u32) -> std::io::Result<(u32, Vec<u32>)> {
        let status_path = format!("/proc/{}/status", pid);
        let contents = std::fs::read_to_string(status_path)?;

        let mut uid: Option<u32> = None;
        let mut groups = Vec::new();

        for line in contents.lines() {
            if let Some(uid_str) = line.strip_prefix("Uid:") {
                if let Some(real_uid) = uid_str.split_whitespace().next() {
                    uid = real_uid.parse::<u32>().ok();
                }
            } else if let Some(gids_str) = line.strip_prefix("Groups:") {
                groups = gids_str
                    .split_whitespace()
                    .filter_map(|s| s.parse::<u32>().ok())
                    .collect();
            }
        }

        Ok((uid.unwrap_or(u32::MAX), groups))
    }
}

/// Permission level for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    /// Denied
    Denied,
    /// Full access (all operations)
    Full,
}

/// Actions that require authorization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Query operations (list packages, search, etc.)
    Query,
    /// Install packages
    Install,
    /// Remove packages
    Remove,
    /// Update packages
    Update,
    /// Background capability/metadata enhancement of converted packages
    Enhance,
    /// Cancel a job
    CancelJob,
}

/// Authorization checker
pub struct AuthChecker {
    /// Exact GIDs authorized by the Unix socket contract.
    trusted_gids: Vec<u32>,
}

impl Default for AuthChecker {
    fn default() -> Self {
        Self {
            trusted_gids: Vec::new(),
        }
    }
}

impl AuthChecker {
    /// Create a new authorization checker
    pub fn new() -> Self {
        Self::default()
    }

    /// Authorize members of an exact configured Unix socket group.
    pub fn add_trusted_gid(mut self, gid: u32) -> Self {
        self.trusted_gids.push(gid);
        self.trusted_gids.sort_unstable();
        self.trusted_gids.dedup();
        self
    }

    /// Check permission for an action
    pub fn check(&self, creds: &PeerCredentials, _action: Action) -> Permission {
        // Root always gets full access
        if creds.is_root() {
            return Permission::Full;
        }

        // The daemon service identity is trusted to operate its own API.
        if creds.matches_daemon_identity(nix::unistd::geteuid().as_raw()) {
            return Permission::Full;
        }

        // The configured socket group is the complete non-root authorization
        // contract. Check primary and supplementary groups with PID/UID
        // revalidation in `has_any_gid`.
        if creds.has_any_gid(&self.trusted_gids) {
            return Permission::Full;
        }

        Permission::Denied
    }

    /// Check if an action is allowed (convenience method)
    pub fn is_allowed(&self, creds: &PeerCredentials, action: Action) -> bool {
        self.check(creds, action) == Permission::Full
    }
}

/// Audit log entry
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Peer credentials
    pub credentials: PeerCredentials,
    /// Action attempted
    pub action: Action,
    /// Whether the action was allowed
    pub allowed: bool,
    /// Additional details
    pub details: Option<String>,
}

impl AuditEntry {
    /// Create a new audit entry
    pub fn new(credentials: PeerCredentials, action: Action, allowed: bool) -> Self {
        Self {
            timestamp: chrono::Utc::now(),
            credentials,
            action,
            allowed,
            details: None,
        }
    }

    /// Add details to the entry
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Format as log message
    pub fn to_log_message(&self) -> String {
        let allowed_str = if self.allowed { "ALLOWED" } else { "DENIED" };
        let details_str = self.details.as_deref().unwrap_or("");

        format!(
            "[{}] {} {:?} uid={} gid={} pid={} {}",
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            allowed_str,
            self.action,
            self.credentials.uid,
            self.credentials.gid,
            self.credentials.pid,
            details_str
        )
    }
}

/// Audit logger
pub struct AuditLogger {
    /// Log entries (in-memory for now)
    entries: Vec<AuditEntry>,
    /// Maximum number of entries to keep
    max_entries: usize,
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 10000,
        }
    }
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum number of entries to keep
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// Log an audit entry
    pub fn log(&mut self, entry: AuditEntry) {
        // Log to system logger
        let msg = entry.to_log_message();
        if entry.allowed {
            log::info!("AUDIT: {}", msg);
        } else {
            log::warn!("AUDIT: {}", msg);
        }

        // Keep in-memory history
        self.entries.push(entry);

        // Trim if over limit
        if self.entries.len() > self.max_entries {
            let drain_count = self.entries.len() - self.max_entries;
            self.entries.drain(0..drain_count);
        }
    }

    /// Log an action check
    pub fn log_action(
        &mut self,
        credentials: PeerCredentials,
        action: Action,
        allowed: bool,
        details: Option<&str>,
    ) {
        let mut entry = AuditEntry::new(credentials, action, allowed);
        if let Some(d) = details {
            entry = entry.with_details(d);
        }
        self.log(entry);
    }

    /// Get recent audit entries
    pub fn recent_entries(&self, count: usize) -> &[AuditEntry] {
        let start = self.entries.len().saturating_sub(count);
        &self.entries[start..]
    }

    /// Get all entries
    pub fn all_entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_non_daemon_user() -> PeerCredentials {
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let synthetic_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        PeerCredentials {
            // Use the current PID so supplementary-group checks fail closed on
            // UID mismatch instead of depending on some unrelated live process.
            pid: std::process::id(),
            uid: synthetic_uid,
            gid: synthetic_uid,
        }
    }

    #[test]
    fn test_peer_credentials_is_root() {
        let root = PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        };
        assert!(root.is_root());

        let user = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        };
        assert!(!user.is_root());
    }

    #[test]
    fn test_peer_credentials_matches_daemon_identity() {
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let daemon_user = PeerCredentials {
            pid: 1000,
            uid: daemon_uid,
            gid: daemon_uid,
        };
        let root = PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        };
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };
        let other_user = PeerCredentials {
            pid: 2000,
            uid: other_uid,
            gid: other_uid,
        };

        assert!(daemon_user.matches_daemon_identity(daemon_uid));
        assert!(root.matches_daemon_identity(daemon_uid));
        assert!(!other_user.matches_daemon_identity(daemon_uid));
    }

    #[test]
    fn test_peer_credentials_matches_current_process_identity() {
        let creds = PeerCredentials {
            pid: std::process::id(),
            uid: nix::unistd::getuid().as_raw(),
            gid: nix::unistd::getgid().as_raw(),
        };

        assert!(creds.matches_current_process_identity());
    }

    #[test]
    fn test_peer_credentials_rejects_changed_process_identity() {
        let actual_uid = nix::unistd::getuid().as_raw();
        let fake_uid = actual_uid.wrapping_add(1);
        let creds = PeerCredentials {
            pid: std::process::id(),
            uid: fake_uid,
            gid: nix::unistd::getgid().as_raw(),
        };

        assert!(!creds.matches_current_process_identity());
    }

    #[test]
    fn test_read_proc_status_current_process() {
        let pid = std::process::id();
        let result = PeerCredentials::read_proc_status(pid);
        assert!(result.is_ok());
        let (uid, _groups) = result.unwrap();
        assert_eq!(uid, nix::unistd::getuid().as_raw());
    }

    #[test]
    fn test_read_proc_status_nonexistent_pid() {
        let result = PeerCredentials::read_proc_status(u32::MAX);
        assert!(result.is_err());
    }

    #[test]
    fn test_pid_reuse_detection() {
        let pid = std::process::id();
        let actual_uid = nix::unistd::getuid().as_raw();
        let fake_uid = actual_uid.wrapping_add(1);
        let result = PeerCredentials::read_proc_status(pid);
        assert!(result.is_ok());
        let (proc_uid, _) = result.unwrap();
        assert_ne!(proc_uid, fake_uid);
    }

    #[test]
    fn test_auth_checker_root() {
        let checker = AuthChecker::new();
        let root = PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        };

        assert_eq!(checker.check(&root, Action::Query), Permission::Full);
        assert_eq!(checker.check(&root, Action::Install), Permission::Full);
        assert_eq!(checker.check(&root, Action::Remove), Permission::Full);
    }

    #[test]
    fn test_auth_checker_daemon_identity() {
        let checker = AuthChecker::new();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let daemon_user = PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        };

        assert_eq!(checker.check(&daemon_user, Action::Query), Permission::Full);
        assert_eq!(
            checker.check(&daemon_user, Action::Install),
            Permission::Full
        );
        assert_eq!(
            checker.check(&daemon_user, Action::CancelJob),
            Permission::Full
        );
    }

    #[test]
    fn test_default_checker_does_not_trust_undeclared_gids() {
        let checker = AuthChecker::new();

        for gid in [10, 27] {
            let user = PeerCredentials {
                pid: std::process::id(),
                uid: synthetic_non_daemon_user().uid,
                gid,
            };

            assert_eq!(checker.check(&user, Action::Query), Permission::Denied);
            assert_eq!(checker.check(&user, Action::Install), Permission::Denied);
            assert_eq!(checker.check(&user, Action::CancelJob), Permission::Denied);
        }
    }

    #[test]
    fn test_configured_socket_gid_grants_full_access() {
        let user = synthetic_non_daemon_user();
        let checker = AuthChecker::new().add_trusted_gid(user.gid);

        assert_eq!(checker.check(&user, Action::Query), Permission::Full);
        assert_eq!(checker.check(&user, Action::Install), Permission::Full);
    }

    #[test]
    fn test_auth_checker_regular_user() {
        let checker = AuthChecker::new();
        let user = synthetic_non_daemon_user();

        assert_eq!(checker.check(&user, Action::Query), Permission::Denied);
        assert_eq!(checker.check(&user, Action::Install), Permission::Denied);
    }

    #[test]
    fn test_audit_entry() {
        let creds = PeerCredentials {
            pid: 1234,
            uid: 1000,
            gid: 1000,
        };
        let entry = AuditEntry::new(creds, Action::Install, true).with_details("installed nginx");

        let msg = entry.to_log_message();
        assert!(msg.contains("ALLOWED"));
        assert!(msg.contains("Install"));
        assert!(msg.contains("uid=1000"));
        assert!(msg.contains("installed nginx"));
    }

    #[test]
    fn test_audit_logger() {
        let mut logger = AuditLogger::new().with_max_entries(5);

        let creds = PeerCredentials {
            pid: 1234,
            uid: 0,
            gid: 0,
        };

        // Log some entries
        for i in 0..10 {
            logger.log_action(creds, Action::Query, true, Some(&format!("query {}", i)));
        }

        // Should only keep last 5
        assert_eq!(logger.all_entries().len(), 5);

        // Recent entries
        let recent = logger.recent_entries(3);
        assert_eq!(recent.len(), 3);
    }
}
