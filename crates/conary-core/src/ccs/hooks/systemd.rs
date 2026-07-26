// conary-core/src/ccs/hooks/systemd.rs

//! Exact systemd command adapter for CCS lifecycle hooks.
//!
//! Offline unit state changes use the documented `systemctl --root=` grammar
//! exposed by the persisted target capability inventory. Conary does not
//! partially reimplement the systemd unit-file grammar.

use super::HookExecutor;
use crate::ccs::manifest::ServiceAction;
use anyhow::{Context, Result};
use std::process::Command;
use tracing::info;

impl HookExecutor {
    pub(super) fn systemd_set_enabled(&self, unit: &str, enabled: bool) -> Result<()> {
        self.validate_systemd_unit(unit)?;
        let action = if enabled { "enable" } else { "disable" };
        self.run_systemctl(action, Some(unit))?;
        info!(
            "{} systemd unit '{}'",
            if enabled { "Enabled" } else { "Disabled" },
            unit
        );
        Ok(())
    }

    pub(super) fn execute_service_action(&self, name: &str, action: &ServiceAction) -> Result<()> {
        self.validate_systemd_unit(name)?;
        let runtime_action = match action {
            ServiceAction::Enable => {
                self.run_systemctl("enable", Some(name))?;
                None
            }
            ServiceAction::Disable => {
                self.run_systemctl("disable", Some(name))?;
                None
            }
            ServiceAction::Start => Some(crate::activation::SystemdActivationAction::Start),
            ServiceAction::Stop => Some(crate::activation::SystemdActivationAction::Stop),
            ServiceAction::Reload => Some(crate::activation::SystemdActivationAction::Reload),
            ServiceAction::Restart => Some(crate::activation::SystemdActivationAction::Restart),
            ServiceAction::TryRestart => {
                Some(crate::activation::SystemdActivationAction::TryRestart)
            }
            ServiceAction::ReloadOrRestart => {
                Some(crate::activation::SystemdActivationAction::ReloadOrRestart)
            }
            ServiceAction::ReloadOrTryRestart => {
                Some(crate::activation::SystemdActivationAction::ReloadOrTryRestart)
            }
        };
        if let Some(action) = runtime_action {
            let invocation = crate::activation::SystemdActivationInvocation {
                action,
                units: vec![name.to_string()],
                job_mode: None,
                all: false,
                no_block: false,
                no_ask_password: true,
                quiet: false,
                no_warn: false,
                wait: false,
                show_transaction: false,
            };
            invocation.validate()?;
            self.activation_invocations
                .borrow_mut()
                .push(super::CapturedCcsActivation {
                    source: super::CcsActivationSource::DeclarativeService,
                    source_entry: name.to_string(),
                    invocation: invocation.into(),
                });
        }
        info!("Applied systemd service action '{action:?}' to '{name}'");
        Ok(())
    }

    fn validate_systemd_unit(&self, unit: &str) -> Result<()> {
        if !is_safe_declarative_unit_name(unit) {
            anyhow::bail!(
                "invalid declarative systemd unit '{}': unit names must be pathless, nonempty, and contain no NUL bytes",
                unit
            );
        }
        Ok(())
    }

    fn run_systemctl(&self, action: &str, unit: Option<&str>) -> Result<()> {
        let executable = self.capabilities.systemctl_path().ok_or_else(|| {
            anyhow::anyhow!(
                "systemctl command interface is absent from the persisted host capability inventory; run `conary system init`"
            )
        })?;
        let mut command = Command::new(executable);
        let root = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "selected root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        command.arg(format!("--root={root}"));
        command.arg(action);
        if let Some(unit) = unit {
            command.arg("--").arg(unit);
        }

        let status = command.status().with_context(|| {
            format!(
                "failed to execute persisted systemctl interface {}",
                executable.display()
            )
        })?;
        if status.success() {
            return Ok(());
        }

        anyhow::bail!(
            "systemctl {}{} failed for selected root {} with exit status {:?}",
            action,
            unit.map(|name| format!(" {name}")).unwrap_or_default(),
            self.root.display(),
            status.code()
        )
    }
}

