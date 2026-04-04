// conary-core/src/scriptlet/mod.rs

//! Scriptlet execution for package install/remove hooks
//!
//! This module handles executing package scriptlets with cross-distro support
//! for RPM, DEB, and Arch packages. Key features:
//!
//! - Distro-specific argument handling:
//!   - RPM: Integer count ($1=1 install, $1=2 upgrade, $1=0 remove)
//!   - DEB: Action words per Debian Policy ($1=install/configure/remove/upgrade)
//!   - Arch: Version strings ($1=new_version, $2=old_version for upgrades)
//! - Arch .INSTALL function wrapper generation
//! - Timeout protection (60 seconds)
//! - stdin nullification to prevent hangs
//! - Target root support: scriptlets can run inside a target filesystem
//! - Optional container isolation for untrusted scripts
//!
//! ## Target Root Support
//!
//! When installing to a target root (root != "/"), scriptlets are executed
//! inside a chroot or container rooted at the target path. This allows:
//! - Bootstrap: Running package scripts during system construction
//! - Container images: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems
//!
//! The target root must have a working shell and interpreter for scriptlets
//! to execute successfully.

use crate::container::{BindMount, ContainerConfig, Sandbox, ScriptRisk, analyze_script};
use crate::capability::enforcement::EnforcementMode;
use crate::db::models::ScriptletEntry;
use crate::error::{Error, Result};
use crate::packages::traits::{Scriptlet, ScriptletPhase};
use runtime::{
    apply_sanitized_command_env, build_scriptlet_seccomp, chroot_mount_private_flags,
    chroot_namespace_flags, current_seccomp_mode, log_script_output, wait_and_capture,
    write_executable_script,
};
use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tracing::{debug, info, warn};

mod runtime;

/// Sandbox mode for scriptlet execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    /// No sandboxing - direct execution
    None,
    /// Automatic - sandbox based on script risk analysis
    Auto,
    /// Always sandbox all scripts
    #[default]
    Always,
}

impl SandboxMode {
    /// Parse sandbox mode from string (auto, always, never)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "never" | "none" | "off" | "false" => Some(Self::None),
            "auto" => Some(Self::Auto),
            "always" | "on" | "true" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Default timeout for scriptlet execution (60 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const LIVE_SANDBOX_PROTECTED_ETC_FILES: [&str; 3] = ["/etc/passwd", "/etc/shadow", "/etc/sudoers"];

/// Package format types for argument handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Rpm,
    Deb,
    Arch,
}

impl PackageFormat {
    /// Parse from string representation
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rpm" => Some(Self::Rpm),
            "deb" => Some(Self::Deb),
            "arch" => Some(Self::Arch),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Arch => "arch",
        }
    }
}

/// Execution mode determines arguments passed to scriptlets
#[derive(Debug, Clone)]
pub enum ExecutionMode {
    /// Fresh install
    Install,
    /// Package removal
    Remove,
    /// Upgrade from old version (for NEW package scriptlets)
    Upgrade { old_version: String },
    /// Upgrade removal (for OLD package scriptlets during upgrade)
    /// RPM: $1=1 (not 0, signaling "another version remains")
    /// DEB: "upgrade <new_version>" (not "remove")
    /// Arch: Should NOT be used - Arch skips old package scripts during upgrade
    UpgradeRemoval { new_version: String },
}

/// Scriptlet executor with cross-distro support
pub struct ScriptletExecutor {
    root: PathBuf,
    package_name: String,
    package_version: String,
    package_format: PackageFormat,
    timeout: Duration,
    sandbox_mode: SandboxMode,
}

