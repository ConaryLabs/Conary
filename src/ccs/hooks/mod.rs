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
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::warn;

/// Result of executing a single hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    /// Type of hook (user, group, directory, systemd, etc.)
    pub hook_type: HookType,
    /// Name/identifier of the hook (e.g., user name, unit name)
    pub name: String,
    /// Whether the hook succeeded
    pub success: bool,
    /// Exit code if applicable (None for native operations)
    pub exit_code: Option<i32>,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Types of hooks that can be executed
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HookType {
    /// User creation
    User,
    /// Group creation
    Group,
    /// Directory creation
    Directory,
    /// Systemd unit enable/disable
    Systemd,
    /// Tmpfiles.d entry
    Tmpfiles,
    /// Sysctl setting
    Sysctl,
    /// Update-alternatives
    Alternatives,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Group => write!(f, "group"),
            Self::Directory => write!(f, "directory"),
            Self::Systemd => write!(f, "systemd"),
            Self::Tmpfiles => write!(f, "tmpfiles"),
            Self::Sysctl => write!(f, "sysctl"),
            Self::Alternatives => write!(f, "alternatives"),
        }
    }
}

/// Aggregate results from hook execution phase
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookExecutionResults {
    /// Individual hook results
    pub results: Vec<HookResult>,
    /// Total execution time in milliseconds
    pub total_duration_ms: u64,
    /// Number of successful hooks
    pub succeeded: usize,
    /// Number of failed hooks
    pub failed: usize,
}

impl HookExecutionResults {
    /// Create empty results
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a hook result
    pub fn add(&mut self, result: HookResult) {
        if result.success {
            self.succeeded += 1;
        } else {
            self.failed += 1;
        }
        self.results.push(result);
    }