/// Validate the safety envelope for an author-declared CCS unit name.
///
/// This deliberately rejects path operands. It is not systemd's full unit-name
/// grammar: the persisted target systemctl remains semantic authority after
/// Conary has established that the declarative value cannot escape into a
/// filesystem path.
pub(crate) fn is_safe_declarative_unit_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains(['/', '\\', '\0'])
        && !matches!(name, "." | "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::hooks::{HostCapabilityInventory, InitSystemCapability, SystemdInterface};
    use crate::ccs::manifest::{Hooks, Service, SystemdHook};
    use crate::test_support::{HostToolFixture, SYSTEMCTL_CAPTURE_PATH, link_host_tool};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn executor_with_recording_systemctl(root: &Path, tools: &Path) -> HookExecutor {
        let executable = tools.join("systemctl");
        link_host_tool(&executable, HostToolFixture::Systemd);
        let capabilities = HostCapabilityInventory {
            init_system: InitSystemCapability::Systemd,
            systemd: Some(SystemdInterface::probe(executable, true).unwrap()),
            ..HostCapabilityInventory::default()
        };
        HookExecutor::new(root, capabilities)
    }

    #[cfg(unix)]
    #[test]
    fn target_enable_and_disable_use_systemctl_root_contract() {
        let target = TempDir::new().unwrap();
        let tools = TempDir::new().unwrap();
        let log = target.path().join(SYSTEMCTL_CAPTURE_PATH);
        let executor = executor_with_recording_systemctl(target.path(), tools.path());

        executor
            .systemd_set_enabled("portable.service", true)
            .unwrap();
        executor
            .systemd_set_enabled("portable.service", false)
            .unwrap();

        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.contains(&format!(
            "--root={} enable -- portable.service",
            target.path().display()
        )));
        assert!(calls.contains(&format!(
            "--root={} disable -- portable.service",
            target.path().display()
        )));
    }

    #[cfg(unix)]
    #[test]
    fn post_hooks_execute_generic_services_and_false_means_disable() {
        let target = TempDir::new().unwrap();
        let tools = TempDir::new().unwrap();
        let log = target.path().join(SYSTEMCTL_CAPTURE_PATH);
        let executor = executor_with_recording_systemctl(target.path(), tools.path());
        let mut hooks = Hooks::default();
        hooks.systemd.push(SystemdHook {
            unit: "disabled.service".to_string(),
            enable: false,
            reversible: Some(true),
        });
        hooks.services.push(Service {
            name: "enabled.service".to_string(),
            action: ServiceAction::Enable,
            reversible: Some(true),
        });

        let results = executor.execute_post_hooks_with_results(&hooks);

        assert!(results.all_succeeded());
        assert_eq!(results.results.len(), 2);
        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.contains(&format!(
            "--root={} disable -- disabled.service",
            target.path().display()
        )));
        assert!(calls.contains(&format!(
            "--root={} enable -- enabled.service",
            target.path().display()
        )));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_service_action_becomes_activation_intent_without_target_systemctl_call() {
        let target = TempDir::new().unwrap();
        let tools = TempDir::new().unwrap();
        let log = target.path().join(SYSTEMCTL_CAPTURE_PATH);
        let executor = executor_with_recording_systemctl(target.path(), tools.path());
        let mut hooks = Hooks::default();
        hooks.services.push(Service {
            name: "api.service".to_string(),
            action: ServiceAction::ReloadOrTryRestart,
            reversible: Some(true),
        });

        let results = executor.execute_post_hooks_with_results(&hooks);

        assert!(results.all_succeeded());
        assert!(
            !log.exists(),
            "runtime action signaled selected-root systemctl"
        );
        let intents = executor.take_activation_invocations();
        assert_eq!(intents.len(), 1);
        let crate::activation::RuntimeActivationInvocation::Systemd(invocation) =
            &intents[0].invocation
        else {
            panic!("expected systemd activation");
        };
        assert_eq!(
            invocation.action,
            crate::activation::SystemdActivationAction::ReloadOrTryRestart
        );
        assert_eq!(invocation.units, vec!["api.service"]);
        assert_eq!(
            intents[0].source,
            crate::ccs::hooks::CcsActivationSource::DeclarativeService
        );
    }

    #[test]
    fn declarative_unit_names_are_pathless_before_systemctl_validation() {
        assert!(is_safe_declarative_unit_name("sshd.service"));
        assert!(is_safe_declarative_unit_name("demo@instance.service"));
        assert!(!is_safe_declarative_unit_name("../evil.service"));
        assert!(!is_safe_declarative_unit_name(
            "/usr/lib/systemd/system/evil.service"
        ));
        assert!(!is_safe_declarative_unit_name("evil\\unit.service"));
        assert!(!is_safe_declarative_unit_name(""));
        assert!(!is_safe_declarative_unit_name("evil\0unit.service"));
        assert!(!is_safe_declarative_unit_name(&"x".repeat(256)));
    }
}
