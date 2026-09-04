// conary-core/src/ccs/native_export/rpm.rs
//! RPM package generator
//!
//! Generates RPM packages from CCS build results using the `rpm` crate's
//! PackageBuilder for programmatic RPM creation.

use super::{CommonHookGenerator, GenerationResult, HookConverter, LossReport, arch_for_format};
use crate::ccs::builder::BuildResult;
use crate::ccs::manifest::{Hooks, RpmRelation, RpmRelationOperator};
use crate::payload::PayloadNodeKind;
use anyhow::{Context, Result};
use rpm::PackageBuilder;
use std::collections::HashSet;
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

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
    let hardlinks = super::hardlinks::Topology::validate(result)?;

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
        for requirement in &rpm_export.requires {
            builder.requires(rpm_relation(requirement, "requires")?);
        }

        // Add explicit provides
        for provide in &rpm_export.provides {
            builder.provides(rpm_relation(provide, "provides")?);
        }
    }

    if !manifest.requirements.is_empty() {
        loss_report.add_dependency_note(
            "Typed CCS requirements are not rewritten into RPM syntax; declare exact native_export.rpm.requires entries",
        );
    }

    let mut implicit_parent_dirs = HashSet::<PathBuf>::new();
    for file in hardlinks.ordered_files() {
        let mut parent = Path::new(&file.path).parent();
        while let Some(path) = parent {
            implicit_parent_dirs.insert(path.to_path_buf());
            parent = path.parent();
        }
    }

    // Write files to temp dir and add to RPM.
    for file in hardlinks.ordered_files() {
        validate_rpm_node_xattrs(&result.manifest, &file.path, &file.node)?;
        if matches!(&file.node.kind, PayloadNodeKind::Directory) {
            let has_descendant = implicit_parent_dirs.contains(Path::new(&file.path));
            let has_default_parent_metadata = file.node.mode & 0o7777 == 0o755;
            if has_descendant && has_default_parent_metadata {
                // RPM creates default-mode parent directories for descendant
                // entries. Do not claim shared paths such as /usr/bin, which
                // belong to the target's filesystem package.
                continue;
            }
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

                if matches!(
                    file.node.kind,
                    PayloadNodeKind::Regular {
                        hardlink_identity: Some(_)
                    }
                ) {
                    set_hardlink_source_mtime(&temp_path, &file.node)?;
                }

                // Determine file options
                let options =
                    rpm::FileOptions::new(&file.path).permissions((file.node.mode & 0o7777) as u16);
                let options = rpm_file_security_options(options, &result.manifest, &file.path)?;
                let options = match &file.node.kind {
                    PayloadNodeKind::Regular {
                        hardlink_identity: Some(identity),
                    } => hardlink_file_options(options.hardlink(identity), &file.node)?,
                    _ => options,
                };

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
            PayloadNodeKind::Directory => {
                let options =
                    rpm::FileOptions::dir(&file.path).permissions((file.node.mode & 0o7777) as u16);
                let options = rpm_node_ownership(options, &file.node)?;
                builder
                    .with_dir_entry(options)
                    .with_context(|| format!("Failed to add directory: {}", file.path))?;
            }
            PayloadNodeKind::Hardlink { identity, .. } => {
                let anchor = hardlinks.anchor(identity);
                let content_hash = &anchor
                    .content
                    .as_ref()
                    .expect("validated hardlink anchor has regular content authority")
                    .sha256;
                let temp_path = temp_dir.path().join(content_hash);
                let options = rpm::FileOptions::new(&file.path)
                    .permissions((file.node.mode & 0o7777) as u16)
                    .hardlink(identity);
                let options = hardlink_file_options(options, &file.node)?;
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
                    .with_context(|| format!("Failed to add hardlink: {}", file.path))?;
            }
            other => anyhow::bail!(
                "RPM generator does not yet encode {} node {}",
                payload_kind_name(other),
                file.path
            ),
        }
    }
    for config in manifest.config.files.iter().filter(|config| {
        !config.remove_on_upgrade()
            && config.payload()
                == crate::packages::config_authority::ConfigPayloadAssociation::Absent
    }) {
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

fn rpm_relation(declaration: &RpmRelation, field: &str) -> Result<rpm::Dependency> {
    let name = declaration.name.trim();
    if name.is_empty() {
        anyhow::bail!("RPM native_export.rpm.{field} name must not be empty");
    }

    let version = || {
        declaration
            .version
            .as_deref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "RPM native_export.rpm.{field} relation '{name}' requires a version"
                )
            })
            .and_then(|value| rpm_relation_version(field, name, value))
    };
    Ok(match declaration.relation {
        RpmRelationOperator::Any => {
            if declaration.version.is_some() {
                anyhow::bail!(
                    "RPM native_export.rpm.{field} unversioned relation '{name}' must not declare a version"
                );
            }
            rpm::Dependency::any(name)
        }
        RpmRelationOperator::Equal => rpm::Dependency::eq(name, version()?),
        RpmRelationOperator::Less => rpm::Dependency::less(name, version()?),
        RpmRelationOperator::LessOrEqual => rpm::Dependency::less_eq(name, version()?),
        RpmRelationOperator::Greater => rpm::Dependency::greater(name, version()?),
        RpmRelationOperator::GreaterOrEqual => rpm::Dependency::greater_eq(name, version()?),
    })
}

