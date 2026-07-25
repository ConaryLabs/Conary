// conary-core/src/capability/enforcement/mod.rs
//! Capability enforcement via Linux kernel security features
//!
//! This module provides runtime enforcement of package capability declarations
//! using landlock (filesystem access control) and seccomp-bpf (syscall filtering).
//!
//! ## Hook Point
//!
//! Enforcement is applied in the child process after fork, after namespace
//! isolation and resource limits, but before exec. This is a single-threaded
//! context, ideal for both landlock and seccomp setup.
//!
//! ## Ordering
//!
//! 1. Landlock (filesystem) is applied first
//! 2. Seccomp (syscalls) is applied last — once active, some syscalls needed
//!    for landlock setup would be blocked

pub mod landlock_enforce;
pub mod seccomp_enforce;

use crate::capability::{FilesystemCapabilities, NetworkCapabilities, SyscallCapabilities};
use std::fmt;

/// Enforcement mode for capability restrictions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    /// Use non-blocking audit support where the kernel API provides it.
    /// Landlock plans are not applied in this mode.
    Audit,
    /// Warn without blocking. Landlock plans are not applied in this mode.
    Warn,
    /// Block violations at kernel level
    Enforce,
}

impl fmt::Display for EnforcementMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit => write!(f, "audit"),
            Self::Warn => write!(f, "warn"),
            Self::Enforce => write!(f, "enforce"),
        }
    }
}

/// Enforcement policy combining all capability types
#[derive(Debug, Clone)]
pub struct EnforcementPolicy {
    /// How strictly to enforce
    pub mode: EnforcementMode,
    /// Filesystem access rules (via landlock)
    pub filesystem: Option<FilesystemCapabilities>,
    /// Exact TCP ConnectTcp/BindTcp rules (via Landlock ABI V4).
    pub network: Option<NetworkCapabilities>,
    /// Syscall filter rules (via seccomp-bpf)
    pub syscalls: Option<SyscallCapabilities>,
    /// Executor-owned syscall contract, when the syscall list comes from a
    /// protected lifecycle executor rather than a package declaration.
    pub syscall_contract: Option<&'static str>,
    /// Whether to isolate network (via CLONE_NEWNET)
    pub network_isolation: bool,
}

/// Result of applying enforcement policies
#[derive(Debug, Clone)]
pub struct EnforcementReport {
    /// Mode that was applied
    pub mode: EnforcementMode,
    /// Whether landlock was successfully applied
    pub landlock_applied: bool,
    /// Whether seccomp was successfully applied
    pub seccomp_applied: bool,
    /// Exact executor-owned syscall contract that was applied, if any.
    pub syscall_contract: Option<&'static str>,
    /// Whether network isolation was applied
    pub network_isolated: bool,
    /// Exact remote TCP ports granted by the applied Landlock ruleset.
    pub connect_tcp_rules: Vec<u16>,
    /// Exact local TCP ports granted by the applied Landlock ruleset.
    pub bind_tcp_rules: Vec<u16>,
    /// Number of deny paths that conflict with allowed parents
    pub deny_conflicts_count: usize,
    /// Issues encountered during enforcement setup
    pub warnings: Vec<EnforcementWarning>,
}

/// Warning from enforcement setup (non-fatal issue)
#[derive(Debug, Clone)]
pub struct EnforcementWarning {
    pub category: String,
    pub message: String,
}

impl fmt::Display for EnforcementWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.category, self.message)
    }
}

/// Apply all enforcement policies in the child process (post-fork, pre-exec)
///
/// This should be called after namespace setup and resource limits,
/// but before execing the target binary. The ordering is important:
/// landlock first (filesystem), seccomp last (syscalls).
pub fn apply_enforcement(
    policy: &EnforcementPolicy,
) -> std::result::Result<EnforcementReport, EnforcementError> {
    let mut report = EnforcementReport {
        mode: policy.mode,
        landlock_applied: false,
        seccomp_applied: false,
        syscall_contract: policy.syscall_contract,
        network_isolated: policy.network_isolation,
        connect_tcp_rules: Vec::new(),
        bind_tcp_rules: Vec::new(),
        deny_conflicts_count: 0,
        warnings: Vec::new(),
    };

    // Apply one Landlock ruleset for filesystem and exact TCP restrictions.
    if (policy.filesystem.is_some() || policy.network.is_some())
        && policy.mode != EnforcementMode::Enforce
    {
        report.warnings.push(EnforcementWarning {
            category: "landlock".to_string(),
            message: "Landlock has no non-blocking audit mode; exact filesystem/TCP rules were described but not applied".to_string(),
        });
    } else if policy.filesystem.is_some() || policy.network.is_some() {
        match landlock_enforce::apply_landlock_rules(
            policy.filesystem.as_ref(),
            policy.network.as_ref(),
            policy.mode,
        ) {
            Ok(()) => {
                report.landlock_applied = true;
                if let Some(network) = &policy.network {
                    report.connect_tcp_rules = network.connect_tcp.clone();
                    report.bind_tcp_rules = network.bind_tcp.clone();
                }
            }
            Err(e) => {
                if policy.mode == EnforcementMode::Enforce {
                    return Err(e);
                }
                report.warnings.push(EnforcementWarning {
                    category: "landlock".to_string(),
                    message: format!("Landlock not applied: {}", e),
                });
            }
        }
    }

    // Apply seccomp syscall filter LAST — once active, some syscalls
    // needed for landlock setup would be blocked
    if let Some(ref syscall_caps) = policy.syscalls {
        match seccomp_enforce::apply_seccomp_filter(syscall_caps, policy.mode) {
            Ok(()) => {
                report.seccomp_applied = true;
            }
            Err(e) => {
                if policy.mode == EnforcementMode::Enforce {
                    return Err(e);
                }
                report.warnings.push(EnforcementWarning {
                    category: "seccomp".to_string(),
                    message: format!("Seccomp not applied: {}", e),
                });
            }
        }
    }

    Ok(report)
}

