// conary-core/src/scriptlet/process.rs

use super::ScriptletExecutor;
use super::activation_capture::ActivationCaptureSession;
use super::boot_runtime_capture::BootRuntimeCaptureSession;
use super::boundary::{SCRIPTLET_BOUNDARY_ABI_V2, ScriptletBoundary};
use super::runtime::{apply_sanitized_command_env, wait_and_capture};
use crate::error::{Error, Result, ScriptletFailureKind};
use nix::errno::Errno;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{Mode, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::debug;
use uuid::Uuid;

const TARGET_SCRIPT_NAME: &str = "scriptlet";

mod device_projection;
use device_projection::{TargetDeviceProjection, attach_device_projection};

#[derive(Clone, Copy)]
pub(super) struct ScriptletProcess<'a> {
    pub(super) phase: &'a str,
    pub(super) interpreter: &'a str,
    pub(super) interpreter_args: &'a [String],
    pub(super) args: &'a [String],
    pub(super) env: &'a [(&'a str, &'a str)],
    pub(super) stdin: &'a [u8],
}

#[derive(Debug)]
pub(super) struct TargetRootScript {
    tmp_fd: OwnedFd,
    directory_fd: OwnedFd,
    directory_name: String,
    path: PathBuf,
    root: PathBuf,
    extra_files: Vec<String>,
    extra_directories: Vec<String>,
}

#[derive(Debug)]
#[must_use = "the selected-root boundary guard must live until its child process exits"]
pub(crate) struct TargetExecutionBoundary {
    pub(crate) contract: &'static str,
    _device_projection: TargetDeviceProjection,
}

#[derive(Debug, Clone)]
pub(super) struct TargetCommandBindMount {
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
}

