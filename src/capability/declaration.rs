// src/capability/declaration.rs
//! Capability declaration types for CCS packages
//!
//! This module defines the structures for declaring what system resources
//! a package needs (network, filesystem, syscalls). These declarations enable:
//! - Documentation of package requirements
//! - Audit mode to compare declared vs observed behavior
//! - Future enforcement via landlock/seccomp

use serde::{Deserialize, Serialize};

/// Complete capability declaration for a package
///
/// This declares what system resources a package requires to function.
/// Used for audit mode and future enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    /// Version of the capability schema (for forward compatibility)
    #[serde(default = "default_version")]
    pub version: u32,

    /// Human-readable rationale explaining why these capabilities are needed
    #[serde(default)]
    pub rationale: Option<String>,

    /// Network access requirements
    #[serde(default)]
    pub network: NetworkCapabilities,

    /// Filesystem access requirements
    #[serde(default)]
    pub filesystem: FilesystemCapabilities,

    /// Syscall requirements
    #[serde(default)]
    pub syscalls: SyscallCapabilities,
}

fn default_version() -> u32 {
    1
}

impl Default for CapabilityDeclaration {
    fn default() -> Self {
        Self {
            version: default_version(),
            rationale: None,
            network: NetworkCapabilities {
                outbound: Vec::new(),
                listen: Vec::new(),
                none: false,
            },
            filesystem: FilesystemCapabilities {
                read: Vec::new(),
                write: Vec::new(),
                execute: Vec::new(),
                deny: Vec::new(),
            },
            syscalls: SyscallCapabilities {
                allow: Vec::new(),
                deny: Vec::new(),
                profile: None,
            },
        }
    }
}

impl CapabilityDeclaration {
    /// Create a new capability declaration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if any capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.network.is_empty() && self.filesystem.is_empty() && self.syscalls.is_empty()
    }

    /// Validate the declaration for consistency
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        // Validate network
        self.network.validate()?;

        // Validate filesystem
        self.filesystem.validate()?;

        // Validate syscalls
        self.syscalls.validate()?;

        Ok(())
    }
}

/// Network capability declarations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkCapabilities {
    /// Allowed outbound ports (e.g., ["443", "80", "53"])
    #[serde(default)]
    pub outbound: Vec<String>,

    /// Ports this package listens on (e.g., ["80", "443"])
    #[serde(default)]
    pub listen: Vec<String>,

    /// If true, no network access is required
    #[serde(default)]
    pub none: bool,
}

impl NetworkCapabilities {
    /// Check if no network capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.outbound.is_empty() && self.listen.is_empty() && !self.none
    }

    /// Validate network capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        // If none is true, outbound and listen should be empty
        if self.none && (!self.outbound.is_empty() || !self.listen.is_empty()) {
            return Err(CapabilityValidationError::ConflictingNetwork {
                message: "network.none=true conflicts with outbound/listen ports".to_string(),
            });
        }

        // Validate port specifications
        for port in &self.outbound {
            validate_port_spec(port).map_err(|e| CapabilityValidationError::InvalidPort {
                port: port.clone(),
                reason: e,
            })?;
        }

        for port in &self.listen {
            validate_port_spec(port).map_err(|e| CapabilityValidationError::InvalidPort {
                port: port.clone(),
                reason: e,
            })?;
        }

        Ok(())
    }
}

/// Filesystem capability declarations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemCapabilities {
    /// Paths that can be read (e.g., ["/etc/ssl/certs", "/usr/share"])
    #[serde(default)]
    pub read: Vec<String>,

    /// Paths that can be written (e.g., ["/var/log/nginx", "/var/cache/nginx"])
    #[serde(default)]
    pub write: Vec<String>,

    /// Paths that can be executed (e.g., ["/usr/bin", "/usr/lib"])
    #[serde(default)]
    pub execute: Vec<String>,

    /// Paths that should be denied even if parent allows (e.g., ["/etc/shadow"])
    #[serde(default)]
    pub deny: Vec<String>,
}

