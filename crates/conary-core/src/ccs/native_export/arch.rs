// conary-core/src/ccs/native_export/arch.rs
//! Arch Linux package generator
//!
//! Generates Arch .pkg.tar.zst packages from CCS build results.
//! Arch packages are zstd-compressed tarballs containing:
//! - .PKGINFO: package metadata
//! - .INSTALL: optional install/upgrade hooks
//! - Actual files at their install paths

use super::{CommonHookGenerator, GenerationResult, HookConverter, LossReport, arch_for_format};
use crate::ccs::builder::BuildResult;
use crate::ccs::manifest::Hooks;
use crate::payload::PayloadNodeKind;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{self, Write};
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
        if let Some(hook) = &hooks.post_install {
            lines.push(hook.script.clone());
        }

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn pre_remove(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = Vec::new();

        // Stop services
        lines.extend(CommonHookGenerator::systemd_commands(hooks, false));
        if let Some(hook) = &hooks.pre_remove {
            lines.push(hook.script.clone());
        }

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
    let release = &manifest.package.release;
    let description = &manifest.package.description;
    let arch = arch_for_format(
        manifest
            .package
            .platform
            .as_ref()
            .and_then(|p| p.arch.as_deref()),
        "arch",
    )?;

    // Calculate installed size
    let installed_size = result.total_size;

    // Build .PKGINFO
    let mut pkginfo = String::new();
    pkginfo.push_str(&format!("pkgname = {}\n", name));
    pkginfo.push_str(&format!("pkgbase = {}\n", name));

    // Arch version format: version-pkgrel, with the required manifest release.
    let arch_version = version.replace('-', "_");
    if arch_version != version.as_str() {
        loss_report.add_unsupported(
            "Arch pkgver grammar replaces '-' in the CCS version with '_' during native export",
        );
    }
    let pkgver = format!("{arch_version}-{release}");
    pkginfo.push_str(&format!("pkgver = {}\n", pkgver));
    pkginfo.push_str(&format!("pkgdesc = {}\n", description));
    pkginfo.push_str(&format!("arch = {}\n", arch));
    pkginfo.push_str(&format!("size = {}\n", installed_size));

    // ALPM PKGINFO v1 requires URL and packager. These values must come from
    // exact CCS metadata rather than invented placeholders.
    let url = manifest
        .package
        .homepage
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Arch export requires exact package.homepage metadata for the PKGINFO url field"
            )
        })?;
    pkginfo.push_str(&format!("url = {url}\n"));

    // License
    if let Some(license) = &manifest.package.license {
        pkginfo.push_str(&format!("license = {}\n", license));
    }

    // Apply Arch-specific native export overrides.
    if let Some(native_export) = &manifest.native_export
        && let Some(arch_export) = &native_export.arch
    {
        for group in &arch_export.groups {
            pkginfo.push_str(&format!("group = {}\n", group));
        }
        for dependency in &arch_export.depends {
            pkginfo.push_str(&format!("depend = {}\n", dependency));
        }
        for provide in &arch_export.provides {
            pkginfo.push_str(&format!("provides = {}\n", provide));
        }
    }

    if !manifest.requirements.is_empty() {
        loss_report.add_dependency_note(
            "Typed CCS requirements are not rewritten into Arch syntax; declare exact native_export.arch.depends entries",
        );
    }

    // Optional dependencies (suggests)
    for cap in &manifest.suggests.capabilities {
        pkginfo.push_str(&format!("optdepend = {}\n", cap));
    }

    // Build timestamp
    let builddate = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock predates the Unix epoch; cannot write exact Arch builddate")?
        .as_secs();
    pkginfo.push_str(&format!("builddate = {}\n", builddate));

    // Packager
    let packager = manifest
        .package
        .authors
        .as_ref()
        .and_then(|a| a.maintainers.first())
        .map(String::as_str)
        .filter(|packager| !packager.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Arch export requires at least one exact package.authors.maintainers entry for the PKGINFO packager field"
            )
        })?;
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
        // Use safe_join to prevent path traversal from untrusted package paths
        let dest_path = crate::filesystem::safe_join(&pkg_root, &file.path)
            .with_context(|| format!("Unsafe file path: {}", file.path))?;
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        match &file.node.kind {
            PayloadNodeKind::Regular { .. } => {
                super::copy_file_content(result, file, &dest_path)?;

                // Set permissions
                let mut perms = fs::metadata(&dest_path)?.permissions();
                perms.set_mode(file.node.mode & 0o7777);
                fs::set_permissions(&dest_path, perms)?;
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
                "Arch generator does not yet encode {:?} node {}",
                other,
                file.path
            ),
        }
    }

    // Write backup array for config files
    if !manifest.config.files.is_empty() {
        let backup: Vec<_> = manifest
            .config
            .files
            .iter()
            .map(|config| {
                if !config.noreplace() || config.ghost() || config.remove_on_upgrade() {
                    anyhow::bail!(
                        "Arch export cannot represent config semantics for {}",
                        config.path()
                    );
                }
                Ok(config.path().trim_start_matches('/'))
            })
            .collect::<Result<Vec<_>>>()?;

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

/// Write a shell function block: `name() {\n    body\n}\n\n`
fn write_shell_function(script: &mut String, name: &str, body: &str) {
    script.push_str(&format!("{}() {{\n", name));
    for line in body.lines() {
        script.push_str(&format!("    {}\n", line));
    }
    script.push_str("}\n\n");
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
        write_shell_function(&mut script, "pre_install", &content);
        write_shell_function(&mut script, "pre_upgrade", &content);
    }

    if let Some(content) = post_install {
        write_shell_function(&mut script, "post_install", &content);
        write_shell_function(&mut script, "post_upgrade", &content);
    }

    if let Some(content) = pre_remove {
        write_shell_function(&mut script, "pre_remove", &content);
    }

    if let Some(content) = post_remove {
        write_shell_function(&mut script, "post_remove", &content);
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
        archive.follow_symlinks(false);

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

    // Compress from disk so native export does not re-buffer the full tarball.
    let mut tar_file = File::open(&tar_path)?;
    let output_file = File::create(output_path)?;
    let mut encoder = zstd::Encoder::new(output_file, 19)?; // Level 19 for high compression
    io::copy(&mut tar_file, &mut encoder)?;
    encoder.finish()?;
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
    use crate::ccs::manifest::{ArchExport, CcsManifest, NativeExport};
    use crate::packages::PackageFormat;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let mut manifest = CcsManifest::new_minimal("test-arch-package", "1.0.0");
        manifest.package.homepage = Some("https://example.invalid/test-arch-package".to_string());
        manifest.package.authors = Some(crate::ccs::manifest::Authors {
            maintainers: vec!["Test Packager <packager@example.invalid>".to_string()],
            upstream: None,
        });
        manifest.package.platform = Some(crate::ccs::manifest::Platform {
            arch: Some("x86_64".to_string()),
            ..Default::default()
        });
        BuildResult {
            manifest,
            components: HashMap::new(),
            files: vec![],
            payloads: Vec::new(),
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
    fn arch_native_export_relations_round_trip_through_the_parser() {
        let mut result = create_test_build_result();
        result.manifest.native_export = Some(NativeExport {
            arch: Some(ArchExport {
                depends: vec!["bash".to_string()],
                provides: vec!["virtual-test-tool".to_string()],
                ..ArchExport::default()
            }),
            ..NativeExport::default()
        });
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("relations.pkg.tar.zst");

        generate(&result, &output_path).unwrap();
        let package = crate::packages::arch::ArchPackage::parse(output_path.to_str().unwrap())
            .expect("parse generated Arch package");

        assert_eq!(package.requirements().len(), 1);
        assert!(
            package
                .resolution_capabilities()
                .unwrap()
                .iter()
                .any(|provide| provide.name == "virtual-test-tool")
        );
    }

    #[test]
    fn arch_generation_rejects_missing_required_metadata() {
        let mut result = create_test_build_result();
        result.manifest.package.homepage = None;
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("missing-url.pkg.tar.zst");
        let error = generate(&result, &output_path).unwrap_err();
        assert!(error.to_string().contains("exact package.homepage"));

        let mut result = create_test_build_result();
        result.manifest.package.authors = None;
        let output_path = temp_dir.path().join("missing-packager.pkg.tar.zst");
        let error = generate(&result, &output_path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exact package.authors.maintainers")
        );
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
            reversible: None,
        });
        hooks.systemd.push(crate::ccs::manifest::SystemdHook {
            unit: "myapp.service".to_string(),
            enable: true,
            reversible: None,
        });

        let converter = ArchHookConverter;
        let script = generate_install_script(&converter, &hooks).unwrap();

        assert!(script.contains("pre_install()"));
        assert!(script.contains("post_install()"));
        assert!(script.contains("useradd"));
        assert!(script.contains("systemctl"));
    }

    #[test]
    fn test_install_script_preserves_script_hooks() {
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

        let converter = ArchHookConverter;
        let script = generate_install_script(&converter, &hooks).unwrap();

        assert!(script.contains("post_install()"));
        assert!(script.contains("pre_remove()"));
        assert!(script.contains("echo installed > /var/lib/myapp/installed"));
        assert!(script.contains("echo removed > /var/lib/myapp/removed"));
    }

    #[test]
    fn test_pkgver_format() {
        // Arch version shouldn't contain hyphens in the version part
        let version = "1.0.0-beta";
        let release = "7";
        let pkgver = format!("{}-{release}", version.replace('-', "_"));
        assert_eq!(pkgver, "1.0.0_beta-7");
    }
}
