// conary-core/src/scriptlet/boundary.rs

//! Stable selected-root security boundary for native lifecycle execution.
//!
//! Ordinary libc and helper syscalls are not compatibility authority. The
//! boundary isolates host-global resources, removes process capabilities that
//! are not needed to mutate the selected filesystem, and denies only
//! host-escape or kernel-control syscall classes.

use crate::error::{Error, Result, ScriptletFailureKind};
use caps::{CapSet, Capability};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
use std::collections::{BTreeMap, HashSet};
use std::io;

pub(crate) const SCRIPTLET_BOUNDARY_ABI_V2: &str = "scriptlet-boundary-v2";

const CAPABILITY_ABI_V3: u32 = 0x2008_0522;

const RETAINED_FILESYSTEM_CAPABILITIES: &[Capability] = &[
    Capability::CAP_CHOWN,
    Capability::CAP_DAC_OVERRIDE,
    Capability::CAP_DAC_READ_SEARCH,
    Capability::CAP_FOWNER,
    Capability::CAP_FSETID,
    Capability::CAP_SETGID,
    Capability::CAP_SETUID,
    Capability::CAP_SETFCAP,
];

const SETUP_CAPABILITIES: &[Capability] = &[
    Capability::CAP_SETPCAP,
    Capability::CAP_SYS_ADMIN,
    Capability::CAP_SYS_CHROOT,
];

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
const BLOCKED_COMMON_SYSCALLS: &[(&str, i64)] = &[
    ("acct", libc::SYS_acct),
    ("add_key", libc::SYS_add_key),
    ("adjtimex", libc::SYS_adjtimex),
    ("bpf", libc::SYS_bpf),
    ("chroot", libc::SYS_chroot),
    ("clock_adjtime", libc::SYS_clock_adjtime),
    ("clock_settime", libc::SYS_clock_settime),
    ("delete_module", libc::SYS_delete_module),
    ("finit_module", libc::SYS_finit_module),
    ("fsconfig", libc::SYS_fsconfig),
    ("fsmount", libc::SYS_fsmount),
    ("fsopen", libc::SYS_fsopen),
    ("fspick", libc::SYS_fspick),
    ("init_module", libc::SYS_init_module),
    ("kcmp", libc::SYS_kcmp),
    ("keyctl", libc::SYS_keyctl),
    ("kill", libc::SYS_kill),
    ("kexec_load", libc::SYS_kexec_load),
    ("mount", libc::SYS_mount),
    ("mount_setattr", libc::SYS_mount_setattr),
    ("move_mount", libc::SYS_move_mount),
    ("name_to_handle_at", libc::SYS_name_to_handle_at),
    ("open_by_handle_at", libc::SYS_open_by_handle_at),
    ("open_tree", libc::SYS_open_tree),
    ("perf_event_open", libc::SYS_perf_event_open),
    ("pidfd_getfd", libc::SYS_pidfd_getfd),
    ("pidfd_open", libc::SYS_pidfd_open),
    ("pidfd_send_signal", libc::SYS_pidfd_send_signal),
    ("pivot_root", libc::SYS_pivot_root),
    ("process_madvise", libc::SYS_process_madvise),
    ("process_mrelease", libc::SYS_process_mrelease),
    ("process_vm_readv", libc::SYS_process_vm_readv),
    ("process_vm_writev", libc::SYS_process_vm_writev),
    ("ptrace", libc::SYS_ptrace),
    ("quotactl", libc::SYS_quotactl),
    ("quotactl_fd", libc::SYS_quotactl_fd),
    ("reboot", libc::SYS_reboot),
    ("request_key", libc::SYS_request_key),
    ("setpgid", libc::SYS_setpgid),
    ("setsid", libc::SYS_setsid),
    ("setns", libc::SYS_setns),
    ("settimeofday", libc::SYS_settimeofday),
    ("swapoff", libc::SYS_swapoff),
    ("swapon", libc::SYS_swapon),
    ("syslog", libc::SYS_syslog),
    ("tgkill", libc::SYS_tgkill),
    ("tkill", libc::SYS_tkill),
    ("umount2", libc::SYS_umount2),
];

#[cfg(target_arch = "x86_64")]
const BLOCKED_ARCH_SYSCALLS: &[(&str, i64)] = &[
    ("ioperm", libc::SYS_ioperm),
    ("iopl", libc::SYS_iopl),
    ("kexec_file_load", libc::SYS_kexec_file_load),
];

#[cfg(target_arch = "aarch64")]
const BLOCKED_ARCH_SYSCALLS: &[(&str, i64)] = &[("kexec_file_load", libc::SYS_kexec_file_load)];

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const BLOCKED_COMMON_SYSCALLS: &[(&str, i64)] = &[];

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const BLOCKED_ARCH_SYSCALLS: &[(&str, i64)] = &[];

#[derive(Debug)]
pub(super) struct ScriptletBoundary {
    seccomp_filter: BpfProgram,
    capabilities: CapabilityPlan,
}

