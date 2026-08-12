// conary-core/src/ccs/hooks/openrc.rs

//! OpenRC target-provider adapter for declarative service hooks.

use super::HookExecutor;
use crate::activation::{OpenRcActivationAction, OpenRcActivationInvocation};
use crate::ccs::hooks::InitSystemCapability;
use crate::ccs::manifest::ServiceAction;
use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

impl HookExecutor {
    pub(super) fn execute_service_action(&self, name: &str, action: &ServiceAction) -> Result<()> {
        match self.capabilities.init_system {
            InitSystemCapability::Systemd => self.execute_systemd_service_action(name, action),
            InitSystemCapability::OpenRc => self.execute_openrc_service_action(name, action),
            InitSystemCapability::Unsupported => {
                bail!("generic service hook requires a typed target service-manager capability")
            }
        }
    }

    fn execute_openrc_service_action(&self, name: &str, action: &ServiceAction) -> Result<()> {
        self.validate_systemd_unit(name)?;
        match action {
            ServiceAction::Enable => self.set_openrc_enabled(name, true),
            ServiceAction::Disable => self.set_openrc_enabled(name, false),
            ServiceAction::Start => self.capture_openrc(OpenRcActivationInvocation::simple(
                OpenRcActivationAction::Start,
                name,
            )),
            ServiceAction::Stop => self.capture_openrc(OpenRcActivationInvocation::simple(
                OpenRcActivationAction::Stop,
                name,
            )),
            ServiceAction::Reload => self.capture_openrc(OpenRcActivationInvocation::simple(
                OpenRcActivationAction::Reload,
                name,
            )),
            ServiceAction::Restart => self.capture_openrc(OpenRcActivationInvocation::simple(
                OpenRcActivationAction::Restart,
                name,
            )),
            ServiceAction::TryRestart => {
                self.capture_openrc(OpenRcActivationInvocation::try_restart(name))
            }
            ServiceAction::ReloadOrRestart | ServiceAction::ReloadOrTryRestart => {
                bail!("OpenRC has no exact combined reload-or-restart service action contract")
            }
        }
    }

    fn set_openrc_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.require_selected_root()?;
        let interface = self.capabilities.openrc.as_ref().ok_or_else(|| {
            anyhow::anyhow!("OpenRC interface is absent from the persisted host inventory")
        })?;
        let init_script =
            crate::filesystem::path::safe_join(&self.root, format!("/etc/init.d/{name}"))?;
        let metadata = init_script
            .symlink_metadata()
            .with_context(|| format!("OpenRC service '{name}' has no selected-root init script"))?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            bail!(
                "OpenRC service '{name}' selected-root init script is not an executable regular file"
            );
        }
        let runlevel = crate::filesystem::path::safe_join(
            &self.root,
            format!("/etc/runlevels/{}", interface.default_runlevel),
        )?;
        fs::create_dir_all(&runlevel)?;
        let link = runlevel.join(name);
        let expected = std::path::PathBuf::from(format!("../../init.d/{name}"));
        match fs::symlink_metadata(&link) {
            Ok(metadata) => {
                if !metadata.file_type().is_symlink() || fs::read_link(&link)? != expected {
                    bail!(
                        "OpenRC runlevel projection {} has conflicting authority",
                        link.display()
                    );
                }
                if !enabled {
                    fs::remove_file(&link)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if enabled {
                    symlink(&expected, &link)?;
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn capture_openrc(&self, invocation: OpenRcActivationInvocation) -> Result<()> {
        invocation.validate().map_err(anyhow::Error::new)?;
        let source_entry = invocation.service.clone();
        self.activation_invocations
            .borrow_mut()
            .push(super::CapturedCcsActivation {
                source: super::CcsActivationSource::DeclarativeService,
                source_entry,
                invocation: invocation.into(),
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activation::RuntimeActivationInvocation;
    use crate::ccs::hooks::{HostCapabilityInventory, OpenRcInterface};
    use crate::test_support::{HostToolFixture, link_host_tool};
    use std::path::Path;

    #[cfg(unix)]
    fn executor(root: &Path, tools: &Path) -> HookExecutor {
        let executable = tools.join("rc-service");
        link_host_tool(&executable, HostToolFixture::OpenRc);
        HookExecutor::new(
            root,
            HostCapabilityInventory {
                init_system: InitSystemCapability::OpenRc,
                openrc: Some(OpenRcInterface::probe(executable).unwrap()),
                ..HostCapabilityInventory::default()
            },
        )
    }

    #[cfg(unix)]
    fn install_init_script(root: &Path, name: &str) {
        let directory = root.join("etc/init.d");
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join(name);
        fs::write(&script, "#!/sbin/openrc-run\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(script, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn enable_and_disable_project_the_exact_default_runlevel_link() {
        let root = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        install_init_script(root.path(), "sshd");
        let executor = executor(root.path(), tools.path());

        executor
            .execute_service_action("sshd", &ServiceAction::Enable)
            .unwrap();
        let link = root.path().join("etc/runlevels/default/sshd");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../init.d/sshd")
        );

        executor
            .execute_service_action("sshd", &ServiceAction::Disable)
            .unwrap();
        assert!(!link.exists());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_action_is_deferred_as_a_typed_openrc_invocation() {
        let root = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let executor = executor(root.path(), tools.path());

        executor
            .execute_service_action("sshd", &ServiceAction::TryRestart)
            .unwrap();
        let captured = executor.take_activation_invocations();
        assert_eq!(captured.len(), 1);
        let RuntimeActivationInvocation::OpenRc(invocation) = &captured[0].invocation else {
            panic!("expected OpenRC activation invocation")
        };
        assert_eq!(
            invocation.rc_service_args(),
            ["--ifstarted", "--", "sshd", "restart"].map(str::to_string)
        );
    }

    #[cfg(unix)]
    #[test]
    fn enable_rejects_symlinked_init_script_and_conflicting_runlevel_authority() {
        let root = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/init.d")).unwrap();
        symlink("/bin/true", root.path().join("etc/init.d/escaped")).unwrap();
        let executor = executor(root.path(), tools.path());
        assert!(
            executor
                .execute_service_action("escaped", &ServiceAction::Enable)
                .is_err()
        );

        install_init_script(root.path(), "sshd");
        fs::create_dir_all(root.path().join("etc/runlevels/default")).unwrap();
        let link = root.path().join("etc/runlevels/default/sshd");
        symlink("../../init.d/other", &link).unwrap();
        assert!(
            executor
                .execute_service_action("sshd", &ServiceAction::Enable)
                .is_err()
        );
        assert_eq!(
            fs::read_link(link).unwrap(),
            Path::new("../../init.d/other")
        );
    }
}
