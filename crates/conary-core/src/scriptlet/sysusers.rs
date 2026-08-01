// conary-core/src/scriptlet/sysusers.rs

//! Typed execution of RPM's pre-payload sysusers target interface.

use super::ScriptletExecutor;
use super::runtime::{apply_sanitized_command_env, wait_and_capture};
use crate::ccs::{ExecutableInterface, HostExecutableContract};
use crate::error::{Error, Result, ScriptletFailureKind};
use std::ffi::OsString;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt as _;
use std::path::{Component, Path};
use std::process::{Command, Stdio};

impl ScriptletExecutor {
    /// Validate the fixed RPM sysusers invocation before transaction mutation.
    pub fn preflight_rpm_sysusers(
        &self,
        interface: &ExecutableInterface,
        source_path: Option<&str>,
    ) -> Result<()> {
        self.require_target_root()?;
        if !self.root.is_dir() {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                format!(
                    "RPM sysusers selected root is not a directory: {}",
                    self.root.display()
                ),
            ));
        }
        let executable = &interface.executable;
        if interface.contract != HostExecutableContract::SystemdSysusers {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ContractViolation,
                format!(
                    "RPM sysusers executor received the wrong target interface contract: {:?}",
                    interface.contract
                ),
            ));
        }
        if !interface.is_available()
            || !executable.is_absolute()
            || !executable.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        {
            return Err(Error::scriptlet(
                ScriptletFailureKind::ProgramUnavailable,
                format!(
                    "persisted RPM sysusers target interface is unavailable: {}",
                    executable.display()
                ),
            ));
        }
        if let Some(source_path) = source_path {
            validate_source_path(source_path)?;
        }
        Ok(())
    }

    /// Execute RPM sysusers through the exact persisted target interface.
    ///
    /// The executable remains outside the selected root because RPM schedules
    /// this event before payload visibility. The fixed `--root` grammar keeps
    /// all account mutation inside that selected root.
    pub fn execute_rpm_sysusers(
        &self,
        interface: &ExecutableInterface,
        source_path: Option<&str>,
        stdin: &[u8],
    ) -> Result<()> {
        self.preflight_rpm_sysusers(interface, source_path)?;
        let executable = &interface.executable;

        let mut root_argument = OsString::from("--root=");
        root_argument.push(&self.root);
        let mut stdin_file = tempfile::tempfile()?;
        stdin_file.write_all(stdin)?;
        stdin_file.seek(SeekFrom::Start(0))?;

        let mut command = Command::new(executable);
        command
            .arg(root_argument)
            .stdin(Stdio::from(stdin_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(source_path) = source_path {
            command.args(["--replace", source_path]);
        }
        command.arg("-");
        apply_sanitized_command_env(
            &mut command,
            &[
                ("CONARY_PACKAGE_NAME", self.package_name.as_str()),
                ("CONARY_PACKAGE_VERSION", self.package_version.as_str()),
            ],
        );
        // The timeout helper kills process groups. Establish the typed target
        // interface as its own group so descendants cannot escape the event.
        unsafe {
            command.pre_exec(|| {
                if nix::libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(|error| {
            Error::scriptlet(
                ScriptletFailureKind::ProcessSetupFailed,
                format!(
                    "failed to spawn persisted RPM sysusers target interface {}: {error}",
                    executable.display()
                ),
            )
        })?;
        let context = format!(" (offline root: {})", self.root.display());
        wait_and_capture(&mut child, self.timeout, "rpm-sysusers", &context)
    }
}

fn validate_source_path(source_path: &str) -> Result<()> {
    if source_path.contains('\0') || !Path::new(source_path).is_absolute() {
        return Err(Error::scriptlet(
            ScriptletFailureKind::ContractViolation,
            format!("RPM sysusers replacement path is not an absolute guest path: {source_path:?}"),
        ));
    }
    let mut has_name = false;
    for component in Path::new(source_path).components() {
        match component {
            Component::RootDir => {}
            Component::Normal(_) => has_name = true,
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::scriptlet(
                    ScriptletFailureKind::ContractViolation,
                    format!(
                        "RPM sysusers replacement path escapes its guest root: {source_path:?}"
                    ),
                ));
            }
        }
    }
    if !has_name {
        return Err(Error::scriptlet(
            ScriptletFailureKind::ContractViolation,
            "RPM sysusers replacement path does not name a configuration file",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scriptlet::PackageFormat;
    use crate::test_support::{HostToolFixture, link_host_tool};

    #[cfg(unix)]
    #[test]
    fn persisted_interface_mutates_only_the_empty_selected_root() {
        let interface_dir = tempfile::tempdir().unwrap();
        let executable = interface_dir.path().join("systemd-sysusers");
        link_host_tool(&executable, HostToolFixture::Systemd);
        let interface = ExecutableInterface::probe_sysusers(executable).unwrap();
        let selected_root = tempfile::tempdir().unwrap();
        let executor = ScriptletExecutor::new(
            selected_root.path(),
            "shadow-utils",
            "1",
            PackageFormat::Rpm,
        );

        assert!(
            !selected_root
                .path()
                .join("usr/bin/systemd-sysusers")
                .exists()
        );
        executor
            .execute_rpm_sysusers(
                &interface,
                Some("/usr/lib/sysusers.d/shadow.conf"),
                b"u shadow -\n",
            )
            .unwrap();

        let capture = selected_root
            .path()
            .join("var/lib/conary-test/systemd-sysusers-stdin");
        assert_eq!(std::fs::read(capture).unwrap(), b"u shadow -\n");
        let argv = std::fs::read_to_string(
            selected_root
                .path()
                .join("var/lib/conary-test/systemd-sysusers-argv"),
        )
        .unwrap();
        assert_eq!(
            argv,
            format!(
                "--root={} --replace /usr/lib/sysusers.d/shadow.conf -\n",
                selected_root.path().display()
            )
        );
    }

    #[test]
    fn replacement_path_must_be_confined_to_the_guest_root() {
        for source_path in ["relative.conf", "/../../host.conf", "/"] {
            let error = validate_source_path(source_path).unwrap_err();
            assert!(
                matches!(
                    &error,
                    Error::ScriptletExecution {
                        kind: ScriptletFailureKind::ContractViolation,
                        ..
                    }
                ),
                "unexpected error: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn wrong_target_interface_contract_is_rejected() {
        let interface_dir = tempfile::tempdir().unwrap();
        let executable = interface_dir.path().join("systemd-tool");
        link_host_tool(&executable, HostToolFixture::Systemd);
        let wrong_interface = ExecutableInterface::probe_tmpfiles(executable).unwrap();
        let selected_root = tempfile::tempdir().unwrap();
        let executor =
            ScriptletExecutor::new(selected_root.path(), "fixture", "1", PackageFormat::Rpm);

        let error = executor
            .preflight_rpm_sysusers(&wrong_interface, None)
            .unwrap_err();
        assert!(matches!(
            error,
            Error::ScriptletExecution {
                kind: ScriptletFailureKind::ContractViolation,
                ..
            }
        ));
    }
}
