// conary-core/src/ccs/hooks/capabilities/openrc.rs

//! Typed OpenRC service-manager interface.

use super::{
    ExecutableInterface, HostCapabilityInventory, HostCapabilityInventoryError,
    HostCapabilityPreflightError, HostCapabilityRequirement, InitSystemCapability,
    SystemdOperation,
};
use crate::ccs::manifest::{Service, ServiceAction};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRcInterface {
    pub rc_service: ExecutableInterface,
    pub default_runlevel: String,
}

impl OpenRcInterface {
    pub fn probe(executable: PathBuf) -> Option<Self> {
        Some(Self {
            rc_service: ExecutableInterface::probe_openrc_service(executable)?,
            default_runlevel: "default".to_string(),
        })
    }

    pub(super) fn validate(&self) -> Result<(), HostCapabilityInventoryError> {
        if self.default_runlevel != "default" {
            return Err(HostCapabilityInventoryError::InvalidOpenRcShape);
        }
        Ok(())
    }
}

impl HostCapabilityInventory {
    pub(super) fn preflight_services(
        &self,
        services: &[Service],
    ) -> Result<(), HostCapabilityPreflightError> {
        match self.init_system {
            InitSystemCapability::Systemd => {
                let interface =
                    self.require_interface(HostCapabilityRequirement::Systemd, "hooks.services")?;
                for service in services {
                    if let Some(operation) = operation_for_service_action(&service.action) {
                        self.require_systemd_operation(interface, operation, "hooks.services")?;
                    }
                }
                Ok(())
            }
            InitSystemCapability::OpenRc => {
                let Some(interface) = &self.openrc else {
                    return Err(HostCapabilityPreflightError::MissingCapability {
                        requirement: HostCapabilityRequirement::OpenRc,
                        hook: "hooks.services",
                    });
                };
                if !interface.rc_service.is_available() {
                    return Err(HostCapabilityPreflightError::InterfaceDrift {
                        requirement: HostCapabilityRequirement::OpenRc,
                        executable: interface.rc_service.executable.clone(),
                    });
                }
                if services.iter().any(|service| {
                    matches!(
                        service.action,
                        ServiceAction::ReloadOrRestart | ServiceAction::ReloadOrTryRestart
                    )
                }) {
                    return Err(HostCapabilityPreflightError::MissingCapability {
                        requirement: HostCapabilityRequirement::OpenRcCombinedReload,
                        hook: "hooks.services",
                    });
                }
                Ok(())
            }
            InitSystemCapability::Unsupported => {
                Err(HostCapabilityPreflightError::MissingCapability {
                    requirement: HostCapabilityRequirement::ServiceManager,
                    hook: "hooks.services",
                })
            }
        }
    }

    pub fn live_openrc_service_path(&self) -> Option<&Path> {
        if self.init_system != InitSystemCapability::OpenRc {
            return None;
        }
        let interface = self.openrc.as_ref()?;
        interface
            .rc_service
            .is_available()
            .then_some(interface.rc_service.executable.as_path())
    }

    pub(super) fn validate_openrc_shape(&self) -> Result<(), HostCapabilityInventoryError> {
        match (&self.init_system, &self.openrc) {
            (InitSystemCapability::OpenRc, Some(interface)) => interface.validate(),
            (
                InitSystemCapability::Systemd | InitSystemCapability::Unsupported,
                Some(interface),
            ) => interface.validate(),
            (InitSystemCapability::Systemd | InitSystemCapability::Unsupported, None) => Ok(()),
            _ => Err(HostCapabilityInventoryError::InvalidOpenRcShape),
        }
    }
}

fn operation_for_service_action(action: &ServiceAction) -> Option<SystemdOperation> {
    match action {
        ServiceAction::Enable => Some(SystemdOperation::OfflineEnable),
        ServiceAction::Disable => Some(SystemdOperation::OfflineDisable),
        ServiceAction::Start
        | ServiceAction::Stop
        | ServiceAction::Reload
        | ServiceAction::Restart
        | ServiceAction::TryRestart
        | ServiceAction::ReloadOrRestart
        | ServiceAction::ReloadOrTryRestart => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::{Hooks, Service};
    use crate::test_support::{HostToolFixture, link_host_tool};

    #[cfg(unix)]
    fn openrc_inventory() -> (tempfile::TempDir, HostCapabilityInventory) {
        let tools = tempfile::tempdir().unwrap();
        let executable = tools.path().join("rc-service");
        link_host_tool(&executable, HostToolFixture::OpenRc);
        let inventory = HostCapabilityInventory {
            init_system: InitSystemCapability::OpenRc,
            openrc: Some(OpenRcInterface::probe(executable).unwrap()),
            ..HostCapabilityInventory::default()
        };
        (tools, inventory)
    }

    #[cfg(unix)]
    #[test]
    fn active_openrc_inventory_preflights_generic_runtime_services() {
        let (_tools, inventory) = openrc_inventory();
        let root = tempfile::tempdir().unwrap();
        let mut hooks = Hooks::default();
        hooks.services.push(Service {
            name: "sshd".to_string(),
            action: ServiceAction::Restart,
            reversible: Some(false),
        });

        inventory.validate().unwrap();
        inventory.preflight_hooks(root.path(), &hooks).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn openrc_rejects_combined_reload_semantics_before_mutation() {
        let (_tools, inventory) = openrc_inventory();
        let root = tempfile::tempdir().unwrap();
        for action in [
            ServiceAction::ReloadOrRestart,
            ServiceAction::ReloadOrTryRestart,
        ] {
            let mut hooks = Hooks::default();
            hooks.services.push(Service {
                name: "sshd".to_string(),
                action,
                reversible: Some(false),
            });
            assert!(matches!(
                inventory.preflight_hooks(root.path(), &hooks),
                Err(HostCapabilityPreflightError::MissingCapability {
                    requirement: HostCapabilityRequirement::OpenRcCombinedReload,
                    hook: "hooks.services"
                })
            ));
        }
    }

    #[test]
    fn active_openrc_requires_one_exact_interface_shape() {
        let inventory = HostCapabilityInventory {
            init_system: InitSystemCapability::OpenRc,
            ..HostCapabilityInventory::default()
        };

        assert!(matches!(
            inventory.validate(),
            Err(HostCapabilityInventoryError::InvalidOpenRcShape)
        ));
    }
}
