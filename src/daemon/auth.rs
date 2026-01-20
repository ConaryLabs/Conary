// src/daemon/auth.rs

//! Authentication and authorization for the daemon
//!
//! Provides:
//! - Peer credential extraction (SO_PEERCRED)
//! - Permission checking (root vs non-root)
//! - PolicyKit integration stub (for future implementation)
//! - Audit logging
//!
//! # Security Model
//!
//! The daemon enforces the following security model:
//!
//! - **Root users** (UID 0): Full access to all operations
//! - **Members of admin groups** (wheel, sudo): Full access (configurable)
//! - **Other users**: Read-only access by default; write access requires PolicyKit
//!
//! # PolicyKit
//!
//! Non-root users can be authorized via PolicyKit for specific operations.
//! This requires the `polkit` feature and installation of a policy file.
//!
//! Policy actions:
//! - `com.conary.daemon.install` - Install packages
//! - `com.conary.daemon.remove` - Remove packages
//! - `com.conary.daemon.update` - Update packages
//! - `com.conary.daemon.rollback` - System rollback

use std::os::unix::net::UnixStream;
use std::io;

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
    /// Uses SO_PEERCRED socket option to get the UID/GID of the connected process.
    pub fn from_stream(stream: &UnixStream) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;

            let fd = stream.as_raw_fd();

            // Use getsockopt with SO_PEERCRED
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

            Ok(PeerCredentials {
                pid: cred.pid as u32,
                uid: cred.uid as u32,
                gid: cred.gid as u32,
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Peer credentials not supported on this platform",
            ))
        }
    }

    /// Check if the peer is running as root
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }

    /// Check if the peer is a member of an admin group
    ///
    /// Checks if the peer's primary GID is wheel (10) or sudo (27).
    pub fn is_admin_group(&self) -> bool {
        // wheel group is typically GID 10
        // sudo group is typically GID 27
        self.gid == 10 || self.gid == 27
    }
}

/// Permission level for an operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    /// Denied
    Denied,
    /// Read-only access (queries, status)
    ReadOnly,
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
    /// System rollback
    Rollback,
    /// System verification
    Verify,
    /// Garbage collection
    GarbageCollect,
    /// Cancel a job
    CancelJob,
}

impl Action {
    /// Get the PolicyKit action ID for this action
    pub fn polkit_action(&self) -> &'static str {
        match self {
            Action::Query => "com.conary.daemon.query",
            Action::Install => "com.conary.daemon.install",
            Action::Remove => "com.conary.daemon.remove",
            Action::Update => "com.conary.daemon.update",
            Action::Rollback => "com.conary.daemon.rollback",
            Action::Verify => "com.conary.daemon.verify",
            Action::GarbageCollect => "com.conary.daemon.gc",
            Action::CancelJob => "com.conary.daemon.cancel",
        }
    }

    /// Check if this is a read-only action
    pub fn is_read_only(&self) -> bool {
        matches!(self, Action::Query)
    }
}

/// Authorization checker
pub struct AuthChecker {
    /// Allow members of admin groups (wheel, sudo) full access
    allow_admin_groups: bool,
    /// Require PolicyKit for non-root write operations
    require_polkit: bool,
    /// Trusted GIDs that get full access
    trusted_gids: Vec<u32>,
}

impl Default for AuthChecker {
    fn default() -> Self {
        Self {
            allow_admin_groups: true,
            require_polkit: true,
            trusted_gids: vec![0, 10, 27], // root, wheel, sudo
        }
    }
}

impl AuthChecker {
    /// Create a new authorization checker
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable admin group access (only root gets full access)
    pub fn disable_admin_groups(mut self) -> Self {
        self.allow_admin_groups = false;
        self
    }

    /// Disable PolicyKit requirement (all authenticated users get full access)
    pub fn disable_polkit(mut self) -> Self {
        self.require_polkit = false;
        self
    }

    /// Add a trusted GID
    pub fn add_trusted_gid(mut self, gid: u32) -> Self {
        self.trusted_gids.push(gid);
        self
    }

    /// Check permission for an action
    pub fn check(&self, creds: &PeerCredentials, action: Action) -> Permission {
        // Root always gets full access
        if creds.is_root() {
            return Permission::Full;
        }

        // Check trusted GIDs
        if self.trusted_gids.contains(&creds.gid) {
            return Permission::Full;
        }

        // Check admin groups
        if self.allow_admin_groups && creds.is_admin_group() {
            return Permission::Full;
        }

        // Read-only actions are always allowed
        if action.is_read_only() {
            return Permission::ReadOnly;
        }

        // For write operations, check PolicyKit
        if self.require_polkit {
            // TODO: Implement PolicyKit check via zbus
            // For now, deny non-root write access
            return Permission::Denied;
        }

        // If PolicyKit is disabled, allow all authenticated users
        Permission::Full
    }

    /// Check if an action is allowed (convenience method)
    pub fn is_allowed(&self, creds: &PeerCredentials, action: Action) -> bool {
        match self.check(creds, action) {
            Permission::Full => true,
            Permission::ReadOnly => action.is_read_only(),
            Permission::Denied => false,
        }
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
        let start = if self.entries.len() > count {
            self.entries.len() - count
        } else {
            0
        };
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
    fn test_peer_credentials_is_admin() {
        let wheel = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 10, // wheel
        };
        assert!(wheel.is_admin_group());

        let sudo = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 27, // sudo
        };
        assert!(sudo.is_admin_group());

        let user = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        };
        assert!(!user.is_admin_group());
    }

    #[test]
    fn test_action_is_read_only() {
        assert!(Action::Query.is_read_only());
        assert!(!Action::Install.is_read_only());
        assert!(!Action::Remove.is_read_only());
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
    fn test_auth_checker_admin_group() {
        let checker = AuthChecker::new();
        let wheel_user = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 10, // wheel
        };

        assert_eq!(checker.check(&wheel_user, Action::Query), Permission::Full);
        assert_eq!(checker.check(&wheel_user, Action::Install), Permission::Full);
    }

    #[test]
    fn test_auth_checker_regular_user() {
        let checker = AuthChecker::new();
        let user = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        };

        // Read-only allowed
        assert_eq!(checker.check(&user, Action::Query), Permission::ReadOnly);

        // Write operations denied (would need PolicyKit)
        assert_eq!(checker.check(&user, Action::Install), Permission::Denied);
    }

    #[test]
    fn test_auth_checker_disabled_polkit() {
        let checker = AuthChecker::new().disable_polkit();
        let user = PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        };

        // Without PolicyKit requirement, all authenticated users get full access
        assert_eq!(checker.check(&user, Action::Install), Permission::Full);
    }

    #[test]
    fn test_audit_entry() {
        let creds = PeerCredentials {
            pid: 1234,
            uid: 1000,
            gid: 1000,
        };
        let entry = AuditEntry::new(creds, Action::Install, true)
            .with_details("installed nginx");

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