impl TargetRootScript {
    pub(super) fn stage(root: &Path, content: &[u8]) -> Result<Self> {
        let root_fd = open(
            root,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let tmp_fd = open_or_create_target_tmp(&root_fd)?;

        let directory_name = loop {
            let candidate = format!(".conary-scriptlet-{}", Uuid::new_v4().simple());
            match mkdirat(&tmp_fd, candidate.as_str(), Mode::from_bits_truncate(0o700)) {
                Ok(()) => break candidate,
                Err(Errno::EEXIST) => continue,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        };
        let directory_fd = match openat(
            &tmp_fd,
            directory_name.as_str(),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(directory_fd) => directory_fd,
            Err(error) => {
                let _ = unlinkat(&tmp_fd, directory_name.as_str(), UnlinkatFlags::RemoveDir);
                return Err(std::io::Error::from(error).into());
            }
        };
        let staged = Self {
            tmp_fd,
            directory_fd,
            root: root.to_path_buf(),
            path: root
                .join("tmp")
                .join(&directory_name)
                .join(TARGET_SCRIPT_NAME),
            directory_name,
            extra_files: Vec::new(),
            extra_directories: Vec::new(),
        };
        let script_fd = openat(
            &staged.directory_fd,
            TARGET_SCRIPT_NAME,
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o700),
        )
        .map_err(std::io::Error::from)?;
        let mut script = File::from(script_fd);
        script.write_all(content)?;
        script.flush()?;
        Ok(staged)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn create_extra_file(
        &mut self,
        name: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<PathBuf> {
        let file_fd = openat(
            &self.directory_fd,
            name,
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(mode),
        )
        .map_err(std::io::Error::from)?;
        let mut file = File::from(file_fd);
        file.write_all(content)?;
        file.flush()?;
        self.extra_files.push(name.to_string());
        Ok(self.root.join("tmp").join(&self.directory_name).join(name))
    }

    pub(super) fn guest_extra_path(&self, name: &str) -> PathBuf {
        Path::new("/")
            .join("tmp")
            .join(&self.directory_name)
            .join(name)
    }

    pub(super) fn create_extra_directory_file(
        &mut self,
        directory: &str,
        name: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<PathBuf> {
        validate_staged_component(directory)?;
        validate_staged_component(name)?;
        mkdirat(
            &self.directory_fd,
            directory,
            Mode::from_bits_truncate(0o700),
        )
        .map_err(std::io::Error::from)?;
        self.extra_directories.push(directory.to_string());
        let directory_fd = openat(
            &self.directory_fd,
            directory,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let file_fd = openat(
            &directory_fd,
            name,
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(mode),
        )
        .map_err(std::io::Error::from)?;
        self.extra_files.push(format!("{directory}/{name}"));
        let mut file = File::from(file_fd);
        file.write_all(content)?;
        file.flush()?;
        Ok(self
            .root
            .join("tmp")
            .join(&self.directory_name)
            .join(directory)
            .join(name))
    }
}

impl Drop for TargetRootScript {
    fn drop(&mut self) {
        for name in self.extra_files.iter().rev() {
            let _ = unlinkat(
                &self.directory_fd,
                name.as_str(),
                UnlinkatFlags::NoRemoveDir,
            );
        }
        for name in self.extra_directories.iter().rev() {
            let _ = unlinkat(&self.directory_fd, name.as_str(), UnlinkatFlags::RemoveDir);
        }
        let _ = unlinkat(
            &self.directory_fd,
            TARGET_SCRIPT_NAME,
            UnlinkatFlags::NoRemoveDir,
        );
        let _ = unlinkat(
            &self.tmp_fd,
            self.directory_name.as_str(),
            UnlinkatFlags::RemoveDir,
        );
    }
}

fn validate_staged_component(value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || Path::new(value).components().count() != 1
        || value.contains('/')
    {
        return Err(Error::scriptlet(
            ScriptletFailureKind::ContractViolation,
            format!("target-root staged file component is invalid: {value:?}"),
        ));
    }
    Ok(())
}

fn open_or_create_target_tmp(root_fd: &OwnedFd) -> Result<OwnedFd> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    match openat(root_fd, "tmp", flags, Mode::empty()) {
        Ok(tmp_fd) => Ok(tmp_fd),
        Err(Errno::ENOENT) => {
            match mkdirat(root_fd, "tmp", Mode::from_bits_truncate(0o1777)) {
                Ok(()) | Err(Errno::EEXIST) => {}
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
            openat(root_fd, "tmp", flags, Mode::empty())
                .map_err(std::io::Error::from)
                .map_err(Error::from)
        }
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

impl ScriptletExecutor {
    /// Execute scriptlet inside a materialized selected/target root.
    ///
    /// This is the key method for bootstrap support. It runs the scriptlet
    /// inside the target filesystem using either:
    /// - chroot (requires root, simpler)
    /// - namespace container (more isolation)
    pub(super) fn execute_in_target(
        &self,
        process: ScriptletProcess<'_>,
        content: &[u8],
    ) -> Result<()> {
        self.require_target_root()?;
        if !nix::unistd::geteuid().is_root() {
            // Non-root: try unshare with user namespace, fall back to error
            tracing::warn!(
                "Target root scriptlet execution requires root privileges or user namespaces"
            );
            return Err(Error::scriptlet(
                ScriptletFailureKind::SandboxSetupUnavailable,
                format!(
                    "Cannot execute {} scriptlet in target root without root privileges",
                    process.phase
                ),
            ));
        }

        let mut target_script = TargetRootScript::stage(&self.root, content)?;
        let capture = ActivationCaptureSession::stage(&mut target_script, &self.root, process.env)?;
        let mut boot_capture = BootRuntimeCaptureSession::stage(
            &mut target_script,
            &self.root,
            process.env,
            self.timeout,
            self.lifecycle_bridge.as_ref(),
        )?;
        let mut mounts = capture.mounts().to_vec();
        mounts.extend_from_slice(boot_capture.mounts());
        let execution =
            self.execute_with_chroot(process, target_script.path(), &mounts, &mut boot_capture);
        let boot_result = boot_capture.finish();
        execution?;
        let boot_invocations = boot_result?;
        let invocations = capture.finish()?;
        self.activation_invocations
            .lock()
            .expect("activation invocation collector poisoned")
            .extend(invocations);
        self.boot_runtime_invocations
            .lock()
            .expect("boot runtime invocation collector poisoned")
            .extend(boot_invocations);
        Ok(())
    }

    /// Execute scriptlet using native chroot + seccomp (requires root)
    ///
    /// Uses `pre_exec` to chroot and apply seccomp in the child process,
    /// instead of spawning the external `chroot` command. This enables
    /// syscall filtering via seccomp-BPF for defense-in-depth.
    pub(super) fn execute_with_chroot(
        &self,
        process: ScriptletProcess<'_>,
        script_path: &Path,
        bind_mounts: &[TargetCommandBindMount],
        boot_capture: &mut BootRuntimeCaptureSession,
    ) -> Result<()> {
        let ScriptletProcess {
            phase,
            interpreter,
            interpreter_args,
            args,
            env,
            stdin,
        } = process;
        // Script path relative to chroot
        let script_in_chroot = script_path.strip_prefix(&self.root).unwrap_or(script_path);
        let script_in_chroot = format!("/{}", script_in_chroot.display());
        let stdin_file = prepared_stdin(stdin)?;

        let mut cmd = Command::new(interpreter);
        cmd.args(interpreter_args)
            .arg(&script_in_chroot)
            .args(args)
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_sanitized_command_env(&mut cmd, env);
        boot_capture.configure_command(&mut cmd)?;
        let boundary =
            configure_target_command_boundary_with_mounts(&mut cmd, &self.root, bind_mounts)?;

        debug!(
            "Executing in chroot {}: {} {} {:?} (boundary: {})",
            self.root.display(),
            interpreter,
            script_in_chroot,
            args,
            boundary.contract,
        );

        let mut child = cmd.spawn().map_err(|e| {
            Error::scriptlet(
                ScriptletFailureKind::SandboxSetupUnavailable,
                format!("Failed to spawn scriptlet in chroot: {e}"),
            )
        })?;
        boot_capture.child_spawned();

        let context = format!(
            " (chroot: {}, boundary: {})",
            self.root.display(),
            boundary.contract,
        );

        wait_and_capture(&mut child, self.timeout, phase, &context)
    }
}

/// Attach Conary's closed lifecycle boundary to a child that executes exact
/// argv inside a materialized selected root.
///
/// The returned guard owns transient selected-root mountpoints and must remain
/// alive until the child exits.
pub(crate) fn configure_target_command_boundary(
    command: &mut Command,
    root: &Path,
) -> Result<TargetExecutionBoundary> {
    configure_target_command_boundary_with_mounts(command, root, &[])
}

pub(super) fn configure_target_command_boundary_with_mounts(
    command: &mut Command,
    root: &Path,
    bind_mounts: &[TargetCommandBindMount],
) -> Result<TargetExecutionBoundary> {
    if root == Path::new("/") {
        return Err(Error::scriptlet(
            ScriptletFailureKind::ContractViolation,
            "Lifecycle execution requires a materialized selected/target root; the host root '/' is not an execution target",
        ));
    }
    if !root.is_absolute() {
        return Err(Error::scriptlet(
            ScriptletFailureKind::ContractViolation,
            format!(
                "Lifecycle execution root must be absolute: {}",
                root.display()
            ),
        ));
    }
    if !nix::unistd::geteuid().is_root() {
        return Err(Error::scriptlet(
            ScriptletFailureKind::SandboxSetupUnavailable,
            format!(
                "Lifecycle execution in target root {} requires root privileges",
                root.display()
            ),
        ));
    }

    let root = root.to_path_buf();
    let bind_mounts = bind_mounts.to_vec();
    let prepared_boundary = ScriptletBoundary::prepare()?;
    let device_projection = TargetDeviceProjection::stage(&root)?;
    let device_projection_plan = device_projection.plan();
    let boundary = TargetExecutionBoundary {
        contract: SCRIPTLET_BOUNDARY_ABI_V2,
        _device_projection: device_projection,
    };

    // Safety: pre_exec runs between fork and exec in the child process.
    // All operations here are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            nix::sched::unshare(prepared_boundary.namespace_flags())
                .map_err(|error| std::io::Error::other(format!("unshare failed: {error}")))?;
            nix::mount::mount::<str, str, str, str>(
                None,
                "/",
                None,
                prepared_boundary.mount_private_flags(),
                None,
            )
            .map_err(|error| {
                std::io::Error::other(format!("mount --make-rprivate failed: {error}"))
            })?;
            attach_device_projection(&root, device_projection_plan).map_err(|error| {
                std::io::Error::other(format!(
                    "failed to project controlled /dev/null into lifecycle root: {error}"
                ))
            })?;
            for bind in &bind_mounts {
                nix::mount::mount(
                    Some(bind.source.as_path()),
                    bind.target.as_path(),
                    None::<&str>,
                    nix::mount::MsFlags::MS_BIND,
                    None::<&str>,
                )
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "failed to bind lifecycle proxy {} over {}: {error}",
                        bind.source.display(),
                        bind.target.display()
                    ))
                })?;
            }
            nix::unistd::chroot(&root).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("chroot failed: {error}"),
                )
            })?;
            nix::unistd::chdir("/")
                .map_err(|error| std::io::Error::other(format!("chdir failed: {error}")))?;

