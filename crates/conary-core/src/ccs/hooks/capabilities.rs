// crates/conary-core/src/ccs/hooks/capabilities.rs

//! Typed host integration capabilities used by declarative CCS hooks.
//!
//! Package source formats never select these interfaces. `conary system init`
//! discovers the interfaces exposed by the current host, persists this
//! document, and install preflight resolves source-independent lifecycle hooks
//! against it before any payload mutation.

use crate::ccs::manifest::Hooks;
use crate::db::models::settings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[path = "capabilities/filesystem_security.rs"]
mod filesystem_security;
#[path = "capabilities/interface_contract.rs"]
mod interface_contract;
#[path = "capabilities/openrc.rs"]
mod openrc;

pub use filesystem_security::{
    ImmutableBackingSecurity, ImmutableBackingSecurityError, ImmutableBackingSecurityMechanism,
};
use interface_contract::systemd_manager_responds;
pub use interface_contract::{
    ExecutableInterface, HostExecutableContract, HostExecutableImplementation,
};
pub use openrc::OpenRcInterface;

pub const HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION: u32 = 5;
pub const HOST_CAPABILITY_INVENTORY_SETTING: &str = "system.host-capability-inventory";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitSystemCapability {
    Systemd,
    OpenRc,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemdOperation {
    Enable,
    Disable,
    Start,
    Stop,
    Restart,
    DaemonReload,
    OfflineEnable,
    OfflineDisable,
}

/// Target-owned parser and executor for the exact `tmpfiles.d` grammar.
///
/// Conary only validates the structural seven-column envelope. Supported line
/// types, modifier combinations, specifiers, modes, identities, ages, and
/// arguments are interpreted by this persisted target executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TmpfilesInterface {
    pub command: ExecutableInterface,
}

impl TmpfilesInterface {
    pub fn probe(executable: PathBuf) -> Option<Self> {
        Some(Self {
            command: ExecutableInterface::probe_tmpfiles(executable)?,
        })
    }

