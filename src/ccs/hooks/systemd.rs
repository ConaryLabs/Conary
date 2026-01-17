// src/ccs/hooks/systemd.rs

//! Systemd integration for CCS hooks
//!
//! Handles enabling/disabling systemd units with support for both
//! live root (using systemctl) and target root (using symlinks).

use super::HookExecutor;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

impl HookExecutor {
    /// Check if systemctl is available
    pub(super) fn has_systemctl(&self) -> bool {
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
    pub(super) fn systemd_daemon_reload(&self) -> Result<()> {
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
    pub(super) fn systemd_enable(&self, unit: &str) -> Result<()> {
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
    pub(super) fn systemd_enable_target(&self, unit: &str) -> Result<()> {
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
}

/// Parse systemd unit file [Install] section for WantedBy/RequiredBy
///
/// Returns a list of target units that this unit should be linked to.
pub fn parse_systemd_install_section(content: &str, key: &str) -> Vec<String> {
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
pub fn compute_relative_unit_path(unit_path: &Path, root: &Path) -> String {
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
}
