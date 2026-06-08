// conary-core/src/scriptlet/sandbox.rs

use super::ScriptletExecutor;
use crate::capability::SyscallCapabilities;
use crate::capability::enforcement::{EnforcementMode, EnforcementPolicy};
use crate::container::{
    BindMount, ContainerConfig, ScriptRisk, analyze_script, isolation_available,
};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const LIVE_SANDBOX_READONLY_ETC_FILES: [&str; 5] = [
    "/etc/passwd",
    "/etc/group",
    "/etc/hosts",
    "/etc/shadow",
    "/etc/sudoers",
];

/// Sandbox mode for scriptlet execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// No sandboxing - direct execution
    #[serde(rename = "never", alias = "none")]
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

    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "never",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// Sandbox boundary actually used for a scriptlet execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveSandbox {
    /// Live-root protected mode with namespace isolation.
    ProtectedLiveRoot,
    /// Direct legacy execution on the live host.
    Direct,
    /// Alternate-root execution for bootstrap/offline targets.
    TargetRoot,
}

impl EffectiveSandbox {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtectedLiveRoot => "protected-live-root",
            Self::Direct => "direct",
            Self::TargetRoot => "target-root",
        }
    }
}

impl ScriptletExecutor {
    pub(super) fn should_use_sandbox(&self, script_content: &str) -> bool {
        match self.sandbox_mode {
            SandboxMode::None => false,
            SandboxMode::Always => true,
            SandboxMode::Auto => analyze_script(script_content).risk >= ScriptRisk::Medium,
        }
    }

    pub(super) fn effective_sandbox(&self, use_sandbox: bool) -> EffectiveSandbox {
        if !self.is_live_root() {
            EffectiveSandbox::TargetRoot
        } else if use_sandbox {
            EffectiveSandbox::ProtectedLiveRoot
        } else {
            EffectiveSandbox::Direct
        }
    }

    pub(super) fn preflight_protected_live_sandbox(&self) -> Result<()> {
        if std::env::var_os("CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE").is_some() {
            return Err(protected_scriptlet_sandbox_unavailable(
                "test override forced namespace preflight failure",
            ));
        }

        let config = self.live_sandbox_config()?;
        if !isolation_available() {
            return Err(protected_scriptlet_sandbox_unavailable(
                "mount/user namespace isolation is unavailable",
            ));
        }

        if let Some(policy) = config.capability_policy.as_ref()
            && policy.mode == EnforcementMode::Enforce
            && policy.syscalls.is_some()
        {
            let support = crate::capability::enforcement::check_enforcement_support();
            if !support.seccomp {
                return Err(Error::ScriptletError(
                    "Protected scriptlet sandboxing requires seccomp enforcement support. \
                     Enable seccomp in the kernel/container runtime or run inside a VM. \
                     Dangerous legacy direct execution is available only with --sandbox=never plus \
                     the live-host mutation acknowledgement, and it records effective_sandbox=direct."
                        .to_string(),
                ));
            }
        }

        Ok(())
    }

    pub(super) fn live_sandbox_config(&self) -> Result<ContainerConfig> {
        let mut config = ContainerConfig::default().for_untrusted();
        config.timeout = self.timeout;
        config
            .bind_mounts
            .retain(|mount| !is_live_sandbox_private_target(&mount.target));

        config.add_private_writable_mount("/etc", 0o755)?;
        config.add_private_writable_mount("/var", 0o755)?;

        for protected in LIVE_SANDBOX_READONLY_ETC_FILES {
            config
                .bind_mounts
                .push(BindMount::readonly(protected, protected));
        }

        config.capability_policy = Some(EnforcementPolicy {
            mode: EnforcementMode::Enforce,
            filesystem: None,
            syscalls: Some(SyscallCapabilities {
                allow: Vec::new(),
                deny: Vec::new(),
                profile: Some("scriptlet".to_string()),
            }),
            network_isolation: config.isolate_network,
        });

        Ok(config)
    }
}

