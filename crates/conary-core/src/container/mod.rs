// conary-core/src/container/mod.rs

//! Container isolation for scriptlet execution
//!
//! Provides lightweight Linux container isolation using namespaces for
//! package scriptlet execution.
//!
//! Package lifecycle uses one boundary: a private mount namespace chrooted into
//! a materialized selected root, with the closed scriptlet seccomp contract.
//! The host root is not a lifecycle execution target.
//!
//! Namespace isolation provides:
//! - Isolating process tree (PID namespace)
//! - Isolating hostname (UTS namespace)
//! - Isolating IPC resources (IPC namespace)
//! - Isolating mount topology (mount namespace)
//! - Applying resource limits (CPU, memory, time)
//!
//! ## Pristine Mode
//!
//! For bootstrap builds where host toolchain contamination must be avoided,
//! "pristine mode" creates a container with no host system directories mounted.
//! The container only has access to explicitly provided paths, ensuring builds
//! are reproducible and don't depend on host system state.
//!
//! Based on concepts from Aeryn OS / Serpent OS container isolation.

use crate::capability::enforcement::{self, EnforcementMode, EnforcementPolicy};
use crate::child_wait::wait_with_output;
use crate::error::{Error, Result, ScriptletFailureKind};
use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::sched::{CloneFlags, unshare};
use nix::unistd::{ForkResult, Gid, Pid, Uid};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tracing::warn;

#[cfg(test)]
use nix::sys::wait::{WaitStatus, waitpid};

mod analysis;
#[cfg(test)]
mod child_fork_safety;
mod child_safety;
mod namespaces;

use child_safety::set_rlimit;

pub use analysis::{ScriptAnalysis, ScriptRisk, analyze_script};
use namespaces::{
    RlimitResource, UserNamespaceSync, chdir_syscall, chroot_syscall,
    configure_user_namespace_root_mapping_for_pid, fork_process, prepare_user_namespace_entrypoint,
    prepare_user_namespace_root, sandbox_host_gid, sandbox_host_uid, sandbox_namespace_flags,
    set_parent_death_signal, set_rlimit_syscall, sethostname_syscall,
    signal_parent_user_namespace_ready,
};

/// Default resource limits for sandboxed execution
pub const DEFAULT_MEMORY_LIMIT: u64 = 512 * 1024 * 1024; // 512 MB
pub const DEFAULT_CPU_TIME_LIMIT: u64 = 60; // 60 seconds CPU time
pub const DEFAULT_FILE_SIZE_LIMIT: u64 = 100 * 1024 * 1024; // 100 MB max file size
pub const DEFAULT_NPROC_LIMIT: u64 = 1024; // Max 1024 processes
const HOST_NOBODY_ID: u32 = 65_534;

/// Paths to bind-mount into the container (read-only by default)
#[derive(Debug, Clone)]
pub struct BindMount {
    /// Source path on host
    pub source: PathBuf,
    /// Target path in container
    pub target: PathBuf,
    /// Whether to mount read-write (default is read-only)
    pub writable: bool,
}

impl BindMount {
    pub fn readonly(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            writable: false,
        }
    }

    pub fn writable(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            writable: true,
        }
    }
}

/// Configuration for container isolation
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Enable PID namespace isolation
    pub isolate_pid: bool,
    /// Enable UTS (hostname) namespace isolation
    pub isolate_uts: bool,
    /// Enable IPC namespace isolation
    pub isolate_ipc: bool,
    /// Enable mount namespace isolation
    pub isolate_mount: bool,
    /// Enable network namespace isolation (blocks all network access)
    ///
    /// When enabled, the container has only a loopback interface with no
    /// external network access. This is critical for hermetic builds where
    /// network access during the build phase must be prevented.
    pub isolate_network: bool,
    /// Memory limit in bytes (0 = no limit)
    pub memory_limit: u64,
    /// CPU time limit in seconds (0 = no limit)
    pub cpu_time_limit: u64,
    /// Max file size in bytes (0 = no limit)
    pub file_size_limit: u64,
    /// Max number of processes (0 = no limit)
    pub nproc_limit: u64,
    /// Wall-clock timeout
    pub timeout: Duration,
    /// Hostname to use in container
    pub hostname: String,
    /// Paths to bind-mount into container
    pub bind_mounts: Vec<BindMount>,
    /// Working directory inside container
    pub workdir: PathBuf,
    /// Optional capability enforcement policy (landlock + seccomp)
    pub capability_policy: Option<EnforcementPolicy>,
    /// Owned tempdirs backing private bind mounts (for example, bootstrap /tmp).
    ///
    /// These are kept on the config so the directories live for the lifetime of
    /// any cloned config handed to a sandbox.
    owned_temp_dirs: Vec<Arc<TempDir>>,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            isolate_pid: true,
            isolate_uts: true,
            isolate_ipc: true,
            isolate_mount: true,
            isolate_network: true, // On by default for security
            memory_limit: DEFAULT_MEMORY_LIMIT,
            cpu_time_limit: DEFAULT_CPU_TIME_LIMIT,
            file_size_limit: DEFAULT_FILE_SIZE_LIMIT,
            nproc_limit: DEFAULT_NPROC_LIMIT,
            timeout: Duration::from_secs(60),
            hostname: "conary-sandbox".to_string(),
            bind_mounts: default_bind_mounts(),
            workdir: PathBuf::from("/"),
            capability_policy: None,
            owned_temp_dirs: Vec::new(),
        }
    }
}

