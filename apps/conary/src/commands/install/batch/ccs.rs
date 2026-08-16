// apps/conary/src/commands/install/batch/ccs.rs

//! Verified CCS preparation for one atomic package batch.

use super::*;
use conary_core::packages::PackageFormat;
use std::collections::{HashMap, HashSet};

pub(crate) fn prepare_ccs_package_for_batch(
    package: &conary_core::ccs::CcsPackage,
    db_path: &str,
    install_reason: InstallReason,
    selection_reason: &str,
    allow_downgrade: bool,
    intent: InstallIntent,
    repository_provenance: Option<RepositoryInstallProvenance>,
) -> Result<PreparedPackage> {
    super::super::ccs_transaction::enforce_ccs_scriptlet_capability_gate(package)?;
    let semantics =
        super::super::ccs_transaction::install_semantics_for_ccs_manifest(package.manifest())?;
    let conn = open_db(db_path)?;
    let upgrade = super::super::ccs_transaction::check_ccs_upgrade_status(
        &conn,
        package,
        &semantics,
        allow_downgrade,
        intent,
        false,
    )?;
    let (is_upgrade, old_trove) = match upgrade {
        UpgradeCheck::FreshInstall => (false, None),
        UpgradeCheck::AlreadyInstalled(trove) => {
            anyhow::bail!(
                "Package {} version {} ({}) is already installed",
                trove.name,
                trove.version,
                trove.architecture.as_deref().unwrap_or("no-arch")
            )
        }
        UpgradeCheck::Upgrade(trove)
        | UpgradeCheck::Downgrade(trove)
        | UpgradeCheck::Replatform(trove) => (true, Some(trove)),
    };

    let mut component_names = package.components().keys().cloned().collect::<Vec<_>>();
    component_names.sort();
    let selected_components = component_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let selected_entries = package
        .file_entries()
        .iter()
        .filter(|entry| selected_components.contains(entry.component.as_str()))
        .collect::<Vec<_>>();
    let selected_paths = selected_entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    let extracted_files = package
        .package_payload()?
        .into_files()
        .into_iter()
        .filter(|file| selected_paths.contains(file.path.as_str()))
        .collect::<Vec<_>>();
    if extracted_files.is_empty() && !selected_entries.is_empty() {
        anyhow::bail!(
            "No payload files matched the selected CCS components: {}",
            component_names.join(", ")
        );
    }

    let mut component_names_by_path = HashMap::new();
    let mut classified_files: HashMap<ComponentType, Vec<String>> = HashMap::new();
    for entry in &selected_entries {
        if let Some(existing) =
            component_names_by_path.insert(entry.path.clone(), entry.component.clone())
            && existing != entry.component
        {
            anyhow::bail!(
                "CCS payload path {} belongs to both component '{}' and '{}'",
                entry.path,
                existing,
                entry.component
            );
        }
        if let Some(component_type) = ComponentType::parse(&entry.component) {
            classified_files
                .entry(component_type)
                .or_default()
                .push(entry.path.clone());
        }
    }
    let installed_components = component_names
        .iter()
        .filter_map(|name| ComponentType::parse(name))
        .collect::<Vec<_>>();
    let native_lifecycle_state =
        NativeLifecycleInstallState::from_bundle(package.manifest().native_lifecycle.as_ref())?;
    let repository_enrollments = package
        .v3_authority()
        .map(|authority| authority.lifecycle.repository_enrollments.clone())
        .unwrap_or_default();
    conary_core::repository::enrollment::validate_payload_bindings(
        &repository_enrollments,
        &extracted_files,
    )?;

    // A converted artifact declares only its source header capabilities, so the
    // payload it is about to install is the only authority for the file
    // providers that satisfy path dependencies.
    let mut provides = package.resolution_capabilities()?;
    super::super::transaction::extend_materialized_payload_provides(
        &mut provides,
        semantics,
        &extracted_files,
    )?;

    Ok(PreparedPackage {
        name: package.name().to_string(),
        version: package.version().to_string(),
        semantics,
        package_release: package.package_release().map(str::to_string),
        debian_multi_arch: package.debian_multi_arch(),
        architecture: package.architecture().map(str::to_string),
        description: package.description().map(str::to_string),
        extracted_files,
        repository_enrollments,
        requirements: package.requirements().to_vec(),
        provides,
        relations: package.relations().to_vec(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: package.config_declarations()?,
        install_reason,
        selection_reason: selection_reason.to_string(),
        is_upgrade,
        old_trove,
        installed_components,
        classified_files,
        installed_component_names: Some(component_names),
        component_names_by_path: Some(component_names_by_path),
        repository_provenance,
        native_lifecycle_state,
        ccs: Some(PreparedCcsMetadata {
            hooks: package.manifest().hooks.clone(),
            capabilities: package.manifest().capabilities.clone(),
            file_capabilities: package.manifest().file_capabilities.clone(),
        }),
    })
}

impl PreparedPackage {
    pub(super) fn normalize_ccs_for_selected_root(&mut self, selected_root: &Path) -> Result<()> {
        let Some(ccs) = self.ccs.as_mut() else {
            return Ok(());
        };

        let extracted_files = std::mem::take(&mut self.extracted_files);
        self.extracted_files =
            crate::commands::ccs::normalize_ccs_payload_files(selected_root, extracted_files)
                .with_context(|| format!("Failed to normalize CCS payload for {}", self.name))?;
        if let Some(component_names_by_path) = self.component_names_by_path.as_mut() {
            *component_names_by_path =
                normalize_component_paths(selected_root, component_names_by_path)?;
        }
        for paths in self.classified_files.values_mut() {
            for path in paths.iter_mut() {
                *path = crate::commands::ccs::normalize_ccs_package_path(selected_root, path)?;
            }
            paths.sort();
            paths.dedup();
        }
        ccs.file_capabilities = crate::commands::ccs::normalize_ccs_file_capabilities(
            selected_root,
            &ccs.file_capabilities,
        )?;
        Ok(())
    }
}

fn normalize_component_paths(
    selected_root: &Path,
    component_names_by_path: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut normalized = HashMap::with_capacity(component_names_by_path.len());
    for (path, component) in component_names_by_path {
        let path = crate::commands::ccs::normalize_ccs_package_path(selected_root, path)?;
        if let Some(existing) = normalized.insert(path.clone(), component.clone())
            && existing != *component
        {
            anyhow::bail!(
                "normalized CCS payload path {} belongs to both component '{}' and '{}'",
                path,
                existing,
                component
            );
        }
    }
    Ok(normalized)
}

pub(super) fn prepare_hook_executors(
    conn: &Connection,
    packages: &[PreparedPackage],
    selected_root: &Path,
) -> Result<Vec<Option<conary_core::ccs::HookExecutor>>> {
    let requires_inventory = packages.iter().any(|package| {
        package.ccs.as_ref().is_some_and(|ccs| {
            super::super::ccs_transaction::ccs_requires_host_capability_inventory(&ccs.hooks)
        })
    });
    let inventory = requires_inventory
        .then(|| conary_core::ccs::HostCapabilityInventory::load_required(conn))
        .transpose()
        .context("CCS lifecycle host capability preflight failed")?;

    packages
        .iter()
        .map(|package| {
            let Some(ccs) = package.ccs.as_ref() else {
                return Ok(None);
            };
            let has_install_hooks = super::super::ccs_transaction::ccs_has_pre_hooks(&ccs.hooks)
                || super::super::ccs_transaction::ccs_has_post_hooks(&ccs.hooks);
            if !has_install_hooks {
                return Ok(None);
            }
            let executor = conary_core::ccs::HookExecutor::new(
                selected_root,
                inventory.clone().unwrap_or_default(),
            );
            executor.preflight_hooks(&ccs.hooks).with_context(|| {
                format!(
                    "CCS lifecycle host capability preflight failed for {}",
                    package.name
                )
            })?;
            Ok(Some(executor))
        })
        .collect()
}

pub(super) fn execute_pre_hooks(
    packages: &[PreparedPackage],
    executors: &mut [Option<conary_core::ccs::HookExecutor>],
) -> Result<()> {
    for (package, executor) in packages.iter().zip(executors) {
        let (Some(ccs), Some(executor)) = (package.ccs.as_ref(), executor.as_mut()) else {
            continue;
        };
        if super::super::ccs_transaction::ccs_has_pre_hooks(&ccs.hooks) {
            executor
                .execute_pre_hooks(&ccs.hooks)
                .with_context(|| format!("CCS pre-install hook failed for {}", package.name))?;
        }
    }
    Ok(())
}
