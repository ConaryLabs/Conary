// src/ccs/legacy/deb.rs
//! DEB package generator
//!
//! Generates Debian .deb packages from CCS build results.
//! DEB packages are ar archives containing:
//! - debian-binary: version string "2.0\n"
//! - control.tar.gz: package metadata and scripts
//! - data.tar.gz: actual file contents

use super::{
    arch_for_format, map_capability_to_package, CommonHookGenerator, GenerationResult,
    HookConverter, LossReport,
};
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::manifest::Hooks;
use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tar::Builder as TarBuilder;

/// DEB-specific hook converter
struct DebHookConverter;

impl HookConverter for DebHookConverter {
    fn pre_install(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // Groups and users should be created in preinst for DEB
        lines.extend(CommonHookGenerator::user_creation_commands(hooks));

        if lines.len() <= 2 {
            return None;
        }

        lines.push("exit 0".to_string());
        Some(lines.join("\n"))
    }

    fn post_install(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // Directories
        lines.extend(CommonHookGenerator::directory_commands(hooks));

        // Systemd
        lines.extend(CommonHookGenerator::systemd_commands(hooks, true));

        // tmpfiles
        lines.extend(CommonHookGenerator::tmpfiles_commands(hooks));

        // sysctl
        lines.extend(CommonHookGenerator::sysctl_commands(hooks));

        // ldconfig for shared libraries
        lines.push("if command -v ldconfig >/dev/null 2>&1; then ldconfig; fi".to_string());

        if lines.len() <= 2 {
            return None;
        }

        lines.push("exit 0".to_string());
        Some(lines.join("\n"))
    }

    fn pre_remove(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // Stop services before removal
        lines.extend(CommonHookGenerator::systemd_commands(hooks, false));

        if lines.len() <= 2 {
            return None;
        }

        lines.push("exit 0".to_string());
        Some(lines.join("\n"))
    }

    fn post_remove(&self, _hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // ldconfig to update library cache
        lines.push("if command -v ldconfig >/dev/null 2>&1; then ldconfig; fi".to_string());

        if lines.len() <= 2 {
            return None;
        }

        lines.push("exit 0".to_string());
        Some(lines.join("\n"))
    }
}

/// Generate a DEB package from a CCS build result
pub fn generate(result: &BuildResult, output_path: &Path) -> Result<GenerationResult> {
    let mut loss_report = LossReport::default();

    // Create temp directory for building
    let temp_dir = tempfile::tempdir()?;
    let control_dir = temp_dir.path().join("control");
    let data_dir = temp_dir.path().join("data");
    fs::create_dir_all(&control_dir)?;
    fs::create_dir_all(&data_dir)?;

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
        "deb",
    );

    // Build control file
    let mut control = format!(
        "Package: {}\n\
         Version: {}\n\
         Architecture: {}\n\
         Maintainer: {}\n\
         Description: {}\n",
        name,
        version,
        arch,
        manifest
            .package
            .authors
            .as_ref()
            .and_then(|a| a.maintainers.first())
            .map(String::as_str)
            .unwrap_or("Unknown <unknown@unknown.org>"),
        description
    );

    // Add section and priority from legacy overrides
    if let Some(legacy) = &manifest.legacy
        && let Some(deb) = &legacy.deb
    {
        if let Some(section) = &deb.section {
            control.push_str(&format!("Section: {}\n", section));
        }
        if let Some(priority) = &deb.priority {
            control.push_str(&format!("Priority: {}\n", priority));
        }

        // Add explicit depends
        if !deb.depends.is_empty() {
            control.push_str(&format!("Depends: {}\n", deb.depends.join(", ")));
        }
    }

    // Map capabilities to dependencies
    let mut deps = Vec::new();
    for cap in &manifest.requires.capabilities {
        let cap_name = cap.name();
        if let Some(pkg) = map_capability_to_package(cap_name, "deb") {
            if let Some(ver) = cap.version() {
                deps.push(format!("{} ({})", pkg, ver));
            } else {
                deps.push(pkg);
            }
        } else {
            loss_report.add_dependency_note(&format!(
                "Capability '{}' has no known DEB mapping",
                cap_name
            ));
        }
    }

    // Add package dependencies
    for pkg_dep in &manifest.requires.packages {
        if let Some(ver) = &pkg_dep.version {
            deps.push(format!("{} ({})", pkg_dep.name, ver));
        } else {
            deps.push(pkg_dep.name.clone());
        }
    }

    if !deps.is_empty() && !control.contains("Depends:") {
        control.push_str(&format!("Depends: {}\n", deps.join(", ")));
    }

    // Add installed-size (in KB)
    let installed_size = result.total_size / 1024;
    control.push_str(&format!("Installed-Size: {}\n", installed_size));

    // Homepage
    if let Some(homepage) = &manifest.package.homepage {
        control.push_str(&format!("Homepage: {}\n", homepage));
    }

    // Write control file
    fs::write(control_dir.join("control"), &control)?;

    // Write conffiles if any
    let conffiles: Vec<_> = manifest.config.files.iter().map(String::as_str).collect();
    if !conffiles.is_empty() {
        fs::write(control_dir.join("conffiles"), conffiles.join("\n") + "\n")?;
    }

    // Generate and write maintainer scripts
    let hook_converter = DebHookConverter;

    if let Some(script) = hook_converter.pre_install(&manifest.hooks) {
        fs::write(control_dir.join("preinst"), &script)?;
        set_executable(&control_dir.join("preinst"))?;
    }

    if let Some(script) = hook_converter.post_install(&manifest.hooks) {
        fs::write(control_dir.join("postinst"), &script)?;
        set_executable(&control_dir.join("postinst"))?;
    }

    if let Some(script) = hook_converter.pre_remove(&manifest.hooks) {
        fs::write(control_dir.join("prerm"), &script)?;
        set_executable(&control_dir.join("prerm"))?;
    }

    if let Some(script) = hook_converter.post_remove(&manifest.hooks) {
        fs::write(control_dir.join("postrm"), &script)?;
        set_executable(&control_dir.join("postrm"))?;
    }

    // Note conversion limitations
    if !manifest.hooks.alternatives.is_empty() {
        loss_report.add_hook_note("Alternatives hooks need manual update-alternatives integration");
    }

    // Write data files
    let mut md5sums = Vec::new();
    for file in &result.files {
        if file.file_type == FileType::Directory {
            continue;
        }

        // Create parent directories
        let dest_path = data_dir.join(file.path.trim_start_matches('/'));
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

                    // Add to md5sums (path without leading /)
                    let rel_path = file.path.trim_start_matches('/');
                    md5sums.push(format!("{}  {}", compute_md5(content), rel_path));
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

    // Write md5sums
    if !md5sums.is_empty() {
        fs::write(control_dir.join("md5sums"), md5sums.join("\n") + "\n")?;
    }

    // Create control.tar.gz
    let control_tar_path = temp_dir.path().join("control.tar.gz");
    create_tarball(&control_dir, &control_tar_path)?;

    // Create data.tar.gz
    let data_tar_path = temp_dir.path().join("data.tar.gz");
    create_tarball(&data_dir, &data_tar_path)?;

    // Create debian-binary
    let debian_binary_path = temp_dir.path().join("debian-binary");
    fs::write(&debian_binary_path, "2.0\n")?;

    // Create the ar archive
    create_deb_archive(
        output_path,
        &debian_binary_path,
        &control_tar_path,
        &data_tar_path,
    )?;

    // Note features that don't map to DEB
    loss_report.add_unsupported("Component-based installation (DEB installs all components)");
    loss_report.add_unsupported("Merkle tree verification");
    loss_report.add_unsupported("Content-addressable storage deduplication");

    let size = fs::metadata(output_path)?.len();

    Ok(GenerationResult { size, loss_report })
}