#[derive(Debug)]
struct CapabilityPlan {
    retained_mask: u64,
    drop_bounding: Vec<u8>,
}

impl ScriptletBoundary {
    pub(super) fn prepare() -> Result<Self> {
        if !crate::capability::enforcement::seccomp_enforce::check_seccomp_support() {
            return Err(enforcement_error("kernel seccomp support is unavailable"));
        }
        Ok(Self {
            seccomp_filter: build_scriptlet_boundary_filter()?,
            capabilities: CapabilityPlan::prepare()?,
        })
    }

    pub(super) fn namespace_flags(&self) -> nix::sched::CloneFlags {
        namespace_flags()
    }

    pub(super) fn mount_private_flags(&self) -> nix::mount::MsFlags {
        nix::mount::MsFlags::MS_PRIVATE | nix::mount::MsFlags::MS_REC
    }

    /// Finish the boundary after bind mounts and chroot are complete.
    ///
    /// This runs in `Command::pre_exec`; every operation is one direct Linux
    /// syscall or a precompiled seccomp program application.
    pub(super) fn install_after_chroot(&self) -> io::Result<()> {
        if unsafe { libc::setpgid(0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        clear_ambient_capabilities()?;
        self.capabilities.drop_bounding_set()?;
        self.capabilities.install_process_sets()?;
        self.capabilities.raise_ambient_set()?;
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        seccompiler::apply_filter(&self.seccomp_filter).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "scriptlet boundary seccomp filter application failed",
            )
        })
    }
}

impl CapabilityPlan {
    fn prepare() -> Result<Self> {
        let effective = caps::read(None, CapSet::Effective).map_err(|error| {
            enforcement_error(format!("cannot read effective capabilities: {error}"))
        })?;
        for required in SETUP_CAPABILITIES {
            if !effective.contains(required) {
                return Err(Error::scriptlet(
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    format!(
                        "{SCRIPTLET_BOUNDARY_ABI_V2} requires {required} while constructing its namespaces and root boundary"
                    ),
                ));
            }
        }

        let permitted = caps::read(None, CapSet::Permitted).map_err(|error| {
            enforcement_error(format!("cannot read permitted capabilities: {error}"))
        })?;
        let bounding = caps::read(None, CapSet::Bounding).map_err(|error| {
            enforcement_error(format!("cannot read bounding capabilities: {error}"))
        })?;
        let retained = RETAINED_FILESYSTEM_CAPABILITIES
            .iter()
            .copied()
            .filter(|capability| permitted.contains(capability) && bounding.contains(capability))
            .collect::<HashSet<_>>();
        let retained_mask = retained
            .iter()
            .fold(0_u64, |mask, capability| mask | capability.bitmask());
        let mut drop_bounding = caps::runtime::thread_all_supported()
            .into_iter()
            .filter(|capability| !retained.contains(capability))
            .map(|capability| capability.index())
            .collect::<Vec<_>>();
        drop_bounding.sort_unstable();

        Ok(Self {
            retained_mask,
            drop_bounding,
        })
    }

