// conary-core/src/ccs/hooks/capabilities/interface_contract.rs

//! Exact executable identities for target-owned hook interfaces.

use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const INTERFACE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const INTERFACE_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const INTERFACE_PROBE_OUTPUT_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostExecutableContract {
    SystemdSystemctl,
    SystemdSysusers,
    SystemdTmpfiles,
    ProcpsSysctl,
    GlibcLdconfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostExecutableImplementation {
    Systemd,
    ProcpsNg,
    Glibc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableInterface {
    /// Absolute executable path as seen inside the target root.
    pub executable: PathBuf,
    /// Exact command grammar and root-semantics contract Conary will use.
    pub contract: HostExecutableContract,
    /// Parsed implementation family, never inferred from a distro name.
    pub implementation: HostExecutableImplementation,
    /// Exact implementation version reported by the executable.
    pub implementation_version: String,
    /// SHA-256 identity of the executable bytes at initialization time.
    pub executable_sha256: String,
}

impl ExecutableInterface {
    pub fn probe_systemctl(executable: PathBuf) -> Option<Self> {
        Self::probe_host(executable, HostExecutableContract::SystemdSystemctl)
    }

    pub fn probe_sysusers(executable: PathBuf) -> Option<Self> {
        Self::probe_host(executable, HostExecutableContract::SystemdSysusers)
    }

    pub fn probe_tmpfiles(executable: PathBuf) -> Option<Self> {
        Self::probe_host(executable, HostExecutableContract::SystemdTmpfiles)
    }

    pub fn probe_sysctl(executable: PathBuf) -> Option<Self> {
        Self::probe_host(executable, HostExecutableContract::ProcpsSysctl)
    }

    pub fn probe_ldconfig(executable: PathBuf) -> Option<Self> {
        Self::probe_host(executable, HostExecutableContract::GlibcLdconfig)
    }

    /// Probe an ldconfig executable inside an offline root while persisting
    /// the absolute path as that executable will see it in the target.
    pub fn probe_ldconfig_in_root(root: &Path, executable: &Path) -> Option<Self> {
        if !executable.is_absolute() {
            return None;
        }
        let actual = root.join(executable.strip_prefix("/").ok()?);
        Self::probe_at(
            actual,
            executable.to_path_buf(),
            HostExecutableContract::GlibcLdconfig,
        )
    }

    pub(super) fn is_available(&self) -> bool {
        self.verify_at(&self.executable)
    }

    pub(super) fn is_available_in_root(&self, root: &Path) -> bool {
        let actual = if root == Path::new("/") {
            self.executable.clone()
        } else {
            let Ok(relative) = self.executable.strip_prefix("/") else {
                return false;
            };
            root.join(relative)
        };
        self.verify_at(&actual)
    }

    pub(super) fn descriptor_is_well_formed(&self) -> bool {
        self.executable.is_absolute()
            && self.implementation == expected_implementation(self.contract)
            && valid_version(&self.implementation_version)
            && self
                .executable_sha256
                .strip_prefix("sha256:")
                .is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
    }

    fn probe_host(executable: PathBuf, contract: HostExecutableContract) -> Option<Self> {
        if !executable.is_absolute() {
            return None;
        }
        Self::probe_at(executable.clone(), executable, contract)
    }

    fn probe_at(
        actual_executable: PathBuf,
        persisted_executable: PathBuf,
        contract: HostExecutableContract,
    ) -> Option<Self> {
        if !persisted_executable.is_absolute() || !executable_is_runnable(&actual_executable) {
            return None;
        }
        let (implementation, implementation_version) =
            implementation_identity(&actual_executable, contract)?;
        if !functional_handshake(&actual_executable, contract) {
            return None;
        }
        let executable_sha256 =
            crate::hash::sha256_prefixed(&std::fs::read(&actual_executable).ok()?);
        Some(Self {
            executable: persisted_executable,
            contract,
            implementation,
            implementation_version,
            executable_sha256,
        })
    }

    fn verify_at(&self, actual_executable: &Path) -> bool {
        let Some(current) = Self::probe_at(
            actual_executable.to_path_buf(),
            self.executable.clone(),
            self.contract,
        ) else {
            return false;
        };
        current == *self
    }
}

pub(super) fn systemd_manager_responds(executable: &Path) -> bool {
    let Some(output) = run_probe(executable, &["show", "--property=Version", "--value"], None)
    else {
        return false;
    };
    output.success
        && std::str::from_utf8(&output.stdout).is_ok_and(|version| !version.trim().is_empty())
}

fn implementation_identity(
    executable: &Path,
    contract: HostExecutableContract,
) -> Option<(HostExecutableImplementation, String)> {
    let output = run_probe(executable, &["--version"], None)?;
    if !output.success {
        return None;
    }
    let text = if output.stdout.is_empty() {
        std::str::from_utf8(&output.stderr).ok()?
    } else {
        std::str::from_utf8(&output.stdout).ok()?
    };
    let first_line = text.lines().next()?.trim();

    match contract {
        HostExecutableContract::SystemdSystemctl
        | HostExecutableContract::SystemdSysusers
        | HostExecutableContract::SystemdTmpfiles => {
            let mut fields = first_line.split_ascii_whitespace();
            (fields.next()? == "systemd").then_some(())?;
            let version = fields.next()?;
            valid_version(version)
                .then(|| (HostExecutableImplementation::Systemd, version.to_string()))
        }
        HostExecutableContract::ProcpsSysctl => {
            let version = first_line.strip_prefix("sysctl from procps-ng ")?;
            let version = version.split_ascii_whitespace().next()?;
            valid_version(version)
                .then(|| (HostExecutableImplementation::ProcpsNg, version.to_string()))
        }
        HostExecutableContract::GlibcLdconfig => {
            let remainder = first_line.strip_prefix("ldconfig (")?;
            let (_, version) = remainder.rsplit_once(") ")?;
            let version = version.split_ascii_whitespace().next()?;
            valid_version(version)
                .then(|| (HostExecutableImplementation::Glibc, version.to_string()))
        }
    }
}

fn expected_implementation(contract: HostExecutableContract) -> HostExecutableImplementation {
    match contract {
        HostExecutableContract::SystemdSystemctl
        | HostExecutableContract::SystemdSysusers
        | HostExecutableContract::SystemdTmpfiles => HostExecutableImplementation::Systemd,
        HostExecutableContract::ProcpsSysctl => HostExecutableImplementation::ProcpsNg,
        HostExecutableContract::GlibcLdconfig => HostExecutableImplementation::Glibc,
    }
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.as_bytes()[0].is_ascii_digit()
        && version.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+' | b'~' | b':')
        })
}

