// src/ccs/inspector.rs
//! CCS package inspection
//!
//! Tools for reading and examining .ccs packages.

use crate::ccs::builder::{ComponentData, FileEntry};
use crate::ccs::manifest::CcsManifest;
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use tar::Archive;

/// Inspected package data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectedPackage {
    /// Package manifest
    pub manifest: CcsManifest,
    /// All files in the package
    pub files: Vec<FileEntry>,
    /// Components
    pub components: HashMap<String, ComponentData>,
}

impl InspectedPackage {
    /// Load a package from a .ccs file
    pub fn from_file(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open package: {}", path.display()))?;

        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        let mut manifest: Option<CcsManifest> = None;
        let mut components: HashMap<String, ComponentData> = HashMap::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?;
            let entry_path_str = entry_path.to_string_lossy();

            // Read MANIFEST.toml
            if entry_path_str == "MANIFEST.toml" || entry_path_str == "./MANIFEST.toml" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                manifest = Some(CcsManifest::parse(&content)?);
            }
            // Read component files (files are stored in components/*.json per spec)
            else if (entry_path_str.starts_with("components/") || entry_path_str.starts_with("./components/"))
                && entry_path_str.ends_with(".json")
            {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                let comp: ComponentData = serde_json::from_str(&content)?;
                components.insert(comp.name.clone(), comp);
            }
        }

        let manifest = manifest.ok_or_else(|| anyhow::anyhow!("Package missing MANIFEST.toml"))?;

        // Collect files from components (spec says files live in components/*.json)
        let files: Vec<FileEntry> = components
            .values()
            .flat_map(|c| c.files.clone())
            .collect();

        Ok(InspectedPackage {
            manifest,
            files,
            components,
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
        self.files.iter().map(|f| f.size).sum()
    }

    /// Get component names
    pub fn component_names(&self) -> Vec<&str> {
        self.components.keys().map(|s| s.as_str()).collect()
    }
}

/// Print package summary
pub fn print_summary(pkg: &InspectedPackage) {
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
        println!("  :{} - {} files ({} bytes){}", name, comp.files.len(), comp.size, marker);
    }
}

/// Print file listing
pub fn print_files(pkg: &InspectedPackage) {
    println!("Files ({}):", pkg.file_count());
    println!();

    for file in &pkg.files {
        let mode_str = format_mode(file.mode);
        let type_char = match file.file_type {
            crate::ccs::builder::FileType::Regular => '-',
            crate::ccs::builder::FileType::Symlink => 'l',
            crate::ccs::builder::FileType::Directory => 'd',
        };

        let size_or_target = if let Some(target) = &file.target {
            format!("-> {}", target)
        } else {
            format!("{:>10}", file.size)
        };

        println!("{}{} :{:<8} {} {}",
            type_char,
            mode_str,
            file.component,
            size_or_target,
            file.path
        );
    }
}

/// Print hooks
pub fn print_hooks(pkg: &InspectedPackage) {
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
            println!("  - {} (mode={}, owner={}:{})",
                dir.path, dir.mode, dir.owner, dir.group);
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
            println!("  - {} -> {} (priority={})", alt.name, alt.path, alt.priority);
        }
        println!();
    }
}

/// Print dependencies
pub fn print_dependencies(pkg: &InspectedPackage) {
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
    if !pkg.manifest.requires.capabilities.is_empty() {
        for cap in &pkg.manifest.requires.capabilities {
            println!("  - {}", cap.name());
        }
    } else {
        println!("  (none declared)");
    }

    if !pkg.manifest.requires.packages.is_empty() {
        println!();
        println!("Package dependencies (fallback):");
        for dep in &pkg.manifest.requires.packages {
            if let Some(ver) = &dep.version {
                println!("  - {} {}", dep.name, ver);
            } else {
                println!("  - {}", dep.name);
            }
        }
    }
}

/// Print as JSON
pub fn print_json(pkg: &InspectedPackage, show_files: bool, show_hooks: bool, show_deps: bool) -> Result<()> {
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
        requires: Option<&'a crate::ccs::manifest::Requires>,
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
        hooks: if show_hooks { Some(&pkg.manifest.hooks) } else { None },
        provides: if show_deps { Some(&pkg.manifest.provides) } else { None },
        requires: if show_deps { Some(&pkg.manifest.requires) } else { None },
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
