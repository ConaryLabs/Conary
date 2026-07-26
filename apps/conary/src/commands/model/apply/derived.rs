// apps/conary/src/commands/model/apply/derived.rs

use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{DerivedOverride, DerivedPackage, DerivedPatch, VersionPolicy};
use conary_core::derived::{build_from_definition, persist_build_artifact};
use conary_core::filesystem::CasStore;
use conary_core::hash::sha256;
use conary_core::model::ModelDerivedPackage;
use rusqlite::Connection;
use std::path::Path;
use tracing::info;

pub(super) fn create_derived_from_model(
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
            .and_then(|name| name.to_str())
            .unwrap_or("patch")
            .to_string();

        let mut patch = DerivedPatch::new(derived_id, (order + 1) as i32, patch_name, patch_hash);
        patch.insert(conn)?;
        cas.store(&patch_content)?;
    }

    for (target_path, source_path) in &model_derived.override_files {
        if source_path.is_empty() || source_path == "REMOVE" {
            let mut override_entry = DerivedOverride::new_remove(derived_id, target_path.clone());
            override_entry.insert(conn)?;
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

            let mut override_entry =
                DerivedOverride::new_replace(derived_id, target_path.clone(), source_hash);
            override_entry.source_path = Some(source_path.clone());
            override_entry.insert(conn)?;
            cas.store(&content)?;
        }
    }

    Ok(derived_id)
}

pub(super) fn build_derived_package(conn: &Connection, name: &str, cas: &CasStore) -> Result<()> {
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
        Err(error) => {
            let error_message = error.to_string();
            derived.mark_error(conn, &error_message)?;
            Err(anyhow!("Build failed for '{}': {}", name, error_message))
        }
    }
}