impl ContainerConfig {
    fn enforce_untrusted_limit(current: u64, maximum: u64) -> u64 {
        match current {
            0 => maximum,
            value => value.min(maximum),
        }
    }

    /// Create a minimal config with just timeout (no namespace isolation)
    pub fn minimal(timeout: Duration) -> Self {
        Self {
            isolate_pid: false,
            isolate_uts: false,
            isolate_ipc: false,
            isolate_mount: false,
            isolate_network: false,
            memory_limit: 0,
            cpu_time_limit: 0,
            file_size_limit: 0,
            nproc_limit: 0,
            timeout,
            hostname: String::new(),
            bind_mounts: Vec::new(),
            workdir: PathBuf::from("/"),
            capability_policy: None,
            owned_temp_dirs: Vec::new(),
        }
    }

    /// Clamp a config to the minimum isolation required for untrusted code.
    pub fn for_untrusted(mut self) -> Self {
        self.isolate_pid = true;
        self.isolate_uts = true;
        self.isolate_ipc = true;
        self.isolate_mount = true;
        self.isolate_network = true;
        self.memory_limit = Self::enforce_untrusted_limit(self.memory_limit, DEFAULT_MEMORY_LIMIT);
        self.cpu_time_limit =
            Self::enforce_untrusted_limit(self.cpu_time_limit, DEFAULT_CPU_TIME_LIMIT);
        self.file_size_limit =
            Self::enforce_untrusted_limit(self.file_size_limit, DEFAULT_FILE_SIZE_LIMIT);
        self.nproc_limit = Self::enforce_untrusted_limit(self.nproc_limit, DEFAULT_NPROC_LIMIT);
        if self.hostname.is_empty() {
            self.hostname = "conary-sandbox".to_string();
        }
        self.deny_network();
        self
    }

