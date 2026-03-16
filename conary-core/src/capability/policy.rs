// conary-core/src/capability/policy.rs
//! Three-tier capability policy engine (allowed/prompt/denied)
//!
//! Replaces the hard-reject behavior for packages declaring capabilities.
//! Policy is evaluated per-capability against three tiers:
//! - **Allowed**: capability is granted without user interaction
//! - **Prompt**: capability requires user confirmation before proceeding
//! - **Denied**: capability is blocked unless explicitly overridden in policy

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Result of evaluating a capability against the policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Capability is allowed without user interaction
    Allowed,
    /// Capability requires user confirmation (contains reason)
    Prompt(String),
    /// Capability is denied (contains reason)
    Denied(String),
}

/// Default tier value for capabilities not explicitly listed
fn default_tier() -> String {
    "prompt".to_string()
}

/// Three-tier capability policy configuration
///
/// Capabilities listed in `allowed` are granted silently. Those in `prompt`
/// require user confirmation. Those in `denied` are blocked. Capabilities
/// not listed in any tier fall back to `default_tier`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    /// Capabilities that are automatically allowed
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Capabilities that require user confirmation
    #[serde(default)]
    pub prompt: Vec<String>,
    /// Capabilities that are blocked unless explicitly overridden
    #[serde(default)]
    pub denied: Vec<String>,
    /// Tier to use for capabilities not listed in any tier ("allowed", "prompt", or "denied")
    #[serde(default = "default_tier")]
    pub default_tier: String,
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self {
            allowed: vec![
                "cap-dac-read-search".into(),
                "cap-chown".into(),
                "cap-fowner".into(),
            ],
            prompt: vec![
                "cap-net-raw".into(),
                "cap-net-bind-service".into(),
                "cap-sys-ptrace".into(),
            ],
            denied: vec![
                "cap-sys-admin".into(),
                "cap-sys-rawio".into(),
                "cap-sys-module".into(),
            ],
            default_tier: default_tier(),
        }
    }
}

impl CapabilityPolicy {
    /// Evaluate a capability name against this policy
    ///
    /// Checks the allowed, denied, and prompt lists in order. If the capability
    /// is not found in any list, falls back to `default_tier`.
    #[must_use]
    pub fn evaluate(&self, capability: &str) -> PolicyDecision {
        if self.allowed.iter().any(|c| c == capability) {
            return PolicyDecision::Allowed;
        }

        if self.denied.iter().any(|c| c == capability) {
            return PolicyDecision::Denied(format!(
                "{capability} requires explicit policy override"
            ));
        }

        if self.prompt.iter().any(|c| c == capability) {
            return PolicyDecision::Prompt(format!("{capability} requires user confirmation"));
        }

        // Capability not in any explicit list -- fall back to default tier
        match self.default_tier.as_str() {
            "allowed" => PolicyDecision::Allowed,
            "denied" => {
                PolicyDecision::Denied(format!("{capability} requires explicit policy override"))
            }
            // "prompt" or any unrecognized value defaults to prompt
            _ => PolicyDecision::Prompt(format!("{capability} requires user confirmation")),
        }
    }

    /// Load a capability policy from a TOML file
    ///
    /// If `path` is `Some`, loads from that path. Otherwise, tries the system
    /// default at `/etc/conary/capability-policy.toml`. If neither exists,
    /// returns the built-in default policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let candidates: Vec<&str> = match path {
            Some(p) => vec![p],
            None => vec!["/etc/conary/capability-policy.toml"],
        };

        for candidate in candidates {
            let candidate_path = Path::new(candidate);
            if candidate_path.exists() {
                let contents = std::fs::read_to_string(candidate_path)?;
                let policy: Self = toml::from_str(&contents)?;
                return Ok(policy);
            }
        }

        Ok(Self::default())
    }
}