impl FilesystemCapabilities {
    /// Check if no filesystem capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.read.is_empty()
            && self.write.is_empty()
            && self.execute.is_empty()
            && self.deny.is_empty()
    }

    /// Validate filesystem capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        // All paths should be absolute
        for path in &self.read {
            if !path.starts_with('/') {
                return Err(CapabilityValidationError::RelativePath {
                    path: path.clone(),
                    context: "filesystem.read".to_string(),
                });
            }
        }

        for path in &self.write {
            if !path.starts_with('/') {
                return Err(CapabilityValidationError::RelativePath {
                    path: path.clone(),
                    context: "filesystem.write".to_string(),
                });
            }
        }

        for path in &self.execute {
            if !path.starts_with('/') {
                return Err(CapabilityValidationError::RelativePath {
                    path: path.clone(),
                    context: "filesystem.execute".to_string(),
                });
            }
        }

        for path in &self.deny {
            if !path.starts_with('/') {
                return Err(CapabilityValidationError::RelativePath {
                    path: path.clone(),
                    context: "filesystem.deny".to_string(),
                });
            }
        }

        Ok(())
    }
}

/// Syscall capability declarations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyscallCapabilities {
    /// Explicitly allowed syscalls (supports wildcards like "epoll_*")
    #[serde(default)]
    pub allow: Vec<String>,

    /// Explicitly denied syscalls
    #[serde(default)]
    pub deny: Vec<String>,

    /// Use a predefined profile instead of explicit allow/deny
    /// Options: "minimal", "network-server", "gui-app", etc.
    #[serde(default)]
    pub profile: Option<String>,
}

impl SyscallCapabilities {
    /// Check if no syscall capabilities are declared
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty() && self.profile.is_none()
    }

    /// Validate syscall capability configuration
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        // If a profile is specified, allow/deny should ideally be empty
        // (but we allow overrides)
        if let Some(ref profile) = self.profile
            && !is_valid_syscall_profile(profile)
        {
            return Err(CapabilityValidationError::InvalidProfile {
                profile: profile.clone(),
            });
        }

        // Validate syscall names (basic check - just ensure non-empty)
        for syscall in &self.allow {
            if syscall.is_empty() {
                return Err(CapabilityValidationError::EmptySyscall {
                    context: "syscalls.allow".to_string(),
                });
            }
        }

        for syscall in &self.deny {
            if syscall.is_empty() {
                return Err(CapabilityValidationError::EmptySyscall {
                    context: "syscalls.deny".to_string(),
                });
            }
        }

        Ok(())
    }
}

/// Predefined syscall profiles for common use cases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallProfile {
    /// Minimal syscalls for simple CLI tools
    Minimal,
    /// Network server (socket, bind, listen, accept, etc.)
    NetworkServer,
    /// Network client (socket, connect, etc.)
    NetworkClient,
    /// GUI application (includes X11/Wayland syscalls)
    GuiApp,
    /// System daemon (various privileged operations)
    SystemDaemon,
    /// Container/sandbox (restricted set)
    Container,
}

impl SyscallProfile {
    /// Parse a profile name string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "minimal" => Some(Self::Minimal),
            "network-server" => Some(Self::NetworkServer),
            "network-client" => Some(Self::NetworkClient),
            "gui-app" | "gui" => Some(Self::GuiApp),
            "system-daemon" | "daemon" => Some(Self::SystemDaemon),
            "container" | "sandbox" => Some(Self::Container),
            _ => None,
        }
    }

    /// Get the syscalls allowed by this profile
    pub fn allowed_syscalls(&self) -> &'static [&'static str] {
        match self {
            Self::Minimal => MINIMAL_SYSCALLS,
            Self::NetworkServer => NETWORK_SERVER_SYSCALLS,
            Self::NetworkClient => NETWORK_CLIENT_SYSCALLS,
            Self::GuiApp => GUI_APP_SYSCALLS,
            Self::SystemDaemon => SYSTEM_DAEMON_SYSCALLS,
            Self::Container => CONTAINER_SYSCALLS,
        }
    }
}

/// Check if a syscall profile name is valid
fn is_valid_syscall_profile(profile: &str) -> bool {
    SyscallProfile::parse(profile).is_some()
}

