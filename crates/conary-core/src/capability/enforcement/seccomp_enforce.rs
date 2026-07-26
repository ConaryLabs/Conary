// conary-core/src/capability/enforcement/seccomp_enforce.rs
//! Seccomp-BPF syscall enforcement
//!
//! Converts syscall capability declarations into seccomp BPF filters that
//! restrict the process to only the declared syscalls. Uses an allowlist
//! approach: syscalls not in the allow list are blocked.
//!
//! Syscall names are exact names in the target architecture's syscall ABI.
//! Wildcards and semantic aliases are rejected.

use super::{EnforcementError, EnforcementMode};
use crate::capability::SyscallCapabilities;
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
use std::collections::{BTreeMap, HashSet};
use tracing::{debug, warn};

/// Protected scriptlets use this executor-owned syscall contract.
///
/// This identifier is internal execution metadata, not a package-selectable
/// profile name.
pub const SCRIPTLET_EXECUTOR_ABI_V1: &str = "scriptlet-v1";

/// `conary capability run` uses this executor-owned launch contract.
pub const CAPABILITY_RUN_EXECUTOR_ABI_V1: &str = "capability-run-v1";

#[cfg(target_arch = "x86_64")]
const SCRIPTLET_EXECUTOR_V1_SYSCALLS: &[&str] = &[
    "access",
    "accept",
    "accept4",
    "arch_prctl",
    "bind",
    "brk",
    "chdir",
    "clock_gettime",
    "clone",
    "clone3",
    "close",
    "connect",
    "dup",
    "dup2",
    "dup3",
    "epoll_create1",
    "epoll_ctl",
    "epoll_pwait",
    "eventfd2",
    "execve",
    "exit",
    "exit_group",
    "faccessat",
    "fchdir",
    "fchmod",
    "fchmodat",
    "fchown",
    "fchownat",
    "fcntl",
    "flock",
    "fstat",
    "futex",
    "getcwd",
    "getdents64",
    "getegid",
    "geteuid",
    "getgid",
    "getpeername",
    "getpgid",
    "getpid",
    "getpgrp",
    "getppid",
    "getrandom",
    "getresgid",
    "getresuid",
    "getsockname",
    "getsockopt",
    "getuid",
    "ioctl",
    "kill",
    "linkat",
    "listen",
    "lseek",
    "madvise",
    "memfd_create",
    "mkdirat",
    "mmap",
    "mprotect",
    "munmap",
    "newfstatat",
    "openat",
    "pipe2",
    "ppoll",
    "prctl",
    "pread64",
    "prlimit64",
    "pselect6",
    "pwrite64",
    "read",
    "readlinkat",
    "recvfrom",
    "recvmsg",
    "renameat",
    "rseq",
    "rt_sigaction",
    "rt_sigprocmask",
    "rt_sigreturn",
    "sendmsg",
    "sendto",
    "set_robust_list",
    "set_tid_address",
    "setgroups",
    "setpgid",
    "setsid",
    "setsockopt",
    "shmat",
    "shmctl",
    "shmdt",
    "shmget",
    "shutdown",
    "sigaltstack",
    "socket",
    "statx",
    "symlinkat",
    "umask",
    "uname",
    "unlinkat",
    "vfork",
    "wait4",
    "waitid",
    "write",
];

#[cfg(target_arch = "aarch64")]
const SCRIPTLET_EXECUTOR_V1_SYSCALLS: &[&str] = &[
    "accept",
    "accept4",
    "bind",
    "brk",
    "chdir",
    "clock_gettime",
    "clone",
    "clone3",
    "close",
    "connect",
    "dup",
    "dup3",
    "epoll_create1",
    "epoll_ctl",
    "epoll_pwait",
    "eventfd2",
    "execve",
    "exit",
    "exit_group",
    "faccessat",
    "fchdir",
    "fchmod",
    "fchmodat",
    "fchown",
    "fchownat",
    "fcntl",
    "flock",
    "fstat",
    "futex",
    "getcwd",
    "getdents64",
    "getegid",
    "geteuid",
    "getgid",
    "getpeername",
    "getpid",
    "getrandom",
    "getsockname",
    "getsockopt",
    "getuid",
    "ioctl",
    "kill",
    "linkat",
    "listen",
    "lseek",
    "madvise",
    "memfd_create",
    "mkdirat",
    "mmap",
    "mprotect",
    "munmap",
    "newfstatat",
    "openat",
    "pipe2",
    "ppoll",
    "prctl",
    "pread64",
    "prlimit64",
    "pselect6",
    "pwrite64",
    "read",
    "readlinkat",
    "recvfrom",
    "recvmsg",
    "renameat",
    "rseq",
    "rt_sigaction",
    "rt_sigprocmask",
    "sendmsg",
    "sendto",
    "set_robust_list",
    "set_tid_address",
    "setgroups",
    "setsid",
    "setsockopt",
    "shmat",
    "shmctl",
    "shmdt",
    "shmget",
    "shutdown",
    "sigaltstack",
    "socket",
    "statx",
    "symlinkat",
    "umask",
    "unlinkat",
    "wait4",
    "waitid",
    "write",
];

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SCRIPTLET_EXECUTOR_V1_SYSCALLS: &[&str] = &[];