fn rpm_relation_version<'a>(field: &str, name: &str, value: &'a str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("RPM native_export.rpm.{field} relation '{name}' has an empty version");
    }
    Ok(value)
}

fn hardlink_file_options(
    options: rpm::FileOptionsBuilder,
    node: &crate::payload::PayloadNode,
) -> Result<rpm::FileOptionsBuilder> {
    if !node.xattrs.is_empty() {
        anyhow::bail!("RPM hardlink export cannot represent xattr authority exactly");
    }
    rpm_node_ownership(options, node)
}

fn rpm_file_security_options(
    mut options: rpm::FileOptionsBuilder,
    manifest: &crate::ccs::manifest::CcsManifest,
    path: &str,
) -> Result<rpm::FileOptionsBuilder> {
    let declaration = manifest
        .file_capabilities
        .iter()
        .find(|capability| capability.path == path);

    if let Some(capability) = declaration {
        options = options.caps(capability.to_setcap_spec()?)?;
    }

    Ok(options)
}

fn validate_rpm_node_xattrs(
    manifest: &crate::ccs::manifest::CcsManifest,
    path: &str,
    node: &crate::payload::PayloadNode,
) -> Result<()> {
    let declaration = manifest
        .file_capabilities
        .iter()
        .find(|capability| capability.path == path);
    let mut remaining_xattrs = node.xattrs.clone();

    if let Some(capability) = declaration {
        let expected = crate::generation::builder::encode_security_capability_xattr(capability)?;
        if let Some(observed) =
            remaining_xattrs.remove(crate::generation::builder::SECURITY_CAPABILITY_XATTR)
            && observed != expected
        {
            anyhow::bail!(
                "RPM native export file capability declaration disagrees with payload xattr at {path}"
            );
        }
    } else if remaining_xattrs.contains_key(crate::generation::builder::SECURITY_CAPABILITY_XATTR) {
        anyhow::bail!(
            "RPM native export requires a typed file_capabilities declaration for {path}"
        );
    }

    if !remaining_xattrs.is_empty() {
        anyhow::bail!(
            "RPM native export cannot represent payload xattrs exactly at {path}: {:?}",
            remaining_xattrs.keys().collect::<Vec<_>>()
        );
    }

    Ok(())
}

fn rpm_node_ownership(
    options: rpm::FileOptionsBuilder,
    node: &crate::payload::PayloadNode,
) -> Result<rpm::FileOptionsBuilder> {
    let user = rpm_identity(&node.user, "user")?;
    let group = rpm_identity(&node.group, "group")?;
    Ok(options.user(user).group(group))
}

fn rpm_identity(identity: &crate::payload::PayloadIdentity, field: &str) -> Result<String> {
    match identity {
        crate::payload::PayloadIdentity::Named { name } => Ok(name.clone()),
        crate::payload::PayloadIdentity::Numeric { id: 0 } => Ok("root".to_string()),
        crate::payload::PayloadIdentity::Numeric { id } => anyhow::bail!(
            "RPM hardlink export cannot represent numeric {field} identity {id} exactly"
        ),
    }
}

fn set_hardlink_source_mtime(path: &Path, node: &crate::payload::PayloadNode) -> Result<()> {
    if node.mtime.nanoseconds != 0 {
        anyhow::bail!("RPM hardlink export requires whole-second mtime authority");
    }
    let seconds = u32::try_from(node.mtime.seconds)
        .context("RPM hardlink mtime is outside its unsigned 32-bit authority")?;
    let modified = UNIX_EPOCH
        .checked_add(Duration::from_secs(u64::from(seconds)))
        .context("RPM hardlink mtime is outside host filesystem authority")?;
    fs::File::options()
        .write(true)
        .open(path)?
        .set_times(FileTimes::new().set_modified(modified))?;
    Ok(())
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
mod tests;
