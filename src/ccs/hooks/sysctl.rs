// src/ccs/hooks/sysctl.rs

//! Sysctl integration for CCS hooks
//!
//! Handles applying sysctl settings with support for both
//! live root (using sysctl command) and target root (writing config files).

use super::HookExecutor;
use anyhow::{Context, Result};
use std::fs;
use std::process::Command;
use tracing::{debug, info};

impl HookExecutor {
    /// Apply a sysctl setting
    ///
    /// When root == "/", applies the sysctl setting immediately.
    /// When root != "/", writes a config file to /etc/sysctl.d/ in the target
    /// so it will be applied on boot.
    pub(super) fn apply_sysctl(&self, key: &str, value: &str, only_if_lower: bool) -> Result<()> {
        if !self.is_live_root() {
            // For target root: write config file that will be applied on boot
            let sysctl_dir = self.root.join("etc/sysctl.d");
            fs::create_dir_all(&sysctl_dir)?;

            // Use key name as filename
            let safe_key = key.replace(['/', '.'], "-");
            let filename = format!("99-conary-{}.conf", safe_key);
            let config_path = sysctl_dir.join(&filename);

            let config = if only_if_lower {
                format!("# Only apply if current value is lower\n{}={}\n", key, value)
            } else {
                format!("{}={}\n", key, value)
            };

            fs::write(&config_path, &config)
                .with_context(|| format!("Failed to write {}", config_path.display()))?;

            debug!(
                "Wrote sysctl config for '{}' to {}",
                key,
                config_path.display()
            );
            return Ok(());
        }

        // Live root: apply immediately
        if only_if_lower {
            let output = Command::new("sysctl")
                .args(["-n", key])
                .output()
                .context("Failed to read sysctl value")?;

            if output.status.success() {
                let current = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let (Ok(current_val), Ok(new_val)) =
                    (current.parse::<i64>(), value.parse::<i64>())
                    && current_val >= new_val
                {
                    debug!(
                        "sysctl '{}' already {} (>= {}), skipping",
                        key, current_val, new_val
                    );
                    return Ok(());
                }
            }
        }

        let setting = format!("{}={}", key, value);
        let status = Command::new("sysctl")
            .args(["-w", &setting])
            .status()
            .context("Failed to run sysctl")?;

        if status.success() {
            info!("Applied sysctl '{}={}'", key, value);
            Ok(())
        } else {
            Err(anyhow::anyhow!("sysctl -w failed for {}", key))
        }
    }
}