fn functional_handshake(executable: &Path, contract: HostExecutableContract) -> bool {
    match contract {
        HostExecutableContract::SystemdSystemctl => command_succeeds(
            executable,
            &["--root=/", "--no-pager", "--no-legend", "list-unit-files"],
            None,
        ),
        HostExecutableContract::SystemdSysusers => command_succeeds(
            executable,
            &["--dry-run", "-"],
            Some(b"u conary-interface-probe -\n"),
        ),
        HostExecutableContract::SystemdTmpfiles => command_succeeds(
            executable,
            &["--dry-run", "--create", "-"],
            Some(b"d /tmp/conary-interface-probe 0755 root root -\n"),
        ),
        HostExecutableContract::ProcpsSysctl => {
            command_succeeds(executable, &["-N", "kernel.ostype"], None)
        }
        HostExecutableContract::GlibcLdconfig => command_succeeds(executable, &["-p"], None),
    }
}

fn command_succeeds(executable: &Path, args: &[&str], stdin: Option<&[u8]>) -> bool {
    run_probe_discarding_output(executable, args, stdin).is_some_and(|output| output.success)
}

struct ProbeOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_probe(executable: &Path, args: &[&str], stdin: Option<&[u8]>) -> Option<ProbeOutput> {
    run_probe_with_timeout(executable, args, stdin, INTERFACE_PROBE_TIMEOUT, true)
}