    /// Check if all hooks succeeded
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0
    }

    /// Get failed hooks only
    pub fn failures(&self) -> impl Iterator<Item = &HookResult> {
        self.results.iter().filter(|r| !r.success)
    }
}

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
        let _ = self.execute_post_hooks_with_results(hooks);
        Ok(())
    }

    /// Execute pre-install hooks with detailed results for journaling
    ///
    /// Same as `execute_pre_hooks` but returns detailed results that can be
    /// written to the transaction journal for crash recovery and auditing.
    pub fn execute_pre_hooks_with_results(&mut self, hooks: &Hooks) -> HookExecutionResults {
        let start = Instant::now();
        let mut results = HookExecutionResults::new();

        // Groups first (users may depend on them)
        for group in &hooks.groups {
            let hook_start = Instant::now();
            let result = self.create_group(&group.name, group.system);
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(created) => {
                    if created {
                        self.applied_hooks.push(AppliedHook::Group(group.name.clone()));
                    }
                    HookResult {
                        hook_type: HookType::Group,
                        name: group.name.clone(),
                        success: true,
                        exit_code: None,
                        error: None,
                        duration_ms: duration.as_millis() as u64,
                    }
                }
                Err(e) => HookResult {
                    hook_type: HookType::Group,
                    name: group.name.clone(),
                    success: false,
                    exit_code: None,
                    error: Some(e.to_string()),
                    duration_ms: duration.as_millis() as u64,
                },
            };
            results.add(hook_result);
        }

        // Then users
        for user in &hooks.users {
            let hook_start = Instant::now();
            let result = self.create_user(
                &user.name,
                user.system,
                user.home.as_deref(),
                user.shell.as_deref(),
                user.group.as_deref(),
            );
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(created) => {
                    if created {
                        self.applied_hooks.push(AppliedHook::User(user.name.clone()));
                    }
                    HookResult {
                        hook_type: HookType::User,
                        name: user.name.clone(),
                        success: true,
                        exit_code: None,
                        error: None,
                        duration_ms: duration.as_millis() as u64,
                    }
                }
                Err(e) => HookResult {
                    hook_type: HookType::User,
                    name: user.name.clone(),
                    success: false,
                    exit_code: None,
                    error: Some(e.to_string()),
                    duration_ms: duration.as_millis() as u64,
                },
            };
            results.add(hook_result);
        }

        // Then directories
        for dir in &hooks.directories {
            let path = self.root.join(dir.path.trim_start_matches('/'));
            let hook_start = Instant::now();
            let result = self.create_directory(&path, &dir.mode, &dir.owner, &dir.group);
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(created) => {
                    self.applied_hooks.push(AppliedHook::Directory(path, created));
                    HookResult {
                        hook_type: HookType::Directory,
                        name: dir.path.clone(),
                        success: true,
                        exit_code: None,
                        error: None,
                        duration_ms: duration.as_millis() as u64,
                    }
                }
                Err(e) => HookResult {
                    hook_type: HookType::Directory,
                    name: dir.path.clone(),
                    success: false,
                    exit_code: None,
                    error: Some(e.to_string()),
                    duration_ms: duration.as_millis() as u64,
                },
            };
            results.add(hook_result);
        }

        results.total_duration_ms = start.elapsed().as_millis() as u64;
        results
    }

    /// Execute post-install hooks with detailed results for journaling
    ///
    /// Same as `execute_post_hooks` but returns detailed results that can be
    /// written to the transaction journal for crash recovery and auditing.
    pub fn execute_post_hooks_with_results(&self, hooks: &Hooks) -> HookExecutionResults {
        let start = Instant::now();
        let mut results = HookExecutionResults::new();
        let mut had_systemd_hooks = false;

        // Systemd units
        for unit in &hooks.systemd {
            had_systemd_hooks = true;
            if unit.enable {
                let hook_start = Instant::now();
                let result = self.systemd_enable(&unit.unit);
                let duration = hook_start.elapsed();

                let hook_result = match result {
                    Ok(()) => HookResult {
                        hook_type: HookType::Systemd,
                        name: unit.unit.clone(),
                        success: true,
                        exit_code: Some(0),
                        error: None,
                        duration_ms: duration.as_millis() as u64,
                    },
                    Err(e) => {
                        warn!("Failed to enable systemd unit '{}': {}", unit.unit, e);
                        HookResult {
                            hook_type: HookType::Systemd,
                            name: unit.unit.clone(),
                            success: false,
                            exit_code: None,
                            error: Some(e.to_string()),
                            duration_ms: duration.as_millis() as u64,
                        }
                    }
                };
                results.add(hook_result);
            }
        }

        // Daemon reload if we touched any units
        if had_systemd_hooks {
            let hook_start = Instant::now();
            if let Err(e) = self.systemd_daemon_reload() {
                warn!("Failed to reload systemd daemon: {}", e);
                results.add(HookResult {
                    hook_type: HookType::Systemd,
                    name: "daemon-reload".to_string(),
                    success: false,
                    exit_code: None,
                    error: Some(e.to_string()),
                    duration_ms: hook_start.elapsed().as_millis() as u64,
                });
            }
        }

        // Tmpfiles
        for tmpfile in &hooks.tmpfiles {
            let hook_start = Instant::now();
            let result = self.apply_tmpfile(tmpfile);
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(()) => HookResult {
                    hook_type: HookType::Tmpfiles,
                    name: tmpfile.path.clone(),
                    success: true,
                    exit_code: Some(0),
                    error: None,
                    duration_ms: duration.as_millis() as u64,
                },
                Err(e) => {
                    warn!("Failed to apply tmpfiles entry '{}': {}", tmpfile.path, e);
                    HookResult {
                        hook_type: HookType::Tmpfiles,
                        name: tmpfile.path.clone(),
                        success: false,
                        exit_code: None,
                        error: Some(e.to_string()),
                        duration_ms: duration.as_millis() as u64,
                    }
                }
            };
            results.add(hook_result);
        }

        // Sysctl
        for sysctl in &hooks.sysctl {
            let hook_start = Instant::now();
            let result = self.apply_sysctl(&sysctl.key, &sysctl.value, sysctl.only_if_lower);
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(()) => HookResult {
                    hook_type: HookType::Sysctl,
                    name: sysctl.key.clone(),
                    success: true,
                    exit_code: None,
                    error: None,
                    duration_ms: duration.as_millis() as u64,
                },
                Err(e) => {
                    warn!("Failed to apply sysctl '{}': {}", sysctl.key, e);
                    HookResult {
                        hook_type: HookType::Sysctl,
                        name: sysctl.key.clone(),
                        success: false,
                        exit_code: None,
                        error: Some(e.to_string()),
                        duration_ms: duration.as_millis() as u64,
                    }
                }
            };
            results.add(hook_result);
        }

        // Alternatives
        for alt in &hooks.alternatives {
            let hook_start = Instant::now();
            let result = self.update_alternatives(&alt.name, &alt.path, alt.priority);
            let duration = hook_start.elapsed();

            let hook_result = match result {
                Ok(()) => HookResult {
                    hook_type: HookType::Alternatives,
                    name: alt.name.clone(),
                    success: true,
                    exit_code: Some(0),
                    error: None,
                    duration_ms: duration.as_millis() as u64,
                },
                Err(e) => {
                    warn!(
                        "Failed to update alternative '{}' -> '{}': {}",
                        alt.name, alt.path, e
                    );
                    HookResult {
                        hook_type: HookType::Alternatives,
                        name: alt.name.clone(),
                        success: false,
                        exit_code: None,
                        error: Some(e.to_string()),
                        duration_ms: duration.as_millis() as u64,
                    }
                }
            };
            results.add(hook_result);
        }

        results.total_duration_ms = start.elapsed().as_millis() as u64;
        results
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

    #[test]
    fn test_hook_result_creation() {
        let result = HookResult {
            hook_type: HookType::User,
            name: "nginx".to_string(),
            success: true,
            exit_code: None,
            error: None,
            duration_ms: 50,
        };

        assert!(result.success);
        assert_eq!(result.hook_type, HookType::User);
        assert_eq!(result.name, "nginx");
    }

    #[test]
    fn test_hook_execution_results_tracking() {
        let mut results = HookExecutionResults::new();

        // Add a successful hook
        results.add(HookResult {
            hook_type: HookType::Group,
            name: "www-data".to_string(),
            success: true,
            exit_code: None,
            error: None,
            duration_ms: 10,
        });

        // Add a failed hook
        results.add(HookResult {
            hook_type: HookType::User,
            name: "nginx".to_string(),
            success: false,
            exit_code: Some(1),
            error: Some("user already exists".to_string()),
            duration_ms: 5,
        });

        assert_eq!(results.succeeded, 1);
        assert_eq!(results.failed, 1);
        assert!(!results.all_succeeded());
        assert_eq!(results.failures().count(), 1);
    }

    #[test]
    fn test_hook_execution_results_all_succeeded() {
        let mut results = HookExecutionResults::new();

        results.add(HookResult {
            hook_type: HookType::Directory,
            name: "/var/lib/nginx".to_string(),
            success: true,
            exit_code: None,
            error: None,
            duration_ms: 2,
        });

        results.add(HookResult {
            hook_type: HookType::Systemd,
            name: "nginx.service".to_string(),
            success: true,
            exit_code: Some(0),
            error: None,
            duration_ms: 100,
        });

        assert!(results.all_succeeded());
        assert_eq!(results.succeeded, 2);
        assert_eq!(results.failed, 0);
    }

    #[test]
    fn test_hook_type_display() {
        assert_eq!(format!("{}", HookType::User), "user");
        assert_eq!(format!("{}", HookType::Group), "group");
        assert_eq!(format!("{}", HookType::Directory), "directory");
        assert_eq!(format!("{}", HookType::Systemd), "systemd");
        assert_eq!(format!("{}", HookType::Tmpfiles), "tmpfiles");
        assert_eq!(format!("{}", HookType::Sysctl), "sysctl");
        assert_eq!(format!("{}", HookType::Alternatives), "alternatives");
    }

    #[test]
    fn test_hook_result_serialization() {
        let result = HookResult {
            hook_type: HookType::Systemd,
            name: "nginx.service".to_string(),
            success: true,
            exit_code: Some(0),
            error: None,
            duration_ms: 150,
        };

        // Ensure it serializes to JSON (for journal records)
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("nginx.service"));
        assert!(json.contains("Systemd"));

        // Ensure it deserializes back
        let parsed: HookResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "nginx.service");
        assert_eq!(parsed.hook_type, HookType::Systemd);
    }

    #[test]
    fn test_hook_execution_results_serialization() {
        let mut results = HookExecutionResults::new();
        results.add(HookResult {
            hook_type: HookType::User,
            name: "testuser".to_string(),
            success: true,
            exit_code: None,
            error: None,
            duration_ms: 25,
        });
        results.total_duration_ms = 30;

        let json = serde_json::to_string(&results).expect("serialize");
        let parsed: HookExecutionResults = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.succeeded, 1);
        assert_eq!(parsed.total_duration_ms, 30);
    }
}
