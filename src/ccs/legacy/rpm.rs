// src/ccs/legacy/rpm.rs
//! RPM package generator
//!
//! Generates RPM packages from CCS build results using the `rpm` crate's
//! PackageBuilder for programmatic RPM creation.

use super::{
    arch_for_format, map_capability_to_package, CommonHookGenerator, GenerationResult,
    HookConverter, LossReport,
};
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::manifest::Hooks;
use anyhow::{Context, Result};
use rpm::PackageBuilder;
use std::fs;
use std::path::Path;

/// RPM-specific hook converter
struct RpmHookConverter;

impl HookConverter for RpmHookConverter {
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

        // Systemd commands
        lines.extend(CommonHookGenerator::systemd_commands(hooks, true));

        // tmpfiles
        lines.extend(CommonHookGenerator::tmpfiles_commands(hooks));

        // sysctl
        lines.extend(CommonHookGenerator::sysctl_commands(hooks));

        // ldconfig
        lines.push("/sbin/ldconfig".to_string());

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn pre_remove(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = Vec::new();

        // Stop services before removal
        lines.extend(CommonHookGenerator::systemd_commands(hooks, false));

        if lines.is_empty() {
            return None;
        }

        Some(lines.join("\n"))
    }

    fn post_remove(&self, _hooks: &Hooks) -> Option<String> {
        // ldconfig
        Some("/sbin/ldconfig".to_string())
    }
}

/// Generate an RPM package from a CCS build result
pub fn generate(result: &BuildResult, output_path: &Path) -> Result<GenerationResult> {
    let mut loss_report = LossReport::default();

    // Create temp directory for building
    let temp_dir = tempfile::tempdir()?;

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
        "rpm",
    );

    // License (default to MIT if not specified)
    let license = manifest
        .package
        .license
        .as_deref()
        .unwrap_or("Unspecified");

    // Start building the RPM
    let mut builder = PackageBuilder::new(name, version, license, &arch, description);

    // Set compression (use Gzip for compatibility)
    builder = builder.compression(rpm::CompressionType::Gzip);

    // Add URL if present
    if let Some(url) = &manifest.package.homepage {
        builder = builder.url(url);
    }

    // Add group from legacy overrides
    if let Some(legacy) = &manifest.legacy
        && let Some(rpm_legacy) = &legacy.rpm
    {
        if let Some(group) = &rpm_legacy.group {
            builder = builder.group(group);
        }

        // Add explicit requires
        for req in &rpm_legacy.requires {
            builder = builder.requires(rpm::Dependency::any(req));
        }

        // Add explicit provides
        for prov in &rpm_legacy.provides {
            builder = builder.provides(rpm::Dependency::any(prov));
        }
    }

    // Map capabilities to RPM dependencies
    for cap in &manifest.requires.capabilities {
        let cap_name = cap.name();
        if let Some(pkg) = map_capability_to_package(cap_name, "rpm") {
            if let Some(ver) = cap.version() {
                // Parse version constraint
                let dep = parse_rpm_dependency(&pkg, ver);
                builder = builder.requires(dep);
            } else {
                builder = builder.requires(rpm::Dependency::any(&pkg));
            }
        } else {
            loss_report.add_dependency_note(&format!(
                "Capability '{}' has no known RPM mapping",
                cap_name
            ));
        }
    }

    // Add package dependencies
    for pkg_dep in &manifest.requires.packages {
        if let Some(ver) = &pkg_dep.version {
            let dep = parse_rpm_dependency(&pkg_dep.name, ver);
            builder = builder.requires(dep);
        } else {
            builder = builder.requires(rpm::Dependency::any(&pkg_dep.name));
        }
    }

    // Write files to temp dir and add to RPM
    for file in &result.files {
        if file.file_type == FileType::Directory {
            continue;
        }

        match file.file_type {
            FileType::Regular => {
                if let Some(content) = result.blobs.get(&file.hash) {
                    // Write to temp location
                    let temp_path = temp_dir.path().join(file.hash.clone());
                    fs::write(&temp_path, content)?;

                    // Determine file options
                    let options = rpm::FileOptions::new(&file.path)
                        .mode(rpm::FileMode::from(file.mode as i32));

                    // Check if config file
                    let options = if manifest.config.files.contains(&file.path) {
                        if manifest.config.noreplace {
                            options.is_config_noreplace()
                        } else {
                            options.is_config()
                        }
                    } else {
                        options
                    };

                    builder = builder
                        .with_file(&temp_path, options)
                        .context(format!("Failed to add file: {}", file.path))?;
                }
            }
            FileType::Symlink => {
                if let Some(target) = &file.target {
                    // RPM handles symlinks differently - create in temp and add
                    let temp_path = temp_dir.path().join(format!("link_{}", file.hash));
                    #[cfg(unix)]
                    {
                        let _ = fs::remove_file(&temp_path);
                        std::os::unix::fs::symlink(target, &temp_path)?;
                    }

                    let options = rpm::FileOptions::new(&file.path);
                    builder = builder
                        .with_file(&temp_path, options)
                        .context(format!("Failed to add symlink: {}", file.path))?;
                }
            }
            FileType::Directory => {}
        }
    }

    // Add scriptlets
    let hook_converter = RpmHookConverter;

    if let Some(script) = hook_converter.pre_install(&manifest.hooks) {
        builder = builder.pre_install_script(script);
    }

    if let Some(script) = hook_converter.post_install(&manifest.hooks) {
        builder = builder.post_install_script(script);
    }

    if let Some(script) = hook_converter.pre_remove(&manifest.hooks) {
        builder = builder.pre_uninstall_script(script);
    }

    if let Some(script) = hook_converter.post_remove(&manifest.hooks) {
        builder = builder.post_uninstall_script(script);
    }

    // Note conversion limitations
    if !manifest.hooks.alternatives.is_empty() {
        loss_report.add_hook_note("Alternatives hooks need manual alternatives integration");
    }

    // Build the package (unsigned)
    let package = builder.build().context("Failed to build RPM package")?;

    // Write to output path
    let mut output_file = fs::File::create(output_path)?;
    package
        .write(&mut output_file)
        .context("Failed to write RPM")?;

    // Note features that don't map to RPM
    loss_report.add_unsupported("Component-based installation (RPM installs all components)");
    loss_report.add_unsupported("Merkle tree verification");
    loss_report.add_unsupported("Content-addressable storage deduplication");

    let size = fs::metadata(output_path)?.len();

    Ok(GenerationResult { size, loss_report })
}

