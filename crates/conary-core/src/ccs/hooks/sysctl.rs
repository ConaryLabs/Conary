// conary-core/src/ccs/hooks/sysctl.rs

//! Sysctl integration for CCS hooks
//!
//! Persists exact sysctl desired state in a materialized selected root.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::fs;
use tracing::debug;

/// Validate sysctl key contains only safe characters
pub(crate) fn validate_sysctl_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(anyhow::anyhow!("Sysctl key cannot be empty"));
    }
    // sysctl keys should only contain alphanumeric, dots, slashes, underscores, hyphens
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '_' | '-'))
    {
        return Err(anyhow::anyhow!(
            "Sysctl key contains invalid characters: {}",
            key
        ));
    }
    if is_denied_sysctl_key(key) {
        return Err(anyhow::anyhow!(
            "Sysctl key is denied for security reasons: {}",
            key
        ));
    }
    Ok(())
}

pub(crate) fn is_denied_sysctl_key(key: &str) -> bool {
    matches!(
        key.replace('/', ".").as_str(),
        "kernel.randomize_va_space"
            | "kernel.kptr_restrict"
            | "kernel.modules_disabled"
            | "kernel.dmesg_restrict"
            | "kernel.perf_event_paranoid"
            | "kernel.yama.ptrace_scope"
            | "kernel.unprivileged_bpf_disabled"
            | "kernel.unprivileged_userns_clone"
            | "fs.protected_hardlinks"
            | "fs.protected_symlinks"
            | "fs.protected_regular"
            | "fs.protected_fifos"
    )
}

/// Validate sysctl value contains no newline characters
pub(crate) fn validate_sysctl_value(value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        return Err(anyhow::anyhow!("Sysctl value contains newline characters"));
    }
    Ok(())
}

impl HookExecutor {
    /// Apply a sysctl setting
    ///
    pub(super) fn apply_sysctl(&self, key: &str, value: &str) -> Result<()> {
        validate_sysctl_key(key)?;
        validate_sysctl_value(value)?;
        let sysctl_dir = self.root.join("etc/sysctl.d");
        fs::create_dir_all(&sysctl_dir)?;
        let safe_key = key.replace(['/', '.'], "-");
        let filename = format!("99-conary-{}.conf", safe_key);
        let config_path = sysctl_dir.join(&filename);
        fs::write(&config_path, format!("{}={}\n", key, value))
            .with_context(|| format!("Failed to write {}", config_path.display()))?;
        debug!(
            "Persisted sysctl desired state '{}' in {}",
            key,
            config_path.display()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_sysctl_keys() {
        assert!(validate_sysctl_key("net.ipv4.ip_forward").is_ok());
        assert!(validate_sysctl_key("kernel/shmmax").is_ok());
        assert!(validate_sysctl_key("vm.swappiness").is_ok());
        assert!(validate_sysctl_key("net.core.rmem-max").is_ok());
    }

    #[test]
    fn test_invalid_sysctl_keys() {
        assert!(validate_sysctl_key("").is_err());
        assert!(validate_sysctl_key("key;rm -rf /").is_err());
        assert!(validate_sysctl_key("key\nvalue").is_err());
        assert!(validate_sysctl_key("key=value").is_err());
        assert!(validate_sysctl_key("key with spaces").is_err());
    }

    #[test]
    fn test_valid_sysctl_values() {
        assert!(validate_sysctl_value("1").is_ok());
        assert!(validate_sysctl_value("65536").is_ok());
        assert!(validate_sysctl_value("some_value").is_ok());
    }

    #[test]
    fn test_invalid_sysctl_values() {
        assert!(validate_sysctl_value("value\nnewline").is_err());
        assert!(validate_sysctl_value("value\rreturn").is_err());
        assert!(validate_sysctl_value("1\n2").is_err());
    }

    #[test]
    fn test_denied_sysctl_keys() {
        assert!(is_denied_sysctl_key("kernel.randomize_va_space"));
        assert!(is_denied_sysctl_key("kernel.kptr_restrict"));
        assert!(is_denied_sysctl_key("kernel.modules_disabled"));
        assert!(!is_denied_sysctl_key("net.ipv4.ip_forward"));
    }
}
