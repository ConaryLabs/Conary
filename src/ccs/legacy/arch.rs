// src/ccs/legacy/arch.rs
//! Arch Linux package generator
//!
//! Generates Arch .pkg.tar.zst packages from CCS build results.
//! Arch packages are zstd-compressed tarballs containing:
//! - .PKGINFO: package metadata
//! - .INSTALL: optional install/upgrade hooks
//! - Actual files at their install paths

use super::{
    arch_for_format, map_capability_to_package, CommonHookGenerator, GenerationResult,
    HookConverter, LossReport,
};
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::manifest::Hooks;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tar::Builder as TarBuilder;

/// Arch-specific hook converter
///
/// Arch uses .INSTALL scripts with specific functions:
/// - pre_install(version)
/// - post_install(version)
/// - pre_upgrade(new_version, old_version)
/// - post_upgrade(new_version, old_version)
/// - pre_remove(version)
/// - post_remove(version)
struct ArchHookConverter;

impl HookConverter for ArchHookConverter {
    fn pre_install(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = Vec::new();

        // Create groups and users
        lines.extend(CommonHookGenerator::user_creation_commands(hooks));

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn post_install(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = Vec::new();

        // Directory creation
        lines.extend(CommonHookGenerator::directory_commands(hooks));

        // Systemd
        lines.extend(CommonHookGenerator::systemd_commands(hooks, true));

        // tmpfiles
        lines.extend(CommonHookGenerator::tmpfiles_commands(hooks));

        // sysctl
        lines.extend(CommonHookGenerator::sysctl_commands(hooks));

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn pre_remove(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = Vec::new();

        // Stop services
        lines.extend(CommonHookGenerator::systemd_commands(hooks, false));

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn post_remove(&self, _hooks: &Hooks) -> Option<String> {
        // Arch typically doesn't need post-remove for our use cases
        None
    }
}

/// Generate an Arch Linux package from a CCS build result
pub fn generate(result: &BuildResult, output_path: &Path) -> Result<GenerationResult> {
    let mut loss_report = LossReport::default();

    // Create temp directory for building
    let temp_dir = tempfile::tempdir()?;
    let pkg_root = temp_dir.path().join("pkg");
    fs::create_dir_all(&pkg_root)?;

    // Extract package info
    let manifest = &result.manifest;
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let description = &manifest.package.description;
    let arch = arch_for_format(
        manifest
            .package
            .platform
            .as_ref()
            .and_then(|p| p.arch.as_deref()),
        "arch",
    );

    // Calculate installed size
    let installed_size = result.total_size;

    // Build .PKGINFO
    let mut pkginfo = String::new();
    pkginfo.push_str(&format!("pkgname = {}\n", name));

    // Arch version format: version-pkgrel (we use 1 as default pkgrel)
    let pkgver = format!("{}-1", version.replace('-', "_"));
    pkginfo.push_str(&format!("pkgver = {}\n", pkgver));
    pkginfo.push_str(&format!("pkgdesc = {}\n", description));
    pkginfo.push_str(&format!("arch = {}\n", arch));
    pkginfo.push_str(&format!("size = {}\n", installed_size));

    // URL
    if let Some(url) = &manifest.package.homepage {
        pkginfo.push_str(&format!("url = {}\n", url));
    }

    // License
    if let Some(license) = &manifest.package.license {
        pkginfo.push_str(&format!("license = {}\n", license));
    }

    // Groups from legacy overrides
    if let Some(legacy) = &manifest.legacy
        && let Some(arch_legacy) = &legacy.arch
    {
        for group in &arch_legacy.groups {
            pkginfo.push_str(&format!("group = {}\n", group));
        }
    }

    // Map capabilities to Arch dependencies
    for cap in &manifest.requires.capabilities {
        let cap_name = cap.name();
        if let Some(pkg) = map_capability_to_package(cap_name, "arch") {
            if let Some(ver) = cap.version() {
                pkginfo.push_str(&format!("depend = {}{}\n", pkg, ver));
            } else {
                pkginfo.push_str(&format!("depend = {}\n", pkg));
            }
        } else {
            loss_report.add_dependency_note(&format!(
                "Capability '{}' has no known Arch mapping",
                cap_name
            ));
        }
    }

    // Add package dependencies
    for pkg_dep in &manifest.requires.packages {
        if let Some(ver) = &pkg_dep.version {
            pkginfo.push_str(&format!("depend = {}{}\n", pkg_dep.name, ver));
        } else {
            pkginfo.push_str(&format!("depend = {}\n", pkg_dep.name));
        }
    }

    // Optional dependencies (suggests)
    for cap in &manifest.suggests.capabilities {
        pkginfo.push_str(&format!("optdepend = {}\n", cap));
    }

    // Build timestamp
    let builddate = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    pkginfo.push_str(&format!("builddate = {}\n", builddate));

    // Packager
    let packager = manifest
        .package
        .authors
        .as_ref()
        .and_then(|a| a.maintainers.first())
        .map(String::as_str)
        .unwrap_or("Unknown Packager <unknown@unknown.org>");
    pkginfo.push_str(&format!("packager = {}\n", packager));

    // Write .PKGINFO
    fs::write(pkg_root.join(".PKGINFO"), &pkginfo)?;

    // Generate .INSTALL script if we have hooks
    let hook_converter = ArchHookConverter;
    let install_script = generate_install_script(&hook_converter, &manifest.hooks);
    if let Some(script) = &install_script {
        fs::write(pkg_root.join(".INSTALL"), script)?;
    }

    // Note hook limitations
    if !manifest.hooks.alternatives.is_empty() {
        loss_report.add_hook_note("Alternatives not supported in Arch (no alternatives system)");
    }

    // Write data files
    for file in &result.files {
        if file.file_type == FileType::Directory {
            continue;
        }

        // Create parent directories
        let rel_path = file.path.trim_start_matches('/');
        let dest_path = pkg_root.join(rel_path);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        match file.file_type {
            FileType::Regular => {
                if let Some(content) = result.blobs.get(&file.hash) {
                    fs::write(&dest_path, content)?;

                    // Set permissions
                    let mut perms = fs::metadata(&dest_path)?.permissions();
                    perms.set_mode(file.mode);
                    fs::set_permissions(&dest_path, perms)?;
                }
            }
            FileType::Symlink => {
                if let Some(target) = &file.target {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, &dest_path)?;
                }
            }
            FileType::Directory => {}
        }
    }

    // Write backup array for config files
    if !manifest.config.files.is_empty() {
        let backup: Vec<_> = manifest
            .config
            .files
            .iter()
            .map(|f| f.trim_start_matches('/'))
            .collect();

        // Append backup entries to .PKGINFO
        let mut pkginfo_file = fs::OpenOptions::new()
            .append(true)
            .open(pkg_root.join(".PKGINFO"))?;

        for file in backup {
            writeln!(pkginfo_file, "backup = {}", file)?;
        }
    }

    // Create the tarball with zstd compression
    create_arch_package(&pkg_root, output_path)?;

    // Note features that don't map to Arch
    loss_report.add_unsupported("Component-based installation (Arch installs all components)");
    loss_report.add_unsupported("Merkle tree verification");
    loss_report.add_unsupported("Content-addressable storage deduplication");

    let size = fs::metadata(output_path)?.len();

    Ok(GenerationResult { size, loss_report })
}

/// Generate the .INSTALL script from hooks
fn generate_install_script(converter: &ArchHookConverter, hooks: &Hooks) -> Option<String> {
    let pre_install = converter.pre_install(hooks);
    let post_install = converter.post_install(hooks);
    let pre_remove = converter.pre_remove(hooks);
    let post_remove = converter.post_remove(hooks);

    if pre_install.is_none()
        && post_install.is_none()
        && pre_remove.is_none()
        && post_remove.is_none()
    {
        return None;
    }

    let mut script = String::new();

    if let Some(content) = pre_install {
        script.push_str("pre_install() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");

        // pre_upgrade uses same content
        script.push_str("pre_upgrade() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");
    }

    if let Some(content) = post_install {
        script.push_str("post_install() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");

        // post_upgrade uses same content
        script.push_str("post_upgrade() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");
    }

    if let Some(content) = pre_remove {
        script.push_str("pre_remove() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");
    }

    if let Some(content) = post_remove {
        script.push_str("post_remove() {\n");
        for line in content.lines() {
            script.push_str(&format!("    {}\n", line));
        }
        script.push_str("}\n\n");
    }

    Some(script)
}

/// Create the final .pkg.tar.zst package
fn create_arch_package(pkg_root: &Path, output_path: &Path) -> Result<()> {
    // Create tar archive in memory first
    let tar_path = pkg_root.with_extension("tar");
    {
        let file = File::create(&tar_path)?;
        let mut archive = TarBuilder::new(file);

        // Add .PKGINFO first (required)
        let pkginfo_path = pkg_root.join(".PKGINFO");
        if pkginfo_path.exists() {
            archive.append_path_with_name(&pkginfo_path, ".PKGINFO")?;
        }

        // Add .INSTALL if present
        let install_path = pkg_root.join(".INSTALL");
        if install_path.exists() {
            archive.append_path_with_name(&install_path, ".INSTALL")?;
        }

        // Add all other files
        for entry in walkdir(pkg_root)? {
            let rel_path = entry
                .strip_prefix(pkg_root)
                .context("Failed to get relative path")?;
            let rel_str = rel_path.to_string_lossy();

            // Skip metadata files we already added
            if rel_str == ".PKGINFO" || rel_str == ".INSTALL" {
                continue;
            }

            if entry.is_file() || entry.is_symlink() {
                archive.append_path_with_name(&entry, rel_path)?;
            } else if entry.is_dir() && rel_path.as_os_str() != "" {
                archive.append_dir(rel_path, &entry)?;
            }
        }

        archive.finish()?;
    }

    // Compress with zstd
    let tar_data = fs::read(&tar_path)?;
    let compressed = zstd::encode_all(&tar_data[..], 19)?; // Level 19 for high compression

    fs::write(output_path, compressed)?;
    fs::remove_file(&tar_path)?;

    Ok(())
}

/// Simple recursive directory walker
fn walkdir(path: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut entries = Vec::new();

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();

            entries.push(path.clone());

            if path.is_dir() {
                entries.extend(walkdir(&path)?);
            }
        }
    }

    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::CcsManifest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let manifest = CcsManifest::new_minimal("test-arch-package", "1.0.0");
        BuildResult {
            manifest,
            components: HashMap::new(),
            files: vec![],
            blobs: HashMap::new(),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        }
    }

    #[test]
    fn test_arch_generation_empty() {
        let result = create_test_build_result();
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("test.pkg.tar.zst");

        let gen_result = generate(&result, &output_path).unwrap();
        assert!(output_path.exists());
        assert!(gen_result.size > 0);
    }

    #[test]
    fn test_install_script_generation() {
        let mut hooks = Hooks::default();
        hooks.users.push(crate::ccs::manifest::UserHook {
            name: "myapp".to_string(),
            system: true,
            home: Some("/var/lib/myapp".to_string()),
            shell: None,
            group: None,
        });
        hooks.systemd.push(crate::ccs::manifest::SystemdHook {
            unit: "myapp.service".to_string(),
            enable: true,
        });

        let converter = ArchHookConverter;
        let script = generate_install_script(&converter, &hooks).unwrap();

        assert!(script.contains("pre_install()"));
        assert!(script.contains("post_install()"));
        assert!(script.contains("useradd"));
        assert!(script.contains("systemctl"));
    }

    #[test]
    fn test_pkgver_format() {
        // Arch version shouldn't contain hyphens in the version part
        let version = "1.0.0-beta";
        let pkgver = format!("{}-1", version.replace('-', "_"));
        assert_eq!(pkgver, "1.0.0_beta-1");
    }
}
