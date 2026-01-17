// src/ccs/hooks/mod.rs

//! CCS declarative hook execution
//!
//! This module handles execution of CCS package hooks (users, groups,
//! systemd units, directories, etc.) using native Rust calls to system
//! utilities instead of shell scripts.
//!
//! ## Target Root Support
//!
//! All hook operations support installing into a target root directory
//! other than `/`. This is critical for:
//! - Bootstrap: Building a new system from scratch
//! - Container image creation: Populating rootfs without affecting host
//! - Offline installations: Installing packages into mounted filesystems
//!
//! When root != `/`:
//! - Users/groups are created in target's /etc/passwd and /etc/group
//! - Systemd units are enabled via symlinks, not `systemctl`
//! - Directories are created under the target root
//! - Host system is never modified

mod alternatives;
mod directory;
mod sysctl;
mod systemd;
mod tmpfiles;
mod user_group;

// Re-export helper functions that may be useful externally
pub use systemd::{compute_relative_unit_path, parse_systemd_install_section};
pub use tmpfiles::hash_string;

use crate::ccs::manifest::Hooks;
use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::warn;

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
