// conary-core/src/ccs/native_export/rpm.rs
//! RPM package generator
//!
//! Generates RPM packages from CCS build results using the `rpm` crate's
//! PackageBuilder for programmatic RPM creation.

use super::{CommonHookGenerator, GenerationResult, HookConverter, LossReport, arch_for_format};
use crate::ccs::builder::BuildResult;
use crate::ccs::manifest::Hooks;
use crate::payload::PayloadNodeKind;
use anyhow::{Context, Result};
use rpm::PackageBuilder;
use std::fs;
use std::path::Path;

/// RPM-specific hook converter
struct RpmHookConverter;

impl HookConverter for RpmHookConverter {
    fn pre_install(&self, hooks: &Hooks) -> Option<String> {
        let mut lines = vec!["#!/bin/sh".to_string(), "set -e".to_string()];

        // Create groups and users
        lines.extend(CommonHookGenerator::user_creation_commands(hooks));

        if lines.len() <= 2 {
            return None;
        }

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

        if lines.len() <= 2 {
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

        Some(lines.join("\n"))
    }

    fn post_remove(&self, _hooks: &Hooks) -> Option<String> {
        None
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
    let release = &manifest.package.release;
    let description = &manifest.package.description;
    let arch = arch_for_format(
        manifest
            .package
            .platform
            .as_ref()
            .and_then(|p| p.arch.as_deref()),
        "rpm",
    )?;

    // RPM carries a package-content License tag. Inventing a token here would
    // turn missing CCS authority into false package metadata.
    let license = manifest
        .package
        .license
        .as_deref()
        .filter(|license| !license.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "RPM export requires exact package.license metadata; no implicit license token is allowed"
            )
        })?;

    // Start building the RPM
    let mut builder = PackageBuilder::new(name, version, license, &arch, description);
    builder.release(release);

    // rpm 0.19+ uses gzip compression by default

    // Add URL if present
    if let Some(url) = &manifest.package.homepage {
        builder.url(url);
    }

    // Apply RPM-specific native export overrides.
    if let Some(native_export) = &manifest.native_export
        && let Some(rpm_export) = &native_export.rpm
    {
        if let Some(group) = &rpm_export.group {
            builder.group(group);
        }

        // Add explicit requires
        for req in &rpm_export.requires {
            builder.requires(rpm::Dependency::any(req));
        }

        // Add explicit provides
        for prov in &rpm_export.provides {
            builder.provides(rpm::Dependency::any(prov));
        }
    }

    if !manifest.requirements.is_empty() {
        loss_report.add_dependency_note(
            "Typed CCS requirements are not rewritten into RPM syntax; declare exact native_export.rpm.requires entries",
        );
    }

    // Write files to temp dir and add to RPM
    for file in &result.files {
        if matches!(&file.node.kind, PayloadNodeKind::Directory) {
            continue;
        }

        match &file.node.kind {
            PayloadNodeKind::Regular { .. } => {
                // Write to temp location
                let content_hash = &file
                    .content
                    .as_ref()
                    .expect("regular node content validated by CCS builder")
                    .sha256;
                let temp_path = temp_dir.path().join(content_hash);
                super::copy_file_content(result, file, &temp_path)?;

                // Determine file options
                let options =
                    rpm::FileOptions::new(&file.path).permissions((file.node.mode & 0o7777) as u16);

                // Check if config file
                let options = if let Some(config) = manifest
                    .config
                    .files
                    .iter()
                    .find(|config| config.path() == file.path)
                {
                    if config.remove_on_upgrade() || config.ghost() {
                        anyhow::bail!(
                            "RPM payload config {} carries absent-payload semantics",
                            config.path()
                        );
                    }
                    if config.noreplace() {
                        options.config().noreplace()
                    } else {
                        options.config()
                    }
                } else {
                    options
                };

                builder
                    .with_file(&temp_path, options)
                    .context(format!("Failed to add file: {}", file.path))?;
            }
            PayloadNodeKind::Symlink { target } => {
                let options = rpm::FileOptions::symlink(&file.path, target);
                let options = if let Some(config) = manifest
                    .config
                    .files
                    .iter()
                    .find(|config| config.path() == file.path)
                {
                    if config.remove_on_upgrade() || config.ghost() {
                        anyhow::bail!(
                            "RPM payload config {} carries absent-payload semantics",
                            config.path()
                        );
                    }
                    if config.noreplace() {
                        options.config().noreplace()
                    } else {
                        options.config()
                    }
                } else {
                    options
                };
                builder
                    .with_symlink(options)
                    .context(format!("Failed to add symlink: {}", file.path))?;
            }
            PayloadNodeKind::Directory => unreachable!("directories filtered above"),
            other => anyhow::bail!(
                "RPM generator does not yet encode {} node {}",
                payload_kind_name(other),
                file.path
            ),
        }
    }