    /// Create a strict config with maximum isolation
    pub fn strict() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            ..Self::default()
        }
    }

    /// Create a pristine config with NO host system mounts
    ///
    /// This is critical for bootstrap builds where host toolchain contamination
    /// must be avoided. The container will only have access to paths explicitly
    /// added via `add_bind_mount()` after creation.
    ///
    /// Use this when:
    /// - Building stage 0/1 toolchains for bootstrap
    /// - Creating reproducible builds that don't depend on host
    /// - Testing package builds in isolation
    ///
    /// Note: The container will need explicit mounts for:
    /// - Source code directory
    /// - Destination/install directory
    /// - Any toolchain (e.g., /tools for cross-compiler)
    ///
    /// # Example
    /// ```ignore
    /// let mut config = ContainerConfig::pristine();
    /// // Mount only the toolchain and build directories
    /// config.add_bind_mount(BindMount::readonly("/tools", "/tools"));
    /// config.add_bind_mount(BindMount::readonly("/src", "/src"));
    /// config.add_bind_mount(BindMount::writable("/build", "/build"));
    /// ```
    pub fn pristine() -> Self {
        Self {
            isolate_pid: true,
            isolate_uts: true,
            isolate_ipc: true,
            isolate_mount: true,
            isolate_network: true, // Pristine mode includes network isolation for hermetic builds
            memory_limit: DEFAULT_MEMORY_LIMIT,
            cpu_time_limit: 0,  // No CPU limit for long builds
            file_size_limit: 0, // No file size limit for builds
            nproc_limit: DEFAULT_NPROC_LIMIT,
            timeout: Duration::from_secs(3600), // 1 hour for builds
            hostname: "conary-pristine".to_string(),
            bind_mounts: Vec::new(), // No host mounts!
            workdir: PathBuf::from("/"),
            capability_policy: None,
            owned_temp_dirs: Vec::new(),
        }
    }

    /// Create a pristine config suitable for bootstrap builds
    ///
    /// This is a convenience method that creates a pristine container
    /// pre-configured for bootstrap scenarios with a specific sysroot.
    ///
    /// # Arguments
    /// * `sysroot` - Path to the toolchain sysroot (e.g., /opt/stage0)
    /// * `source_dir` - Path to source code directory
    /// * `build_dir` - Path to build directory (writable)
    /// * `dest_dir` - Path to install destination (writable)
    pub fn pristine_for_bootstrap(
        sysroot: &Path,
        source_dir: &Path,
        build_dir: &Path,
        dest_dir: &Path,
    ) -> Self {
        let mut config = Self::pristine();

        // Mount host essentials (read-only) so the sandbox has a working
        // shell, dynamic linker, and basic tools (bash, make, coreutils).
        // The toolchain gcc is in the sysroot; everything else comes from
        // the host.
        for dir in &[
            "/usr/bin",
            "/usr/lib",
            "/usr/lib64",
            "/lib64",
            "/bin",
            "/lib",
        ] {
            let p = std::path::Path::new(dir);
            if p.exists() {
                config.add_bind_mount(BindMount::readonly(p, p));
            }
        }

        add_private_tmp_mount(&mut config);

        // Mount the toolchain sysroot (read-only)
        config.add_bind_mount(BindMount::readonly(sysroot, sysroot));

        // Standard toolchain paths often expected at /tools
        if sysroot != Path::new("/tools") {
            config.add_bind_mount(BindMount::readonly(sysroot, "/tools"));
        }

        // Source code (read-only to prevent accidental modification)
        config.add_bind_mount(BindMount::readonly(source_dir, source_dir));

        // Build directory (writable for object files, etc.)
        config.add_bind_mount(BindMount::writable(build_dir, build_dir));

        // Destination directory (writable for `make install DESTDIR=...`)
        config.add_bind_mount(BindMount::writable(dest_dir, dest_dir));

        // Set working directory to build directory
        config.workdir = build_dir.to_path_buf();

        config
    }

    /// Create a hermetic build config backed only by a configured sysroot.
    ///
    /// Unlike `pristine_for_bootstrap`, this does not bind host system tool
    /// directories. Standard tool paths inside the sandbox are backed by the
    /// operator-supplied sysroot so M2a hermetic evidence can honestly claim
    /// no host system mounts.
    pub fn hermetic_for_sysroot(
        sysroot: &Path,
        source_dir: &Path,
        build_dir: &Path,
        dest_dir: &Path,
    ) -> Self {
        let mut config = Self::pristine();

        add_private_tmp_mount(&mut config);

        config.add_bind_mount(BindMount::readonly(sysroot, sysroot));
        if sysroot != Path::new("/tools") {
            config.add_bind_mount(BindMount::readonly(sysroot, "/tools"));
        }

        for (source, target) in [
            ("bin", "/bin"),
            ("sbin", "/sbin"),
            ("usr/bin", "/usr/bin"),
            ("usr/sbin", "/usr/sbin"),
            ("lib", "/lib"),
            ("lib64", "/lib64"),
            ("usr/lib", "/usr/lib"),
            ("usr/lib64", "/usr/lib64"),
        ] {
            config.add_bind_mount(BindMount::readonly(sysroot.join(source), target));
        }

        config.add_bind_mount(BindMount::readonly(source_dir, source_dir));
        config.add_bind_mount(BindMount::writable(build_dir, build_dir));
        config.add_bind_mount(BindMount::writable(dest_dir, dest_dir));
        config.workdir = build_dir.to_path_buf();

        config
    }

    /// Add a custom bind mount
    pub fn add_bind_mount(&mut self, mount: BindMount) {
        self.bind_mounts.push(mount);
    }

    /// Add a writable sandbox layer backed by an owned temporary directory.
    ///
    /// The tempdir is kept alive by the config and is owned by the host UID/GID
    /// that root maps to inside the sandbox user namespace, so scriptlets can
    /// write there without touching the corresponding live host path.
    pub fn add_private_writable_mount(
        &mut self,
        target: impl Into<PathBuf>,
        mode: u32,
    ) -> Result<PathBuf> {
        let target = target.into();
        let private_dir = Arc::new(TempDir::new()?);
        let private_path = private_dir.path();
        let host_uid = sandbox_host_uid(Uid::effective().as_raw());
        let host_gid = sandbox_host_gid(Gid::effective().as_raw());
        let metadata = fs::metadata(private_path)?;

        if metadata.uid() != host_uid || metadata.gid() != host_gid {
            nix::unistd::chown(
                private_path,
                Some(Uid::from_raw(host_uid)),
                Some(Gid::from_raw(host_gid)),
            )
            .map_err(|error| {
                Error::scriptlet(
                    ScriptletFailureKind::SandboxSetupUnavailable,
                    format!(
                        "failed to assign private sandbox layer {} ownership to {host_uid}:{host_gid}: {error}",
                        private_path.display()
                    ),
                )
            })?;
        }

        let mut permissions = fs::metadata(private_path)?.permissions();
        permissions.set_mode(mode);
        fs::set_permissions(private_path, permissions)?;

        let source = private_path.to_path_buf();
        self.add_bind_mount(BindMount::writable(&source, target));
        self.owned_temp_dirs.push(private_dir);

        Ok(source)
    }

    /// Check if this is a pristine (no host mounts) configuration.
    /// Mounting specific directories like /usr/bin for host tools is
    /// acceptable for bootstrap; mounting all of /usr is not.
    pub fn is_pristine(&self) -> bool {
        !self.bind_mounts.iter().any(|m| {
            let src = m.source.to_string_lossy();
            src == "/usr" || src == "/sbin" || src == "/"
        })
    }

    /// Create a hermetic config for BuildStream-grade reproducible builds
    ///
    /// This configuration provides maximum isolation for fully reproducible builds:
    /// - Complete network isolation (only loopback interface)
    /// - No host system mounts (pristine filesystem)
    /// - All namespaces isolated (PID, UTS, IPC, mount, network)
    ///
    /// Use this for builds that must be 100% reproducible and cannot depend
    /// on any external state or network resources.
    pub fn hermetic() -> Self {
        Self::pristine() // pristine() already includes network isolation
    }

    /// Allow network access in the container
    ///
    /// Disables network namespace isolation. Use this for:
    /// - Fetch phases that need to download sources
    /// - Scriptlets that legitimately need network access
    ///
    /// Note: This reduces build reproducibility guarantees.
    pub fn allow_network(&mut self) {
        self.isolate_network = false;
        // Add resolv.conf mount when network is allowed
        if !self
            .bind_mounts
            .iter()
            .any(|m| m.target.to_string_lossy().contains("resolv.conf"))
        {
            self.bind_mounts
                .push(BindMount::readonly("/etc/resolv.conf", "/etc/resolv.conf"));
        }
    }

    /// Deny network access in the container
    ///
    /// Enables network namespace isolation. The container will have
    /// only a loopback interface with no external network access.
    pub fn deny_network(&mut self) {
        self.isolate_network = true;
        // Remove resolv.conf mount when network is denied (useless without network)
        self.bind_mounts
            .retain(|m| !m.target.to_string_lossy().contains("resolv.conf"));
    }
}

