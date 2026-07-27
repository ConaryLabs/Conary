// conary-core/src/ccs/inspector.rs
//! Explicitly non-authoritative CCS package inspection.
//!
//! Tools for reading and examining .ccs packages.

use crate::ccs::archive_reader::inspect_untrusted_ccs_archive;
use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use crate::payload::PayloadNodeKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// Structurally decoded package data that grants no trust or mutation authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntrustedPackageInspection {
    /// Package manifest
    pub manifest: CcsManifest,
    /// All files in the package
    pub files: Vec<FileEntry>,
    /// Components
    pub components: HashMap<String, ComponentData>,
}

impl UntrustedPackageInspection {
    /// Decode a package for diagnostics without authenticating it.
    pub fn inspect_untrusted_file(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open package: {}", path.display()))?;

        let contents = inspect_untrusted_ccs_archive(file)?;

        // Files come from the signed authority. The archive no longer carries a
        // duplicated `components/*.json` copy of the same records.
        let files: Vec<FileEntry> =
            crate::ccs::v2::component_view::file_entries(&contents.v2_authority);

        Ok(Self {
            manifest: contents.manifest,
            files,
            components: contents.components,
        })
    }

    /// Get package name
    pub fn name(&self) -> &str {
        &self.manifest.package.name
    }

    /// Get package version
    pub fn version(&self) -> &str {
        &self.manifest.package.version
    }

    /// Get total file count
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Get total size
    pub fn total_size(&self) -> u64 {
        self.files
            .iter()
            .filter_map(|file| file.content.as_ref().map(|content| content.size))
            .sum()
    }

    /// Get component names
    pub fn component_names(&self) -> Vec<&str> {
        self.components.keys().map(|s| s.as_str()).collect()
    }
}

/// Print package summary
pub fn print_summary(pkg: &UntrustedPackageInspection) {
    println!("Package: {} v{}", pkg.name(), pkg.version());
    println!("Description: {}", pkg.manifest.package.description);

    if let Some(license) = &pkg.manifest.package.license {
        println!("License: {}", license);
    }

    println!();
    println!("Total files: {}", pkg.file_count());
    println!("Total size: {} bytes", pkg.total_size());

    println!();
    println!("Components:");
    let mut comp_names: Vec<_> = pkg.components.keys().collect();
    comp_names.sort();
    for name in comp_names {
        let comp = &pkg.components[name];
        let is_default = pkg.manifest.components.default.contains(name);
        let marker = if is_default { " (default)" } else { "" };
        println!(
            "  :{} - {} files ({} bytes){}",
            name,
            comp.files.len(),
            comp.size,
            marker
        );
    }
}

/// Print file listing
pub fn print_files(pkg: &UntrustedPackageInspection) {
    println!("Files ({}):", pkg.file_count());
    println!();

    for file in &pkg.files {
        let mode_str = format_mode(file.node.mode);
        let type_char = match &file.node.kind {
            PayloadNodeKind::Regular { .. } => '-',
            PayloadNodeKind::Symlink { .. } => 'l',
            PayloadNodeKind::Hardlink { .. } => 'h',
            PayloadNodeKind::Directory => 'd',
            PayloadNodeKind::BlockDevice { .. } => 'b',
            PayloadNodeKind::CharacterDevice { .. } => 'c',
            PayloadNodeKind::Fifo => 'p',
            PayloadNodeKind::Socket => 's',
        };

        let size_or_target = match (&file.node.kind, &file.content) {
            (PayloadNodeKind::Symlink { target } | PayloadNodeKind::Hardlink { target, .. }, _) => {
                format!("-> {target}")
            }
            (
                PayloadNodeKind::BlockDevice { major, minor }
                | PayloadNodeKind::CharacterDevice { major, minor },
                _,
            ) => format!("{major}:{minor}"),
            (_, Some(content)) => format!("{:>10}", content.size),
            (_, None) => "          ".to_string(),
        };

        println!(
            "{}{} :{:<8} {} {}",
            type_char, mode_str, file.component, size_or_target, file.path
        );
    }
}

