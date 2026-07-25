// conary-core/src/ccs/manifest/hooks.rs

use super::*;

/// Declarative hooks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    #[serde(default)]
    pub users: Vec<UserHook>,

    #[serde(default)]
    pub groups: Vec<GroupHook>,

    #[serde(default)]
    pub directories: Vec<DirectoryHook>,

    #[serde(default)]
    pub services: Vec<Service>,

    #[serde(default)]
    pub systemd: Vec<SystemdHook>,

    #[serde(default)]
    pub tmpfiles: Vec<TmpfilesHook>,

    #[serde(default)]
    pub sysctl: Vec<SysctlHook>,

    #[serde(default)]
    pub alternatives: Vec<AlternativeHook>,

    /// Post-install script hook (runs after files are deployed)
    #[serde(default)]
    pub post_install: Option<ScriptHook>,

    /// Pre-remove script hook (runs before files are removed)
    #[serde(default)]
    pub pre_remove: Option<ScriptHook>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookExecutionRoot {
    TryRoot,
    GenerationRoot,
    HostRoot,
}

impl Hooks {
    pub fn has_script_hooks(&self) -> bool {
        self.post_install.is_some() || self.pre_remove.is_some()
    }

    pub fn has_service_hooks(&self) -> bool {
        !self.services.is_empty()
    }

    pub fn has_declarative_hooks(&self) -> bool {
        !self.users.is_empty()
            || !self.groups.is_empty()
            || !self.directories.is_empty()
            || !self.systemd.is_empty()
            || !self.tmpfiles.is_empty()
            || !self.sysctl.is_empty()
            || !self.alternatives.is_empty()
    }

    pub fn has_irreversible_hooks_for_try_root(&self, execution_root: HookExecutionRoot) -> bool {
        if matches!(execution_root, HookExecutionRoot::HostRoot) {
            return self.has_script_hooks()
                || self.has_service_hooks()
                || self.has_declarative_hooks();
        }

        self.services
            .iter()
            .any(|hook| !hook.reversible.unwrap_or(false))
            || self
                .post_install
                .as_ref()
                .is_some_and(|hook| !hook.reversible.unwrap_or(false))
            || self
                .pre_remove
                .as_ref()
                .is_some_and(|hook| !hook.reversible.unwrap_or(false))
            || self
                .users
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .groups
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .directories
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .systemd
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .tmpfiles
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .sysctl
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
            || self
                .alternatives
                .iter()
                .any(|hook| !hook.reversible.unwrap_or(true))
    }
}

/// Scriptlet-scoped declarations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScriptletDeclarations {
    /// Narrow host-integration capabilities requested by scriptlets.
    #[serde(default)]
    pub capabilities: Vec<ScriptletCapabilityDeclaration>,
}

impl ScriptletDeclarations {
    /// Whether any scriptlet capability declarations are present.
    pub fn has_capability_declarations(&self) -> bool {
        !self.capabilities.is_empty()
    }

    pub(super) fn validate(&self) -> Result<(), ManifestError> {
        for capability in &self.capabilities {
            capability.validate()?;
        }
        Ok(())
    }
}

/// A narrow host-integration capability requested by a package scriptlet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptletCapabilityDeclaration {
    pub name: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

impl ScriptletCapabilityDeclaration {
    fn validate(&self) -> Result<(), ManifestError> {
        let Some(allowed_paths) = supported_scriptlet_capability_paths(&self.name) else {
            return Err(ManifestError::Invalid(format!(
                "unknown scriptlet capability '{}'; declare a supported capability or run in a VM until enforcement exists",
                self.name
            )));
        };

        for path in &self.paths {
            if !path.starts_with('/') {
                return Err(ManifestError::Invalid(format!(
                    "relative path not allowed in scriptlets.capabilities '{}': {}",
                    self.name, path
                )));
            }
            if !allowed_paths.contains(&path.as_str()) {
                return Err(ManifestError::Invalid(format!(
                    "unsupported path '{}' for scriptlet capability '{}'; supported paths: {}",
                    path,
                    self.name,
                    allowed_paths.join(", ")
                )));
            }
        }

        Ok(())
    }
}

fn supported_scriptlet_capability_paths(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "systemd-service-registration" => Some(&["/etc/systemd/system"]),
        "tmpfiles-registration" => Some(&["/usr/lib/tmpfiles.d", "/etc/tmpfiles.d"]),
        "dbus-service-registration" => {
            Some(&["/usr/share/dbus-1/system-services", "/etc/dbus-1/system.d"])
        }
        _ => None,
    }
}

/// Linux file capabilities for an executable shipped by the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCapability {
    pub path: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default = "default_true")]
    pub permitted: bool,
    #[serde(default = "default_true")]
    pub effective: bool,
    #[serde(default)]
    pub inheritable: bool,
}