/// Get default bind mounts for scriptlet execution
///
/// Note: `/etc/resolv.conf` is NOT included by default since network isolation
/// is enabled by default. Use `allow_network()` to add it when needed.
fn default_bind_mounts() -> Vec<BindMount> {
    vec![
        // Essential system directories (read-only)
        BindMount::readonly("/usr", "/usr"),
        BindMount::readonly("/lib", "/lib"),
        BindMount::readonly("/lib64", "/lib64"),
        BindMount::readonly("/bin", "/bin"),
        BindMount::readonly("/sbin", "/sbin"),
        // Config files scripts might need (no resolv.conf - network is isolated by default)
        BindMount::readonly("/etc/passwd", "/etc/passwd"),
        BindMount::readonly("/etc/group", "/etc/group"),
        BindMount::readonly("/etc/hosts", "/etc/hosts"),
    ]
}

fn add_private_tmp_mount(config: &mut ContainerConfig) {
    let private_tmp = Arc::new(TempDir::new().expect("failed to create private sandbox tmpdir"));
    let mut perms = fs::metadata(private_tmp.path())
        .expect("failed to stat private sandbox tmpdir")
        .permissions();
    perms.set_mode(0o1777);
    fs::set_permissions(private_tmp.path(), perms).expect("failed to chmod private sandbox tmpdir");
    config.add_bind_mount(BindMount::writable(private_tmp.path(), "/tmp"));
    config.owned_temp_dirs.push(private_tmp);
}

/// Write script content to a file and set it executable (mode 0o700).
pub fn write_executable_script(path: &Path, content: &str) -> Result<()> {
    write_executable_script_bytes(path, content.as_bytes())
}

/// Write exact script bytes to a file and set it executable (mode 0o700).
pub fn write_executable_script_bytes(path: &Path, content: &[u8]) -> Result<()> {
    let mut f = File::create(path)?;
    f.write_all(content)?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn prepared_stdin(content: &[u8]) -> Result<File> {
    let mut file = tempfile::tempfile()?;
    file.write_all(content)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

/// Container sandbox for executing scriptlets
pub struct Sandbox {
    config: ContainerConfig,
}

mod execution;

/// Check if namespace isolation is available
pub fn isolation_available() -> bool {
    // Check if we're root
    if nix::unistd::geteuid().is_root() {
        return true;
    }

    // Check for unprivileged user namespaces (Debian/Ubuntu specific)
    let path = Path::new("/proc/sys/kernel/unprivileged_userns_clone");
    if path.exists() {
        if let Ok(content) = fs::read_to_string(path)
            && content.trim() == "1"
        {
            return true;
        }
        return false;
    }

    // On standard kernels, unprivileged userns are enabled by default
    true
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