/// Print hooks
pub fn print_hooks(pkg: &UntrustedPackageInspection) {
    let hooks = &pkg.manifest.hooks;

    if hooks.users.is_empty()
        && hooks.groups.is_empty()
        && hooks.directories.is_empty()
        && hooks.systemd.is_empty()
        && hooks.tmpfiles.is_empty()
        && hooks.sysctl.is_empty()
        && hooks.alternatives.is_empty()
    {
        println!("No hooks defined");
        return;
    }

    if !hooks.users.is_empty() {
        println!("Users:");
        for user in &hooks.users {
            let sys = if user.system { " (system)" } else { "" };
            println!("  - {}{}", user.name, sys);
            if let Some(home) = &user.home {
                println!("      home: {}", home);
            }
        }
        println!();
    }

    if !hooks.groups.is_empty() {
        println!("Groups:");
        for group in &hooks.groups {
            let sys = if group.system { " (system)" } else { "" };
            println!("  - {}{}", group.name, sys);
        }
        println!();
    }

    if !hooks.directories.is_empty() {
        println!("Directories:");
        for dir in &hooks.directories {
            println!(
                "  - {} (mode={}, owner={}:{})",
                dir.path, dir.mode, dir.owner, dir.group
            );
        }
        println!();
    }

    if !hooks.systemd.is_empty() {
        println!("Systemd units:");
        for unit in &hooks.systemd {
            let enabled = if unit.enable { " [enable]" } else { "" };
            println!("  - {}{}", unit.unit, enabled);
        }
        println!();
    }

    if !hooks.alternatives.is_empty() {
        println!("Alternatives:");
        for alt in &hooks.alternatives {
            println!(
                "  - {} -> {} (priority={})",
                alt.name, alt.path, alt.priority
            );
        }
        println!();
    }
}

/// Print dependencies
pub fn print_dependencies(pkg: &UntrustedPackageInspection) {
    println!("Provides:");
    if !pkg.manifest.provides.capabilities.is_empty() {
        for cap in &pkg.manifest.provides.capabilities {
            println!("  - {}", cap);
        }
    } else {
        println!("  (none declared)");
    }

    println!();
    println!("Requires:");
    if pkg.manifest.requirements.is_empty() {
        println!("  (none declared)");
    } else {
        for requirement in &pkg.manifest.requirements {
            if let Some(native_text) = &requirement.native_text {
                println!("  - {native_text}");
            } else {
                println!(
                    "  - {}",
                    serde_json::to_string(requirement)
                        .expect("typed CCS requirement is JSON serializable")
                );
            }
        }
    }
}

/// Print as JSON
pub fn print_json(
    pkg: &UntrustedPackageInspection,
    show_files: bool,
    show_hooks: bool,
    show_deps: bool,
) -> Result<()> {
    #[derive(Serialize)]
    struct JsonOutput<'a> {
        name: &'a str,
        version: &'a str,
        description: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        license: &'a Option<String>,
        file_count: usize,
        total_size: u64,
        components: &'a HashMap<String, ComponentData>,
        #[serde(skip_serializing_if = "Option::is_none")]
        files: Option<&'a Vec<FileEntry>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hooks: Option<&'a crate::ccs::manifest::Hooks>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provides: Option<&'a crate::ccs::manifest::Provides>,
        #[serde(skip_serializing_if = "Option::is_none")]
        requirements:
            Option<&'a Vec<crate::repository::dependency_model::RepositoryRequirementGroup>>,
    }

    let output = JsonOutput {
        name: pkg.name(),
        version: pkg.version(),
        description: &pkg.manifest.package.description,
        license: &pkg.manifest.package.license,
        file_count: pkg.file_count(),
        total_size: pkg.total_size(),
        components: &pkg.components,
        files: if show_files { Some(&pkg.files) } else { None },
        hooks: if show_hooks {
            Some(&pkg.manifest.hooks)
        } else {
            None
        },
        provides: if show_deps {
            Some(&pkg.manifest.provides)
        } else {
            None
        },
        requirements: if show_deps {
            Some(&pkg.manifest.requirements)
        } else {
            None
        },
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Format Unix mode as rwxrwxrwx string
fn format_mode(mode: u32) -> String {
    let user = format_triplet((mode >> 6) & 0o7);
    let group = format_triplet((mode >> 3) & 0o7);
    let other = format_triplet(mode & 0o7);
    format!("{}{}{}", user, group, other)
}

fn format_triplet(bits: u32) -> String {
    let r = if bits & 0o4 != 0 { 'r' } else { '-' };
    let w = if bits & 0o2 != 0 { 'w' } else { '-' };
    let x = if bits & 0o1 != 0 { 'x' } else { '-' };
    format!("{}{}{}", r, w, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_mode() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o644), "rw-r--r--");
        assert_eq!(format_mode(0o777), "rwxrwxrwx");
        assert_eq!(format_mode(0o000), "---------");
    }
}
