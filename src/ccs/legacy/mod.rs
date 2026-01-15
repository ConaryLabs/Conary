// src/ccs/legacy/mod.rs
//! Legacy package format generators
//!
//! This module provides generators to convert CCS packages to legacy formats
//! (DEB, RPM, Arch) for compatibility with existing package managers.

pub mod arch;
pub mod deb;
pub mod rpm;

use crate::ccs::manifest::Hooks;
use std::collections::HashMap;

/// Information that may be lost when converting to legacy formats
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

/// Result of a legacy format generation
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
            commands.push(format!("getent group {} >/dev/null || groupadd {}{}",
                group.name, flags, group.name));
        }

        for user in &hooks.users {
            let mut flags = Vec::new();
            if user.system {
                flags.push("--system".to_string());
            }
            if let Some(home) = &user.home {
                flags.push(format!("--home-dir {}", home));
            }
            if let Some(shell) = &user.shell {
                flags.push(format!("--shell {}", shell));
            } else if user.system {
                flags.push("--shell /usr/sbin/nologin".to_string());
            }
            if let Some(group) = &user.group {
                flags.push(format!("--gid {}", group));
            }

            let flags_str = if flags.is_empty() {
                String::new()
            } else {
                format!("{} ", flags.join(" "))
            };

            commands.push(format!("getent passwd {} >/dev/null || useradd {}{}",
                user.name, flags_str, user.name));
        }

        commands
    }

    /// Generate directory creation commands
    pub fn directory_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for dir in &hooks.directories {
            commands.push(format!(
                "install -d -m {} -o {} -g {} {}",
                dir.mode, dir.owner, dir.group, dir.path
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
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl daemon-reload; systemctl enable {}; fi",
                    unit.unit
                ));
            } else if !enable {
                // Stop on removal
                commands.push(format!(
                    "if command -v systemctl >/dev/null 2>&1; then systemctl stop {} 2>/dev/null || true; fi",
                    unit.unit
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
    pub fn sysctl_commands(hooks: &Hooks) -> Vec<String> {
        let mut commands = Vec::new();

        for sysctl in &hooks.sysctl {
            if sysctl.only_if_lower {
                commands.push(format!(
                    "current=$(sysctl -n {} 2>/dev/null || echo 0); if [ \"$current\" -lt {} ]; then sysctl -w {}={}; fi",
                    sysctl.key, sysctl.value, sysctl.key, sysctl.value
                ));
            } else {
                commands.push(format!("sysctl -w {}={}", sysctl.key, sysctl.value));
            }
        }

        commands
    }
}

/// Map a CCS dependency to a format-specific package name
/// This is a best-effort mapping and may not be accurate for all cases
pub fn map_capability_to_package(capability: &str, format: &str) -> Option<String> {
    // Common mappings for well-known capabilities
    let mappings: HashMap<&str, HashMap<&str, &str>> = HashMap::from([
        (
            "glibc",
            HashMap::from([("deb", "libc6"), ("rpm", "glibc"), ("arch", "glibc")]),
        ),
        (
            "openssl",
            HashMap::from([
                ("deb", "libssl3"),
                ("rpm", "openssl-libs"),
                ("arch", "openssl"),
            ]),
        ),
        (
            "zlib",
            HashMap::from([("deb", "zlib1g"), ("rpm", "zlib"), ("arch", "zlib")]),
        ),
        (
            "libcurl",
            HashMap::from([
                ("deb", "libcurl4"),
                ("rpm", "libcurl"),
                ("arch", "curl"),
            ]),
        ),
    ]);

    mappings
        .get(capability)
        .and_then(|m| m.get(format))
        .map(|s| (*s).to_string())
}

/// Get the architecture string for a format
pub fn arch_for_format(arch: Option<&str>, format: &str) -> String {
    let arch = arch.unwrap_or("x86_64");

    match format {
        "deb" => match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "i686" | "i386" => "i386",
            "armv7l" | "armhf" => "armhf",
            _ => arch,
        },
        "rpm" => match arch {
            "amd64" => "x86_64",
            "arm64" => "aarch64",
            _ => arch,
        },
        "arch" => match arch {
            "amd64" => "x86_64",
            _ => arch,
        },
        _ => arch,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_mapping() {
        assert_eq!(arch_for_format(Some("x86_64"), "deb"), "amd64");
        assert_eq!(arch_for_format(Some("aarch64"), "deb"), "arm64");
        assert_eq!(arch_for_format(Some("amd64"), "rpm"), "x86_64");
        assert_eq!(arch_for_format(None, "arch"), "x86_64");
    }

    #[test]
    fn test_capability_mapping() {
        assert_eq!(
            map_capability_to_package("glibc", "deb"),
            Some("libc6".to_string())
        );
        assert_eq!(
            map_capability_to_package("openssl", "rpm"),
            Some("openssl-libs".to_string())
        );
        assert_eq!(map_capability_to_package("unknown", "deb"), None);
    }

    #[test]
    fn test_loss_report() {
        let mut report = LossReport::default();
        assert!(report.is_empty());

        report.add_unsupported("merkle tree verification");
        assert!(!report.is_empty());
        assert_eq!(report.unsupported_features.len(), 1);
    }
}