#[cfg(target_arch = "x86_64")]
const CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS: &[&str] = &[
    "access",
    "arch_prctl",
    "brk",
    "clock_gettime",
    "close",
    "execve",
    "exit",
    "exit_group",
    "futex",
    "getcwd",
    "getrandom",
    "madvise",
    "mmap",
    "mprotect",
    "munmap",
    "newfstatat",
    "openat",
    "pread64",
    "prlimit64",
    "read",
    "readlink",
    "rseq",
    "rt_sigaction",
    "rt_sigprocmask",
    "set_robust_list",
    "set_tid_address",
    "write",
];

#[cfg(target_arch = "aarch64")]
const CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS: &[&str] = &[
    "brk",
    "clock_gettime",
    "close",
    "execve",
    "exit",
    "exit_group",
    "faccessat",
    "futex",
    "getcwd",
    "getrandom",
    "madvise",
    "mmap",
    "mprotect",
    "munmap",
    "newfstatat",
    "openat",
    "pread64",
    "prlimit64",
    "read",
    "readlinkat",
    "rseq",
    "rt_sigaction",
    "rt_sigprocmask",
    "set_robust_list",
    "set_tid_address",
    "write",
];

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS: &[&str] = &[];

/// Materialize the protected scriptlet executor's exact current-architecture
/// syscall contract.
pub fn scriptlet_executor_v1_capabilities() -> SyscallCapabilities {
    SyscallCapabilities {
        allow: SCRIPTLET_EXECUTOR_V1_SYSCALLS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        deny: Vec::new(),
    }
}

/// Build an ephemeral package-plus-executor syscall contract.
///
/// This never changes the stored package declaration. A package deny that
/// conflicts with the executor ABI is a typed setup failure instead of being
/// silently overridden.
pub fn capability_run_executor_v1_capabilities(
    declared: &SyscallCapabilities,
) -> Result<SyscallCapabilities, EnforcementError> {
    declared
        .validate_for_current_target()
        .map_err(|error| EnforcementError::Seccomp(error.to_string()))?;
    if CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS.is_empty() {
        return Err(EnforcementError::Unsupported {
            feature: format!(
                "{CAPABILITY_RUN_EXECUTOR_ABI_V1} on {}",
                std::env::consts::ARCH
            ),
        });
    }

    let denied: HashSet<&str> = declared.deny.iter().map(String::as_str).collect();
    if let Some(syscall) = CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS
        .iter()
        .find(|syscall| denied.contains(**syscall))
    {
        return Err(EnforcementError::Seccomp(format!(
            "package denies executor-required syscall '{syscall}' from {CAPABILITY_RUN_EXECUTOR_ABI_V1}"
        )));
    }

    let mut allow = declared.allow.clone();
    allow.extend(
        CAPABILITY_RUN_EXECUTOR_V1_SYSCALLS
            .iter()
            .map(|syscall| (*syscall).to_string()),
    );
    allow.sort();
    allow.dedup();
    Ok(SyscallCapabilities {
        allow,
        deny: declared.deny.clone(),
    })
}

/// Apply seccomp BPF filter based on declared syscall capabilities
///
/// Builds and installs a seccomp filter from the capability declaration.
/// After this call, only declared syscalls are allowed. The default action
/// for undeclared syscalls depends on the enforcement mode:
/// - Enforce: kill the process
/// - Warn/Audit: log the violation but allow
pub fn apply_seccomp_filter(
    caps: &SyscallCapabilities,
    mode: EnforcementMode,
) -> Result<(), EnforcementError> {
    if !check_seccomp_support() {
        if mode == EnforcementMode::Enforce {
            return Err(EnforcementError::Unsupported {
                feature: "seccomp".to_string(),
            });
        }
        warn!("Seccomp not supported, skipping syscall enforcement");
        return Ok(());
    }

    let bpf = build_seccomp_filter(caps, mode)?;

    // Ensure NO_NEW_PRIVS is set (required for unprivileged seccomp)
    // This is idempotent — landlock's restrict_self() may have already set it
    unsafe {
        let ret = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        if ret != 0 {
            return Err(EnforcementError::Seccomp(
                "Failed to set PR_SET_NO_NEW_PRIVS".to_string(),
            ));
        }
    }

    seccompiler::apply_filter(&bpf)
        .map_err(|e| EnforcementError::Seccomp(format!("Failed to install filter: {e}")))?;

    debug!("Seccomp filter applied in {} mode", mode);
    Ok(())
}