/// Check if the kernel supports landlock and seccomp
pub fn check_enforcement_support() -> EnforcementSupport {
    EnforcementSupport {
        landlock: landlock_enforce::check_landlock_support(),
        landlock_network: landlock_enforce::check_landlock_network_support(),
        seccomp: seccomp_enforce::check_seccomp_support(),
    }
}

/// Kernel support status for enforcement features
#[derive(Debug, Clone)]
pub struct EnforcementSupport {
    pub landlock: bool,
    pub landlock_network: bool,
    pub seccomp: bool,
}

/// Errors from enforcement operations
#[derive(Debug, thiserror::Error)]
pub enum EnforcementError {
    #[error("Landlock error: {0}")]
    Landlock(String),

    #[error("Seccomp error: {0}")]
    Seccomp(String),

    #[error("Kernel does not support {feature}")]
    Unsupported { feature: String },

    #[error(
        "{count} deny path(s) conflict with allowed parents and cannot be enforced by landlock"
    )]
    DenyConflict { count: usize },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::SyscallCapabilities;

    #[test]
    fn test_enforcement_mode_display() {
        assert_eq!(EnforcementMode::Audit.to_string(), "audit");
        assert_eq!(EnforcementMode::Warn.to_string(), "warn");
        assert_eq!(EnforcementMode::Enforce.to_string(), "enforce");
    }

    #[test]
    fn test_enforcement_warning_display() {
        let warning = EnforcementWarning {
            category: "landlock".to_string(),
            message: "Not supported on this kernel".to_string(),
        };
        assert_eq!(
            warning.to_string(),
            "[landlock] Not supported on this kernel"
        );
    }

    #[test]
    fn test_enforcement_support_check() {
        // Just ensure it doesn't panic
        let support = check_enforcement_support();
        let _ = support.landlock;
        let _ = support.seccomp;
    }

    #[test]
    fn test_apply_enforcement_empty_policy() {
        let policy = EnforcementPolicy {
            mode: EnforcementMode::Audit,
            filesystem: None,
            network: None,
            syscalls: None,
            syscall_contract: None,
            network_isolation: false,
        };
        let report = apply_enforcement(&policy).unwrap();
        assert!(!report.landlock_applied);
        assert!(!report.seccomp_applied);
        assert!(!report.network_isolated);
        assert!(report.connect_tcp_rules.is_empty());
        assert!(report.bind_tcp_rules.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn test_enforcement_mode_equality() {
        assert_eq!(EnforcementMode::Enforce, EnforcementMode::Enforce);
        assert_ne!(EnforcementMode::Enforce, EnforcementMode::Audit);
    }

    #[test]
    fn test_apply_enforcement_warns_on_seccomp_setup_failure() {
        let policy = EnforcementPolicy {
            mode: EnforcementMode::Warn,
            filesystem: None,
            network: None,
            syscalls: Some(SyscallCapabilities {
                allow: Vec::new(),
                deny: vec!["epoll_*".to_string()],
            }),
            syscall_contract: None,
            network_isolation: false,
        };

        let report = apply_enforcement(&policy).expect("warn mode should not fail closed");
        assert!(!report.seccomp_applied);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.category == "seccomp"
                    && warning.message.contains("wildcard syscall"))
        );
    }

    #[test]
    fn audit_mode_does_not_misrepresent_landlock_enforcement_as_non_blocking() {
        let policy = EnforcementPolicy {
            mode: EnforcementMode::Audit,
            filesystem: Some(FilesystemCapabilities {
                read: vec!["/".to_string()],
                ..Default::default()
            }),
            network: None,
            syscalls: None,
            syscall_contract: None,
            network_isolation: false,
        };

        let report = apply_enforcement(&policy).unwrap();
        assert!(!report.landlock_applied);
        assert!(report.warnings.iter().any(|warning| {
            warning.category == "landlock" && warning.message.contains("no non-blocking audit mode")
        }));
    }

    #[test]
    fn test_apply_enforcement_enforce_mode_propagates_seccomp_failure() {
        let policy = EnforcementPolicy {
            mode: EnforcementMode::Enforce,
            filesystem: None,
            network: None,
            syscalls: Some(SyscallCapabilities {
                allow: vec!["definitely-not-a-real-syscall".to_string()],
                deny: Vec::new(),
            }),
            syscall_contract: None,
            network_isolation: false,
        };

        let error = apply_enforcement(&policy).expect_err("enforce mode should fail closed");
        assert!(error.to_string().contains("not an exact name"));
    }
}