            prepared_boundary.install_after_chroot()
        });
    }
    Ok(boundary)
}

fn prepared_stdin(content: &[u8]) -> Result<File> {
    let mut file = tempfile::tempfile()?;
    file.write_all(content)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::super::{PackageFormat, ScriptletExecutor};
    use super::{ScriptletProcess, configure_target_command_boundary};
    use crate::scriptlet::test_support::materialized_root;
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};

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

        let executor = ScriptletExecutor::new(target_root, "test-pkg", "1.0.0", PackageFormat::Rpm);

        let result = executor.execute_in_target(
            ScriptletProcess {
                phase: "post-install",
                interpreter: "/bin/sh",
                interpreter_args: &[],
                args: &["1".to_string()],
                env: &[("CONARY_PACKAGE_NAME", "test-pkg")],
                stdin: &[],
            },
            b"echo hello",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("root privileges"),
            "Expected root-required error, got: {}",
            err
        );
    }

    #[test]
    fn target_root_script_staging_rejects_hostile_tmp_symlink() {
        let target_root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), target_root.path().join("tmp")).unwrap();

        let error = super::TargetRootScript::stage(target_root.path(), b"exit 0")
            .expect_err("target-root staging must not follow a tmp symlink");

        assert!(
            error.to_string().contains("I/O error"),
            "unexpected staging error: {error}"
        );
        assert_eq!(
            std::fs::read_dir(outside.path()).unwrap().count(),
            0,
            "staging escaped through the target root's tmp symlink"
        );
    }

    #[test]
    fn target_execution_rejects_the_host_root_before_staging() {
        let executor = ScriptletExecutor::new(Path::new("/"), "fixture", "1", PackageFormat::Deb);
        let error = executor
            .execute_in_target(
                ScriptletProcess {
                    phase: "post-install",
                    interpreter: "/bin/sh",
                    interpreter_args: &[],
                    args: &[],
                    env: &[],
                    stdin: &[],
                },
                b"exit 0",
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("host root '/'"));
    }

    #[test]
    fn rpm_shell_lifecycle_projects_dev_null_without_persisting_a_device_tree() {
        const TEST_NAME: &str = "scriptlet::process::tests::rpm_shell_lifecycle_projects_dev_null_without_persisting_a_device_tree";
        const CRYPTO_POLICIES_PRE: &[u8] =
            br#"# Drop removed javasystem backend; can be dropped in F43
rm -f "/etc/crypto-policies/back-ends/javasystem.config" 2>/dev/null || :
# Drop removed openssl backend; can be dropped in F44
rm -f "/etc/crypto-policies/back-ends/openssl.config" 2>/dev/null || :
exit 0"#;
        let Some(root) = materialized_root(TEST_NAME, &["/bin/sh", "/bin/rm"]) else {
            return;
        };
        assert_eq!(
            crate::hash::sha256(CRYPTO_POLICIES_PRE),
            "aabf5a4deffd8880422c0fcf0abc16b0831e78841d104dcdd6e29b0aeaa30d69"
        );
        let backends = root.host_path("/etc/crypto-policies/back-ends");
        fs::create_dir_all(&backends).unwrap();
        let javasystem = backends.join("javasystem.config");
        let openssl = backends.join("openssl.config");
        fs::write(&javasystem, b"stale java policy\n").unwrap();
        fs::write(&openssl, b"stale openssl policy\n").unwrap();

        let script = root.host_path("/tmp/crypto-policies-pre");
        fs::write(&script, CRYPTO_POLICIES_PRE).unwrap();

        let mut command = Command::new("/bin/sh");
        command
            .arg("/tmp/crypto-policies-pre")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        let boundary = configure_target_command_boundary(&mut command, root.path())
            .expect("prepare selected-root lifecycle boundary");
        let output = command.output().expect("execute exact Fedora RPM pre body");
        drop(boundary);

        assert!(
            output.status.success(),
            "unexpected status: {:?}",
            output.status
        );
        assert!(
            output.stderr.is_empty(),
            "exact Fedora body wrote warnings: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!javasystem.exists(), "stale javasystem backend survived");
        assert!(!openssl.exists(), "stale openssl backend survived");
        assert!(
            !root.host_path("/dev").exists(),
            "ephemeral device projection leaked into the selected root"
        );
    }
}