/// Resolve exact package-declared allow/deny inputs.
///
/// Returns a sorted, deduplicated list. Packages cannot select or expand an
/// executor-owned syscall contract.
fn resolve_allowed_syscalls(caps: &SyscallCapabilities) -> Result<Vec<String>, EnforcementError> {
    let mut allowed: Vec<String> = Vec::new();

    // Merge exact explicit allow names.
    for syscall in &caps.allow {
        if syscall.contains('*') {
            return Err(EnforcementError::Seccomp(format!(
                "wildcard syscall declarations are unsupported: {syscall}"
            )));
        }
        allowed.push(syscall.clone());
    }

    // Remove exact denied syscalls (deny overrides allow).
    if let Some(wildcard) = caps.deny.iter().find(|name| name.contains('*')) {
        return Err(EnforcementError::Seccomp(format!(
            "wildcard syscall declarations are unsupported: {wildcard}"
        )));
    }
    let deny_set: HashSet<String> = caps.deny.iter().cloned().collect();

    allowed.retain(|s| !deny_set.contains(s));

    // Deduplicate
    allowed.sort();
    allowed.dedup();

    Ok(allowed)
}

/// Build a seccomp BPF program from syscall capabilities
pub fn build_seccomp_filter(
    caps: &SyscallCapabilities,
    mode: EnforcementMode,
) -> Result<BpfProgram, EnforcementError> {
    let allowed = resolve_allowed_syscalls(caps)?;

    // Build the BPF filter.
    // In Enforce mode, unauthorized syscalls kill the process.
    // In Warn and Audit modes, unauthorized syscalls are logged but still
    // allowed to execute. This is intentional for capability discovery:
    // Warn mode lets users observe which syscalls a scriptlet actually uses
    // before switching to Enforce mode. Neither Warn nor Audit blocks syscalls.
    let default_action = match mode {
        EnforcementMode::Enforce => SeccompAction::KillProcess,
        EnforcementMode::Warn | EnforcementMode::Audit => SeccompAction::Log,
    };

    // Build syscall rules map: allowed syscalls get empty rules (match unconditionally)
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for name in &allowed {
        let number = syscall_name_to_number(name).ok_or_else(|| {
            EnforcementError::Seccomp(format!(
                "syscall '{name}' is not an exact name in the {} ABI",
                std::env::consts::ARCH
            ))
        })?;
        rules.entry(number).or_default();
    }

    // Determine target architecture
    let arch: seccompiler::TargetArch = std::env::consts::ARCH.try_into().map_err(|_| {
        EnforcementError::Seccomp(format!(
            "Unsupported architecture for seccomp: {}",
            std::env::consts::ARCH
        ))
    })?;

    debug!(
        "Building seccomp filter: {} allowed syscalls, mode: {}",
        allowed.len(),
        mode
    );

    let filter = SeccompFilter::new(
        rules,
        default_action,       // for syscalls NOT in the map
        SeccompAction::Allow, // for syscalls IN the map
        arch,
    )
    .map_err(|e| EnforcementError::Seccomp(format!("Failed to build filter: {e}")))?;

    let bpf: BpfProgram = filter
        .try_into()
        .map_err(|e| EnforcementError::Seccomp(format!("Failed to compile BPF: {e}")))?;

    Ok(bpf)
}

/// Check if the kernel supports seccomp
pub fn check_seccomp_support() -> bool {
    unsafe {
        // PR_GET_SECCOMP returns 0 if seccomp is disabled for this thread,
        // or the current mode (1/2) if enabled.
        // Returns -1 with EINVAL if not supported.
        let ret = libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0);
        ret >= 0
    }
}

