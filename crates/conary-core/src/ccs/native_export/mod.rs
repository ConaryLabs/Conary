// conary-core/src/ccs/native_export/mod.rs
//! Native package format exporters
//!
//! This module exports CCS packages as DEB, RPM, and Arch packages for native
//! package-manager consumption.

pub mod arch;
pub mod deb;
pub mod rpm;

use crate::ccs::builder::FileEntry;
use crate::ccs::manifest::Hooks;
use anyhow::anyhow;
use std::collections::HashMap;

/// Retrieve file content from the blob map, handling both whole-file and
/// CDC-chunked storage.
///
/// When a file has `chunks: Some(chunk_hashes)`, the content is assembled by
/// concatenating the blobs keyed by each chunk hash in order. When `chunks`
/// is `None`, the content is looked up by the whole-file hash directly.
pub fn get_file_content(
    file: &FileEntry,
    blobs: &HashMap<String, Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let authority = file.content.as_ref().ok_or_else(|| {
        anyhow!(
            "Non-regular payload node {} has no content authority",
            file.path
        )
    })?;
    if let Some(chunk_hashes) = &file.chunks {
        let mut assembled = Vec::with_capacity(authority.size as usize);
        for chunk_hash in chunk_hashes {
            let chunk = blobs
                .get(chunk_hash)
                .ok_or_else(|| anyhow!("Chunk {} not found for file {}", chunk_hash, file.path))?;
            assembled.extend_from_slice(chunk);
        }
        if assembled.len() as u64 != authority.size
            || crate::hash::sha256(&assembled) != authority.sha256
        {
            anyhow::bail!(
                "Assembled content does not match authority for {}",
                file.path
            );
        }
        Ok(assembled)
    } else {
        let content = blobs
            .get(&authority.sha256)
            .cloned()
            .ok_or_else(|| anyhow!("Blob not found for file {}", file.path))?;
        if content.len() as u64 != authority.size
            || crate::hash::sha256(&content) != authority.sha256
        {
            anyhow::bail!("Blob does not match authority for {}", file.path);
        }
        Ok(content)
    }
}

/// Information that may be lost when exporting to a native package format.
#[derive(Debug, Default)]
pub struct LossReport {
    /// Features that couldn't be represented in the target format
    pub unsupported_features: Vec<String>,
    /// Hooks that were approximated or skipped
    pub hook_notes: Vec<String>,
    /// Dependency mappings that may be inaccurate
    pub dependency_notes: Vec<String>,
}

impl LossReport {
    pub fn is_empty(&self) -> bool {
        self.unsupported_features.is_empty()
            && self.hook_notes.is_empty()
            && self.dependency_notes.is_empty()
    }

    pub fn add_unsupported(&mut self, feature: &str) {
        self.unsupported_features.push(feature.to_string());
    }

    pub fn add_hook_note(&mut self, note: &str) {
        self.hook_notes.push(note.to_string());
    }

    pub fn add_dependency_note(&mut self, note: &str) {
        self.dependency_notes.push(note.to_string());
    }

    /// Print a summary of what was lost in conversion
    pub fn print_summary(&self, format_name: &str) {
        if self.is_empty() {
            return;
        }

        println!("  Conversion notes for {}:", format_name);

        for note in &self.unsupported_features {
            println!("    [UNSUPPORTED] {}", note);
        }

        for note in &self.hook_notes {
            println!("    [HOOK] {}", note);
        }

        for note in &self.dependency_notes {
            println!("    [DEPENDENCY] {}", note);
        }
    }
}

/// Result of native package generation.
#[derive(Debug)]
pub struct GenerationResult {
    /// Size of the generated package in bytes
    pub size: u64,
    /// Any lossy conversion notes
    pub loss_report: LossReport,
}

/// Convert CCS hooks to format-specific install scripts
pub trait HookConverter {
    /// Generate pre-install script content
    fn pre_install(&self, hooks: &Hooks) -> Option<String>;

    /// Generate post-install script content
    fn post_install(&self, hooks: &Hooks) -> Option<String>;

    /// Generate pre-remove script content
    fn pre_remove(&self, hooks: &Hooks) -> Option<String>;

    /// Generate post-remove script content
    fn post_remove(&self, hooks: &Hooks) -> Option<String>;
}

/// Common functionality for generating install scripts from CCS hooks
pub struct CommonHookGenerator;

impl CommonHookGenerator {
    /// Generate user creation commands
    pub fn user_creation_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for group in &hooks.groups {
            let flags = if group.system { "--system " } else { "" };
            let name = shell_escape(&group.name);
            commands.push(format!(
                "getent group {name} >/dev/null || groupadd {flags}{name}"
            ));
        }

        for user in &hooks.users {
            let mut flags = Vec::new();
            if user.system {
                flags.push("--system".to_string());
            }
            if let Some(home) = &user.home {
                flags.push(format!("--home-dir {}", shell_escape(home)));
            }
            if let Some(shell) = &user.shell {
                flags.push(format!("--shell {}", shell_escape(shell)));
            } else if user.system {
                flags.push("--shell /usr/sbin/nologin".to_string());
            }
            if let Some(group) = &user.group {
                flags.push(format!("--gid {}", shell_escape(group)));
            }

            let flags_str = if flags.is_empty() {
                String::new()
            } else {
                format!("{} ", flags.join(" "))
            };

            let name = shell_escape(&user.name);
            commands.push(format!(
                "getent passwd {name} >/dev/null || useradd {flags_str}{name}"
            ));
        }

