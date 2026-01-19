// src/ccs/hooks/user_group.rs

//! User and group management for CCS hooks
//!
//! Handles creation and deletion of system users and groups,
//! with support for both live root and target root operations.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::Command;
use tracing::{debug, info};

impl HookExecutor {
    /// Check if we're operating on the live root
    pub(super) fn is_live_root(&self) -> bool {
        self.root == std::path::Path::new("/")
    }

    /// Check if a user exists
    ///
    /// When root == "/", uses nix to query the live system.
    /// When root != "/", parses the target's /etc/passwd directly.
    pub(super) fn user_exists(&self, name: &str) -> bool {
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
            if let Some(username) = line.split(':').next()
                && username == name
            {
                return true;
            }
        }
        false
    }

    /// Check if a group exists
    ///
    /// When root == "/", uses nix to query the live system.
    /// When root != "/", parses the target's /etc/group directly.
    pub(super) fn group_exists(&self, name: &str) -> bool {
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
            if let Some(groupname) = line.split(':').next()
                && groupname == name
            {
                return true;
            }
        }
        false
    }

    /// Create a user. Returns true if created, false if already exists.
    ///
    /// When root != "/", uses `useradd --root <target>` to create the user
    /// in the target filesystem's /etc/passwd without affecting the host.
    pub(super) fn create_user(
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
    pub(super) fn delete_user(&self, name: &str) -> Result<()> {
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
    pub(super) fn create_group(&self, name: &str, system: bool) -> Result<bool> {
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
    pub(super) fn delete_group(&self, name: &str) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
    fn test_is_live_root() {
        let executor_live = HookExecutor::new(std::path::Path::new("/"));
        assert!(executor_live.is_live_root());

        let executor_target = HookExecutor::new(std::path::Path::new("/tmp/rootfs"));
        assert!(!executor_target.is_live_root());
    }
}
