// conary-core/src/container/namespaces.rs

use std::ffi::CStr;
use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use nix::sched::CloneFlags;
use nix::unistd::{ForkResult, Pid, fork};

use crate::error::{Error, Result, ScriptletFailureKind};

/// C-library resource identifier accepted by `setrlimit`.
///
/// glibc declares the parameter as `__rlimit_resource_t` while musl declares it
/// as `int`, and each library types its own `RLIMIT_*` constants to match. This
/// alias is the single owner of that ABI difference so callers stay identical
/// on every libc.
#[cfg(target_env = "musl")]
pub(super) type RlimitResource = libc::c_int;
#[cfg(not(target_env = "musl"))]
pub(super) type RlimitResource = libc::__rlimit_resource_t;

pub(super) struct UserNamespaceSync {
    pub(super) request_fd: OwnedFd,
    pub(super) ack_fd: OwnedFd,
}

pub(super) fn fork_process() -> nix::Result<ForkResult> {
    // SAFETY: This is a thin wrapper so callers can keep all fork-specific
    // handling in one place and exercise it in targeted tests.
    unsafe { fork() }
}

pub(super) fn sethostname_syscall(hostname: &CStr, len: usize) -> std::io::Result<()> {
    // SAFETY: `hostname` points to a valid NUL-terminated buffer and `len`
    // matches the intended hostname length for the syscall.
    if unsafe { libc::sethostname(hostname.as_ptr(), len) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(super) fn chroot_syscall(root: &CStr) -> std::io::Result<()> {
    // SAFETY: `root` points to a valid NUL-terminated buffer for the syscall.
    if unsafe { libc::chroot(root.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(super) fn chdir_syscall(path: &CStr) -> std::io::Result<()> {
    // SAFETY: `path` points to a valid NUL-terminated buffer for the syscall.
    if unsafe { libc::chdir(path.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(super) fn set_rlimit_syscall(
    resource: RlimitResource,
    limit: &libc::rlimit,
) -> std::io::Result<()> {
    // SAFETY: `limit` points to initialized memory owned by the caller.
    if unsafe { libc::setrlimit(resource, limit) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(super) fn set_parent_death_signal(signal: libc::c_int) -> std::io::Result<()> {
    // SAFETY: PR_SET_PDEATHSIG accepts the signal number as its second
    // argument and does not dereference any caller-owned memory.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(super) fn sandbox_namespace_flags(flags: CloneFlags) -> CloneFlags {
    if flags.is_empty() {
        flags
    } else {
        flags | CloneFlags::CLONE_NEWUSER
    }
}

pub(super) fn sandbox_host_uid(euid: u32) -> u32 {
    if euid == 0 {
        super::HOST_NOBODY_ID
    } else {
        euid
    }
}

pub(super) fn sandbox_host_gid(egid: u32) -> u32 {
    if egid == 0 {
        super::HOST_NOBODY_ID
    } else {
        egid
    }
}

pub(super) fn namespace_map_contents(host_id: u32) -> String {
    format!("0 {host_id} 1\n")
}

fn write_namespace_map(path: &str, contents: &str) -> Result<()> {
    fs::write(path, contents).map_err(|e| {
        Error::scriptlet(
            ScriptletFailureKind::SandboxSetupUnavailable,
            format!("Failed to write {path}: {e}"),
        )
    })?;
    Ok(())
}

pub(super) fn configure_user_namespace_root_mapping_for_pid(
    pid: Pid,
    host_uid: u32,
    host_gid: u32,
) -> Result<()> {
    let proc_root = format!("/proc/{}", pid.as_raw());
    write_namespace_map(&format!("{proc_root}/setgroups"), "deny")?;
    write_namespace_map(
        &format!("{proc_root}/uid_map"),
        &namespace_map_contents(host_uid),
    )?;
    write_namespace_map(
        &format!("{proc_root}/gid_map"),
        &namespace_map_contents(host_gid),
    )?;
    Ok(())
}

pub(super) fn prepare_user_namespace_entrypoint(root: &Path, script_path: &Path) -> Result<()> {
    prepare_user_namespace_root(root)?;

    let mut script_perms = fs::metadata(script_path)?.permissions();
    script_perms.set_mode(script_perms.mode() | 0o055);
    fs::set_permissions(script_path, script_perms)?;

    Ok(())
}

pub(super) fn prepare_user_namespace_root(root: &Path) -> Result<()> {
    let mut root_perms = fs::metadata(root)?.permissions();
    root_perms.set_mode(root_perms.mode() | 0o011);
    fs::set_permissions(root, root_perms)?;
    Ok(())
}

pub(super) fn signal_parent_user_namespace_ready(
    sync: Option<&UserNamespaceSync>,
    user_namespace_enabled: bool,
) -> Result<()> {
    let Some(sync) = sync else {
        return Ok(());
    };

    let message = if user_namespace_enabled { b"U" } else { b"N" };
    nix::unistd::write(&sync.request_fd, message).map_err(|e| {
        Error::scriptlet(
            ScriptletFailureKind::SandboxSetupUnavailable,
            format!("User namespace handshake request failed: {e}"),
        )
    })?;

    let mut ack = [0_u8; 1];
    let bytes_read = nix::unistd::read(&sync.ack_fd, &mut ack).map_err(|e| {
        Error::scriptlet(
            ScriptletFailureKind::SandboxSetupUnavailable,
            format!("User namespace handshake ack failed: {e}"),
        )
    })?;
    if bytes_read != 1 || ack[0] != b'O' {
        return Err(Error::scriptlet(
            ScriptletFailureKind::SandboxSetupUnavailable,
            "User namespace handshake was not acknowledged",
        ));
    }

    Ok(())
}