    fn drop_bounding_set(&self) -> io::Result<()> {
        for capability in &self.drop_bounding {
            if unsafe {
                libc::prctl(
                    libc::PR_CAPBSET_DROP,
                    libc::c_ulong::from(*capability),
                    0,
                    0,
                    0,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn install_process_sets(&self) -> io::Result<()> {
        let header = LinuxCapabilityHeader {
            version: CAPABILITY_ABI_V3,
            pid: 0,
        };
        let data = LinuxCapabilityData {
            effective_low: self.retained_mask as u32,
            permitted_low: self.retained_mask as u32,
            inheritable_low: self.retained_mask as u32,
            effective_high: (self.retained_mask >> 32) as u32,
            permitted_high: (self.retained_mask >> 32) as u32,
            inheritable_high: (self.retained_mask >> 32) as u32,
        };
        if unsafe { libc::syscall(libc::SYS_capset, &header, &data) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn raise_ambient_set(&self) -> io::Result<()> {
        for capability in RETAINED_FILESYSTEM_CAPABILITIES {
            if self.retained_mask & capability.bitmask() == 0 {
                continue;
            }
            if unsafe {
                libc::prctl(
                    libc::PR_CAP_AMBIENT,
                    libc::PR_CAP_AMBIENT_RAISE,
                    libc::c_ulong::from(capability.index()),
                    0,
                    0,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[repr(C)]
struct LinuxCapabilityHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
struct LinuxCapabilityData {
    effective_low: u32,
    permitted_low: u32,
    inheritable_low: u32,
    effective_high: u32,
    permitted_high: u32,
    inheritable_high: u32,
}

fn clear_ambient_capabilities() -> io::Result<()> {
    if unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn namespace_flags() -> nix::sched::CloneFlags {
    nix::sched::CloneFlags::CLONE_NEWNS
        | nix::sched::CloneFlags::CLONE_NEWNET
        | nix::sched::CloneFlags::CLONE_NEWIPC
        | nix::sched::CloneFlags::CLONE_NEWUTS
        | nix::sched::CloneFlags::CLONE_NEWCGROUP
}

pub(super) fn build_scriptlet_boundary_filter() -> Result<BpfProgram> {
    if BLOCKED_COMMON_SYSCALLS.is_empty() {
        return Err(enforcement_error(format!(
            "{SCRIPTLET_BOUNDARY_ABI_V2} is unsupported on {}",
            std::env::consts::ARCH
        )));
    }
    let rules = blocked_syscalls()
        .map(|(_, number)| (number, Vec::<SeccompRule>::new()))
        .collect::<BTreeMap<_, _>>();
    let arch: seccompiler::TargetArch = std::env::consts::ARCH.try_into().map_err(|_| {
        enforcement_error(format!(
            "{SCRIPTLET_BOUNDARY_ABI_V2} cannot target architecture {}",
            std::env::consts::ARCH
        ))
    })?;
    SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|error| enforcement_error(format!("cannot build boundary filter: {error}")))?
    .try_into()
    .map_err(|error| enforcement_error(format!("cannot compile boundary filter: {error}")))
}

fn blocked_syscalls() -> impl Iterator<Item = (&'static str, i64)> {
    BLOCKED_COMMON_SYSCALLS
        .iter()
        .chain(BLOCKED_ARCH_SYSCALLS)
        .copied()
}

fn enforcement_error(message: impl Into<String>) -> Error {
    Error::scriptlet(
        ScriptletFailureKind::EnforcementSetupFailed,
        format!("{SCRIPTLET_BOUNDARY_ABI_V2}: {}", message.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn boundary_uses_resource_and_identity_namespaces() {
        let flags = namespace_flags();
        for required in [
            nix::sched::CloneFlags::CLONE_NEWNS,
            nix::sched::CloneFlags::CLONE_NEWNET,
            nix::sched::CloneFlags::CLONE_NEWIPC,
            nix::sched::CloneFlags::CLONE_NEWUTS,
            nix::sched::CloneFlags::CLONE_NEWCGROUP,
        ] {
            assert!(flags.contains(required));
        }
    }

    #[test]
    fn boundary_retains_only_selected_root_filesystem_capabilities() {
        for retained in [
            Capability::CAP_CHOWN,
            Capability::CAP_DAC_OVERRIDE,
            Capability::CAP_FOWNER,
            Capability::CAP_SETFCAP,
        ] {
            assert!(RETAINED_FILESYSTEM_CAPABILITIES.contains(&retained));
        }
        for forbidden in [
            Capability::CAP_KILL,
            Capability::CAP_MKNOD,
            Capability::CAP_NET_ADMIN,
            Capability::CAP_NET_RAW,
            Capability::CAP_SETPCAP,
            Capability::CAP_SYS_ADMIN,
            Capability::CAP_SYS_CHROOT,
            Capability::CAP_SYS_MODULE,
            Capability::CAP_SYS_PTRACE,
        ] {
            assert!(!RETAINED_FILESYSTEM_CAPABILITIES.contains(&forbidden));
        }
    }

    #[test]
    fn boundary_denies_host_control_not_ordinary_helper_abis() {
        let blocked = blocked_syscalls()
            .map(|(name, _)| name)
            .collect::<std::collections::BTreeSet<_>>();
        for forbidden in [
            "bpf",
            "chroot",
            "kill",
            "mount",
            "open_by_handle_at",
            "ptrace",
            "reboot",
            "setpgid",
            "setns",
            "setsid",
        ] {
            assert!(blocked.contains(forbidden), "{forbidden}");
        }
        for ordinary in [
            "clone",
            "clone3",
            "execve",
            "io_uring_setup",
            "mkdir",
            "mkdirat",
            "openat",
            "statx",
            "unshare",
            "userfaultfd",
            "vfork",
        ] {
            assert!(!blocked.contains(ordinary), "{ordinary}");
        }
        let unique_numbers = blocked_syscalls()
            .map(|(_, number)| number)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique_numbers.len(), blocked.len());
        build_scriptlet_boundary_filter().expect("boundary deny filter must compile");
    }

    #[test]
    fn boundary_filter_enforces_the_class_contract_in_a_disposable_process() {
        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "scriptlet::boundary::tests::boundary_filter_subprocess",
                "--ignored",
            ])
            .status()
            .expect("spawn disposable seccomp test process");
        assert!(status.success());
    }

    #[test]
    #[ignore = "invoked in a disposable process by the boundary contract test"]
    fn boundary_filter_subprocess() {
        let root = tempfile::tempdir().expect("create ordinary filesystem test root");
        let filter = build_scriptlet_boundary_filter().expect("compile boundary filter");
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
            0,
            "enable no_new_privs"
        );
        seccompiler::apply_filter(&filter).expect("apply boundary filter");

        std::fs::create_dir(root.path().join("ordinary-mkdir"))
            .expect("ordinary filesystem ABI stays available");
        assert_eq!(
            unsafe { libc::unshare(libc::CLONE_NEWNS) },
            -1,
            "host-boundary control syscall must be rejected"
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    }
}