/// Parse a version constraint string into an RPM Dependency
fn parse_rpm_dependency(name: &str, version_constraint: &str) -> rpm::Dependency {
    // Parse constraint like ">=1.0" or "=2.0" or "<3.0"
    let trimmed = version_constraint.trim();

    if let Some(ver) = trimmed.strip_prefix(">=") {
        rpm::Dependency::greater_eq(name, ver.trim())
    } else if let Some(ver) = trimmed.strip_prefix("<=") {
        rpm::Dependency::less_eq(name, ver.trim())
    } else if let Some(ver) = trimmed.strip_prefix('>') {
        rpm::Dependency::greater(name, ver.trim())
    } else if let Some(ver) = trimmed.strip_prefix('<') {
        rpm::Dependency::less(name, ver.trim())
    } else if let Some(ver) = trimmed.strip_prefix('=') {
        rpm::Dependency::eq(name, ver.trim())
    } else {
        // No operator, assume exact version
        rpm::Dependency::eq(name, trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::CcsManifest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let manifest = CcsManifest::new_minimal("test-rpm-package", "1.0.0");
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
    fn test_rpm_generation_empty() {
        let result = create_test_build_result();
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("test.rpm");

        let gen_result = generate(&result, &output_path).unwrap();
        assert!(output_path.exists());
        assert!(gen_result.size > 0);
    }

    #[test]
    fn test_parse_rpm_dependency() {
        let dep = parse_rpm_dependency("foo", ">=1.0");
        // Just verify it doesn't panic
        let _ = format!("{:?}", dep);

        let dep = parse_rpm_dependency("bar", "<2.0");
        let _ = format!("{:?}", dep);
    }

    #[test]
    fn test_hook_converter_post_install() {
        let mut hooks = Hooks::default();
        hooks.systemd.push(crate::ccs::manifest::SystemdHook {
            unit: "myapp.service".to_string(),
            enable: true,
        });

        let converter = RpmHookConverter;
        let script = converter.post_install(&hooks).unwrap();
        assert!(script.contains("systemctl"));
        assert!(script.contains("myapp.service"));
    }
}