    for config in manifest.config.files.iter().filter(|config| config.ghost()) {
        if config.remove_on_upgrade() {
            anyhow::bail!(
                "RPM ghost config {} cannot also be remove-on-upgrade",
                config.path()
            );
        }
        let options = rpm::FileOptions::ghost(config.path()).config();
        let options = if config.noreplace() {
            options.noreplace()
        } else {
            options
        };
        builder
            .with_ghost(options)
            .with_context(|| format!("Failed to add ghost config: {}", config.path()))?;
    }
    if let Some(config) = manifest
        .config
        .files
        .iter()
        .find(|config| config.remove_on_upgrade())
    {
        anyhow::bail!(
            "RPM export cannot represent remove-on-upgrade config path {}",
            config.path()
        );
    }

    // Add scriptlets
    let hook_converter = RpmHookConverter;

    if let Some(script) = hook_converter.pre_install(&manifest.hooks) {
        builder.pre_install_script(script);
    }

    if let Some(script) = hook_converter.post_install(&manifest.hooks) {
        builder.post_install_script(script);
    }

    if let Some(script) = hook_converter.pre_remove(&manifest.hooks) {
        builder.pre_uninstall_script(script);
    }

    if let Some(script) = hook_converter.post_remove(&manifest.hooks) {
        builder.post_uninstall_script(script);
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

fn payload_kind_name(kind: &PayloadNodeKind) -> &'static str {
    match kind {
        PayloadNodeKind::Regular { .. } => "regular",
        PayloadNodeKind::Directory => "directory",
        PayloadNodeKind::Symlink { .. } => "symlink",
        PayloadNodeKind::Hardlink { .. } => "hardlink",
        PayloadNodeKind::BlockDevice { .. } => "block-device",
        PayloadNodeKind::CharacterDevice { .. } => "character-device",
        PayloadNodeKind::Fifo => "fifo",
        PayloadNodeKind::Socket => "socket",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::manifest::CcsManifest;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_build_result() -> BuildResult {
        let mut manifest = CcsManifest::new_minimal("test-rpm-package", "1.0.0");
        manifest.package.license = Some("MIT".to_string());
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
    fn test_rpm_generation_empty() {
        let result = create_test_build_result();
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("test.rpm");

        let gen_result = generate(&result, &output_path).unwrap();
        assert!(output_path.exists());
        assert!(gen_result.size > 0);
    }

    #[test]
    fn rpm_generation_rejects_missing_license_authority() {
        let mut result = create_test_build_result();
        result.manifest.package.license = None;
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("missing-license.rpm");

        let error = generate(&result, &output_path).unwrap_err();
        assert!(error.to_string().contains("exact package.license"));
        assert!(!output_path.exists());
    }

    #[test]
    fn test_hook_converter_post_install() {
        let mut hooks = Hooks::default();
        hooks.systemd.push(crate::ccs::manifest::SystemdHook {
            unit: "myapp.service".to_string(),
            enable: true,
            reversible: None,
        });

        let converter = RpmHookConverter;
        let script = converter.post_install(&hooks).unwrap();
        assert!(script.contains("systemctl"));
        assert!(script.contains("myapp.service"));
    }

    #[test]
    fn empty_hooks_do_not_invent_ldconfig_scriptlets() {
        let converter = RpmHookConverter;
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

        let converter = RpmHookConverter;
        let post = converter.post_install(&hooks).unwrap();
        let pre_remove = converter.pre_remove(&hooks).unwrap();

        assert!(post.contains("echo installed > /var/lib/myapp/installed"));
        assert!(pre_remove.contains("echo removed > /var/lib/myapp/removed"));
    }
}
