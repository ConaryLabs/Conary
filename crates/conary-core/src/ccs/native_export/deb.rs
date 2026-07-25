// conary-core/src/ccs/native_export/deb.rs
//! DEB package generator
//!
//! Generates Debian .deb packages from CCS build results.
//! DEB packages are ar archives containing:
//! - debian-binary: version string "2.0\n"
//! - control.tar.gz: package metadata and scripts
//! - data.tar.gz: actual file contents

use super::{CommonHookGenerator, GenerationResult, HookConverter, LossReport, arch_for_format};
use crate::ccs::builder::BuildResult;
use crate::ccs::manifest::Hooks;
use crate::payload::PayloadNodeKind;
use anyhow::{Context, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
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

        lines.extend(CommonHookGenerator::directory_commands(hooks));
        lines.extend(CommonHookGenerator::systemd_commands(hooks, true));
        lines.extend(CommonHookGenerator::tmpfiles_commands(hooks));
        lines.extend(CommonHookGenerator::sysctl_commands(hooks));
        if let Some(hook) = &hooks.post_install {
            lines.push(hook.script.clone());
        }

        lines.push("exit 0".to_string());
        if lines.len() <= 3 {
            return None;
        }
        Some(lines.join("\n"))
    }

    fn pre_remove(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // Stop services before removal
        lines.extend(CommonHookGenerator::systemd_commands(hooks, false));
        if let Some(hook) = &hooks.pre_remove {
            lines.push(hook.script.clone());
        }

        if lines.len() <= 2 {
            return None;
        }

        lines.push("exit 0".to_string());
        Some(lines.join("\n"))
    }

    fn post_remove(&self, _hooks: &Hooks) -> Option<String> {
        None
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
    let release = &manifest.package.release;
    let description = &manifest.package.description;
    let arch = arch_for_format(
        manifest
            .package
            .platform
            .as_ref()
            .and_then(|p| p.arch.as_deref()),
        "deb",
    )?;

    let package_version = format!("{version}-{release}");

    // Build control file
    let mut control = format!(
        "Package: {}\n\
         Version: {}\n\
         Architecture: {}\n\
         Description: {}\n",
        name, package_version, arch, description
    );
    // Debian marks Maintainer as recommended rather than required. Preserve an
    // exact CCS maintainer when supplied and omit the field otherwise.
    if let Some(maintainer) = manifest
        .package
        .authors
        .as_ref()
        .and_then(|authors| authors.maintainers.first())
        .filter(|maintainer| !maintainer.trim().is_empty())
    {
        control.push_str(&format!("Maintainer: {maintainer}\n"));
    }

    // Apply Debian-specific native export overrides.
    if let Some(native_export) = &manifest.native_export
        && let Some(deb) = &native_export.deb
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

    if !manifest.requirements.is_empty() {
        loss_report.add_dependency_note(
            "Typed CCS requirements are not rewritten into Debian syntax; declare exact native_export.deb.depends entries",
        );
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
        // Use safe_join to prevent path traversal from untrusted package paths
        let dest_path = crate::filesystem::safe_join(&data_dir, &file.path)
            .with_context(|| format!("Unsafe file path: {}", file.path))?;
        let rel_path = dest_path
            .strip_prefix(&data_dir)
            .unwrap_or(&dest_path)
            .to_string_lossy();
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        match &file.node.kind {
            PayloadNodeKind::Regular { .. } => {
                let content = super::get_file_content(file, &result.blobs)?;
                fs::write(&dest_path, &content)?;

                // Set permissions
                let mut perms = fs::metadata(&dest_path)?.permissions();
                perms.set_mode(file.node.mode & 0o7777);
                fs::set_permissions(&dest_path, perms)?;

                // Add to md5sums using the already-computed relative path.
                md5sums.push(format!("{}  {}", compute_md5(&content), rel_path));
            }
            PayloadNodeKind::Symlink { target } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &dest_path)?;
            }
            PayloadNodeKind::Directory => {
                fs::create_dir_all(&dest_path)?;
                let mut perms = fs::metadata(&dest_path)?.permissions();
                perms.set_mode(file.node.mode & 0o7777);
                fs::set_permissions(&dest_path, perms)?;
            }
            other => anyhow::bail!(
                "DEB generator does not yet encode {:?} node {}",
                other,
                file.path
            ),
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

    // debian-binary must be first (use append_file with explicit name to ensure
    // the ar entry is named exactly "debian-binary", not a temp directory path)
    let mut debian_binary_file = File::open(debian_binary)?;
    archive
        .append_file(b"debian-binary", &mut debian_binary_file)
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
    hex::encode(Md5::digest(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::CcsManifest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let mut manifest = CcsManifest::new_minimal("test-package", "1.0.0");
        manifest.package.platform = Some(crate::ccs::manifest::Platform {
            arch: Some("x86_64".to_string()),
            ..Default::default()
        });
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
            reversible: None,
        });

        let converter = DebHookConverter;
        let script = converter.pre_install(&hooks).unwrap();
        assert!(script.contains("useradd"));
        assert!(script.contains("myapp"));
        assert!(script.contains("--system"));
    }

    #[test]
    fn empty_hooks_do_not_invent_ldconfig_scriptlets() {
        let converter = DebHookConverter;
        assert!(converter.post_install(&Hooks::default()).is_none());
        assert!(converter.post_remove(&Hooks::default()).is_none());
    }

    #[test]
    fn test_hook_converter_preserves_script_hooks() {
        let hooks = Hooks {
            post_install: Some(crate::ccs::manifest::ScriptHook {
                script: "echo installed > /var/lib/myapp/installed".to_string(),
                reversible: None,
            }),
            pre_remove: Some(crate::ccs::manifest::ScriptHook {
                script: "echo removed > /var/lib/myapp/removed".to_string(),
                reversible: None,
            }),
            ..Default::default()
        };

        let converter = DebHookConverter;
        let post = converter.post_install(&hooks).unwrap();
        let pre_remove = converter.pre_remove(&hooks).unwrap();

        assert!(post.contains("echo installed > /var/lib/myapp/installed"));
        assert!(pre_remove.contains("echo removed > /var/lib/myapp/removed"));
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