impl ScriptletExecutor {
    /// Create a new executor
    pub fn new(root: &Path, name: &str, version: &str, format: PackageFormat) -> Self {
        Self {
            root: root.to_path_buf(),
            package_name: name.to_string(),
            package_version: version.to_string(),
            package_format: format,
            timeout: DEFAULT_TIMEOUT,
            sandbox_mode: SandboxMode::default(),
        }
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set sandbox mode for scriptlet execution
    pub fn with_sandbox_mode(mut self, mode: SandboxMode) -> Self {
        self.sandbox_mode = mode;
        self
    }

    /// Execute a scriptlet from package parsing
    pub fn execute(&self, scriptlet: &Scriptlet, mode: &ExecutionMode) -> Result<()> {
        self.execute_impl(
            &scriptlet.phase.to_string(),
            &scriptlet.interpreter,
            &scriptlet.content,
            scriptlet.flags.as_deref(),
            mode,
        )
    }

    /// Execute a scriptlet from database entry
    pub fn execute_entry(&self, entry: &ScriptletEntry, mode: &ExecutionMode) -> Result<()> {
        self.execute_impl(
            &entry.phase,
            &entry.interpreter,
            &entry.content,
            entry.flags.as_deref(),
            mode,
        )
    }

    /// Check if we're operating on the live root
    fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    /// Core execution implementation
    fn execute_impl(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        _flags: Option<&str>,
        mode: &ExecutionMode,
    ) -> Result<()> {
        // Prepare script content (Arch needs wrapper generation)
        let script_content = if self.package_format == PackageFormat::Arch {
            self.prepare_arch_wrapper(content, phase)
        } else {
            content.to_string()
        };

        // Analyze script for dangerous patterns
        let analysis = analyze_script(&script_content);

        // Determine if we should sandbox based on mode and risk
        let use_sandbox = match self.sandbox_mode {
            SandboxMode::None => false,
            SandboxMode::Always => true,
            SandboxMode::Auto => {
                // Sandbox if risk is Medium or higher
                analysis.risk >= ScriptRisk::Medium
            }
        };

        if !analysis.patterns.is_empty() {
            info!(
                "{} scriptlet risk analysis: {} - {:?}",
                phase,
                analysis.risk.as_str(),
                analysis.patterns
            );
        }

        // Resolve interpreter (Arch always uses bash for wrapper)
        let interpreter_path = if self.package_format == PackageFormat::Arch {
            "/bin/bash".to_string()
        } else {
            interpreter.to_string()
        };

        // For target root installs, validate interpreter exists IN TARGET
        // For live root, validate it exists on the host
        let interpreter_check_path = if self.is_live_root() {
            PathBuf::from(&interpreter_path)
        } else {
            self.root.join(interpreter_path.trim_start_matches('/'))
        };

        if !interpreter_check_path.exists() {
            if self.is_live_root() {
                return Err(Error::ScriptletError(format!(
                    "Interpreter not found: {}. Cannot execute {} scriptlet.",
                    interpreter_path, phase
                )));
            } else {
                // For target root, warn but don't fail - the scriptlet might not be needed
                // or the target might be in early bootstrap (no shell yet)
                warn!(
                    "Interpreter {} not found in target root {}, skipping {} scriptlet",
                    interpreter_path,
                    self.root.display(),
                    phase
                );
                return Ok(());
            }
        }

        // Prepare arguments based on distro, mode, and phase
        let args = self.get_args(mode, phase);

        // Build environment variables
        let env = [
            ("CONARY_PACKAGE_NAME", self.package_name.as_str()),
            ("CONARY_PACKAGE_VERSION", self.package_version.as_str()),
            ("CONARY_ROOT", "/"), // Always "/" from script's perspective
            ("CONARY_PHASE", phase),
        ];

        info!(
            "Executing {} scriptlet for {} v{} (root: {}, sandbox: {})",
            phase,
            self.package_name,
            self.package_version,
            self.root.display(),
            use_sandbox
        );

        if self.is_live_root() {
            // Live root execution
            if use_sandbox {
                self.execute_sandbox_live(phase, &interpreter_path, &script_content, &args, &env)
            } else {
                self.execute_direct(phase, &interpreter_path, &script_content, &args, &env)
            }
        } else {
            // Target root execution - always use chroot/container
            self.execute_in_target(phase, &interpreter_path, &script_content, &args, &env)
        }
    }

    /// Execute scriptlet in sandbox on live root
    fn execute_sandbox_live(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        // NOTE: This sandbox mode provides namespace isolation (PID, mount,
        // network) but NOT full write isolation for /var and /etc. The
        // container runtime does not create tmpfs overlays, so these must
        // remain writable for scriptlets that update ldconfig, systemd
        // state, etc. A small set of critical account-management files is
        // rebound read-only after the writable /etc mount. True isolation
        // still requires the target-root chroot path (execute_in_target)
        // or a future overlay-backed sandbox.
        //
        // TODO: Add tmpfs overlay support to the container runtime so
        // sandbox_live can capture scriptlet writes without mutating the
        // host. Until then, this mode provides process/network isolation
        // only, not filesystem isolation for /var and /etc.
        let mut sandbox = Sandbox::new(self.live_sandbox_config());
        let (code, stdout, stderr) = sandbox.execute(interpreter, content, args, env)?;

        log_script_output(phase, &stdout, &stderr);

        if code == 0 {
            info!("{} scriptlet completed successfully (sandboxed)", phase);
            Ok(())
        } else {
            Err(Error::ScriptletError(format!(
                "{} scriptlet failed with exit code {} (sandboxed)",
                phase, code
            )))
        }
    }

    fn live_sandbox_config(&self) -> ContainerConfig {
        let mut config = ContainerConfig::default().for_untrusted();
        config.timeout = self.timeout;
        config.bind_mounts.retain(|mount| {
            !LIVE_SANDBOX_PROTECTED_ETC_FILES
                .iter()
                .any(|protected| mount.target == Path::new(protected))
        });
        config.bind_mounts.push(BindMount::writable("/var", "/var"));
        config.bind_mounts.push(BindMount::writable("/etc", "/etc"));

        for protected in LIVE_SANDBOX_PROTECTED_ETC_FILES {
            config
                .bind_mounts
                .push(BindMount::readonly(protected, protected));
        }

        config
    }

    /// Execute scriptlet inside a target root using chroot/container
    ///
    /// This is the key method for bootstrap support. It runs the scriptlet
    /// inside the target filesystem using either:
    /// - chroot (requires root, simpler)
    /// - namespace container (more isolation)
    fn execute_in_target(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("scriptlet.sh");
        write_executable_script(&script_path, content)?;

        // Copy script into target root temporarily
        let target_script_dir = self.root.join("tmp/conary-scriptlets");
        fs::create_dir_all(&target_script_dir)?;
        let target_script_path = target_script_dir.join("scriptlet.sh");
        fs::copy(&script_path, &target_script_path)?;

        // Build chroot command
        // Using unshare for isolation when available, falling back to plain chroot
        let result = if nix::unistd::geteuid().is_root() {
            self.execute_with_chroot(phase, interpreter, &target_script_path, args, env)
        } else {
            // Non-root: try unshare with user namespace, fall back to error
            warn!("Target root scriptlet execution requires root privileges or user namespaces");
            Err(Error::ScriptletError(format!(
                "Cannot execute {} scriptlet in target root without root privileges",
                phase
            )))
        };

        // Cleanup
        let _ = fs::remove_file(&target_script_path);
        let _ = fs::remove_dir(&target_script_dir);

        result
    }

    /// Execute scriptlet using native chroot + seccomp (requires root)
    ///
    /// Uses `pre_exec` to chroot and apply seccomp in the child process,
    /// instead of spawning the external `chroot` command. This enables
    /// syscall filtering via seccomp-BPF for defense-in-depth.
    fn execute_with_chroot(
        &self,
        phase: &str,
        interpreter: &str,
        script_path: &Path,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        // Script path relative to chroot
        let script_in_chroot = script_path.strip_prefix(&self.root).unwrap_or(script_path);
        let script_in_chroot = format!("/{}", script_in_chroot.display());
        let root = self.root.clone();

        // Build seccomp BPF filter in parent process (avoids allocation after fork)
        let seccomp_mode = current_seccomp_mode();
        let bpf_filter = build_scriptlet_seccomp(seccomp_mode);
        let seccomp_enabled = bpf_filter.is_some();

        debug!(
            "Executing in chroot {}: {} {} {:?} (seccomp: {})",
            self.root.display(),
            interpreter,
            script_in_chroot,
            args,
            if seccomp_enabled {
                "enabled"
            } else {
                "unavailable"
            }
        );

        // Use interpreter directly with pre_exec for native chroot + seccomp
        let mut cmd = Command::new(interpreter);
        cmd.arg(&script_in_chroot)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            cmd.env(*key, *value);
        }

        // Safety: pre_exec runs between fork and exec in the child process.
        // All operations (chroot, chdir, prctl, seccomp) are async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                // 1. Isolate mount topology before entering the target root.
                nix::sched::unshare(chroot_namespace_flags())
                    .map_err(|e| std::io::Error::other(format!("unshare failed: {e}")))?;
                nix::mount::mount::<str, str, str, str>(
                    None,
                    "/",
                    None,
                    chroot_mount_private_flags(),
                    None,
                )
                .map_err(|e| std::io::Error::other(format!("mount --make-rprivate failed: {e}")))?;

                // 2. chroot into the target root
                nix::unistd::chroot(&root).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("chroot failed: {e}"),
                    )
                })?;
                nix::unistd::chdir("/")
                    .map_err(|e| std::io::Error::other(format!("chdir failed: {e}")))?;

                // 3. Set NO_NEW_PRIVS (required for unprivileged seccomp)
                let ret = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
                if ret != 0 {
                    return Err(std::io::Error::last_os_error());
                }

                // 4. Apply seccomp filter
                if let Some(ref filter) = bpf_filter
                    && seccompiler::apply_filter(filter).is_err()
                {
                    // Use raw write of a static string — no heap allocation,
                    // safe after fork in a multi-threaded process.
                    const MSG: &[u8] = b"[conary] seccomp filter application failed\n";
                    let _ = libc::write(2, MSG.as_ptr().cast(), MSG.len());

                    if seccomp_mode == EnforcementMode::Enforce {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "seccomp filter application failed in enforce mode",
                        ));
                    }
                }

                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            Error::ScriptletError(format!("Failed to spawn scriptlet in chroot: {e}"))
        })?;

        let context = format!(
            " (chroot: {}, seccomp: {})",
            self.root.display(),
            if seccomp_enabled {
                "enabled"
            } else {
                "unavailable"
            }
        );

        wait_and_capture(&mut child, self.timeout, phase, &context)
    }

    /// Execute scriptlet directly without sandbox
    fn execute_direct(
        &self,
        phase: &str,
        interpreter: &str,
        content: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Result<()> {
        let temp_dir = TempDir::new()?;
        let script_path = temp_dir.path().join("scriptlet.sh");
        write_executable_script(&script_path, content)?;

        debug!(
            "Executing script: {} {} {:?}",
            interpreter,
            script_path.display(),
            args
        );

        let mut cmd = Command::new(interpreter);
        cmd.arg(&script_path)
            .args(args)
            .stdin(Stdio::null()) // CRITICAL: Prevent stdin hangs
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_sanitized_command_env(&mut cmd, env);

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::ScriptletError(format!("Failed to spawn scriptlet: {}", e)))?;

        wait_and_capture(&mut child, self.timeout, phase, "")
    }

    /// Get arguments based on distro and execution mode
    ///
    /// Each distro has different argument semantics:
    /// - RPM: Integer count of packages remaining after operation
    /// - DEB: Action word + optional version string (per Debian Policy)
    /// - Arch: Version string(s)
    fn get_args(&self, mode: &ExecutionMode, phase: &str) -> Vec<String> {
        match self.package_format {
            PackageFormat::Rpm => {
                // RPM uses integer arguments (count of packages remaining):
                // Install: $1 = 1
                // Upgrade (new pkg): $1 = 2
                // Upgrade (old pkg removal): $1 = 1 (NOT 0! another version remains)
                // Remove: $1 = 0
                match mode {
                    ExecutionMode::Install => vec!["1".to_string()],
                    ExecutionMode::Remove => vec!["0".to_string()],
                    ExecutionMode::Upgrade { .. } => vec!["2".to_string()],
                    ExecutionMode::UpgradeRemoval { .. } => vec!["1".to_string()],
                }
            }
            PackageFormat::Deb => {
                // DEB uses action words + version strings (per Debian Policy):
                // preinst: install | upgrade <old-version>
                // postinst: configure <most-recently-configured-version>
                // prerm: remove | upgrade <new-version>
                // postrm: remove | upgrade <new-version>
                match mode {
                    ExecutionMode::Install => match phase {
                        "pre-install" => vec!["install".to_string()],
                        "post-install" => vec!["configure".to_string()],
                        _ => vec!["install".to_string()],
                    },
                    ExecutionMode::Remove => {
                        vec!["remove".to_string()]
                    }
                    ExecutionMode::Upgrade { old_version } => {
                        // For NEW package scripts during upgrade
                        match phase {
                            "pre-install" => vec!["upgrade".to_string(), old_version.clone()],
                            "post-install" => vec!["configure".to_string(), old_version.clone()],
                            _ => vec!["upgrade".to_string(), old_version.clone()],
                        }
                    }
                    ExecutionMode::UpgradeRemoval { new_version } => {
                        // For OLD package scripts during upgrade
                        // prerm/postrm get "upgrade <new_version>"
                        vec!["upgrade".to_string(), new_version.clone()]
                    }
                }
            }
            PackageFormat::Arch => {
                // Arch uses version strings:
                // Install: $1 = new_version
                // Remove: $1 = old_version
                // Upgrade: $1 = new_version, $2 = old_version
                // UpgradeRemoval: Should NOT be called for Arch!
                match mode {
                    ExecutionMode::Install => vec![self.package_version.clone()],
                    ExecutionMode::Remove => vec![self.package_version.clone()],
                    ExecutionMode::Upgrade { old_version } => {
                        vec![self.package_version.clone(), old_version.clone()]
                    }
                    ExecutionMode::UpgradeRemoval { .. } => {
                        // This should never be called for Arch - log warning
                        // Arch does NOT run old package scripts during upgrade
                        warn!("UpgradeRemoval mode called for Arch package - this is a bug!");
                        vec![self.package_version.clone()]
                    }
                }
            }
        }
    }

    /// Generate wrapper script for Arch .INSTALL function libraries
    ///
    /// Arch .INSTALL files define functions like post_install(), pre_upgrade(), etc.
    /// but don't call them. We need to source the file and call the appropriate function.
    fn prepare_arch_wrapper(&self, content: &str, phase: &str) -> String {
        // Map phase to Arch function name
        let function_name = match phase {
            "pre-install" => "pre_install",
            "post-install" => "post_install",
            "pre-remove" => "pre_remove",
            "post-remove" => "post_remove",
            "pre-upgrade" => "pre_upgrade",
            "post-upgrade" => "post_upgrade",
            _ => "post_install", // Fallback
        };

        format!(
            "#!/bin/bash\nset -e\n\n# Arch .INSTALL content:\n{}\n\n# Call the function if it exists\nif declare -f {} > /dev/null; then\n    {} \"$@\"\nfi\n",
            content, function_name, function_name
        )
    }
}