fn is_live_sandbox_private_target(target: &Path) -> bool {
    target == Path::new("/etc")
        || target.starts_with("/etc/")
        || target == Path::new("/var")
        || target.starts_with("/var/")
}

fn protected_scriptlet_sandbox_unavailable(reason: &str) -> Error {
    Error::ScriptletError(format!(
        "Protected scriptlet sandboxing requires mount and user namespace support. \
         Enable the required kernel/container namespace support or run inside a VM. \
         Dangerous legacy direct execution is available only with --sandbox=never plus \
         the live-host mutation acknowledgement, and it records effective_sandbox=direct. \
         ({reason})"
    ))
}

#[cfg(test)]
mod tests {
    use super::super::runtime::ENV_LOCK;
    use super::super::{ExecutionMode, PackageFormat, ScriptletExecutor};
    use super::SandboxMode;
    use crate::capability::enforcement::EnforcementMode;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
    use std::path::{Path, PathBuf};

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
    fn sandbox_mode_serde_round_trips_goal7_matrix_spellings() {
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"never\"").expect("never deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"none\"").expect("none alias deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"auto\"").expect("auto deserializes"),
            SandboxMode::Auto
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"always\"").expect("always deserializes"),
            SandboxMode::Always
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::None).expect("serialize none"),
            "\"never\""
        );
    }

    #[test]
    fn test_live_sandbox_config_rebinds_critical_etc_files_readonly() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");
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
    fn test_live_sandbox_config_uses_private_layers_for_writable_etc_and_var() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        for protected_dir in ["/etc", "/var"] {
            let mount = config
                .bind_mounts
                .iter()
                .find(|mount| mount.target == Path::new(protected_dir) && mount.writable)
                .unwrap_or_else(|| panic!("missing writable {protected_dir} sandbox layer"));
            assert_ne!(
                mount.source,
                PathBuf::from(protected_dir),
                "{protected_dir} must use a private writable layer, not the live host path"
            );
            assert!(
                mount.source.exists(),
                "private layer backing {protected_dir} should exist for the sandbox lifetime"
            );
        }
    }

    #[test]
    fn test_live_sandbox_config_fails_closed_on_protection_setup_failures() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        let policy = config
            .capability_policy
            .as_ref()
            .expect("protected live sandbox should carry enforce-mode metadata");
        assert_eq!(policy.mode, EnforcementMode::Enforce);
    }

    #[test]
    fn test_live_sandbox_config_installs_scriptlet_seccomp_profile() {
        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);

        let config = executor.live_sandbox_config().expect("live sandbox config");

        let syscalls = config
            .capability_policy
            .as_ref()
            .and_then(|policy| policy.syscalls.as_ref())
            .expect("protected live sandbox should install the scriptlet seccomp profile");
        assert_eq!(syscalls.profile.as_deref(), Some("scriptlet"));
        assert!(syscalls.allow.is_empty());
        assert!(syscalls.deny.is_empty());
    }

    #[test]
    fn test_protected_live_root_preflight_reports_operator_diagnostic() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(
                "CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE",
                "1",
            );
        }

        let executor =
            ScriptletExecutor::new(Path::new("/"), "test-pkg", "1.0.0", PackageFormat::Rpm);
        let scriptlet = Scriptlet {
            phase: ScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo ok".to_string(),
            flags: None,
        };

        let err = executor
            .preflight(&scriptlet, &ExecutionMode::Install)
            .expect_err("forced protected sandbox preflight failure should be fatal");

        unsafe {
            std::env::remove_var("CONARY_TEST_FORCE_SCRIPTLET_SANDBOX_PREFLIGHT_UNAVAILABLE");
        }

        let message = err.to_string();
        assert!(
            message.contains(
                "Protected scriptlet sandboxing requires mount and user namespace support"
            ),
            "unexpected error: {message}"
        );
        assert!(message.contains("--sandbox=never"));
        assert!(message.contains("effective_sandbox=direct"));
    }
}
