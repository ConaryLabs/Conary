// conary-core/src/ccs/hooks/user_group.rs

//! User and group management for CCS hooks
//!
//! Handles creation and deletion of system users and groups in a materialized
//! selected root.

use super::HookExecutor;
use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path};
use std::process::Command;
use std::sync::OnceLock;
use tracing::{debug, info};

pub(crate) fn validate_username(name: &str) -> Result<()> {
    static USERNAME_RE: OnceLock<Regex> = OnceLock::new();

    if name.is_empty() {
        return Err(anyhow::anyhow!("username cannot be empty"));
    }
    if name.len() > 32 {
        return Err(anyhow::anyhow!("username exceeds 32 characters: {}", name));
    }

    let username_re =
        USERNAME_RE.get_or_init(|| Regex::new(r"^[a-z_][a-z0-9_-]*$").expect("valid regex"));
    if !username_re.is_match(name) {
        return Err(anyhow::anyhow!("invalid system username: {}", name));
    }

    Ok(())
}

pub(crate) fn validate_shell(shell: &str) -> Result<()> {
    if shell.is_empty() {
        return Err(anyhow::anyhow!("login shell path cannot be empty"));
    }
    let path = Path::new(shell);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(anyhow::anyhow!(
            "login shell must be an absolute traversal-free path: {shell}"
        ));
    }
    Ok(())
}

impl HookExecutor {
    /// Check if a user exists in the selected root's `/etc/passwd`.
    pub(super) fn user_exists(&self, name: &str) -> bool {
        self.user_exists_in_target(name)
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

    /// Check if a group exists in the selected root's `/etc/group`.
    pub(super) fn group_exists(&self, name: &str) -> bool {
        self.group_exists_in_target(name)
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
    /// Uses `useradd --root <selected>` so the active host account database is
    /// never consulted or mutated.
    pub(super) fn create_user(
        &self,
        name: &str,
        system: bool,
        home: Option<&str>,
        shell: Option<&str>,
        group: Option<&str>,
    ) -> Result<bool> {
        validate_username(name)?;
        if let Some(shell) = shell {
            validate_shell(shell)?;
        }
        if let Some(group) = group {
            validate_username(group)?;
        }

        if self.user_exists(name) {
            debug!("User '{}' already exists, skipping", name);
            return Ok(false);
        }

        let etc_path = self.root.join("etc");
        fs::create_dir_all(&etc_path)
            .with_context(|| format!("Failed to create {}", etc_path.display()))?;

        let mut cmd = Command::new("useradd");
        let root_str = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "install root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        cmd.args(["--root", root_str]);

        if system {
            cmd.arg("--system");
        }

        if let Some(h) = home {
            cmd.args(["--home-dir", h]);
            // Payload/directory hooks own home materialization.
            cmd.arg("--no-create-home");
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
            info!("Created user '{}' (root: {})", name, self.root.display());
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
    /// Uses `userdel --root <selected>`.
    pub(super) fn delete_user(&self, name: &str) -> Result<()> {
        if !self.user_exists(name) {
            return Ok(());
        }

        let root = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "install root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        let mut cmd = Command::new("userdel");
        cmd.args(["--root", root]);

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
    /// Uses `groupadd --root <selected>` so the active host account database is
    /// never consulted or mutated.
    pub(super) fn create_group(&self, name: &str, system: bool) -> Result<bool> {
        validate_username(name)?;

        if self.group_exists(name) {
            debug!("Group '{}' already exists, skipping", name);
            return Ok(false);
        }

        let etc_path = self.root.join("etc");
        fs::create_dir_all(&etc_path)
            .with_context(|| format!("Failed to create {}", etc_path.display()))?;

        let mut cmd = Command::new("groupadd");
        let root = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "install root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        cmd.args(["--root", root]);

        if system {
            cmd.arg("--system");
        }

        cmd.arg(name);

        let status = cmd.status().context("Failed to run groupadd")?;

        if status.success() {
            info!("Created group '{}' (root: {})", name, self.root.display());
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
    /// Uses `groupdel --root <selected>`.
    pub(super) fn delete_group(&self, name: &str) -> Result<()> {
        if !self.group_exists(name) {
            return Ok(());
        }

        let root = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "install root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        let mut cmd = Command::new("groupdel");
        cmd.args(["--root", root]);

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
    fn test_validate_username_accepts_system_names() {
        assert!(validate_username("root").is_ok());
        assert!(validate_username("_daemon").is_ok());
        assert!(validate_username("system-user").is_ok());
        assert!(validate_username("service_01").is_ok());
    }

    #[test]
    fn test_validate_username_rejects_invalid_names() {
        assert!(validate_username("").is_err());
        assert!(validate_username("Root").is_err());
        assert!(validate_username("9daemon").is_err());
        assert!(validate_username("daemon!").is_err());
        assert!(validate_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn test_validate_shell_accepts_package_declared_absolute_paths() {
        assert!(validate_shell("/usr/sbin/nologin").is_ok());
        assert!(validate_shell("/sbin/nologin").is_ok());
        assert!(validate_shell("/bin/false").is_ok());
        assert!(validate_shell("/bin/sh").is_ok());
        assert!(validate_shell("/usr/bin/zsh").is_ok());
    }

    #[test]
    fn test_validate_shell_rejects_non_absolute_or_traversing_paths() {
        assert!(validate_shell("").is_err());
        assert!(validate_shell("bin/sh").is_err());
        assert!(validate_shell("/usr/bin/../bin/sh").is_err());
        assert!(validate_shell("./bin/sh").is_err());
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

        let executor = HookExecutor::new(temp_dir.path(), Default::default());
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

        let executor = HookExecutor::new(temp_dir.path(), Default::default());
        assert!(executor.group_exists_in_target("root"));
        assert!(executor.group_exists_in_target("wheel"));
        assert!(!executor.group_exists_in_target("nonexistent"));
    }
}