pub fn set_seccomp_warn_override(enabled: bool) {
    runtime::set_seccomp_warn_override(enabled);
}

/// Convert ScriptletPhase to string for database storage
pub fn phase_to_string(phase: ScriptletPhase) -> String {
    phase.to_string()
}

/// Parse phase string back to ScriptletPhase
pub fn phase_from_string(s: &str) -> Option<ScriptletPhase> {
    match s {
        "pre-install" => Some(ScriptletPhase::PreInstall),
        "post-install" => Some(ScriptletPhase::PostInstall),
        "pre-remove" => Some(ScriptletPhase::PreRemove),
        "post-remove" => Some(ScriptletPhase::PostRemove),
        "pre-upgrade" => Some(ScriptletPhase::PreUpgrade),
        "post-upgrade" => Some(ScriptletPhase::PostUpgrade),
        "pre-transaction" => Some(ScriptletPhase::PreTransaction),
        "post-transaction" => Some(ScriptletPhase::PostTransaction),
        "trigger" => Some(ScriptletPhase::Trigger),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::enforcement::EnforcementMode;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn test_package_format_from_str() {
        assert_eq!(PackageFormat::parse("rpm"), Some(PackageFormat::Rpm));
        assert_eq!(PackageFormat::parse("deb"), Some(PackageFormat::Deb));
        assert_eq!(PackageFormat::parse("arch"), Some(PackageFormat::Arch));
        assert_eq!(PackageFormat::parse("unknown"), None);
    }

    #[test]
    fn test_rpm_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["1"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["2"]
        );
        // UpgradeRemoval: old package scripts get $1=1 (NOT 0!)
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["1"]
        );
    }

    #[test]
    fn test_deb_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb);

        // Fresh install
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "pre-install"),
            vec!["install"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["configure"]
        );

        // Remove
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["remove"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "post-remove"),
            vec!["remove"]
        );

        // Upgrade
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "pre-install"
            ),
            vec!["upgrade", "0.9.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-install"
            ),
            vec!["configure", "0.9.0"]
        );
        // UpgradeRemoval: OLD package scripts get "upgrade <new_version>"
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "pre-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::UpgradeRemoval {
                    new_version: "1.0.0".to_string()
                },
                "post-remove"
            ),
            vec!["upgrade", "1.0.0"]
        );
    }

    #[test]
    fn test_arch_args() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        assert_eq!(
            executor.get_args(&ExecutionMode::Install, "post-install"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(&ExecutionMode::Remove, "pre-remove"),
            vec!["1.0.0"]
        );
        assert_eq!(
            executor.get_args(
                &ExecutionMode::Upgrade {
                    old_version: "0.9.0".to_string()
                },
                "post-upgrade"
            ),
            vec!["1.0.0", "0.9.0"]
        );
    }

    #[test]
    fn test_arch_wrapper_generation() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Arch);

        let content = "post_install() {\n    echo \"Hello\"\n}";
        let wrapper = executor.prepare_arch_wrapper(content, "post-install");

        assert!(wrapper.contains("#!/bin/bash"));
        assert!(wrapper.contains("set -e"));
        assert!(wrapper.contains(content));
        assert!(wrapper.contains("post_install \"$@\""));
    }

    #[test]
    fn test_phase_conversion() {
        assert_eq!(phase_to_string(ScriptletPhase::PreInstall), "pre-install");
        assert_eq!(
            phase_from_string("pre-install"),
            Some(ScriptletPhase::PreInstall)
        );
        assert_eq!(phase_from_string("invalid"), None);
    }

    #[test]
    fn test_sandbox_mode_default_is_always() {
        assert_eq!(SandboxMode::default(), SandboxMode::Always);
    }

    #[test]
    fn test_sandbox_mode_parse() {
        // "none" variants
        assert_eq!(SandboxMode::parse("never"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("none"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("off"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("false"), Some(SandboxMode::None));

        // "auto"
        assert_eq!(SandboxMode::parse("auto"), Some(SandboxMode::Auto));

        // "always" variants
        assert_eq!(SandboxMode::parse("always"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("on"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("true"), Some(SandboxMode::Always));

        // Case insensitivity
        assert_eq!(SandboxMode::parse("AUTO"), Some(SandboxMode::Auto));
        assert_eq!(SandboxMode::parse("NEVER"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("Always"), Some(SandboxMode::Always));

        // Invalid
        assert_eq!(SandboxMode::parse("invalid"), None);
        assert_eq!(SandboxMode::parse(""), None);
    }

    #[test]
    fn test_executor_default_sandbox_is_always() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        assert_eq!(executor.sandbox_mode, SandboxMode::Always);
    }

    #[test]
    fn test_execute_basic_success() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "echo hello",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_script_failure() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "exit 42",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed with exit code 42"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_execute_none_sandbox_runs_directly() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Deb)
                .with_sandbox_mode(SandboxMode::None);

        // Verify it runs and can produce output without error
        let result = executor.execute_direct(
            "pre-install",
            "/bin/sh",
            "echo 'running unsandboxed'; true",
            &["install".to_string()],
            &[
                ("CONARY_PACKAGE_NAME", "test-pkg"),
                ("CONARY_PHASE", "pre-install"),
            ],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_timeout() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_timeout(Duration::from_secs(1))
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "sleep 30",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "unexpected error: {}", err);
    }

    #[test]
    fn test_execute_with_env_vars() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "my-package", "2.5.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        // Script that checks environment variables are set
        let script = r#"
            test "$CONARY_PACKAGE_NAME" = "my-package" || exit 1
            test "$CONARY_PACKAGE_VERSION" = "2.5.0" || exit 2
        "#;

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            script,
            &["1".to_string()],
            &[
                ("CONARY_PACKAGE_NAME", "my-package"),
                ("CONARY_PACKAGE_VERSION", "2.5.0"),
            ],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_direct_clears_host_environment() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("CONARY_SCRIPTLET_LEAK", "host-secret");
        }

        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "test -z \"$CONARY_SCRIPTLET_LEAK\"",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );

        unsafe {
            std::env::remove_var("CONARY_SCRIPTLET_LEAK");
        }

        assert!(
            result.is_ok(),
            "direct scriptlet execution should not inherit host environment variables: {result:?}"
        );
    }

    #[test]
    fn test_live_sandbox_config_rebinds_critical_etc_files_readonly() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config();
        let etc_index = config
            .bind_mounts
            .iter()
            .position(|mount| mount.target == Path::new("/etc") && mount.writable)
            .expect("writable /etc bind mount missing");

        for protected in ["/etc/passwd", "/etc/shadow", "/etc/sudoers"] {
            let mount_index = config
                .bind_mounts
                .iter()
                .position(|mount| mount.target == Path::new(protected))
                .unwrap_or_else(|| panic!("missing protected mount for {protected}"));
            let mount = &config.bind_mounts[mount_index];
            assert!(
                !mount.writable,
                "{protected} should be rebound read-only inside the live sandbox"
            );
            assert!(
                mount_index > etc_index,
                "{protected} should be mounted after writable /etc so it is not shadowed"
            );
        }
    }

    #[test]
    fn test_execute_impl_missing_interpreter() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_impl(
            "post-install",
            "/nonexistent/interpreter",
            "echo hello",
            None,
            &ExecutionMode::Install,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Interpreter not found"),
            "unexpected error: {}",
            err
        );
    }

    // -- GAP 3: build_scriptlet_seccomp() ------------------------------------

    #[test]
    fn test_build_scriptlet_seccomp_returns_filter() {
        // On Linux with seccomp support, this should return Some(bpf).
        // On other platforms or kernels without seccomp, it returns None.
        let result = build_scriptlet_seccomp(EnforcementMode::Warn);
        // We cannot assert Some unconditionally (CI may lack seccomp),
        // but we verify the function does not panic and returns a valid option.
        if crate::capability::enforcement::seccomp_enforce::check_seccomp_support() {
            assert!(
                result.is_some(),
                "build_scriptlet_seccomp should return Some when seccomp is supported"
            );
        } else {
            assert!(
                result.is_none(),
                "build_scriptlet_seccomp should return None when seccomp is unsupported"
            );
        }
    }

    #[test]
    fn test_current_seccomp_mode_defaults_to_enforce() {
        set_seccomp_warn_override(false);
        assert_eq!(current_seccomp_mode(), EnforcementMode::Enforce);
    }

    #[test]
    fn test_chroot_namespace_flags_include_mount_namespace() {
        assert!(chroot_namespace_flags().contains(nix::sched::CloneFlags::CLONE_NEWNS));
    }

    #[test]
    fn test_chroot_mount_propagation_is_private_recursive() {
        let flags = chroot_mount_private_flags();
        assert!(flags.contains(nix::mount::MsFlags::MS_PRIVATE));
        assert!(flags.contains(nix::mount::MsFlags::MS_REC));
    }

    // -- GAP 4: execute_direct double-wait fix (stdout/stderr + timeout) ------

    #[test]
    fn test_execute_direct_captures_stdout_stderr_without_echild() {
        // Exercises the take-handles-before-wait pattern that prevents
        // ECHILD on double-wait when the child produces output.
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_sandbox_mode(SandboxMode::None);

        let script = r#"
            echo "stdout line"
            echo "stderr line" >&2
        "#;

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            script,
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(
            result.is_ok(),
            "Script with stdout/stderr should complete without ECHILD: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn test_execute_direct_timeout_no_double_wait_panic() {
        // The timeout path kills the child and returns an error.
        // Before the fix, calling wait_with_output after wait_timeout could
        // panic with ECHILD. This test verifies the timeout path is safe.
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm)
                .with_timeout(Duration::from_secs(1))
                .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_direct(
            "post-install",
            "/bin/sh",
            "sleep 30",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out"),
            "Expected timeout error, got: {}",
            err
        );
    }

    // -- GAP 6: Native chroot + seccomp (root-gated) -------------------------

    #[test]
    fn test_execute_with_chroot_requires_root() {
        // Non-root users cannot chroot. Verify execute_in_target returns
        // an appropriate error when not running as root.
        if nix::unistd::geteuid().is_root() {
            // Skip this test when running as root; the root test below covers it.
            return;
        }

        let temp_dir = tempfile::TempDir::new().unwrap();
        let target_root = temp_dir.path();

        // Create minimal structure expected by execute_in_target
        std::fs::create_dir_all(target_root.join("tmp")).unwrap();
        std::fs::create_dir_all(target_root.join("bin")).unwrap();

        let executor = ScriptletExecutor::new(target_root, "test-pkg", "1.0.0", PackageFormat::Rpm)
            .with_sandbox_mode(SandboxMode::None);

        let result = executor.execute_in_target(
            "post-install",
            "/bin/sh",
            "echo hello",
            &["1".to_string()],
            &[("CONARY_PACKAGE_NAME", "test-pkg")],
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("root privileges"),
            "Expected root-required error, got: {}",
            err
        );
    }
}