    fn is_available(&self) -> bool {
        self.command.is_available()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemdInterface {
    pub command: ExecutableInterface,
    pub operations: BTreeSet<SystemdOperation>,
}

impl SystemdInterface {
    pub fn probe(executable: PathBuf, manager_running: bool) -> Option<Self> {
        let mut operations = BTreeSet::from([
            SystemdOperation::Enable,
            SystemdOperation::Disable,
            SystemdOperation::OfflineEnable,
            SystemdOperation::OfflineDisable,
        ]);
        if manager_running {
            operations.extend([
                SystemdOperation::Start,
                SystemdOperation::Stop,
                SystemdOperation::Restart,
                SystemdOperation::DaemonReload,
            ]);
        }
        Some(Self {
            command: ExecutableInterface::probe_systemctl(executable)?,
            operations,
        })
    }

    pub fn supports(&self, operation: SystemdOperation) -> bool {
        self.operations.contains(&operation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCapabilityInventory {
    pub schema_version: u32,
    pub init_system: InitSystemCapability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub systemd: Option<SystemdInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrc: Option<OpenRcInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysusers: Option<ExecutableInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmpfiles: Option<TmpfilesInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl: Option<ExecutableInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ldconfig: Option<ExecutableInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immutable_backing_security: Option<ImmutableBackingSecurity>,
}

impl Default for HostCapabilityInventory {
    fn default() -> Self {
        Self {
            schema_version: HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION,
            init_system: InitSystemCapability::Unsupported,
            systemd: None,
            openrc: None,
            sysusers: None,
            tmpfiles: None,
            sysctl: None,
            ldconfig: None,
            immutable_backing_security: None,
        }
    }
}

impl HostCapabilityInventory {
    /// Discover exact command interfaces and the active init manager without
    /// consulting a distro name, release file, package manager, or source feed.
    pub fn discover() -> Result<Self, HostCapabilityInventoryError> {
        let immutable_backing_security = ImmutableBackingSecurity::probe(Path::new("/usr"))?;
        Ok(Self::discover_in_paths_with_security(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            immutable_backing_security,
        ))
    }

    #[cfg(test)]
    fn discover_in_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::discover_in_paths_with_runtime(paths, None, false, false)
    }

    fn discover_in_paths_with_security(
        paths: impl IntoIterator<Item = PathBuf>,
        immutable_backing_security: Option<ImmutableBackingSecurity>,
    ) -> Self {
        Self::discover_in_paths_with_runtime(
            paths,
            immutable_backing_security,
            Path::new("/run/systemd/system").is_dir(),
            Path::new("/run/openrc/softlevel").is_file(),
        )
    }

    fn discover_in_paths_with_runtime(
        paths: impl IntoIterator<Item = PathBuf>,
        immutable_backing_security: Option<ImmutableBackingSecurity>,
        systemd_booted: bool,
        openrc_booted: bool,
    ) -> Self {
        let paths = paths.into_iter().collect::<Vec<_>>();
        let systemd = discover_candidate("systemctl", &paths)
            .and_then(|executable| SystemdInterface::probe(executable, systemd_booted));
        let manager_running = systemd_booted
            && systemd
                .as_ref()
                .is_some_and(|interface| systemd_manager_responds(&interface.command.executable));
        let openrc = discover_candidate("rc-service", &paths).and_then(OpenRcInterface::probe);
        let init_system = if manager_running {
            InitSystemCapability::Systemd
        } else if openrc_booted && openrc.is_some() {
            InitSystemCapability::OpenRc
        } else {
            InitSystemCapability::Unsupported
        };

        Self {
            schema_version: HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION,
            init_system,
            systemd: systemd.map(|mut interface| {
                if !manager_running {
                    interface.operations.retain(|operation| {
                        matches!(
                            operation,
                            SystemdOperation::Enable
                                | SystemdOperation::Disable
                                | SystemdOperation::OfflineEnable
                                | SystemdOperation::OfflineDisable
                        )
                    });
                }
                interface
            }),
            openrc,
            sysusers: discover_candidate("systemd-sysusers", &paths)
                .and_then(ExecutableInterface::probe_sysusers),
            tmpfiles: discover_candidate("systemd-tmpfiles", &paths)
                .and_then(TmpfilesInterface::probe),
            sysctl: discover_candidate("sysctl", &paths)
                .and_then(ExecutableInterface::probe_sysctl),
            ldconfig: discover_candidate("ldconfig", &paths)
                .and_then(ExecutableInterface::probe_ldconfig),
            immutable_backing_security,
        }
    }

    pub fn persist(&self, conn: &rusqlite::Connection) -> Result<(), HostCapabilityInventoryError> {
        self.validate()?;
        let encoded = serde_json::to_string(self)?;
        settings::set(conn, HOST_CAPABILITY_INVENTORY_SETTING, &encoded)?;
        Ok(())
    }

    pub fn load_required(
        conn: &rusqlite::Connection,
    ) -> Result<Self, HostCapabilityInventoryError> {
        let encoded = settings::get(conn, HOST_CAPABILITY_INVENTORY_SETTING)?.ok_or(
            HostCapabilityInventoryError::Missing {
                setting: HOST_CAPABILITY_INVENTORY_SETTING,
            },
        )?;
        let inventory: Self = serde_json::from_str(&encoded)?;
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn validate(&self) -> Result<(), HostCapabilityInventoryError> {
        if self.schema_version != HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION {
            return Err(HostCapabilityInventoryError::UnsupportedSchema {
                found: self.schema_version,
                expected: HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION,
            });
        }

        for (interface, executable) in self.interface_paths() {
            if !executable.is_absolute() {
                return Err(HostCapabilityInventoryError::RelativeExecutable {
                    interface,
                    executable: executable.to_path_buf(),
                });
            }
        }
        for (name, descriptor, expected_contract) in self.interface_descriptors() {
            if descriptor.contract != expected_contract {
                return Err(HostCapabilityInventoryError::WrongContract {
                    interface: name,
                    found: descriptor.contract,
                    expected: expected_contract,
                });
            }
            if !descriptor.descriptor_is_well_formed() {
                return Err(HostCapabilityInventoryError::InvalidDescriptor { interface: name });
            }
        }
        if let Some(capability) = &self.immutable_backing_security {
            capability.validate()?;
        }
        self.validate_systemd_shape()?;
        self.validate_openrc_shape()?;
        Ok(())
    }

    pub fn preflight_hooks(
        &self,
        root: &Path,
        hooks: &Hooks,
    ) -> Result<(), HostCapabilityPreflightError> {
        if root == Path::new("/") || !root.is_absolute() {
            return Err(HostCapabilityPreflightError::InvalidExecutionRoot {
                root: root.to_path_buf(),
            });
        }

        if !hooks.services.is_empty() {
            self.preflight_services(&hooks.services)?;
        }

        if !hooks.systemd.is_empty() {
            self.require_systemd_target("hooks.systemd")?;
            let interface =
                self.require_interface(HostCapabilityRequirement::Systemd, "hooks.systemd")?;
            for hook in &hooks.systemd {
                let operation = if hook.enable {
                    SystemdOperation::OfflineEnable
                } else {
                    SystemdOperation::OfflineDisable
                };
                self.require_systemd_operation(interface, operation, "hooks.systemd")?;
            }
        }

        if !hooks.tmpfiles.is_empty() {
            let Some(interface) = self.tmpfiles.as_ref() else {
                return Err(HostCapabilityPreflightError::MissingCapability {
                    requirement: HostCapabilityRequirement::Tmpfiles,
                    hook: "hooks.tmpfiles",
                });
            };
            if !interface.is_available() {
                return Err(HostCapabilityPreflightError::InterfaceDrift {
                    requirement: HostCapabilityRequirement::Tmpfiles,
                    executable: interface.command.executable.clone(),
                });
            }
        }

        Ok(())
    }

    pub(super) fn systemctl_path(&self) -> Option<&Path> {
        self.systemd
            .as_ref()
            .map(|interface| interface.command.executable.as_path())
    }

    /// Return the exact persisted systemctl executable only while the booted
    /// systemd manager and executable identity still satisfy the inventory.
    pub fn live_systemctl_path(&self) -> Option<&Path> {
        if self.init_system != InitSystemCapability::Systemd {
            return None;
        }
        let interface = self.systemd.as_ref()?;
        interface
            .command
            .is_available()
            .then_some(interface.command.executable.as_path())
    }

    pub(super) fn tmpfiles_path(&self) -> Option<&Path> {
        self.tmpfiles
            .as_ref()
            .map(|interface| interface.command.executable.as_path())
    }

    /// Return the exact persisted sysusers target interface after repeating
    /// its implementation handshake and executable identity check.
    pub fn sysusers_interface(&self) -> Result<&ExecutableInterface, HostCapabilityPreflightError> {
        let Some(interface) = self.sysusers.as_ref() else {
            return Err(HostCapabilityPreflightError::MissingCapability {
                requirement: HostCapabilityRequirement::Sysusers,
                hook: "RPM sysusers native event",
            });
        };
        if !interface.is_available() {
            return Err(HostCapabilityPreflightError::InterfaceDrift {
                requirement: HostCapabilityRequirement::Sysusers,
                executable: interface.executable.clone(),
            });
        }
        Ok(interface)
    }

    /// Return the exact ldconfig program path inside `root` only when libalpm's
    /// two structural guards hold: `/etc/ld.so.conf` exists and the configured
    /// target-root program is executable.
    pub fn arch_ldconfig_path(&self, root: &Path) -> Option<&Path> {
        if !rooted_path(root, Path::new("/etc/ld.so.conf")).exists() {
            return None;
        }
        self.arch_ldconfig_executable_path(root)
    }

    /// Return the configured target-root ldconfig program when it is
    /// executable, without re-reading the payload-dependent configuration
    /// guard. Transaction planners use this after proving the final path set.
    pub fn arch_ldconfig_executable_path(&self, root: &Path) -> Option<&Path> {
        let interface = self.ldconfig.as_ref()?;
        interface
            .is_available_in_root(root)
            .then_some(interface.executable.as_path())
    }

    fn interface_paths(&self) -> Vec<(&'static str, &Path)> {
        let mut paths = Vec::new();
        if let Some(interface) = &self.systemd {
            paths.push(("systemd", interface.command.executable.as_path()));
        }
        if let Some(interface) = &self.openrc {
            paths.push(("openrc", interface.rc_service.executable.as_path()));
        }
        if let Some(interface) = &self.sysusers {
            paths.push(("sysusers", interface.executable.as_path()));
        }
        if let Some(interface) = &self.tmpfiles {
            paths.push(("tmpfiles", interface.command.executable.as_path()));
        }
        if let Some(interface) = &self.sysctl {
            paths.push(("sysctl", interface.executable.as_path()));
        }
        if let Some(interface) = &self.ldconfig {
            paths.push(("ldconfig", interface.executable.as_path()));
        }
        paths
    }

    fn interface_descriptors(
        &self,
    ) -> Vec<(&'static str, &ExecutableInterface, HostExecutableContract)> {
        let mut descriptors = Vec::new();
        if let Some(interface) = &self.systemd {
            descriptors.push((
                "systemd",
                &interface.command,
                HostExecutableContract::SystemdSystemctl,
            ));
        }
        if let Some(interface) = &self.openrc {
            descriptors.push((
                "openrc",
                &interface.rc_service,
                HostExecutableContract::OpenRcService,
            ));
        }
        if let Some(interface) = &self.sysusers {
            descriptors.push((
                "sysusers",
                interface,
                HostExecutableContract::SystemdSysusers,
            ));
        }
        if let Some(interface) = &self.tmpfiles {
            descriptors.push((
                "tmpfiles",
                &interface.command,
                HostExecutableContract::SystemdTmpfiles,
            ));
        }
        if let Some(interface) = &self.sysctl {
            descriptors.push(("sysctl", interface, HostExecutableContract::ProcpsSysctl));
        }
        if let Some(interface) = &self.ldconfig {
            descriptors.push(("ldconfig", interface, HostExecutableContract::GlibcLdconfig));
        }
        descriptors
    }

    fn validate_systemd_shape(&self) -> Result<(), HostCapabilityInventoryError> {
        let offline_operations = BTreeSet::from([
            SystemdOperation::Enable,
            SystemdOperation::Disable,
            SystemdOperation::OfflineEnable,
            SystemdOperation::OfflineDisable,
        ]);
        let mut live_operations = offline_operations.clone();
        live_operations.extend([
            SystemdOperation::Start,
            SystemdOperation::Stop,
            SystemdOperation::Restart,
            SystemdOperation::DaemonReload,
        ]);

        match (&self.init_system, &self.systemd) {
            (InitSystemCapability::Systemd, Some(interface))
                if interface.operations == live_operations =>
            {
                Ok(())
            }
            (InitSystemCapability::OpenRc | InitSystemCapability::Unsupported, Some(interface))
                if interface.operations == offline_operations =>
            {
                Ok(())
            }
            (InitSystemCapability::OpenRc | InitSystemCapability::Unsupported, None) => Ok(()),
            _ => Err(HostCapabilityInventoryError::InvalidSystemdShape),
        }
    }

    fn require_systemd_target(
        &self,
        hook: &'static str,
    ) -> Result<(), HostCapabilityPreflightError> {
        if self.init_system != InitSystemCapability::Systemd {
            return Err(HostCapabilityPreflightError::MissingCapability {
                requirement: HostCapabilityRequirement::SystemdManager,
                hook,
            });
        }
        Ok(())
    }

    fn require_interface(
        &self,
        requirement: HostCapabilityRequirement,
        hook: &'static str,
    ) -> Result<&SystemdInterface, HostCapabilityPreflightError> {
        let Some(interface) = self.systemd.as_ref() else {
            return Err(HostCapabilityPreflightError::MissingCapability { requirement, hook });
        };
        if !interface.command.is_available() {
            return Err(HostCapabilityPreflightError::InterfaceDrift {
                requirement,
                executable: interface.command.executable.clone(),
            });
        }
        Ok(interface)
    }

    fn require_systemd_operation(
        &self,
        interface: &SystemdInterface,
        operation: SystemdOperation,
        hook: &'static str,
    ) -> Result<(), HostCapabilityPreflightError> {
        if !interface.supports(operation) {
            return Err(HostCapabilityPreflightError::MissingCapability {
                requirement: HostCapabilityRequirement::SystemdOperation(operation),
                hook,
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostCapabilityInventoryError {
    #[error("host capability inventory is not initialized ({setting}); run `conary system init`")]
    Missing { setting: &'static str },
    #[error(
        "host capability inventory schema {found} is unsupported; expected {expected}; run `conary system init`"
    )]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error(
        "host capability inventory interface {interface} has a relative executable path: {executable}"
    )]
    RelativeExecutable {
        interface: &'static str,
        executable: PathBuf,
    },
    #[error("host capability inventory interface {interface} has an invalid typed descriptor")]
    InvalidDescriptor { interface: &'static str },
    #[error(
        "host capability inventory interface {interface} carries contract {found:?}, expected {expected:?}"
    )]
    WrongContract {
        interface: &'static str,
        found: HostExecutableContract,
        expected: HostExecutableContract,
    },
    #[error("host capability inventory init-system and systemd operation sets are inconsistent")]
    InvalidSystemdShape,
    #[error("host capability inventory init-system and OpenRC interface are inconsistent")]
    InvalidOpenRcShape,
    #[error(transparent)]
    ImmutableBackingSecurity(#[from] ImmutableBackingSecurityError),
    #[error(transparent)]
    Database(#[from] crate::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCapabilityRequirement {
    ServiceManager,
    SystemdManager,
    Systemd,
    SystemdOperation(SystemdOperation),
    OpenRc,
    OpenRcCombinedReload,
    Sysusers,
    Tmpfiles,
    Sysctl,
    Ldconfig,
}

impl std::fmt::Display for HostCapabilityRequirement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServiceManager => formatter.write_str("a typed service manager"),
            Self::SystemdManager => formatter.write_str("active systemd service manager"),
            Self::Systemd => formatter.write_str("systemctl command interface"),
            Self::SystemdOperation(operation) => {
                write!(formatter, "systemctl {operation:?} operation")
            }
            Self::OpenRc => formatter.write_str("rc-service command interface"),
            Self::OpenRcCombinedReload => formatter
                .write_str("an OpenRC provider for reload-or-restart and reload-or-try-restart"),
            Self::Sysusers => formatter.write_str("systemd-sysusers target interface"),
            Self::Tmpfiles => formatter.write_str("systemd-tmpfiles command interface"),
            Self::Sysctl => formatter.write_str("sysctl command interface"),
            Self::Ldconfig => formatter.write_str("target-root ldconfig command interface"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostCapabilityPreflightError {
    #[error(
        "{hook} requires {requirement}, but the persisted host capability inventory does not provide it; install or configure that structural interface and rerun `conary system init`"
    )]
    MissingCapability {
        requirement: HostCapabilityRequirement,
        hook: &'static str,
    },
    #[error(
        "persisted {requirement} executable is no longer a valid command interface: {executable}; rerun `conary system init`"
    )]
    InterfaceDrift {
        requirement: HostCapabilityRequirement,
        executable: PathBuf,
    },
    #[error(
        "CCS hook execution requires an absolute materialized selected root other than '/': {root}"
    )]
    InvalidExecutionRoot { root: PathBuf },
}

fn discover_candidate(command: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .map(|directory| directory.join(command))
        .find(|candidate| candidate.is_file())
}

fn rooted_path(root: &Path, absolute_path: &Path) -> PathBuf {
    if root == Path::new("/") {
        absolute_path.to_path_buf()
    } else {
        root.join(absolute_path.strip_prefix("/").unwrap_or(absolute_path))
    }
}

#[cfg(test)]
mod tests;
