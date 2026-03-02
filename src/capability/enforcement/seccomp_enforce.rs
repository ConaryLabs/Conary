// src/capability/enforcement/seccomp_enforce.rs
//! Seccomp-BPF syscall enforcement
//!
//! Converts syscall capability declarations into seccomp BPF filters that
//! restrict the process to only the declared syscalls. Uses an allowlist
//! approach: syscalls not in the allow list are blocked.
//!
//! ## Profiles
//!
//! Syscall profiles (e.g., "minimal", "network-server") expand to predefined
//! syscall lists. Explicit `allow` entries are merged with profile syscalls.
//! Explicit `deny` entries override both.
//!
//! ## Wildcard Support
//!
//! Syscall names support glob-style wildcards: `epoll_*` expands to
//! `epoll_create`, `epoll_create1`, `epoll_ctl`, `epoll_wait`, `epoll_pwait`.

use super::{EnforcementError, EnforcementMode};
use crate::capability::{SyscallCapabilities, SyscallProfile};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
use std::collections::BTreeMap;
use tracing::{debug, warn};

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

/// Build a seccomp BPF program from syscall capabilities
pub fn build_seccomp_filter(
    caps: &SyscallCapabilities,
    mode: EnforcementMode,
) -> Result<BpfProgram, EnforcementError> {
    // Collect allowed syscalls from profile + explicit allow list
    let mut allowed: Vec<String> = Vec::new();

    // Expand profile to syscall list
    if let Some(ref profile_name) = caps.profile {
        if let Some(profile) = SyscallProfile::parse(profile_name) {
            for syscall in profile.allowed_syscalls() {
                allowed.push((*syscall).to_string());
            }
        } else {
            return Err(EnforcementError::Seccomp(format!(
                "Unknown syscall profile: {profile_name}"
            )));
        }
    }

    // Merge explicit allow list (with wildcard expansion)
    for syscall in &caps.allow {
        if syscall.contains('*') {
            allowed.extend(expand_wildcard(syscall));
        } else {
            allowed.push(syscall.clone());
        }
    }

    // Remove denied syscalls (deny overrides allow)
    let deny_set: Vec<String> = caps
        .deny
        .iter()
        .flat_map(|s| {
            if s.contains('*') {
                expand_wildcard(s)
            } else {
                vec![s.clone()]
            }
        })
        .collect();

    allowed.retain(|s| !deny_set.contains(s));

    // Deduplicate
    allowed.sort();
    allowed.dedup();

    // Build the BPF filter
    let default_action = match mode {
        EnforcementMode::Enforce => SeccompAction::KillProcess,
        EnforcementMode::Warn | EnforcementMode::Audit => SeccompAction::Log,
    };

    // Build syscall rules map: allowed syscalls get empty rules (match unconditionally)
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    let mut unmapped = Vec::new();

    for name in &allowed {
        if let Some(num) = syscall_name_to_number(name) {
            rules.entry(num).or_default();
        } else {
            unmapped.push(name.clone());
        }
    }

    if !unmapped.is_empty() {
        debug!(
            "Skipped {} unmapped syscalls: {}",
            unmapped.len(),
            unmapped.join(", ")
        );
    }

    // Determine target architecture
    let arch: seccompiler::TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(|_| EnforcementError::Seccomp(format!(
            "Unsupported architecture for seccomp: {}",
            std::env::consts::ARCH
        )))?;

    debug!(
        "Building seccomp filter: {} allowed syscalls, mode: {}",
        allowed.len(),
        mode
    );

    let filter = SeccompFilter::new(
        rules,
        default_action,        // for syscalls NOT in the map
        SeccompAction::Allow,  // for syscalls IN the map
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

/// Expand a wildcard syscall pattern (e.g., "epoll_*") against known syscalls
pub fn expand_wildcard(pattern: &str) -> Vec<String> {
    if !pattern.contains('*') {
        return vec![pattern.to_string()];
    }

    let prefix = pattern.trim_end_matches('*');

    KNOWN_SYSCALL_NAMES
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| (*name).to_string())
        .collect()
}

/// Get summary of what a seccomp filter would do (for reporting without applying)
pub fn describe_seccomp_filter(
    caps: &SyscallCapabilities,
    mode: EnforcementMode,
) -> SeccompFilterInfo {
    let mut allowed: Vec<String> = Vec::new();

    if let Some(ref profile_name) = caps.profile
        && let Some(profile) = SyscallProfile::parse(profile_name)
    {
        for syscall in profile.allowed_syscalls() {
            allowed.push((*syscall).to_string());
        }
    }

    for syscall in &caps.allow {
        if syscall.contains('*') {
            allowed.extend(expand_wildcard(syscall));
        } else {
            allowed.push(syscall.clone());
        }
    }

    let deny_set: Vec<String> = caps
        .deny
        .iter()
        .flat_map(|s| {
            if s.contains('*') {
                expand_wildcard(s)
            } else {
                vec![s.clone()]
            }
        })
        .collect();

    allowed.retain(|s| !deny_set.contains(s));
    allowed.sort();
    allowed.dedup();

    let unmapped: Vec<String> = allowed
        .iter()
        .filter(|name| syscall_name_to_number(name).is_none())
        .cloned()
        .collect();

    SeccompFilterInfo {
        mode,
        profile: caps.profile.clone(),
        allowed_count: allowed.len(),
        denied_explicit: deny_set.len(),
        unmapped_names: unmapped,
        allowed_syscalls: allowed,
    }
}

/// Information about a seccomp filter (for reporting)
#[derive(Debug, Clone)]
pub struct SeccompFilterInfo {
    pub mode: EnforcementMode,
    pub profile: Option<String>,
    pub allowed_count: usize,
    pub denied_explicit: usize,
    pub unmapped_names: Vec<String>,
    pub allowed_syscalls: Vec<String>,
}

/// Map a syscall name to its number on the current architecture
#[cfg(target_arch = "x86_64")]
fn syscall_name_to_number(name: &str) -> Option<i64> {
    Some(match name {
        // Basic I/O
        "read" => libc::SYS_read,
        "write" => libc::SYS_write,
        "open" => libc::SYS_open,
        "close" => libc::SYS_close,
        "stat" => libc::SYS_stat,
        "fstat" => libc::SYS_fstat,
        "lstat" => libc::SYS_lstat,
        "poll" => libc::SYS_poll,
        "lseek" => libc::SYS_lseek,
        "pread64" => libc::SYS_pread64,
        "pwrite64" => libc::SYS_pwrite64,
        "access" => libc::SYS_access,
        "pipe" => libc::SYS_pipe,
        "select" => libc::SYS_select,
        "dup" => libc::SYS_dup,
        "dup2" => libc::SYS_dup2,
        "ioctl" => libc::SYS_ioctl,
        "fcntl" => libc::SYS_fcntl,
        "flock" => libc::SYS_flock,

        // Memory management
        "mmap" => libc::SYS_mmap,
        "mprotect" => libc::SYS_mprotect,
        "munmap" => libc::SYS_munmap,
        "brk" => libc::SYS_brk,

        // Signals
        "rt_sigaction" | "sigaction" => libc::SYS_rt_sigaction,
        "rt_sigprocmask" => libc::SYS_rt_sigprocmask,
        "sigaltstack" => libc::SYS_sigaltstack,

        // Process
        "clone" => libc::SYS_clone,
        "fork" => libc::SYS_fork,
        "execve" => libc::SYS_execve,
        "exit" => libc::SYS_exit,
        "exit_group" => libc::SYS_exit_group,
        "wait4" => libc::SYS_wait4,
        "waitid" => libc::SYS_waitid,
        "kill" => libc::SYS_kill,
        "getpid" => libc::SYS_getpid,
        "getuid" => libc::SYS_getuid,
        "getgid" => libc::SYS_getgid,
        "geteuid" => libc::SYS_geteuid,
        "getegid" => libc::SYS_getegid,
        "prctl" => libc::SYS_prctl,
        "arch_prctl" => libc::SYS_arch_prctl,
        "set_tid_address" => libc::SYS_set_tid_address,
        "set_robust_list" => libc::SYS_set_robust_list,
        "futex" => libc::SYS_futex,
        "setsid" => libc::SYS_setsid,
        "umask" => libc::SYS_umask,
        "clone3" => libc::SYS_clone3,

        // Networking
        "socket" => libc::SYS_socket,
        "connect" => libc::SYS_connect,
        "accept" => libc::SYS_accept,
        "accept4" => libc::SYS_accept4,
        "bind" => libc::SYS_bind,
        "listen" => libc::SYS_listen,
        "sendto" => libc::SYS_sendto,
        "recvfrom" => libc::SYS_recvfrom,
        "sendmsg" => libc::SYS_sendmsg,
        "recvmsg" => libc::SYS_recvmsg,
        "shutdown" => libc::SYS_shutdown,
        "setsockopt" => libc::SYS_setsockopt,
        "getsockopt" => libc::SYS_getsockopt,
        "getsockname" => libc::SYS_getsockname,
        "getpeername" => libc::SYS_getpeername,

        // Epoll
        "epoll_create" => libc::SYS_epoll_create,
        "epoll_create1" => libc::SYS_epoll_create1,
        "epoll_ctl" => libc::SYS_epoll_ctl,
        "epoll_wait" => libc::SYS_epoll_wait,
        "epoll_pwait" => libc::SYS_epoll_pwait,
        "pselect6" => libc::SYS_pselect6,

        // Filesystem
        "openat" => libc::SYS_openat,
        "newfstatat" => libc::SYS_newfstatat,
        "statx" => libc::SYS_statx,
        "getdents64" => libc::SYS_getdents64,
        "mkdir" => libc::SYS_mkdir,
        "rmdir" => libc::SYS_rmdir,
        "unlink" => libc::SYS_unlink,
        "rename" => libc::SYS_rename,
        "link" => libc::SYS_link,
        "symlink" => libc::SYS_symlink,
        "readlink" => libc::SYS_readlink,
        "chmod" => libc::SYS_chmod,
        "fchmod" => libc::SYS_fchmod,
        "chown" => libc::SYS_chown,
        "fchown" => libc::SYS_fchown,
        "chroot" => libc::SYS_chroot,
        "chdir" => libc::SYS_chdir,
        "fchdir" => libc::SYS_fchdir,

        // Privilege
        "setuid" => libc::SYS_setuid,
        "setgid" => libc::SYS_setgid,
        "setgroups" => libc::SYS_setgroups,

        // IPC / shared memory
        "pipe2" => libc::SYS_pipe2,
        "eventfd2" => libc::SYS_eventfd2,
        "shmat" => libc::SYS_shmat,
        "shmdt" => libc::SYS_shmdt,
        "shmget" => libc::SYS_shmget,
        "shmctl" => libc::SYS_shmctl,
        "memfd_create" => libc::SYS_memfd_create,

        // Random
        "getrandom" => libc::SYS_getrandom,

        // Newer syscalls (numeric fallback if libc doesn't define them)
        "rseq" => 334,
        "eventfd" => 284,

        _ => return None,
    })
}

#[cfg(not(target_arch = "x86_64"))]
fn syscall_name_to_number(_name: &str) -> Option<i64> {
    // TODO: Add aarch64 syscall mapping
    None
}

/// Known syscall names for wildcard expansion
static KNOWN_SYSCALL_NAMES: &[&str] = &[
    "accept",
    "accept4",
    "access",
    "arch_prctl",
    "bind",
    "brk",
    "chdir",
    "chmod",
    "chown",
    "chroot",
    "clone",
    "clone3",
    "close",
    "connect",
    "dup",
    "dup2",
    "epoll_create",
    "epoll_create1",
    "epoll_ctl",
    "epoll_pwait",
    "epoll_wait",
    "eventfd",
    "eventfd2",
    "execve",
    "exit",
    "exit_group",
    "fchdir",
    "fchmod",
    "fchown",
    "fcntl",
    "flock",
    "fork",
    "fstat",
    "futex",
    "getrandom",
    "getdents64",
    "getegid",
    "geteuid",
    "getgid",
    "getpeername",
    "getpid",
    "getsockname",
    "getsockopt",
    "getuid",
    "ioctl",
    "kill",
    "link",
    "listen",
    "lseek",
    "lstat",
    "memfd_create",
    "mkdir",
    "mmap",
    "mprotect",
    "munmap",
    "newfstatat",
    "open",
    "openat",
    "pipe",
    "pipe2",
    "poll",
    "prctl",
    "pread64",
    "pselect6",
    "pwrite64",
    "read",
    "readlink",
    "recvfrom",
    "recvmsg",
    "rename",
    "rmdir",
    "rseq",
    "rt_sigaction",
    "rt_sigprocmask",
    "select",
    "sendmsg",
    "sendto",
    "set_robust_list",
    "set_tid_address",
    "setgid",
    "setgroups",
    "setsid",
    "setsockopt",
    "setuid",
    "shmat",
    "shmctl",
    "shmdt",
    "shmget",
    "shutdown",
    "sigaction",
    "sigaltstack",
    "socket",
    "stat",
    "statx",
    "symlink",
    "umask",
    "unlink",
    "wait4",
    "waitid",
    "write",
];

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
    fn test_sigaction_alias() {
        #[cfg(target_arch = "x86_64")]
        {
            // "sigaction" should map to rt_sigaction
            assert_eq!(
                syscall_name_to_number("sigaction"),
                syscall_name_to_number("rt_sigaction")
            );
        }
    }

    #[test]
    fn test_wildcard_expansion_epoll() {
        let expanded = expand_wildcard("epoll_*");
        assert!(expanded.contains(&"epoll_create".to_string()));
        assert!(expanded.contains(&"epoll_create1".to_string()));
        assert!(expanded.contains(&"epoll_ctl".to_string()));
        assert!(expanded.contains(&"epoll_wait".to_string()));
        assert!(expanded.contains(&"epoll_pwait".to_string()));
        assert_eq!(expanded.len(), 5);
    }

    #[test]
    fn test_wildcard_expansion_no_match() {
        let expanded = expand_wildcard("zzz_nonexistent_*");
        assert!(expanded.is_empty());
    }

    #[test]
    fn test_wildcard_expansion_no_wildcard() {
        let expanded = expand_wildcard("read");
        assert_eq!(expanded, vec!["read".to_string()]);
    }

    #[test]
    fn test_wildcard_expansion_shm() {
        let expanded = expand_wildcard("shm*");
        assert!(expanded.contains(&"shmat".to_string()));
        assert!(expanded.contains(&"shmdt".to_string()));
        assert!(expanded.contains(&"shmget".to_string()));
        assert!(expanded.contains(&"shmctl".to_string()));
    }

    #[test]
    fn test_describe_filter_minimal_profile() {
        let caps = SyscallCapabilities {
            profile: Some("minimal".to_string()),
            allow: Vec::new(),
            deny: Vec::new(),
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Audit);
        assert_eq!(info.profile, Some("minimal".to_string()));
        assert!(info.allowed_count > 0);
        assert!(info.allowed_syscalls.contains(&"read".to_string()));
        assert!(info.allowed_syscalls.contains(&"write".to_string()));
    }

    #[test]
    fn test_describe_filter_with_explicit_allow() {
        let caps = SyscallCapabilities {
            profile: Some("minimal".to_string()),
            allow: vec!["socket".to_string(), "connect".to_string()],
            deny: Vec::new(),
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Enforce);
        assert!(info.allowed_syscalls.contains(&"socket".to_string()));
        assert!(info.allowed_syscalls.contains(&"connect".to_string()));
        assert!(info.allowed_syscalls.contains(&"read".to_string())); // from profile
    }

    #[test]
    fn test_deny_overrides_allow() {
        let caps = SyscallCapabilities {
            profile: Some("network-server".to_string()),
            allow: Vec::new(),
            deny: vec!["bind".to_string(), "listen".to_string()],
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Enforce);
        // bind and listen should be removed despite being in the network-server profile
        assert!(!info.allowed_syscalls.contains(&"bind".to_string()));
        assert!(!info.allowed_syscalls.contains(&"listen".to_string()));
        // But read should still be present
        assert!(info.allowed_syscalls.contains(&"read".to_string()));
    }

    #[test]
    fn test_deny_wildcard_overrides() {
        let caps = SyscallCapabilities {
            profile: Some("network-server".to_string()),
            allow: Vec::new(),
            deny: vec!["epoll_*".to_string()],
        };

        let info = describe_seccomp_filter(&caps, EnforcementMode::Enforce);
        assert!(!info.allowed_syscalls.contains(&"epoll_create".to_string()));
        assert!(!info.allowed_syscalls.contains(&"epoll_wait".to_string()));
    }

    #[test]
    fn test_build_filter_minimal_profile() {
        let caps = SyscallCapabilities {
            profile: Some("minimal".to_string()),
            allow: Vec::new(),
            deny: Vec::new(),
        };

        // This should succeed on x86_64 (builds the BPF program)
        #[cfg(target_arch = "x86_64")]
        {
            let result = build_seccomp_filter(&caps, EnforcementMode::Audit);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_build_filter_explicit_allow() {
        let caps = SyscallCapabilities {
            profile: None,
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
