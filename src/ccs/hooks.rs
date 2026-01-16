// src/ccs/hooks.rs

//! CCS declarative hook execution
//!
//! This module handles execution of CCS package hooks (users, groups,
//! systemd units, directories, etc.) using native Rust calls to system
//! utilities instead of shell scripts.

use crate::ccs::manifest::Hooks;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Tracks a hook that was successfully applied (for rollback)
#[derive(Debug, Clone)]
pub enum AppliedHook {
    /// User created with userdel
    User(String),
    /// Group created with groupdel
    Group(String),
    /// Directory created (path, was_created)
    Directory(PathBuf, bool),
}

/// Executor for CCS declarative hooks
///
/// Handles pre-install hooks (users, groups, directories) and post-install
/// hooks (systemd, tmpfiles, sysctl, alternatives). Tracks applied hooks
/// for potential rollback on transaction failure.
#[derive(Debug)]
pub struct HookExecutor {
    /// Root filesystem path (usually "/")
    root: PathBuf,
    /// Hooks that were successfully applied (for rollback)
    applied_hooks: Vec<AppliedHook>,
}

impl HookExecutor {
    /// Create a new hook executor
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            applied_hooks: Vec::new(),
        }
    }

    /// Execute pre-install hooks (before transaction)
    ///
    /// Creates groups, users, and directories as specified in the manifest.
    /// These are idempotent operations - if the resource already exists,
    /// it's left unchanged.
    ///
    /// Tracks applied hooks for potential rollback via `revert_pre_hooks()`.
    pub fn execute_pre_hooks(&mut self, hooks: &Hooks) -> Result<()> {
        // Groups first (users may depend on them)
        for group in &hooks.groups {
            if self.create_group(&group.name, group.system)? {
                self.applied_hooks.push(AppliedHook::Group(group.name.clone()));
            }
        }

        // Then users
        for user in &hooks.users {
            if self.create_user(
                &user.name,
                user.system,
                user.home.as_deref(),
                user.shell.as_deref(),
                user.group.as_deref(),
            )? {
                self.applied_hooks.push(AppliedHook::User(user.name.clone()));
            }
        }

        // Then directories
        for dir in &hooks.directories {
            let path = self.root.join(dir.path.trim_start_matches('/'));
            let created = self.create_directory(&path, &dir.mode, &dir.owner, &dir.group)?;
            self.applied_hooks.push(AppliedHook::Directory(path, created));
        }

        Ok(())
    }

    /// Rollback pre-hooks on transaction failure
    ///
    /// Attempts to undo any pre-hooks that were applied:
    /// - Delete created users (userdel)
    /// - Delete created groups (groupdel)
    /// - Remove created directories (if empty)
    ///
    /// Errors are logged but don't cause the rollback to fail.
    pub fn revert_pre_hooks(&mut self) -> Result<()> {
        // Revert in reverse order
        while let Some(hook) = self.applied_hooks.pop() {
            match hook {
                AppliedHook::User(name) => {
                    if let Err(e) = self.delete_user(&name) {
                        warn!("Failed to revert user '{}': {}", name, e);
                    }
                }
                AppliedHook::Group(name) => {
                    if let Err(e) = self.delete_group(&name) {
                        warn!("Failed to revert group '{}': {}", name, e);
                    }
                }
                AppliedHook::Directory(path, was_created) => {
                    if was_created
                        && let Err(e) = self.remove_directory(&path)
                    {
                        warn!("Failed to revert directory '{}': {}", path.display(), e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Execute post-install hooks (after transaction, warn on failure)
    ///
    /// Handles:
    /// - systemd: daemon-reload + enable units
    /// - tmpfiles: systemd-tmpfiles --create
    /// - sysctl: apply settings
    /// - alternatives: update-alternatives
    ///
    /// Failures are logged as warnings but don't fail installation.
    pub fn execute_post_hooks(&self, hooks: &Hooks) -> Result<()> {
        let mut had_systemd_hooks = false;

        // Systemd units
        for unit in &hooks.systemd {
            had_systemd_hooks = true;
            if unit.enable
                && let Err(e) = self.systemd_enable(&unit.unit)
            {
                warn!("Failed to enable systemd unit '{}': {}", unit.unit, e);
            }
        }

        // Daemon reload if we touched any units
        if had_systemd_hooks
            && let Err(e) = self.systemd_daemon_reload()
        {
            warn!("Failed to reload systemd daemon: {}", e);
        }

        // Tmpfiles
        for tmpfile in &hooks.tmpfiles {
            if let Err(e) = self.apply_tmpfile(tmpfile) {
                warn!("Failed to apply tmpfiles entry '{}': {}", tmpfile.path, e);
            }
        }

        // Sysctl
        for sysctl in &hooks.sysctl {
            if let Err(e) = self.apply_sysctl(&sysctl.key, &sysctl.value, sysctl.only_if_lower) {
                warn!("Failed to apply sysctl '{}': {}", sysctl.key, e);
            }
        }

        // Alternatives
        for alt in &hooks.alternatives {
            if let Err(e) = self.update_alternatives(&alt.name, &alt.path, alt.priority) {
                warn!(
                    "Failed to update alternative '{}' -> '{}': {}",
                    alt.name, alt.path, e
                );
            }
        }

        Ok(())
    }

    // =========================================================================
    // User/Group Management
    // =========================================================================

    /// Check if a user exists
    fn user_exists(&self, name: &str) -> bool {
        Command::new("id")
            .arg(name)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Check if a group exists
    fn group_exists(&self, name: &str) -> bool {
        Command::new("getent")
            .args(["group", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Create a user. Returns true if created, false if already exists.
    fn create_user(
        &self,
        name: &str,
        system: bool,
        home: Option<&str>,
        shell: Option<&str>,
        group: Option<&str>,
    ) -> Result<bool> {
        if self.user_exists(name) {
            debug!("User '{}' already exists, skipping", name);
            return Ok(false);
        }

        let mut cmd = Command::new("useradd");

        if system {
            cmd.arg("--system");
        }

        if let Some(h) = home {
            cmd.args(["--home-dir", h]);
            cmd.arg("--create-home");
        } else if system {
            cmd.args(["--home-dir", "/nonexistent"]);
            cmd.arg("--no-create-home");
        }

        if let Some(s) = shell {
            cmd.args(["--shell", s]);
        } else if system {
            cmd.args(["--shell", "/usr/sbin/nologin"]);
        }

        if let Some(g) = group {
            cmd.args(["--gid", g]);
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run useradd")?;

        if status.success() {
            info!("Created user '{}'", name);
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                "useradd failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Delete a user
    fn delete_user(&self, name: &str) -> Result<()> {
        if !self.user_exists(name) {
            return Ok(());
        }

        let status = Command::new("userdel")
            .arg(name)
            .status()
            .context("Failed to run userdel")?;

        if status.success() {
            info!("Deleted user '{}'", name);
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "userdel failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Create a group. Returns true if created, false if already exists.
    fn create_group(&self, name: &str, system: bool) -> Result<bool> {
        if self.group_exists(name) {
            debug!("Group '{}' already exists, skipping", name);
            return Ok(false);
        }

        let mut cmd = Command::new("groupadd");

        if system {
            cmd.arg("--system");
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run groupadd")?;

        if status.success() {
            info!("Created group '{}'", name);
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                "groupadd failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Delete a group
    fn delete_group(&self, name: &str) -> Result<()> {
        if !self.group_exists(name) {
            return Ok(());
        }

        let status = Command::new("groupdel")
            .arg(name)
            .status()
            .context("Failed to run groupdel")?;

        if status.success() {
            info!("Deleted group '{}'", name);
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "groupdel failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    // =========================================================================
    // Directory Management
    // =========================================================================

    /// Create a directory with specified mode and ownership.
    /// Returns true if created, false if already exists.
    fn create_directory(
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
        // Apply mode using chmod
        let status = Command::new("chmod")
            .args([mode, &path.to_string_lossy()])
            .status()
            .context("Failed to run chmod")?;

        if !status.success() {
            warn!("chmod failed for {}", path.display());
        }

        // Apply ownership using chown
        let ownership = format!("{}:{}", owner, group);
        let status = Command::new("chown")
            .args([&ownership, &path.to_string_lossy().to_string()])
            .status()
            .context("Failed to run chown")?;

        if !status.success() {
            warn!("chown failed for {}", path.display());
        }

        Ok(())
    }

    /// Remove a directory (only if empty)
    fn remove_directory(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        std::fs::remove_dir(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
        info!("Removed directory '{}'", path.display());
        Ok(())
    }

    // =========================================================================
    // Systemd Integration
    // =========================================================================

    /// Check if systemctl is available
    fn has_systemctl(&self) -> bool {
        Command::new("systemctl")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Reload systemd daemon
    fn systemd_daemon_reload(&self) -> Result<()> {
        if !self.has_systemctl() {
            debug!("systemctl not available, skipping daemon-reload");
            return Ok(());
        }

        let status = Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .context("Failed to run systemctl daemon-reload")?;

        if status.success() {
            debug!("Reloaded systemd daemon");
            Ok(())
        } else {
            Err(anyhow::anyhow!("systemctl daemon-reload failed"))
        }
    }

    /// Enable a systemd unit
    fn systemd_enable(&self, unit: &str) -> Result<()> {
        if !self.has_systemctl() {
            debug!("systemctl not available, skipping enable for {}", unit);
            return Ok(());
        }

        let status = Command::new("systemctl")
            .args(["enable", unit])
            .status()
            .context("Failed to run systemctl enable")?;

        if status.success() {
            info!("Enabled systemd unit '{}'", unit);
            Ok(())
        } else {
            Err(anyhow::anyhow!("systemctl enable failed for {}", unit))
        }
    }

    // =========================================================================
    // Tmpfiles Integration
    // =========================================================================

    /// Apply a tmpfiles entry
    fn apply_tmpfile(&self, entry: &crate::ccs::manifest::TmpfilesHook) -> Result<()> {
        // Check if systemd-tmpfiles is available
        let has_tmpfiles = Command::new("systemd-tmpfiles")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !has_tmpfiles {
            debug!("systemd-tmpfiles not available, skipping");
            return Ok(());
        }

        // Create a temporary config file
        let config = format!(
            "{} {} {} {} {}",
            entry.entry_type, entry.path, entry.mode, entry.owner, entry.group
        );

        let tmpfile = tempfile::NamedTempFile::new()?;
        std::fs::write(tmpfile.path(), &config)?;

        let status = Command::new("systemd-tmpfiles")
            .args(["--create", &tmpfile.path().to_string_lossy()])
            .status()
            .context("Failed to run systemd-tmpfiles")?;

        if status.success() {
            debug!("Applied tmpfiles entry for '{}'", entry.path);
            Ok(())
        } else {
            Err(anyhow::anyhow!("systemd-tmpfiles --create failed"))
        }
    }

    // =========================================================================
    // Sysctl Integration
    // =========================================================================

    /// Apply a sysctl setting
    fn apply_sysctl(&self, key: &str, value: &str, only_if_lower: bool) -> Result<()> {
        // Read current value if only_if_lower is set
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

    // =========================================================================
    // Alternatives Integration
    // =========================================================================

    /// Update alternatives
    fn update_alternatives(&self, name: &str, path: &str, priority: i32) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_executor_new() {
        let executor = HookExecutor::new(Path::new("/"));
        assert_eq!(executor.root, PathBuf::from("/"));
        assert!(executor.applied_hooks.is_empty());
    }

}