/// Validate a port specification
fn validate_port_spec(port: &str) -> Result<(), String> {
    // Port can be a number, range (e.g., "8080-8090"), or "any"
    if port == "any" {
        return Ok(());
    }

    if let Some((start, end)) = port.split_once('-') {
        let start_num: u16 = start
            .parse()
            .map_err(|_| format!("invalid port range start: {}", start))?;
        let end_num: u16 = end
            .parse()
            .map_err(|_| format!("invalid port range end: {}", end))?;
        if start_num > end_num {
            return Err(format!("port range start {} > end {}", start_num, end_num));
        }
    } else {
        let _: u16 = port
            .parse()
            .map_err(|_| format!("invalid port number: {}", port))?;
    }

    Ok(())
}

/// Validation errors for capability declarations
#[derive(Debug, Clone, thiserror::Error)]
pub enum CapabilityValidationError {
    #[error("conflicting network configuration: {message}")]
    ConflictingNetwork { message: String },

    #[error("invalid port '{port}': {reason}")]
    InvalidPort { port: String, reason: String },

    #[error("relative path not allowed in {context}: {path}")]
    RelativePath { path: String, context: String },

    #[error("invalid syscall profile: {profile}")]
    InvalidProfile { profile: String },

    #[error("empty syscall name in {context}")]
    EmptySyscall { context: String },
}

// Predefined syscall lists for profiles
static MINIMAL_SYSCALLS: &[&str] = &[
    "read",
    "write",
    "open",
    "close",
    "stat",
    "fstat",
    "lstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "access",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "statx",
];

static NETWORK_SERVER_SYSCALLS: &[&str] = &[
    // Include minimal
    "read",
    "write",
    "open",
    "close",
    "stat",
    "fstat",
    "lstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "access",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "statx",
    // Network server specific
    "socket",
    "bind",
    "listen",
    "accept",
    "accept4",
    "connect",
    "sendto",
    "recvfrom",
    "sendmsg",
    "recvmsg",
    "shutdown",
    "setsockopt",
    "getsockopt",
    "getsockname",
    "getpeername",
    "epoll_create",
    "epoll_create1",
    "epoll_ctl",
    "epoll_wait",
    "epoll_pwait",
    "poll",
    "select",
    "pselect6",
    "clone",
    "clone3",
    "wait4",
    "waitid",
    "prctl",
    "sigaction",
    "rt_sigaction",
    "rt_sigprocmask",
    "sigaltstack",
];

static NETWORK_CLIENT_SYSCALLS: &[&str] = &[
    // Include minimal
    "read",
    "write",
    "open",
    "close",
    "stat",
    "fstat",
    "lstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "access",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "statx",
    // Network client specific
    "socket",
    "connect",
    "sendto",
    "recvfrom",
    "sendmsg",
    "recvmsg",
    "shutdown",
    "setsockopt",
    "getsockopt",
    "getsockname",
    "getpeername",
    "poll",
    "select",
];

static GUI_APP_SYSCALLS: &[&str] = &[
    // Most syscalls needed for GUI apps
    "read",
    "write",
    "open",
    "close",
    "stat",
    "fstat",
    "lstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "access",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "statx",
    // GUI specific
    "socket",
    "connect",
    "recvmsg",
    "sendmsg",
    "poll",
    "ioctl",
    "fcntl",
    "dup",
    "dup2",
    "pipe",
    "pipe2",
    "eventfd",
    "eventfd2",
    "memfd_create",
    "shmat",
    "shmdt",
    "shmget",
    "shmctl",
];

