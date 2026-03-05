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

use crate::capability::{FilesystemCapabilities, SyscallCapabilities};
use std::fmt;

/// Enforcement mode for capability restrictions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    /// Log violations but don't block (for capability discovery)
    Audit,
    /// Log violations and warn user, but allow execution
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
    /// Syscall filter rules (via seccomp-bpf)
    pub syscalls: Option<SyscallCapabilities>,
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
    /// Whether network isolation was applied
    pub network_isolated: bool,
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
        network_isolated: policy.network_isolation,
        warnings: Vec::new(),
    };

    // Apply landlock filesystem restrictions first
    if let Some(ref fs_caps) = policy.filesystem {
        match landlock_enforce::apply_landlock_rules(fs_caps, policy.mode) {
            Ok(()) => {
                report.landlock_applied = true;
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
        seccomp: seccomp_enforce::check_seccomp_support(),
    }
}

/// Kernel support status for enforcement features
#[derive(Debug, Clone)]
pub struct EnforcementSupport {
    pub landlock: bool,
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

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

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
            syscalls: None,
            network_isolation: false,
        };
        let report = apply_enforcement(&policy).unwrap();
        assert!(!report.landlock_applied);
        assert!(!report.seccomp_applied);
        assert!(!report.network_isolated);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn test_enforcement_mode_equality() {
        assert_eq!(EnforcementMode::Enforce, EnforcementMode::Enforce);
        assert_ne!(EnforcementMode::Enforce, EnforcementMode::Audit);
    }
}