impl FileCapability {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !self.path.starts_with('/') {
            return Err(ManifestError::Invalid(format!(
                "relative path not allowed in file_capabilities: {}",
                self.path
            )));
        }
        sanitize_path(&self.path).map_err(|error| {
            ManifestError::Invalid(format!(
                "invalid file_capabilities path '{}': {}",
                self.path, error
            ))
        })?;
        if self.capabilities.is_empty() {
            return Err(ManifestError::Invalid(format!(
                "file_capabilities '{}' must declare at least one Linux capability",
                self.path
            )));
        }
        for capability in &self.capabilities {
            if !is_supported_linux_file_capability(capability) {
                return Err(ManifestError::Invalid(format!(
                    "unknown Linux file capability '{}' for {}",
                    capability, self.path
                )));
            }
        }
        if self.effective && !self.permitted {
            return Err(ManifestError::Invalid(format!(
                "effective file capability requires permitted for {}",
                self.path
            )));
        }
        Ok(())
    }

    pub fn to_setcap_spec(&self) -> Result<String, ManifestError> {
        self.validate()?;
        let mut flags = String::new();
        if self.effective {
            flags.push('e');
        }
        if self.inheritable {
            flags.push('i');
        }
        if self.permitted {
            flags.push('p');
        }
        Ok(format!("{}=+{}", self.capabilities.join(","), flags))
    }
}

pub fn is_supported_linux_file_capability(name: &str) -> bool {
    LINUX_FILE_CAPABILITY_NAMES.contains(&name)
}

pub const LINUX_FILE_CAPABILITY_NAMES: &[&str] = &[
    "cap_chown",
    "cap_dac_override",
    "cap_dac_read_search",
    "cap_fowner",
    "cap_fsetid",
    "cap_kill",
    "cap_setgid",
    "cap_setuid",
    "cap_setpcap",
    "cap_linux_immutable",
    "cap_net_bind_service",
    "cap_net_broadcast",
    "cap_net_admin",
    "cap_net_raw",
    "cap_ipc_lock",
    "cap_ipc_owner",
    "cap_sys_module",
    "cap_sys_rawio",
    "cap_sys_chroot",
    "cap_sys_ptrace",
    "cap_sys_pacct",
    "cap_sys_admin",
    "cap_sys_boot",
    "cap_sys_nice",
    "cap_sys_resource",
    "cap_sys_time",
    "cap_sys_tty_config",
    "cap_mknod",
    "cap_lease",
    "cap_audit_write",
    "cap_audit_control",
    "cap_setfcap",
    "cap_mac_override",
    "cap_mac_admin",
    "cap_syslog",
    "cap_wake_alarm",
    "cap_block_suspend",
    "cap_audit_read",
    "cap_perfmon",
    "cap_bpf",
    "cap_checkpoint_restore",
];

/// Script hook -- an arbitrary shell command run during install/remove
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptHook {
    pub script: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

pub type User = UserHook;
pub type Group = GroupHook;

/// Generic service management hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub action: ServiceAction,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceAction {
    Enable,
    Disable,
    Start,
    Stop,
    Reload,
    Restart,
    TryRestart,
    ReloadOrRestart,
    ReloadOrTryRestart,
}

/// User creation hook (sysusers-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserHook {
    pub name: String,

    #[serde(default)]
    pub system: bool,

    #[serde(default)]
    pub home: Option<String>,

    #[serde(default)]
    pub shell: Option<String>,

    #[serde(default)]
    pub group: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Group creation hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupHook {
    pub name: String,

    #[serde(default)]
    pub system: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Directory creation hook (tmpfiles-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryHook {
    pub path: String,

    #[serde(default = "default_mode")]
    pub mode: String,

    #[serde(default = "default_owner")]
    pub owner: String,

    #[serde(default = "default_group")]
    pub group: String,

    #[serde(default)]
    pub cleanup: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

fn default_mode() -> String {
    "0755".to_string()
}

fn default_owner() -> String {
    "root".to_string()
}

fn default_group() -> String {
    "root".to_string()
}

/// Systemd unit hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemdHook {
    pub unit: String,

    #[serde(default)]
    pub enable: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// tmpfiles.d entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TmpfilesHook {
    #[serde(rename = "type")]
    pub entry_type: String,

    pub path: String,

    pub mode: String,

    pub user: String,

    pub group: String,

    pub age: String,

    pub argument: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// sysctl setting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysctlHook {
    pub key: String,
    pub value: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

/// Alternatives system hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeHook {
    /// Exact generic link managed by the alternatives implementation.
    pub link: String,
    pub name: String,
    pub path: String,

    #[serde(default = "default_priority")]
    pub priority: i32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
}

fn default_priority() -> i32 {
    50
}