        commands
    }

    /// Generate directory creation commands
    pub fn directory_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for dir in &hooks.directories {
            commands.push(format!(
                "install -d -m {} -o {} -g {} {}",
                shell_escape(&dir.mode),
                shell_escape(&dir.owner),
                shell_escape(&dir.group),
                shell_escape(&dir.path),
            ));
        }

        commands
    }

    /// Generate systemd enable/disable commands
    pub fn systemd_commands(hooks: &Hooks, enable: bool) -> Vec<String> {
        let mut commands = Vec::new();

        for unit in &hooks.systemd {
            if unit.enable && enable {
                // Only enable if requested
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl daemon-reload; systemctl enable {unit_name}; fi"
                ));
            } else if !enable {
                // Stop on removal
                let unit_name = shell_escape(&unit.unit);
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl stop {unit_name} 2>/dev/null || true; fi"
                ));
            }
        }

        commands
    }

    /// Generate tmpfiles.d commands
    pub fn tmpfiles_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        if !hooks.tmpfiles.is_empty() {
            // systemd-tmpfiles --create will read from /usr/lib/tmpfiles.d/
            commands.push(
                "if command -v systemd-tmpfiles >/dev/null 2>&1; then systemd-tmpfiles --create; fi"
                    .to_string(),
            );
        }

        commands
    }

    /// Generate sysctl commands
    ///
    /// Sysctl keys and values are validated to contain only safe characters
    /// (alphanumeric, dots, underscores, hyphens, digits) and emitted unquoted.
    /// This avoids breaking arithmetic comparisons and sysctl -w assignments
    /// that would fail with shell-escaped (single-quoted) values.
    pub fn sysctl_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for sysctl in &hooks.sysctl {
            let key = &sysctl.key;
            let value = &sysctl.value;

            // Validate that key and value contain only safe characters
            let is_safe = |s: &str| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
            };

            if !is_safe(key) || !is_safe(value) {
                // Fall back to shell-escaped form for safety, but only for
                // non-arithmetic (unconditional) sysctl writes
                let escaped_key = shell_escape(key);
                let escaped_value = shell_escape(value);
                commands.push(format!("sysctl -w {escaped_key}={escaped_value}"));
                continue;
            }

            commands.push(format!("sysctl -w {key}={value}"));
        }

        commands
    }
}

/// Escape a string for safe use in shell commands.
///
/// Wraps the value in single quotes and escapes any embedded single quotes
/// using the `'\''` idiom, which is safe against shell injection.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve a CCS architecture through the exact target-format contract.
pub fn arch_for_format(arch: Option<&str>, format: &str) -> anyhow::Result<String> {
    let arch = arch.ok_or_else(|| anyhow!("CCS package has no target architecture"))?;

    let target = match format {
        "deb" => match arch {
            "x86_64" | "amd64" => "amd64",
            "aarch64" | "arm64" => "arm64",
            "i686" | "i386" => "i386",
            "armv7l" | "armhf" => "armhf",
            "all" => "all",
            other => anyhow::bail!("unsupported Debian target architecture '{other}'"),
        },
        "rpm" => match arch {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            "i686" => "i686",
            "i386" => "i386",
            "armv7l" | "armv7hl" => "armv7hl",
            "noarch" => "noarch",
            other => anyhow::bail!("unsupported RPM target architecture '{other}'"),
        },
        "arch" => match arch {
            "x86_64" | "amd64" => "x86_64",
            "any" => "any",
            other => anyhow::bail!("unsupported ALPM target architecture '{other}'"),
        },
        other => anyhow::bail!("unsupported native package format '{other}'"),
    };
    Ok(target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{PayloadContentAuthority, PayloadNode};

    fn regular_entry(path: &str, bytes: &[u8], chunks: Option<Vec<String>>) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            node: PayloadNode::regular(0o755),
            content: Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(bytes),
                size: bytes.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks,
        }
    }

    #[test]
    fn test_arch_mapping() {
        assert_eq!(arch_for_format(Some("x86_64"), "deb").unwrap(), "amd64");
        assert_eq!(arch_for_format(Some("aarch64"), "deb").unwrap(), "arm64");
        assert_eq!(arch_for_format(Some("amd64"), "rpm").unwrap(), "x86_64");
        assert!(arch_for_format(None, "arch").is_err());
        assert!(arch_for_format(Some("riscv64"), "arch").is_err());
    }

    #[test]
    fn test_loss_report() {
        let mut report = LossReport::default();
        assert!(report.is_empty());

        report.add_unsupported("merkle tree verification");
        assert!(!report.is_empty());
        assert_eq!(report.unsupported_features.len(), 1);
    }

    #[test]
    fn test_get_file_content_whole_blob() {
        let bytes = b"hello world";
        let digest = crate::hash::sha256(bytes);
        let mut blobs = HashMap::new();
        blobs.insert(digest, bytes.to_vec());
        let file = regular_entry("/usr/bin/foo", bytes, None);

        let content = get_file_content(&file, &blobs).unwrap();
        assert_eq!(content, bytes);
    }

    #[test]
    fn test_get_file_content_chunked() {
        let first = b"hello ";
        let second = b"world";
        let first_digest = crate::hash::sha256(first);
        let second_digest = crate::hash::sha256(second);
        let mut blobs = HashMap::new();
        blobs.insert(first_digest.clone(), first.to_vec());
        blobs.insert(second_digest.clone(), second.to_vec());
        let file = regular_entry(
            "/usr/bin/bar",
            b"hello world",
            Some(vec![first_digest, second_digest]),
        );

        let content = get_file_content(&file, &blobs).unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn test_get_file_content_missing_chunk_errors() {
        let blobs = HashMap::new();
        let missing_digest = crate::hash::sha256(b"missing");
        let file = regular_entry("/usr/bin/baz", b"hello", Some(vec![missing_digest.clone()]));

        let result = get_file_content(&file, &blobs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(&missing_digest));
    }
}
