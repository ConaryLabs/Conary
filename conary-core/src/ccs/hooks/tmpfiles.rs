// conary-core/src/ccs/hooks/tmpfiles.rs

//! Tmpfiles integration for CCS hooks
//!
//! Handles applying tmpfiles.d entries with support for both
//! live root (using systemd-tmpfiles) and target root (writing config files).

use super::HookExecutor;
use crate::filesystem::path::sanitize_path;
use anyhow::{Context, Result};
use std::fs;
use std::process::Command;
use tracing::debug;

pub(crate) fn validate_tmpfiles_entry_type(entry_type: &str) -> Result<()> {
    match entry_type {
        "d" | "D" | "e" | "v" | "q" | "Q" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "unsupported tmpfiles entry type '{}'",
            entry_type
        )),
    }
}

impl HookExecutor {
    /// Apply a tmpfiles entry
    ///
    /// When root == "/", uses systemd-tmpfiles to apply immediately.
    /// When root != "/", writes the config to /etc/tmpfiles.d/ in the target
    /// so it will be applied on first boot.
    pub(super) fn apply_tmpfile(&self, entry: &crate::ccs::manifest::TmpfilesHook) -> Result<()> {
        validate_tmpfiles_entry_type(&entry.entry_type)?;
        sanitize_path(&entry.path)?;

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
}

/// Deterministic string hash for generating unique filenames.
///
/// Uses FNV-1a (64-bit), which is stable across Rust versions and platforms,
/// unlike `DefaultHasher` which may change its algorithm between releases.
pub fn hash_string(s: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_string() {
        let hash1 = hash_string("/var/lib/test");
        let hash2 = hash_string("/var/lib/test");
        let hash3 = hash_string("/var/lib/other");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_validate_tmpfiles_entry_type_accepts_safe_types() {
        for entry_type in ["d", "D", "e", "v", "q", "Q"] {
            assert!(
                validate_tmpfiles_entry_type(entry_type).is_ok(),
                "expected {entry_type} to be accepted"
            );
        }
    }

    #[test]
    fn test_validate_tmpfiles_entry_type_rejects_unsafe_types() {
        for entry_type in ["f", "L", "C", "w", "x", ""] {
            assert!(
                validate_tmpfiles_entry_type(entry_type).is_err(),
                "expected {entry_type:?} to be rejected"
            );
        }
    }
}
