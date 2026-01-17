// src/ccs/hooks/directory.rs

//! Directory management for CCS hooks
//!
//! Handles creation and removal of directories with specified
//! permissions and ownership.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

impl HookExecutor {
    /// Create a directory with specified mode and ownership.
    /// Returns true if created, false if already exists.
    pub(super) fn create_directory(
        &self,
        path: &Path,
        mode: &str,
        owner: &str,
        group: &str,
    ) -> Result<bool> {
        let created = if path.exists() {
            debug!("Directory '{}' already exists", path.display());
            false
        } else {
            std::fs::create_dir_all(path)
                .with_context(|| format!("Failed to create directory: {}", path.display()))?;
            info!("Created directory '{}'", path.display());
            true
        };

        // Always apply mode and ownership
        self.apply_directory_permissions(path, mode, owner, group)?;

        Ok(created)
    }

    /// Apply mode and ownership to a directory
    fn apply_directory_permissions(
        &self,
        path: &Path,
        mode: &str,
        owner: &str,
        group: &str,
    ) -> Result<()> {
        use std::os::unix::fs::{chown, PermissionsExt};

        // Parse mode string (e.g., "0755" or "755") as octal
        let mode_val = u32::from_str_radix(mode.trim_start_matches('0'), 8)
            .with_context(|| format!("Invalid mode string: {}", mode))?;

        // Apply mode using std::fs
        let permissions = std::fs::Permissions::from_mode(mode_val);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;

        // Look up uid from owner name
        let uid = nix::unistd::User::from_name(owner)
            .with_context(|| format!("Failed to look up user: {}", owner))?
            .map(|u| u.uid.as_raw());

        // Look up gid from group name
        let gid = nix::unistd::Group::from_name(group)
            .with_context(|| format!("Failed to look up group: {}", group))?
            .map(|g| g.gid.as_raw());

        // Apply ownership using std::os::unix::fs::chown
        chown(path, uid, gid)
            .with_context(|| format!("Failed to set ownership on {}", path.display()))?;

        Ok(())
    }

    /// Remove a directory (only if empty)
    pub(super) fn remove_directory(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        std::fs::remove_dir(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
        info!("Removed directory '{}'", path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_directory_creation_in_target() {
        let temp_dir = TempDir::new().unwrap();
        let executor = HookExecutor::new(temp_dir.path());

        let dir_path = temp_dir.path().join("var/lib/test");

        // Note: ownership change requires root, so we use current user's uid
        let uid = nix::unistd::getuid().as_raw().to_string();
        let gid = nix::unistd::getgid().as_raw().to_string();

        let created = executor
            .create_directory(&dir_path, "0755", &uid, &gid)
            .unwrap();

        assert!(created);
        assert!(dir_path.exists());
    }
}