/// Create a gzipped tarball of a directory
fn create_tarball(source_dir: &Path, output_path: &Path) -> Result<()> {
    let file = File::create(output_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = TarBuilder::new(encoder);

    // Add files from directory
    for entry in fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap();

        if path.is_file() {
            archive.append_path_with_name(&path, name)?;
        } else if path.is_dir() {
            archive.append_dir_all(name, &path)?;
        }
    }

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    Ok(())
}

/// Create the final .deb ar archive
fn create_deb_archive(
    output_path: &Path,
    debian_binary: &Path,
    control_tar: &Path,
    data_tar: &Path,
) -> Result<()> {
    let file = File::create(output_path)?;
    let mut archive = ar::Builder::new(file);

    // debian-binary must be first
    archive
        .append_path(debian_binary)
        .context("Failed to add debian-binary")?;

    // control.tar.gz second
    let mut control_file = File::open(control_tar)?;
    archive
        .append_file(b"control.tar.gz", &mut control_file)
        .context("Failed to add control.tar.gz")?;

    // data.tar.gz third
    let mut data_file = File::open(data_tar)?;
    archive
        .append_file(b"data.tar.gz", &mut data_file)
        .context("Failed to add data.tar.gz")?;

    Ok(())
}

/// Set file as executable
fn set_executable(path: &Path) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

/// Compute MD5 hash of content (for md5sums file)
fn compute_md5(content: &[u8]) -> String {
    use md5::{Digest, Md5};
    let hash = Md5::digest(content);
    format!("{:x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::CcsManifest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let manifest = CcsManifest::new_minimal("test-package", "1.0.0");
        BuildResult {
            manifest,
            components: HashMap::new(),
            files: vec![],
            blobs: HashMap::new(),
            total_size: 0,
        }
    }

    #[test]
    fn test_deb_generation_empty() {
        let result = create_test_build_result();
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("test.deb");

        let gen_result = generate(&result, &output_path).unwrap();
        assert!(output_path.exists());
        assert!(gen_result.size > 0);
    }

    #[test]
    fn test_hook_converter_user_creation() {
        let mut hooks = Hooks::default();
        hooks.users.push(crate::ccs::manifest::UserHook {
            name: "myapp".to_string(),
            system: true,
            home: Some("/var/lib/myapp".to_string()),
            shell: None,
            group: None,
        });

        let converter = DebHookConverter;
        let script = converter.pre_install(&hooks).unwrap();
        assert!(script.contains("useradd"));
        assert!(script.contains("myapp"));
        assert!(script.contains("--system"));
    }

    #[test]
    fn test_compute_md5() {
        // Known MD5 hash of "hello world\n"
        let content = b"hello world\n";
        let hash = compute_md5(content);
        assert_eq!(hash, "6f5902ac237024bdd0c176cb93063dc4");

        // Empty content
        let empty_hash = compute_md5(b"");
        assert_eq!(empty_hash, "d41d8cd98f00b204e9800998ecf8427e");
    }
}
