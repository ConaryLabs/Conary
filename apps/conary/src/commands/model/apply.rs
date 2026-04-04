// src/commands/model/apply.rs

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, DistroPin, Trove, VersionPolicy,
};
use conary_core::derived::{build_from_definition, persist_build_artifact};
use conary_core::filesystem::CasStore;
use conary_core::hash::sha256;
use conary_core::model::parser::SystemModel;
use conary_core::model::{DiffAction, ModelDerivedPackage};
use rusqlite::Connection;
use tracing::{debug, info};

/// Options for `cmd_model_apply`, replacing the former 8-argument signature.
pub struct ApplyOptions<'a> {
    pub model_path: &'a str,
    pub db_path: &'a str,
    #[allow(dead_code)] // reserved for future chroot support
    pub root: &'a str,
    pub dry_run: bool,
    pub skip_optional: bool,
    pub strict: bool,
    pub autoremove: bool,
    pub offline: bool,
}

/// Apply source-policy actions (`SetSourcePin` / `ClearSourcePin`) from the
/// filtered action list. Returns the number of changes applied.
pub(super) fn apply_source_policy_changes(
    conn: &Connection,
    actions: &[&DiffAction],
) -> Result<usize> {
    let mut count = 0;
    for action in actions {
        match action {
            DiffAction::SetSourcePin { distro, strength } => {
                let strength = strength.as_deref().unwrap_or("guarded");
                DistroPin::set(conn, distro, strength)?;
                println!("Updated source policy pin: {} ({})", distro, strength);
                count += 1;
            }
            DiffAction::ClearSourcePin => {
                DistroPin::remove(conn)?;
                println!("Cleared source policy pin");
                count += 1;
            }
            _ => {}
        }
    }
    Ok(count)
}

/// Apply package install/remove actions. Currently stubs that print a
/// manual-action notice and return the pending name lists `(installs, removes)`.
pub(super) fn apply_package_changes(actions: &[&DiffAction]) -> (Vec<String>, Vec<String>) {
    let removes: Vec<String> = actions
        .iter()
        .filter_map(|a| match a {
            DiffAction::Remove { package, .. } => Some(package.clone()),
            _ => None,
        })
        .collect();

    let installs: Vec<String> = actions
        .iter()
        .filter_map(|a| match a {
            DiffAction::Install { package, .. } => Some(package.clone()),
            _ => None,
        })
        .collect();

    if !removes.is_empty() {
        println!("Packages to remove: {}", removes.join(", "));
        println!("  [NOTE: Package removal not yet implemented - run manually]");
        println!();
    }

    if !installs.is_empty() {
        println!("Packages to install: {}", installs.join(", "));
        println!("  [NOTE: Package installation not yet implemented - run manually]");
        println!();
    }

    (installs, removes)
}

/// Apply derived-package build/rebuild actions.
///
/// Returns `(built, rebuilt, errors)`.
pub(super) fn apply_derived_packages(
    conn: &Connection,
    actions: &[&DiffAction],
    model: &SystemModel,
    model_dir: &Path,
    cas: &CasStore,
) -> (usize, usize, Vec<String>) {
    let mut derived_built = 0usize;
    let mut derived_rebuilt = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for action in actions {
        match action {
            DiffAction::BuildDerived {
                name,
                parent,
                needs_parent,
            } => {
                println!("Building derived package '{}'...", name);

                if *needs_parent {
                    println!(
                        "  [WARNING: Parent '{}' needs to be installed first]",
                        parent
                    );
                    errors.push(format!(
                        "Cannot build '{}': parent '{}' not installed",
                        name, parent
                    ));
                    continue;
                }

                let model_def = model.derive.iter().find(|d| d.name == *name);

                if let Some(def) = model_def {
                    match create_derived_from_model(conn, def, model_dir, cas) {
                        Ok(_id) => match build_derived_package(conn, name, cas) {
                            Ok(()) => derived_built += 1,
                            Err(e) => errors.push(format!("Build '{}': {}", name, e)),
                        },
                        Err(e) => errors.push(format!("Create definition '{}': {}", name, e)),
                    }
                } else {
                    errors.push(format!(
                        "Derived package '{}' not found in model file",
                        name
                    ));
                }
            }
            DiffAction::RebuildDerived { name, parent: _ } => {
                println!("Rebuilding derived package '{}'...", name);

                match build_derived_package(conn, name, cas) {
                    Ok(()) => derived_rebuilt += 1,
                    Err(e) => errors.push(format!("Rebuild '{}': {}", name, e)),
                }
            }
            _ => {}
        }
    }

    (derived_built, derived_rebuilt, errors)
}