/// Get summary of what a seccomp filter would do (for reporting without applying)
pub fn describe_seccomp_filter(
    caps: &SyscallCapabilities,
    mode: EnforcementMode,
) -> Result<SeccompFilterInfo, EnforcementError> {
    let allowed = resolve_allowed_syscalls(caps)?;

    let denied_explicit = caps.deny.len();

    if let Some(unmapped) = allowed
        .iter()
        .find(|name| syscall_name_to_number(name).is_none())
    {
        return Err(EnforcementError::Seccomp(format!(
            "syscall '{unmapped}' is not an exact name in the {} ABI",
            std::env::consts::ARCH
        )));
    }

    Ok(SeccompFilterInfo {
        mode,
        allowed_count: allowed.len(),
        denied_explicit,
        allowed_syscalls: allowed,
    })
}

/// Information about a seccomp filter (for reporting)
#[derive(Debug, Clone)]
pub struct SeccompFilterInfo {
    pub mode: EnforcementMode,
    pub allowed_count: usize,
    pub denied_explicit: usize,
    pub allowed_syscalls: Vec<String>,
}

/// Resolve a syscall name through libseccomp's native-architecture table.
///
/// Negative pseudo-syscall numbers are not directly enforceable by the
/// seccompiler BPF map and therefore are not accepted as native ABI names.
fn syscall_name_to_number(name: &str) -> Option<i64> {
    crate::capability::resolve_native_syscall_number(name)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seccomp_support_check() {
        // Should not panic regardless of kernel support
        let _ = check_seccomp_support();
    }

    #[test]
    fn test_syscall_name_to_number_basic() {
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(syscall_name_to_number("read"), Some(libc::SYS_read));
            assert_eq!(syscall_name_to_number("write"), Some(libc::SYS_write));
            assert_eq!(syscall_name_to_number("open"), Some(libc::SYS_open));
            assert_eq!(syscall_name_to_number("close"), Some(libc::SYS_close));
            assert_eq!(syscall_name_to_number("socket"), Some(libc::SYS_socket));
        }
    }

    #[test]
    fn test_syscall_name_unknown() {
        assert_eq!(syscall_name_to_number("nonexistent_syscall"), None);
    }

    #[test]
    fn related_syscall_name_is_not_treated_as_an_alias() {
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(syscall_name_to_number("sigaction"), None);
            assert_eq!(
                syscall_name_to_number("rt_sigaction"),
                Some(libc::SYS_rt_sigaction)
            );
        }
        #[cfg(target_arch = "aarch64")]
        for name in [
            "open",
            "stat",
            "lstat",
            "access",
            "pipe",
            "dup2",
            "epoll_create",
            "epoll_wait",
            "select",
            "eventfd",
        ] {
            assert_eq!(
                syscall_name_to_number(name),
                None,
                "{name} must not authorize a different aarch64 syscall"
            );
        }
    }

    #[test]
    fn wildcard_syscall_contracts_are_rejected() {
        let allow = SyscallCapabilities {
            allow: vec!["epoll_*".to_string()],
            deny: Vec::new(),
        };
        let deny = SyscallCapabilities {
            allow: vec!["read".to_string()],
            deny: vec!["shm*".to_string()],
        };

        assert!(resolve_allowed_syscalls(&allow).is_err());
        assert!(resolve_allowed_syscalls(&deny).is_err());
    }

    #[test]
    fn unmapped_allowed_syscall_fails_filter_construction() {
        let caps = SyscallCapabilities {
            allow: vec!["definitely_not_a_syscall".to_string()],
            deny: Vec::new(),
        };
        let error = build_seccomp_filter(&caps, EnforcementMode::Enforce)
            .expect_err("unmapped allowed syscall must not be silently omitted");
        assert!(error.to_string().contains("not an exact name"));
    }

    #[test]
    fn test_describe_filter_with_explicit_allow() {
        let caps = SyscallCapabilities {
            allow: vec![
                "read".to_string(),
                "socket".to_string(),
                "connect".to_string(),
            ],
            deny: Vec::new(),
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Enforce).unwrap();
        assert!(info.allowed_syscalls.contains(&"socket".to_string()));
        assert!(info.allowed_syscalls.contains(&"connect".to_string()));
        assert!(info.allowed_syscalls.contains(&"read".to_string()));
    }

    #[test]
    fn describe_filter_rejects_invalid_exact_declarations() {
        let caps = SyscallCapabilities {
            allow: vec!["definitely_not_a_syscall".to_string()],
            deny: Vec::new(),
        };

        let error = describe_seccomp_filter(&caps, EnforcementMode::Audit)
            .expect_err("reporting must not replace an invalid policy with an empty one");
        assert!(error.to_string().contains("not an exact name"));
    }

    #[test]
    fn test_deny_overrides_allow() {
        let caps = SyscallCapabilities {
            allow: vec!["read".to_string(), "bind".to_string(), "listen".to_string()],
            deny: vec!["bind".to_string(), "listen".to_string()],
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Enforce).unwrap();
        assert!(!info.allowed_syscalls.contains(&"bind".to_string()));
        assert!(!info.allowed_syscalls.contains(&"listen".to_string()));
        assert!(info.allowed_syscalls.contains(&"read".to_string()));
    }

    #[test]
    fn scriptlet_v1_is_a_closed_exact_current_abi_contract() {
        assert_eq!(SCRIPTLET_EXECUTOR_ABI_V1, "scriptlet-v1");
        let caps = scriptlet_executor_v1_capabilities();
        assert!(!caps.allow.is_empty());
        assert!(caps.deny.is_empty());

        let unique = caps.allow.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), caps.allow.len());
        for syscall in &caps.allow {
            assert!(
                syscall_name_to_number(syscall).is_some(),
                "{SCRIPTLET_EXECUTOR_ABI_V1} contains an unmapped syscall: {syscall}"
            );
        }
        for forbidden in [
            "bpf",
            "chroot",
            "init_module",
            "kexec_load",
            "mount",
            "ptrace",
            "reboot",
            "setgid",
            "setuid",
        ] {
            assert!(
                !caps.allow.iter().any(|syscall| syscall == forbidden),
                "{SCRIPTLET_EXECUTOR_ABI_V1} must not allow {forbidden}"
            );
        }

        build_seccomp_filter(&caps, EnforcementMode::Enforce)
            .expect("scriptlet-v1 must compile for the current architecture");
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn scriptlet_v1_allows_dash_exact_vfork_process_creation() {
        let caps = scriptlet_executor_v1_capabilities();

        assert!(
            caps.allow.iter().any(|syscall| syscall == "vfork"),
            "x86_64 /bin/sh may be dash, whose exact helper-launch ABI uses vfork"
        );
        assert_eq!(syscall_name_to_number("vfork"), Some(libc::SYS_vfork));
    }

    #[test]
    fn capability_run_v1_is_a_closed_ephemeral_executor_contract() {
        assert_eq!(CAPABILITY_RUN_EXECUTOR_ABI_V1, "capability-run-v1");
        let declared = SyscallCapabilities {
            allow: vec!["socket".to_string()],
            deny: Vec::new(),
        };
        let merged = capability_run_executor_v1_capabilities(&declared)
            .expect("current supported architecture has a capability-run executor ABI");

        assert_eq!(declared.allow, ["socket"]);
        assert!(merged.allow.contains(&"socket".to_string()));
        assert!(merged.allow.contains(&"execve".to_string()));
        assert!(merged.allow.contains(&"prlimit64".to_string()));
        let unique = merged
            .allow
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), merged.allow.len());
        for syscall in &merged.allow {
            assert!(
                syscall_name_to_number(syscall).is_some(),
                "{CAPABILITY_RUN_EXECUTOR_ABI_V1} contains an unmapped syscall: {syscall}"
            );
        }
    }

    #[test]
    fn package_deny_cannot_be_silently_overridden_by_capability_run_v1() {
        let declared = SyscallCapabilities {
            allow: Vec::new(),
            deny: vec!["execve".to_string()],
        };
        let error = capability_run_executor_v1_capabilities(&declared)
            .expect_err("executor ABI conflict must fail setup");
        assert!(
            error
                .to_string()
                .contains("executor-required syscall 'execve'")
        );
    }

    #[test]
    fn test_build_filter_explicit_allow() {
        let caps = SyscallCapabilities {
            allow: vec![
                "read".to_string(),
                "write".to_string(),
                "exit_group".to_string(),
            ],
            deny: Vec::new(),
        };

        #[cfg(target_arch = "x86_64")]
        {
            let result = build_seccomp_filter(&caps, EnforcementMode::Enforce);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_enforcement_mode_to_action() {
        // Verify the default action logic
        let enforce_action = match EnforcementMode::Enforce {
            EnforcementMode::Enforce => SeccompAction::KillProcess,
            _ => SeccompAction::Log,
        };
        assert_eq!(enforce_action, SeccompAction::KillProcess);

        let audit_action = match EnforcementMode::Audit {
            EnforcementMode::Enforce => SeccompAction::KillProcess,
            _ => SeccompAction::Log,
        };
        assert_eq!(audit_action, SeccompAction::Log);
    }
}
