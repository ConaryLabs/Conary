// src/ccs/hooks/alternatives.rs

//! Alternatives integration for CCS hooks
//!
//! Handles update-alternatives for managing multiple versions
//! of programs that provide the same functionality.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, info};

impl HookExecutor {
    /// Update alternatives
    ///
    /// When root == "/", uses update-alternatives command.
    /// When root != "/", skips (alternatives will need to be set up on first
    /// boot or via a trigger). This is a limitation - alternatives are complex
    /// to set up without the actual command.
    pub(super) fn update_alternatives(&self, name: &str, path: &str, priority: i32) -> Result<()> {
        if !self.is_live_root() {
            // For target root: we can't easily replicate update-alternatives
            // behavior. Log and skip for now.
            debug!(
                "Skipping update-alternatives for '{}' in target root (will be set up on first boot)",
                name
            );
            // TODO: We could create the alternatives directory structure manually
            // but it's complex and distro-specific
            return Ok(());
        }

        // Check if update-alternatives is available
        let has_alternatives = Command::new("update-alternatives")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !has_alternatives {
            debug!("update-alternatives not available, skipping");
            return Ok(());
        }

        let status = Command::new("update-alternatives")
            .args([
                "--install",
                &format!("/usr/bin/{}", name),
                name,
                path,
                &priority.to_string(),
            ])
            .status()
            .context("Failed to run update-alternatives")?;

        if status.success() {
            info!("Updated alternative '{}' -> '{}' (priority {})", name, path, priority);
            Ok(())
        } else {
            Err(anyhow::anyhow!("update-alternatives --install failed"))
        }
    }
}