/// Apply package metadata changes: pin/unpin, mark explicit/dependency, update.
///
/// Returns the number of changes applied and any errors encountered.
pub(super) fn apply_metadata_changes(
    conn: &Connection,
    actions: &[&DiffAction],
) -> (usize, Vec<String>) {
    let mut applied = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for action in actions {
        match action {
            DiffAction::Pin { package, pattern } => match Trove::find_one_by_name(conn, package) {
                Ok(Some(trove)) => {
                    if let Some(id) = trove.id {
                        if let Err(e) = Trove::pin(conn, id) {
                            errors.push(format!("Pin '{}': {}", package, e));
                        } else {
                            println!("Pinned '{}' to pattern '{}'", package, pattern);
                            applied += 1;
                        }
                    }
                }
                Ok(None) => {
                    errors.push(format!("Pin '{}': package not installed", package));
                }
                Err(e) => errors.push(format!("Pin '{}': {}", package, e)),
            },
            DiffAction::Unpin { package } => match Trove::find_one_by_name(conn, package) {
                Ok(Some(trove)) => {
                    if let Some(id) = trove.id {
                        if let Err(e) = Trove::unpin(conn, id) {
                            errors.push(format!("Unpin '{}': {}", package, e));
                        } else {
                            println!("Unpinned '{}'", package);
                            applied += 1;
                        }
                    }
                }
                Ok(None) => {
                    errors.push(format!("Unpin '{}': package not installed", package));
                }
                Err(e) => errors.push(format!("Unpin '{}': {}", package, e)),
            },
            DiffAction::MarkExplicit { package } => {
                match Trove::promote_to_explicit(
                    conn,
                    package,
                    Some("Marked explicit by model apply"),
                ) {
                    Ok(true) => {
                        println!("Marked '{}' as explicitly installed", package);
                        applied += 1;
                    }
                    Ok(false) => {
                        debug!("'{}' already explicit or not found", package);
                    }
                    Err(e) => errors.push(format!("MarkExplicit '{}': {}", package, e)),
                }
            }
            DiffAction::MarkDependency { package } => {
                match conn.execute(
                    "UPDATE troves SET install_reason = 'dependency' \
                     WHERE name = ?1 AND install_reason = 'explicit' AND type = 'package'",
                    rusqlite::params![package],
                ) {
                    Ok(rows) if rows > 0 => {
                        println!("Marked '{}' as dependency", package);
                        applied += 1;
                    }
                    Ok(_) => {
                        debug!("'{}' already a dependency or not found", package);
                    }
                    Err(e) => errors.push(format!("MarkDependency '{}': {}", package, e)),
                }
            }
            DiffAction::Update {
                package,
                current_version,
                target_version,
            } => {
                println!(
                    "Package '{}' needs update: {} -> {}",
                    package, current_version, target_version
                );
                println!(
                    "  [NOTE: Package update not yet implemented - run 'conary update {}' manually]",
                    package
                );
                applied += 1;
            }
            _ => {}
        }
    }

    (applied, errors)
}

fn create_derived_from_model(
    conn: &Connection,
    model_derived: &ModelDerivedPackage,
    model_dir: &Path,
    cas: &CasStore,
) -> Result<i64> {
    if let Some(existing) = DerivedPackage::find_by_name(conn, &model_derived.name)? {
        info!(
            "Derived package '{}' already exists, updating",
            model_derived.name
        );
        return existing.id.ok_or_else(|| {
            anyhow!(
                "Derived package '{}' exists but has no database id",
                model_derived.name
            )
        });
    }

    let version_policy = if model_derived.version == "inherit" {
        VersionPolicy::Inherit
    } else if model_derived.version.starts_with('+') {
        VersionPolicy::Suffix(model_derived.version.clone())
    } else {
        VersionPolicy::Specific(model_derived.version.clone())
    };

    let mut derived = DerivedPackage::new(model_derived.name.clone(), model_derived.from.clone());
    derived.version_policy = version_policy;
    derived.model_source = Some(model_dir.display().to_string());

    let derived_id = derived.insert(conn)?;
    info!(
        "Created derived package '{}' with id={}",
        model_derived.name, derived_id
    );

    for (order, patch_path) in model_derived.patches.iter().enumerate() {
        let full_path = model_dir.join(patch_path);
        if !full_path.exists() {
            return Err(anyhow!(
                "Patch file not found: {} (for derived package '{}')",
                full_path.display(),
                model_derived.name
            ));
        }

        let patch_content = std::fs::read(&full_path)
            .with_context(|| format!("Failed to read patch file '{}'", full_path.display()))?;
        let patch_hash = sha256(&patch_content);
        let patch_name = Path::new(patch_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("patch")
            .to_string();

        let mut patch = DerivedPatch::new(derived_id, (order + 1) as i32, patch_name, patch_hash);
        patch.insert(conn)?;
        cas.store(&patch_content)?;
    }

    for (target_path, source_path) in &model_derived.override_files {
        if source_path.is_empty() || source_path == "REMOVE" {
            let mut ov = DerivedOverride::new_remove(derived_id, target_path.clone());
            ov.insert(conn)?;
        } else {
            let full_source = model_dir.join(source_path);
            if !full_source.exists() {
                return Err(anyhow!(
                    "Override source file not found: {} (for derived package '{}')",
                    full_source.display(),
                    model_derived.name
                ));
            }

            let content = std::fs::read(&full_source).with_context(|| {
                format!(
                    "Failed to read override source file '{}'",
                    full_source.display()
                )
            })?;
            let source_hash = sha256(&content);

            let mut ov = DerivedOverride::new_replace(derived_id, target_path.clone(), source_hash);
            ov.source_path = Some(source_path.clone());
            ov.insert(conn)?;
            cas.store(&content)?;
        }
    }

    Ok(derived_id)
}

fn build_derived_package(conn: &Connection, name: &str, cas: &CasStore) -> Result<()> {
    let mut derived = DerivedPackage::find_by_name(conn, name)?
        .ok_or_else(|| anyhow!("Derived package '{}' not found", name))?;

    match build_from_definition(conn, &derived, cas) {
        Ok(build_result) => {
            let build_meta = persist_build_artifact(conn, &mut derived, &build_result, cas)?;
            println!(
                "  Built '{}': {} files, {} patches applied ({})",
                name,
                build_result.files.len(),
                build_result.patches_applied.len(),
                build_meta.artifact_path
            );
            Ok(())
        }
        Err(e) => {
            let error_msg = e.to_string();
            derived.mark_error(conn, &error_msg)?;
            Err(anyhow!("Build failed for '{}': {}", name, error_msg))
        }
    }
}