/// Infer required Linux capabilities from a `CapabilityDeclaration`.
///
/// Maps high-level declarations (network ports, filesystem paths, syscalls) to
/// Linux `CAP_*` constant names (lowercase, hyphen-separated). The returned
/// vector is sorted and deduplicated.
pub fn infer_linux_capabilities(decl: &super::CapabilityDeclaration) -> Vec<String> {
    let mut caps = Vec::new();

    // Network: privileged ports (< 1024) need CAP_NET_BIND_SERVICE
    if decl.network.listen.iter().any(|port_spec| {
        // Handle "any", ranges ("80-443"), and single ports ("80")
        if port_spec == "any" {
            // "any" includes privileged ports
            return true;
        }
        if let Some((start, _end)) = port_spec.split_once('-') {
            start.parse::<u16>().is_ok_and(|p| p < 1024)
        } else {
            port_spec.parse::<u16>().is_ok_and(|p| p < 1024)
        }
    }) {
        caps.push("cap-net-bind-service".into());
    }

    // Filesystem: writing outside standard paths may need CAP_DAC_OVERRIDE
    let standard_prefixes = ["/usr/", "/etc/", "/var/", "/opt/"];
    if decl.filesystem.write.iter().any(|path| {
        !standard_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }) {
        caps.push("cap-dac-override".into());
    }

    // Syscalls: specific syscalls imply capabilities
    for syscall in &decl.syscalls.allow {
        match syscall.as_str() {
            "ptrace" => caps.push("cap-sys-ptrace".into()),
            "reboot" => caps.push("cap-sys-admin".into()),
            "mount" | "umount" => caps.push("cap-sys-admin".into()),
            "mknod" => caps.push("cap-mknod".into()),
            _ => {}
        }
    }

    caps.sort();
    caps.dedup();
    caps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_denies_sys_admin() {
        let policy = CapabilityPolicy::default();
        assert_eq!(
            policy.evaluate("cap-sys-admin"),
            PolicyDecision::Denied("cap-sys-admin requires explicit policy override".into())
        );
    }

    #[test]
    fn test_default_policy_prompts_net_raw() {
        let policy = CapabilityPolicy::default();
        assert_eq!(
            policy.evaluate("cap-net-raw"),
            PolicyDecision::Prompt("cap-net-raw requires user confirmation".into())
        );
    }

    #[test]
    fn test_custom_policy_allows_net_raw() {
        let policy = CapabilityPolicy {
            allowed: vec!["cap-net-raw".into()],
            ..Default::default()
        };
        assert_eq!(policy.evaluate("cap-net-raw"), PolicyDecision::Allowed);
    }

    #[test]
    fn test_unlisted_capability_uses_default_tier() {
        let policy = CapabilityPolicy::default();
        // "cap-unknown" is not in any tier, should default to prompt
        match policy.evaluate("cap-unknown") {
            PolicyDecision::Prompt(_) => {}
            other => panic!("Expected Prompt for unlisted cap, got {:?}", other),
        }
    }

    #[test]
    fn test_default_policy_allows_chown() {
        let policy = CapabilityPolicy::default();
        assert_eq!(policy.evaluate("cap-chown"), PolicyDecision::Allowed);
    }

    #[test]
    fn test_load_falls_back_to_default() {
        // No file at a nonexistent path, should return default
        let policy = CapabilityPolicy::load(Some("/nonexistent/path/policy.toml")).unwrap();
        assert_eq!(policy.allowed, CapabilityPolicy::default().allowed);
    }

    #[test]
    fn test_custom_default_tier_denied() {
        let policy = CapabilityPolicy {
            allowed: vec![],
            prompt: vec![],
            denied: vec![],
            default_tier: "denied".into(),
        };
        assert_eq!(
            policy.evaluate("cap-anything"),
            PolicyDecision::Denied("cap-anything requires explicit policy override".into())
        );
    }

    #[test]
    fn test_custom_default_tier_allowed() {
        let policy = CapabilityPolicy {
            allowed: vec![],
            prompt: vec![],
            denied: vec![],
            default_tier: "allowed".into(),
        };
        assert_eq!(policy.evaluate("cap-anything"), PolicyDecision::Allowed);
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = CapabilityPolicy::default();
        let toml_str = toml::to_string(&policy).unwrap();
        let deserialized: CapabilityPolicy = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.allowed, policy.allowed);
        assert_eq!(deserialized.prompt, policy.prompt);
        assert_eq!(deserialized.denied, policy.denied);
        assert_eq!(deserialized.default_tier, policy.default_tier);
    }

    #[test]
    fn test_infer_capabilities_privileged_port() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.network.listen = vec!["80".into(), "443".into()]; // ports < 1024
        let caps = infer_linux_capabilities(&decl);
        assert!(caps.contains(&"cap-net-bind-service".to_string()));
    }

    #[test]
    fn test_infer_capabilities_ptrace_syscall() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.syscalls.allow = vec!["ptrace".into()];
        let caps = infer_linux_capabilities(&decl);
        assert!(caps.contains(&"cap-sys-ptrace".to_string()));
    }

    #[test]
    fn test_infer_empty_declaration() {
        use crate::capability::CapabilityDeclaration;
        let decl = CapabilityDeclaration::new();
        let caps = infer_linux_capabilities(&decl);
        assert!(caps.is_empty(), "Empty declaration should infer no caps");
    }

    #[test]
    fn test_infer_capabilities_nonstandard_write_path() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.filesystem.write = vec!["/home/app/data".into()];
        let caps = infer_linux_capabilities(&decl);
        assert!(caps.contains(&"cap-dac-override".to_string()));
    }

    #[test]
    fn test_infer_capabilities_standard_write_path_no_cap() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.filesystem.write = vec!["/var/log/myapp".into(), "/etc/myapp".into()];
        let caps = infer_linux_capabilities(&decl);
        assert!(
            !caps.contains(&"cap-dac-override".to_string()),
            "Standard paths should not trigger cap-dac-override"
        );
    }

    #[test]
    fn test_infer_capabilities_high_port_no_cap() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.network.listen = vec!["8080".into(), "9090".into()];
        let caps = infer_linux_capabilities(&decl);
        assert!(
            !caps.contains(&"cap-net-bind-service".to_string()),
            "High ports should not trigger cap-net-bind-service"
        );
    }

    #[test]
    fn test_infer_capabilities_mount_syscall() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        decl.syscalls.allow = vec!["mount".into()];
        let caps = infer_linux_capabilities(&decl);
        assert!(caps.contains(&"cap-sys-admin".to_string()));
    }

    #[test]
    fn test_infer_capabilities_deduplicates() {
        use crate::capability::CapabilityDeclaration;
        let mut decl = CapabilityDeclaration::new();
        // Both mount and reboot map to cap-sys-admin
        decl.syscalls.allow = vec!["mount".into(), "reboot".into()];
        let caps = infer_linux_capabilities(&decl);
        assert_eq!(
            caps.iter()
                .filter(|c| c.as_str() == "cap-sys-admin")
                .count(),
            1,
            "cap-sys-admin should appear only once"
        );
    }
}
