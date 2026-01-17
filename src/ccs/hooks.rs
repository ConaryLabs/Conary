// src/ccs/hooks.rs

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

use crate::ccs::manifest::Hooks;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs as unix_fs;
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

    /// Check if we're operating on the live root
    fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    /// Check if a user exists
    ///
    /// When root == "/", uses nix to query the live system.
    /// When root != "/", parses the target's /etc/passwd directly.
    fn user_exists(&self, name: &str) -> bool {
        if self.is_live_root() {
            nix::unistd::User::from_name(name)
                .ok()
                .flatten()
                .is_some()
        } else {
            self.user_exists_in_target(name)
        }
    }

    /// Check if a user exists in the target root's /etc/passwd
    fn user_exists_in_target(&self, name: &str) -> bool {
        let passwd_path = self.root.join("etc/passwd");
        if !passwd_path.exists() {
            return false;
        }

        let file = match fs::File::open(&passwd_path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(username) = line.split(':').next() {
                if username == name {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a group exists
    ///
    /// When root == "/", uses nix to query the live system.
    /// When root != "/", parses the target's /etc/group directly.
    fn group_exists(&self, name: &str) -> bool {
        if self.is_live_root() {
            nix::unistd::Group::from_name(name)
                .ok()
                .flatten()
                .is_some()
        } else {
            self.group_exists_in_target(name)
        }
    }

    /// Check if a group exists in the target root's /etc/group
    fn group_exists_in_target(&self, name: &str) -> bool {
        let group_path = self.root.join("etc/group");
        if !group_path.exists() {
            return false;
        }

        let file = match fs::File::open(&group_path) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(groupname) = line.split(':').next() {
                if groupname == name {
                    return true;
                }
            }
        }
        false
    }

    /// Create a user. Returns true if created, false if already exists.
    ///
    /// When root != "/", uses `useradd --root <target>` to create the user
    /// in the target filesystem's /etc/passwd without affecting the host.
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

        // Ensure target /etc directory exists when not operating on live root
        if !self.is_live_root() {
            let etc_path = self.root.join("etc");
            fs::create_dir_all(&etc_path)
                .with_context(|| format!("Failed to create {}", etc_path.display()))?;
        }

        let mut cmd = Command::new("useradd");

        // Critical: use --root for target installations
        if !self.is_live_root() {
            cmd.args(["--root", self.root.to_str().unwrap_or("/")]);
        }

        if system {
            cmd.arg("--system");
        }

        if let Some(h) = home {
            cmd.args(["--home-dir", h]);
            // For target root, don't try to create home dir (it would fail or
            // create in wrong place). The install phase will handle directories.
            if self.is_live_root() {
                cmd.arg("--create-home");
            } else {
                cmd.arg("--no-create-home");
            }
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
            info!(
                "Created user '{}' (root: {})",
                name,
                self.root.display()
            );
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                "useradd failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Delete a user
    ///
    /// When root != "/", uses `userdel --root <target>`.
    fn delete_user(&self, name: &str) -> Result<()> {
        if !self.user_exists(name) {
            return Ok(());
        }

        let mut cmd = Command::new("userdel");

        if !self.is_live_root() {
            cmd.args(["--root", self.root.to_str().unwrap_or("/")]);
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run userdel")?;

        if status.success() {
            info!("Deleted user '{}' (root: {})", name, self.root.display());
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "userdel failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Create a group. Returns true if created, false if already exists.
    ///
    /// When root != "/", uses `groupadd --root <target>` to create the group
    /// in the target filesystem's /etc/group without affecting the host.
    fn create_group(&self, name: &str, system: bool) -> Result<bool> {
        if self.group_exists(name) {
            debug!("Group '{}' already exists, skipping", name);
            return Ok(false);
        }

        // Ensure target /etc directory exists when not operating on live root
        if !self.is_live_root() {
            let etc_path = self.root.join("etc");
            fs::create_dir_all(&etc_path)
                .with_context(|| format!("Failed to create {}", etc_path.display()))?;
        }

        let mut cmd = Command::new("groupadd");

        // Critical: use --root for target installations
        if !self.is_live_root() {
            cmd.args(["--root", self.root.to_str().unwrap_or("/")]);
        }

        if system {
            cmd.arg("--system");
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run groupadd")?;

        if status.success() {
            info!(
                "Created group '{}' (root: {})",
                name,
                self.root.display()
            );
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                "groupadd failed with exit code: {:?}",
                status.code()
            ))
        }
    }

    /// Delete a group
    ///
    /// When root != "/", uses `groupdel --root <target>`.
    fn delete_group(&self, name: &str) -> Result<()> {
        if !self.group_exists(name) {
            return Ok(());
        }

        let mut cmd = Command::new("groupdel");

        if !self.is_live_root() {
            cmd.args(["--root", self.root.to_str().unwrap_or("/")]);
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run groupdel")?;

        if status.success() {
            info!("Deleted group '{}' (root: {})", name, self.root.display());
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
    ///
    /// When root != "/", this is a no-op since we can't reload the daemon
    /// for a target root. The daemon will reload naturally on first boot.
    fn systemd_daemon_reload(&self) -> Result<()> {
        if !self.is_live_root() {
            debug!("Skipping daemon-reload for target root installation");
            return Ok(());
        }

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
    ///
    /// When root == "/", uses `systemctl enable`.
    /// When root != "/", creates the symlink directly in the target's
    /// /etc/systemd/system directory. This avoids calling systemctl which
    /// would affect the host system.
    fn systemd_enable(&self, unit: &str) -> Result<()> {
        if self.is_live_root() {
            self.systemd_enable_live(unit)
        } else {
            self.systemd_enable_target(unit)
        }
    }

    /// Enable a systemd unit on the live system
    fn systemd_enable_live(&self, unit: &str) -> Result<()> {
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

    /// Enable a systemd unit in a target root by creating symlinks
    ///
    /// This mimics what `systemctl enable` does, but operates entirely within
    /// the target filesystem. It:
    /// 1. Finds the unit file in the target's systemd directories
    /// 2. Reads the [Install] section to determine where to symlink
    /// 3. Creates the appropriate symlinks in /etc/systemd/system/
    fn systemd_enable_target(&self, unit: &str) -> Result<()> {
        // Systemd unit search paths in priority order
        let search_paths = [
            "etc/systemd/system",
            "usr/lib/systemd/system",
            "lib/systemd/system",
        ];

        // Find the unit file
        let unit_path = search_paths
            .iter()
            .map(|p| self.root.join(p).join(unit))
            .find(|p| p.exists());

        let unit_path = match unit_path {
            Some(p) => p,
            None => {
                debug!(
                    "Unit file '{}' not found in target, skipping enable",
                    unit
                );
                return Ok(());
            }
        };

        // Parse the unit file to find WantedBy/RequiredBy
        let content = fs::read_to_string(&unit_path)
            .with_context(|| format!("Failed to read unit file: {}", unit_path.display()))?;

        let wants = parse_systemd_install_section(&content, "WantedBy");
        let requires = parse_systemd_install_section(&content, "RequiredBy");

        if wants.is_empty() && requires.is_empty() {
            debug!(
                "Unit '{}' has no WantedBy/RequiredBy, nothing to enable",
                unit
            );
            return Ok(());
        }

        // Compute the relative path to the unit file from /etc/systemd/system
        let unit_rel_path = compute_relative_unit_path(&unit_path, &self.root);

        // Create symlinks for WantedBy
        for target in &wants {
            let wants_dir = self
                .root
                .join("etc/systemd/system")
                .join(format!("{}.wants", target));
            fs::create_dir_all(&wants_dir)?;

            let symlink_path = wants_dir.join(unit);
            if !symlink_path.exists() {
                unix_fs::symlink(&unit_rel_path, &symlink_path).with_context(|| {
                    format!(
                        "Failed to create symlink {} -> {}",
                        symlink_path.display(),
                        unit_rel_path
                    )
                })?;
                debug!(
                    "Created symlink: {} -> {}",
                    symlink_path.display(),
                    unit_rel_path
                );
            }
        }

        // Create symlinks for RequiredBy
        for target in &requires {
            let requires_dir = self
                .root
                .join("etc/systemd/system")
                .join(format!("{}.requires", target));
            fs::create_dir_all(&requires_dir)?;

            let symlink_path = requires_dir.join(unit);
            if !symlink_path.exists() {
                unix_fs::symlink(&unit_rel_path, &symlink_path).with_context(|| {
                    format!(
                        "Failed to create symlink {} -> {}",
                        symlink_path.display(),
                        unit_rel_path
                    )
                })?;
                debug!(
                    "Created symlink: {} -> {}",
                    symlink_path.display(),
                    unit_rel_path
                );
            }
        }

        info!(
            "Enabled systemd unit '{}' in target root (symlinks created)",
            unit
        );
        Ok(())
    }

    // =========================================================================
    // Tmpfiles Integration
    // =========================================================================

    /// Apply a tmpfiles entry
    ///
    /// When root == "/", uses systemd-tmpfiles to apply immediately.
    /// When root != "/", writes the config to /etc/tmpfiles.d/ in the target
    /// so it will be applied on first boot.
    fn apply_tmpfile(&self, entry: &crate::ccs::manifest::TmpfilesHook) -> Result<()> {
        // Create the config line
        let config = format!(
            "{} {} {} {} {}\n",
            entry.entry_type, entry.path, entry.mode, entry.owner, entry.group
        );

        if !self.is_live_root() {
            // For target root: write config file that will be applied on boot
            let tmpfiles_dir = self.root.join("etc/tmpfiles.d");
            fs::create_dir_all(&tmpfiles_dir)?;

            // Use a hash of the path as filename to avoid collisions
            let filename = format!("conary-{:x}.conf", hash_string(&entry.path));
            let config_path = tmpfiles_dir.join(&filename);

            // Append to existing or create new
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config_path)
                .with_context(|| format!("Failed to create {}", config_path.display()))?;
            file.write_all(config.as_bytes())?;

            debug!(
                "Wrote tmpfiles config for '{}' to {}",
                entry.path,
                config_path.display()
            );
            return Ok(());
        }

        // Live root: apply immediately with systemd-tmpfiles
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

        let tmpfile = tempfile::NamedTempFile::new()?;
        fs::write(tmpfile.path(), &config)?;

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
    ///
    /// When root == "/", applies the sysctl setting immediately.
    /// When root != "/", writes a config file to /etc/sysctl.d/ in the target
    /// so it will be applied on boot.
    fn apply_sysctl(&self, key: &str, value: &str, only_if_lower: bool) -> Result<()> {
        if !self.is_live_root() {
            // For target root: write config file that will be applied on boot
            let sysctl_dir = self.root.join("etc/sysctl.d");
            fs::create_dir_all(&sysctl_dir)?;

            // Use key name as filename
            let safe_key = key.replace('/', "-").replace('.', "-");
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

    // =========================================================================
    // Alternatives Integration
    // =========================================================================

    /// Update alternatives
    ///
    /// When root == "/", uses update-alternatives command.
    /// When root != "/", skips (alternatives will need to be set up on first
    /// boot or via a trigger). This is a limitation - alternatives are complex
    /// to set up without the actual command.
    fn update_alternatives(&self, name: &str, path: &str, priority: i32) -> Result<()> {
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

// =============================================================================
// Helper Functions
// =============================================================================

/// Simple string hash for generating unique filenames
fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Parse systemd unit file [Install] section for WantedBy/RequiredBy
///
/// Returns a list of target units that this unit should be linked to.
fn parse_systemd_install_section(content: &str, key: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut in_install = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Track when we enter [Install] section
        if trimmed.starts_with('[') {
            in_install = trimmed == "[Install]";
            continue;
        }

        // Skip if not in [Install] section
        if !in_install {
            continue;
        }

        // Look for key=value
        if let Some(value) = trimmed.strip_prefix(key) {
            if let Some(value) = value.strip_prefix('=') {
                // Value can be space-separated list
                for target in value.split_whitespace() {
                    if !target.is_empty() {
                        results.push(target.to_string());
                    }
                }
            }
        }
    }

    results
}

/// Compute relative path from /etc/systemd/system to the actual unit file
///
/// When the unit is in /usr/lib/systemd/system, we need a relative path like:
/// ../../../usr/lib/systemd/system/foo.service
fn compute_relative_unit_path(unit_path: &Path, root: &Path) -> String {
    // Strip the root prefix to get the absolute path within the filesystem
    let abs_path = unit_path
        .strip_prefix(root)
        .unwrap_or(unit_path)
        .to_string_lossy();

    // The symlink will be in /etc/systemd/system/<target>.wants/
    // We need to go up 4 levels: .. (wants dir) / .. (system) / .. (systemd) / .. (etc)
    // Then down to the unit file location
    format!("../../../../{}", abs_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_hook_executor_new() {
        let executor = HookExecutor::new(Path::new("/"));
        assert_eq!(executor.root, PathBuf::from("/"));
        assert!(executor.applied_hooks.is_empty());
    }

    #[test]
    fn test_is_live_root() {
        let executor_live = HookExecutor::new(Path::new("/"));
        assert!(executor_live.is_live_root());

        let executor_target = HookExecutor::new(Path::new("/tmp/rootfs"));
        assert!(!executor_target.is_live_root());
    }

    #[test]
    fn test_user_exists_in_target() {
        let temp_dir = TempDir::new().unwrap();
        let etc_dir = temp_dir.path().join("etc");
        fs::create_dir_all(&etc_dir).unwrap();

        // Create a passwd file
        let passwd_path = etc_dir.join("passwd");
        fs::write(
            &passwd_path,
            "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:Nobody:/:/usr/sbin/nologin\n",
        )
        .unwrap();

        let executor = HookExecutor::new(temp_dir.path());
        assert!(executor.user_exists_in_target("root"));
        assert!(executor.user_exists_in_target("nobody"));
        assert!(!executor.user_exists_in_target("nonexistent"));
    }

    #[test]
    fn test_group_exists_in_target() {
        let temp_dir = TempDir::new().unwrap();
        let etc_dir = temp_dir.path().join("etc");
        fs::create_dir_all(&etc_dir).unwrap();

        // Create a group file
        let group_path = etc_dir.join("group");
        fs::write(&group_path, "root:x:0:\nwheel:x:10:user1,user2\n").unwrap();

        let executor = HookExecutor::new(temp_dir.path());
        assert!(executor.group_exists_in_target("root"));
        assert!(executor.group_exists_in_target("wheel"));
        assert!(!executor.group_exists_in_target("nonexistent"));
    }

    #[test]
    fn test_parse_systemd_install_section() {
        let content = r#"[Unit]
Description=Test Service
After=network.target

[Service]
ExecStart=/usr/bin/test

[Install]
WantedBy=multi-user.target graphical.target
RequiredBy=critical.target
"#;

        let wants = parse_systemd_install_section(content, "WantedBy");
        assert_eq!(wants, vec!["multi-user.target", "graphical.target"]);

        let requires = parse_systemd_install_section(content, "RequiredBy");
        assert_eq!(requires, vec!["critical.target"]);
    }

    #[test]
    fn test_parse_systemd_install_section_empty() {
        let content = r#"[Unit]
Description=Test Service

[Service]
ExecStart=/usr/bin/test
"#;

        let wants = parse_systemd_install_section(content, "WantedBy");
        assert!(wants.is_empty());
    }

    #[test]
    fn test_compute_relative_unit_path() {
        let root = Path::new("/tmp/rootfs");
        let unit_path = root.join("usr/lib/systemd/system/sshd.service");

        let rel_path = compute_relative_unit_path(&unit_path, root);
        assert_eq!(rel_path, "../../../../usr/lib/systemd/system/sshd.service");
    }

    #[test]
    fn test_hash_string() {
        let hash1 = hash_string("/var/lib/test");
        let hash2 = hash_string("/var/lib/test");
        let hash3 = hash_string("/var/lib/other");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_systemd_enable_target() {
        let temp_dir = TempDir::new().unwrap();

        // Create systemd unit directories
        let usr_lib_systemd = temp_dir.path().join("usr/lib/systemd/system");
        fs::create_dir_all(&usr_lib_systemd).unwrap();

        // Create a test unit file
        let unit_content = r#"[Unit]
Description=Test Service

[Service]
ExecStart=/usr/bin/test

[Install]
WantedBy=multi-user.target
"#;
        fs::write(usr_lib_systemd.join("test.service"), unit_content).unwrap();

        // Enable the unit
        let executor = HookExecutor::new(temp_dir.path());
        executor.systemd_enable_target("test.service").unwrap();

        // Check that symlink was created
        let symlink_path = temp_dir
            .path()
            .join("etc/systemd/system/multi-user.target.wants/test.service");
        assert!(symlink_path.exists(), "Symlink should exist");

        // Verify symlink target
        let link_target = fs::read_link(&symlink_path).unwrap();
        assert!(
            link_target
                .to_string_lossy()
                .contains("usr/lib/systemd/system/test.service"),
            "Symlink should point to unit file"
        );
    }

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
