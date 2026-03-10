// conary-core/src/ccs/hooks/alternatives.rs

//! Alternatives integration for CCS hooks
//!
//! Handles update-alternatives for managing multiple versions
//! of programs that provide the same functionality.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, info};

/// Validate alternative name - only allow `[a-zA-Z0-9_-]` characters.
fn validate_alternative_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("Alternative name cannot be empty"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow::anyhow!(
            "Alternative name contains invalid characters: {}",
            name
        ));
    }
    Ok(())
}

/// Validate path is absolute and contains no `..` components.
fn validate_alternative_path(path: &str) -> Result<()> {
    if !path.starts_with('/') {
        return Err(anyhow::anyhow!(
            "Alternative path must be absolute: {}",
            path
        ));
    }
    if path.split('/').any(|component| component == "..") {
        return Err(anyhow::anyhow!(
            "Alternative path contains traversal: {}",
            path
        ));
    }
    Ok(())
}

impl HookExecutor {
    /// Update alternatives
    ///
    /// When root == "/", uses update-alternatives command.
    /// When root != "/", skips (alternatives will need to be set up on first
    /// boot or via a trigger). This is a limitation - alternatives are complex
    /// to set up without the actual command.
    pub(super) fn update_alternatives(&self, name: &str, path: &str, priority: i32) -> Result<()> {
        // Validate inputs before any other logic to catch malicious metadata early
        validate_alternative_name(name)?;
        validate_alternative_path(path)?;

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
            info!(
                "Updated alternative '{}' -> '{}' (priority {})",
                name, path, priority
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!("update-alternatives --install failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_alternative_names() {
        assert!(validate_alternative_name("gcc").is_ok());
        assert!(validate_alternative_name("g-plus-plus").is_ok());
        assert!(validate_alternative_name("python3_11").is_ok());
    }

    #[test]
    fn test_invalid_alternative_names() {
        assert!(validate_alternative_name("").is_err());
        assert!(validate_alternative_name("name;rm -rf /").is_err());
        assert!(validate_alternative_name("../etc/passwd").is_err());
        assert!(validate_alternative_name("name with spaces").is_err());
        assert!(validate_alternative_name("name.with.dots").is_err());
    }

    #[test]
    fn test_valid_alternative_paths() {
        assert!(validate_alternative_path("/usr/bin/gcc-12").is_ok());
        assert!(validate_alternative_path("/usr/local/bin/python3").is_ok());
    }

    #[test]
    fn test_invalid_alternative_paths() {
        assert!(validate_alternative_path("relative/path").is_err());
        assert!(validate_alternative_path("/usr/../etc/passwd").is_err());
        assert!(validate_alternative_path("").is_err());
    }
}