fn run_probe_with_timeout(
    executable: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
    capture_output: bool,
) -> Option<ProbeOutput> {
    let mut stdout_file = capture_output.then(tempfile::tempfile).transpose().ok()?;
    let mut stderr_file = capture_output.then(tempfile::tempfile).transpose().ok()?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .env("HOME", "/root")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(
            stdout_file
                .as_ref()
                .and_then(|file| file.try_clone().ok())
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null),
        )
        .stderr(
            stderr_file
                .as_ref()
                .and_then(|file| file.try_clone().ok())
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null),
        );
    // The probe is a bounded, read-only protocol. Give it a private process
    // group so a timed-out implementation cannot leave descendants behind,
    // and cap captured version output before executing any target binary.
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = nix::libc::rlimit {
                rlim_cur: INTERFACE_PROBE_OUTPUT_LIMIT,
                rlim_max: INTERFACE_PROBE_OUTPUT_LIMIT,
            };
            if nix::libc::setrlimit(nix::libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let Ok(mut child) = command.spawn() else {
        return None;
    };
    if let Some(input) = stdin {
        let Some(mut child_stdin) = child.stdin.take() else {
            kill_probe_process_group(&mut child);
            return None;
        };
        if child_stdin.write_all(input).is_err() {
            kill_probe_process_group(&mut child);
            return None;
        }
    }
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() >= timeout {
            kill_probe_process_group(&mut child);
            return None;
        }
        thread::sleep(
            timeout
                .saturating_sub(started.elapsed())
                .min(INTERFACE_PROBE_POLL_INTERVAL),
        );
    };
    let stdout = read_probe_output(stdout_file.as_mut())?;
    let stderr = read_probe_output(stderr_file.as_mut())?;
    Some(ProbeOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn run_probe_discarding_output(
    executable: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Option<ProbeOutput> {
    run_probe_with_timeout(executable, args, stdin, INTERFACE_PROBE_TIMEOUT, false)
}

fn kill_probe_process_group(child: &mut std::process::Child) {
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = child.kill();
    let _ = child.wait();
}

fn read_probe_output(file: Option<&mut std::fs::File>) -> Option<Vec<u8>> {
    let Some(file) = file else {
        return Some(Vec::new());
    };
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut output = Vec::new();
    file.take(INTERFACE_PROBE_OUTPUT_LIMIT + 1)
        .read_to_end(&mut output)
        .ok()?;
    (output.len() as u64 <= INTERFACE_PROBE_OUTPUT_LIMIT).then_some(output)
}

fn executable_is_runnable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    fn executable_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("tool");
        fs::write(&executable, body).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        (directory, executable)
    }

    #[cfg(unix)]
    #[test]
    fn same_name_success_exit_is_not_an_interface_descriptor() {
        let (_directory, executable) = executable_script("#!/bin/sh\nexit 0\n");
        assert!(ExecutableInterface::probe_systemctl(executable).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_revalidation_detects_executable_replacement() {
        let (_directory, executable) = executable_script(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'sysctl from procps-ng 4.0.6'; fi\nexit 0\n",
        );
        let descriptor = ExecutableInterface::probe_sysctl(executable.clone()).unwrap();
        fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'sysctl from procps-ng 4.0.7'; fi\nexit 0\n",
        )
        .unwrap();

        assert!(!descriptor.is_available());
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_preserves_and_revalidates_the_selected_symlink_path() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let selected = directory.path().join("sysctl");
        let body = |version: &str| {
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'sysctl from procps-ng {version}'; fi\nexit 0\n"
            )
        };
        for (path, version) in [(&first, "4.0.6"), (&second, "4.0.7")] {
            fs::write(path, body(version)).unwrap();
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
        symlink(&first, &selected).unwrap();

        let descriptor = ExecutableInterface::probe_sysctl(selected.clone()).unwrap();
        assert_eq!(descriptor.executable, selected);
        fs::remove_file(&selected).unwrap();
        symlink(&second, &selected).unwrap();

        assert!(!descriptor.is_available());
    }

    #[cfg(unix)]
    #[test]
    fn interface_probe_has_a_hard_timeout() {
        let (_directory, executable) = executable_script("#!/bin/sh\nsleep 30 &\nwait\n");
        assert!(
            run_probe_with_timeout(
                &executable,
                &["--version"],
                None,
                Duration::from_millis(25),
                true,
            )
            .is_none()
        );
    }
}