static SYSTEM_DAEMON_SYSCALLS: &[&str] = &[
    // Broad permissions for system daemons
    "read",
    "write",
    "open",
    "close",
    "stat",
    "fstat",
    "lstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "access",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "statx",
    // Daemon specific
    "socket",
    "bind",
    "listen",
    "accept",
    "accept4",
    "connect",
    "sendto",
    "recvfrom",
    "sendmsg",
    "recvmsg",
    "shutdown",
    "setsockopt",
    "getsockopt",
    "getsockname",
    "getpeername",
    "epoll_create",
    "epoll_create1",
    "epoll_ctl",
    "epoll_wait",
    "epoll_pwait",
    "poll",
    "select",
    "pselect6",
    "clone",
    "clone3",
    "wait4",
    "waitid",
    "prctl",
    "sigaction",
    "rt_sigaction",
    "rt_sigprocmask",
    "sigaltstack",
    "setuid",
    "setgid",
    "setgroups",
    "chroot",
    "chdir",
    "fchdir",
    "umask",
    "setsid",
    "ioctl",
    "fcntl",
    "flock",
    "mkdir",
    "rmdir",
    "unlink",
    "rename",
    "chmod",
    "chown",
    "fchmod",
    "fchown",
    "link",
    "symlink",
    "readlink",
    "getdents64",
];

static CONTAINER_SYSCALLS: &[&str] = &[
    // Restricted set for containers
    "read",
    "write",
    "close",
    "fstat",
    "mmap",
    "mprotect",
    "munmap",
    "brk",
    "exit_group",
    "arch_prctl",
    "futex",
    "set_tid_address",
    "set_robust_list",
    "rseq",
    "getrandom",
    "pread64",
    "pwrite64",
    "openat",
    "newfstatat",
    "clone",
    "clone3",
    "wait4",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_capability_declaration() {
        let cap = CapabilityDeclaration::default();
        assert_eq!(cap.version, 1);
        assert!(cap.is_empty());
    }

    #[test]
    fn test_network_none_conflict() {
        let mut cap = CapabilityDeclaration::default();
        cap.network.none = true;
        cap.network.outbound.push("80".to_string());

        let result = cap.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_port_specs() {
        assert!(validate_port_spec("80").is_ok());
        assert!(validate_port_spec("443").is_ok());
        assert!(validate_port_spec("8080-8090").is_ok());
        assert!(validate_port_spec("any").is_ok());
        assert!(validate_port_spec("65535").is_ok());
    }

    #[test]
    fn test_invalid_port_specs() {
        assert!(validate_port_spec("").is_err());
        assert!(validate_port_spec("abc").is_err());
        assert!(validate_port_spec("999999").is_err());
        assert!(validate_port_spec("100-50").is_err()); // range reversed
    }

    #[test]
    fn test_relative_path_rejected() {
        let mut cap = CapabilityDeclaration::default();
        cap.filesystem.read.push("etc/config".to_string());

        let result = cap.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_absolute_path_accepted() {
        let mut cap = CapabilityDeclaration::default();
        cap.filesystem.read.push("/etc/config".to_string());
        cap.filesystem.write.push("/var/log".to_string());

        let result = cap.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_syscall_profile_parsing() {
        assert_eq!(
            SyscallProfile::parse("minimal"),
            Some(SyscallProfile::Minimal)
        );
        assert_eq!(
            SyscallProfile::parse("network-server"),
            Some(SyscallProfile::NetworkServer)
        );
        assert_eq!(SyscallProfile::parse("invalid"), None);
    }

    #[test]
    fn test_full_capability_declaration_parse() {
        let toml = r#"
            version = 1
            rationale = "Web server requiring network listeners and cache access"

            [network]
            outbound = ["443", "80"]
            listen = ["80", "443"]
            none = false

            [filesystem]
            read = ["/etc/nginx", "/etc/ssl/certs"]
            write = ["/var/cache/nginx", "/var/log/nginx"]
            execute = ["/usr/bin", "/usr/lib"]
            deny = ["/home", "/root", "/etc/shadow"]

            [syscalls]
            profile = "network-server"
            allow = ["epoll_*"]
            deny = ["ptrace"]
        "#;

        let cap: CapabilityDeclaration = toml::from_str(toml).unwrap();
        assert_eq!(cap.version, 1);
        assert!(cap.rationale.is_some());
        assert_eq!(cap.network.listen.len(), 2);
        assert_eq!(cap.filesystem.read.len(), 2);
        assert_eq!(cap.syscalls.profile, Some("network-server".to_string()));
    }
}
