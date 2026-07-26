// apps/conary/src/commands/install/batch.rs

//! Batch installer for atomic multi-package installation
//!
//! This module provides `BatchInstaller` for installing multiple packages (typically
//! a package and its dependencies) in a single atomic transaction. If any package
//! fails to install, all changes are rolled back, preventing broken system states.
//!
//! # Design Principles
//!
//! 1. **Single transaction for all packages** - Lock held for entire batch
//! 2. **Stream from disk** - Store paths to temp files, not raw bytes (avoid OOM)
//! 3. **Unified VfsTree** - Single planner accumulates changes across packages
//! 4. **Single DB commit** - All troves inserted in one transaction
//! 5. **Typed lifecycle ordering** - native events preserve package-manager transaction order

use super::super::open_db;
mod config;
mod execution;
mod preparation;
mod relations;

use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::inner;
use super::native_events::{NativeInstallInput, PreparedNativeTransaction};
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::{
    InstallIntent, InstallSemantics, NativeLifecycleInstallState, PackageFormatType,
    RepositoryInstallProvenance, build_execution_mode, detect_package_format,
};
use anyhow::{Context, Result};
use conary_core::components::ComponentType;
use conary_core::db::models::{
    Changeset, Component, InstallReason, InstalledNativeLifecycleBundle, InstalledRequirementGroup,
    Trove,
};
use conary_core::filesystem::CasStore;
use conary_core::packages::payload::PackagePayloadFile;
use conary_core::packages::traits::ConfigFileInfo;
use conary_core::scriptlet::SandboxMode;
#[cfg(test)]
use preparation::BatchConflict;
pub use preparation::prepare_package_for_batch;
use rusqlite::{Connection, Transaction};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use tracing::{debug, info};

/// A package prepared for batch installation
///
/// CRITICAL: This struct stores paths to extracted content on disk, NOT raw bytes.
/// This prevents OOM when installing many packages with large files.
#[derive(Debug)]
pub struct PreparedPackage {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Package format (RPM, DEB, Arch)
    pub format: PackageFormatType,
    /// Architecture
    pub architecture: Option<String>,
    /// Package description
    pub description: Option<String>,
    /// Exact payload descriptors with independently reopenable content sources.
    pub extracted_files: Vec<PackagePayloadFile>,
    /// Exact positive requirement groups declared by the package.
    pub requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    /// Exact source-native capabilities declared by the package.
    pub provides: Vec<conary_core::packages::traits::ProvidedCapability>,
    /// Exact source-native negative/replacement relation groups.
    pub relations: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    /// Installed packages selected by exact relation evaluation immediately
    /// before the batch transaction.
    pub relation_removals: Vec<conary_core::transaction::PackageRelationRemoval>,
    /// Installed packages whose configured state must be lowered before this
    /// exact incoming transaction element.
    pub relation_deconfigurations: Vec<conary_core::transaction::PackageRelationDeconfiguration>,
    /// Configuration files declared by the native package metadata
    pub config_files: Vec<ConfigFileInfo>,
    /// Typed ownership used by dependency and autoremove planning.
    pub install_reason: InstallReason,
    /// Human-facing explanation for why this package was selected.
    pub selection_reason: String,
    /// Whether this is an upgrade of an existing package
    pub is_upgrade: bool,
    /// Old trove being upgraded (if any)
    pub old_trove: Option<Box<Trove>>,
    /// Which components are being installed
    pub installed_components: Vec<ComponentType>,
    /// Files assigned by exact component metadata.
    pub classified_files: HashMap<ComponentType, Vec<String>>,
    /// Repository metadata for packages resolved from synced repository rows.
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    /// Bundle replay plans accepted during preflight, carried through commit.
    pub native_lifecycle_state: NativeLifecycleInstallState,
}

pub(super) struct BatchDbRows {
    pub(super) changeset_id: i64,
    pub(super) trove_ids: Vec<i64>,
    pub(super) retained_upgrade_trove_ids: Vec<i64>,
    pub(super) retain_for_lifecycle_by_pkg: Vec<bool>,
}

impl PreparedPackage {
    pub(super) fn old_trove_id(&self) -> Result<Option<i64>> {
        self.old_trove
            .as_deref()
            .map(|trove| {
                trove.id.with_context(|| {
                    format!(
                        "installed package '{}-{}' selected for replacement has no persisted trove ID",
                        trove.name, trove.version
                    )
                })
            })
            .transpose()
    }

    /// Create a Trove model from this prepared package
    pub fn to_trove(&self, changeset_id: i64) -> Result<Trove> {
        let version_scheme = super::prepare::version_scheme_for_format(self.format);
        if let Some(provenance) = self.repository_provenance.as_ref()
            && provenance.version_scheme != version_scheme
        {
            anyhow::bail!(
                "repository provenance for {} declares {} versioning, but the parsed package owns {} versioning",
                self.name,
                provenance.version_scheme.as_str(),
                version_scheme.as_str()
            );
        }
        let mut trove = Trove::new(
            self.name.clone(),
            self.version.clone(),
            conary_core::db::models::TroveType::Package,
            version_scheme,
        );
        trove.architecture = self.architecture.clone();
        trove.description = self.description.clone();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.install_reason = self.install_reason.clone();
        trove.selection_reason = Some(self.selection_reason.clone());
        if let Some(provenance) = self.repository_provenance.as_ref() {
            trove.install_source = conary_core::db::models::InstallSource::Repository;
            trove.installed_from_repository_id = Some(provenance.repository_id);
            trove.source_profile = provenance.source_profile.clone();
        }

        Ok(trove)
    }
}

/// Batch installer for atomic multi-package installation
///
/// # Usage
///
/// ```ignore
/// let installer = BatchInstaller::new(
///     db_path,
///     sandbox_mode,
/// );
/// installer.install_batch(packages)?;
/// ```
pub struct BatchInstaller<'a> {
    db_path: &'a str,
    sandbox_mode: SandboxMode,
}

impl<'a> BatchInstaller<'a> {
    /// Create a new batch installer
    pub fn new(db_path: &'a str, sandbox_mode: SandboxMode) -> Self {
        Self {
            db_path,
            sandbox_mode,
        }
    }

    /// Install multiple packages atomically
    ///
    /// All packages are installed in a single transaction. If any package fails,
    /// all changes are rolled back.
    ///
    /// # Arguments
    ///
    /// * `packages` - List of prepared packages to install. Should be in dependency
    ///   order (dependencies first, main package last).
    ///
    /// # Returns
    ///
    /// Ok(()) on success, or an error if any package fails to install.
    pub fn install_batch(self, mut packages: Vec<PreparedPackage>) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }
        let package_count = packages.len();
        let main_pkg_name = packages
            .last()
            .map(|p| p.name.clone())
            .expect("non-empty package batch checked above");

        info!(
            "Starting batch install: {} packages (main: {})",
            package_count, main_pkg_name
        );

        // Open database connection
        let mut conn = open_db(self.db_path)?;
        self.plan_package_relations_for_batch(&conn, &mut packages)?;
        let native_inputs = packages
            .iter()
            .map(|package| NativeInstallInput {
                package_name: &package.name,
                package_version: &package.version,
                package_arch: package.architecture.as_deref(),
                version_scheme: super::prepare::version_scheme_for_format(package.format),
                provides: &package.provides,
                new_bundle: package.native_lifecycle_state.bundle_to_persist.as_ref(),
                old_trove: package.old_trove.as_deref(),
                relation_removals: &package.relation_removals,
                relation_deconfigurations: &package.relation_deconfigurations,
                paths: package
                    .extracted_files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect(),
            })
            .collect::<Vec<_>>();
        let native_transaction = PreparedNativeTransaction::prepare_batch(&conn, &native_inputs)?;
        let ccs_removal_hook_plan = CcsRemovalHookPlan::prepare(
            &conn,
            packages
                .iter()
                .filter_map(|package| package.old_trove.as_deref()),
            packages
                .iter()
                .flat_map(|package| package.relation_removals.iter()),
        )?;
        // Build transaction description
        let tx_description = if package_count == 1 {
            format!("Install {}-{}", packages[0].name, packages[0].version)
        } else {
            format!(
                "Install {} + {} dependencies",
                main_pkg_name,
                package_count - 1
            )
        };
        let mut selected_root =
            crate::commands::generation::selected_root::SelectedRootSession::begin(
                &conn,
                self.db_path,
                &tx_description,
            )?;
        let selected_path = selected_root.selected_root().to_path_buf();
        let native_execution_mode = build_execution_mode(None);
        native_transaction.preflight(&selected_path, &native_execution_mode)?;
        let preflighted_ccs_removal_hooks =
            ccs_removal_hook_plan.preflight(&selected_path, self.sandbox_mode)?;
        let rollback_root = selected_root.capture_rollback_authority()?;
        let cas = selected_root.cas().clone();

        info!("Started batch transaction for {}", tx_description);

        // Planning began before we waited for the runtime mutation lock. The
        // selected-root session now owns that lock, so revalidate every exact
        // relation transition against the serialized database state.
        for package in &packages {
            conary_core::transaction::validate_package_relation_transitions(
                &conn,
                &package.relation_removals,
                &package.relation_deconfigurations,
            )
            .with_context(|| {
                format!(
                    "package relation transaction preflight changed for {}",
                    package.name
                )
            })?;
        }

        // Phase 1: Unified planning across all packages
        // Collect all files and detect cross-package conflicts
        let batch_plan = self.plan_batch(&packages, &conn)?;

        // Check for conflicts
        if !batch_plan.conflicts.is_empty() {
            let conflict_msgs: Vec<String> =
                batch_plan.conflicts.iter().map(|c| c.to_string()).collect();
            return Err(anyhow::anyhow!(
                "Batch install conflicts detected:\n  {}",
                conflict_msgs.join("\n  ")
            ));
        }

        info!(
            "Batch plan: {} total files across {} packages",
            batch_plan.total_files, package_count
        );

        self.preflight_file_ownership_for_batch(&conn, &packages)?;

        // Phase 2: Run exact typed lifecycle events before payload mutation.
        preflighted_ccs_removal_hooks.execute()?;

        // Phase 3: Store all package files in CAS, capturing the
        // authoritative hash returned by the store for each file.
        let stored_files_by_pkg = self.store_batch_files_in_cas(&cas, &packages)?;

        info!("Batch CAS storage complete: {} packages", package_count);

        // Phase 4: Single DB transaction for ALL packages
        let summary = format!("Batch install: {main_pkg_name}");
        let transaction_result = self.execute_selected_root_native_graph(
            &mut conn,
            &cas,
            &packages,
            &stored_files_by_pkg,
            &tx_description,
            &summary,
            &native_transaction,
            &native_execution_mode,
            &mut selected_root,
            rollback_root,
        );
        let (_changeset_id, trove_ids, _retained_upgrade_trove_ids) = transaction_result?;

        info!(
            "Batch transaction completed: {} packages installed",
            package_count
        );

        // Print summary
        println!(
            "Batch installed {} package(s) successfully:",
            trove_ids.len()
        );
        for pkg in &packages {
            println!(
                "  {} {} ({} files)",
                pkg.name,
                pkg.version,
                pkg.extracted_files.len()
            );
        }

        Ok(())
    }

    fn preflight_file_ownership_for_batch(
        &self,
        conn: &Connection,
        packages: &[PreparedPackage],
    ) -> Result<()> {
        for pkg in packages {
            inner::preflight_file_ownership(
                conn,
                pkg.extracted_files.iter().map(|file| file.path.as_str()),
                &pkg.name,
                &pkg.relation_removals,
            )?;
        }
        Ok(())
    }

    fn store_batch_files_in_cas(
        &self,
        cas: &CasStore,
        packages: &[PreparedPackage],
    ) -> Result<Vec<Vec<inner::StoredInstallFile>>> {
        let mut stored_files_by_pkg = Vec::with_capacity(packages.len());
        for (pkg_idx, pkg) in packages.iter().enumerate() {
            info!(
                "[{}/{}] Storing files in CAS: {} {}",
                pkg_idx + 1,
                packages.len(),
                pkg.name,
                pkg.version
            );

            let stored_files = inner::store_extracted_files_in_cas(cas, &pkg.extracted_files)
                .with_context(|| format!("Failed to store payload for {}", pkg.name))?;
            stored_files_by_pkg.push(stored_files);
        }
        Ok(stored_files_by_pkg)
    }

    fn insert_batch_db_rows(
        tx: &Transaction<'_>,
        selected_root: &std::path::Path,
        packages: &[PreparedPackage],
        tx_description: &str,
        tx_uuid: Option<String>,
        rollback_root: conary_core::generation::root_manifest::CapturedSelectedRoot,
    ) -> Result<BatchDbRows> {
        let mut changeset = match tx_uuid {
            Some(tx_uuid) => Changeset::with_tx_uuid(tx_description.to_string(), tx_uuid),
            None => Changeset::new(tx_description.to_string()),
        };
        let changeset_id = changeset.insert(tx)?;
        let materialized_directories =
            super::shared_directory::capture_existing_directory_materializations(
                tx,
                selected_root,
            )?;
        super::rollback_snapshot::record_install_rollback_snapshots(
            tx,
            changeset_id,
            packages
                .iter()
                .filter_map(|package| package.old_trove.as_deref()),
            packages
                .iter()
                .flat_map(|package| package.relation_removals.iter()),
            rollback_root,
            materialized_directories,
        )?;

        let mut trove_ids: Vec<i64> = Vec::with_capacity(packages.len());
        let mut retained_upgrade_trove_ids = packages
            .iter()
            .flat_map(|package| {
                package
                    .relation_removals
                    .iter()
                    .map(|removal| removal.trove_id)
            })
            .collect::<Vec<_>>();
        retained_upgrade_trove_ids.sort_unstable();
        retained_upgrade_trove_ids.dedup();
        let mut retain_for_lifecycle_by_pkg = Vec::with_capacity(packages.len());

        for pkg in packages {
            let retain_for_lifecycle = Self::retains_old_payload_for_lifecycle(tx, pkg)?;
            retain_for_lifecycle_by_pkg.push(retain_for_lifecycle);
            if let Some(ref old_trove) = pkg.old_trove
                && let Some(old_id) = old_trove.id
                && retain_for_lifecycle
            {
                retained_upgrade_trove_ids.push(old_id);
            }

            let mut trove = pkg.to_trove(changeset_id)?;
            let trove_id = trove.insert(tx)?;
            InstalledRequirementGroup::insert_groups(
                tx,
                trove_id,
                trove.version_scheme,
                &pkg.requirements,
            )?;
            InstalledRequirementGroup::insert_groups(
                tx,
                trove_id,
                trove.version_scheme,
                &pkg.relations,
            )?;
            trove_ids.push(trove_id);
            if let Some(bundle) = pkg.native_lifecycle_state.bundle_to_persist.as_ref() {
                let mut installed =
                    InstalledNativeLifecycleBundle::new(trove_id, Some(changeset_id), bundle)?;
                if bundle.source_format == conary_core::ccs::native_lifecycle::SourceFormat::Deb {
                    installed.set_lifecycle_state(
                        conary_core::ccs::native_transaction::DebPackageState::Unpacked,
                    );
                }
                installed.insert_or_replace(tx)?;
            }

            super::transaction::persist_declared_provides(
                tx,
                trove_id,
                &pkg.name,
                &pkg.version,
                trove.version_scheme,
                &pkg.provides,
            )?;

            if let Some(old_trove) = pkg.old_trove.as_ref() {
                super::mark_upgraded_parent_deriveds_stale(
                    tx,
                    &pkg.name,
                    Some(old_trove.version.as_str()),
                    &pkg.version,
                )?;
            }

            debug!(
                "Inserted trove {} (id={}) with {} files",
                pkg.name,
                trove_id,
                pkg.extracted_files.len()
            );
        }

        retained_upgrade_trove_ids.sort_unstable();
        retained_upgrade_trove_ids.dedup();
        Ok(BatchDbRows {
            changeset_id,
            trove_ids,
            retained_upgrade_trove_ids,
            retain_for_lifecycle_by_pkg,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_batch_payload_rows(
        tx: &Transaction<'_>,
        changeset_id: i64,
        pkg: &PreparedPackage,
        trove_id: i64,
        files: &[inner::ResolvedInstallFile],
        directory_plan: &super::shared_directory::DirectoryInstallPlan,
    ) -> Result<()> {
        let mut component_ids = HashMap::new();
        for comp_type in &pkg.installed_components {
            let mut component = Component::from_type(trove_id, *comp_type);
            component.description = Some(format!("{} files", comp_type.as_str()));
            component_ids.insert(*comp_type, component.insert(tx)?);
        }
        let mut path_to_component = HashMap::new();
        for (comp_type, paths) in &pkg.classified_files {
            if let Some(component_id) = component_ids.get(comp_type) {
                for path in paths {
                    path_to_component.insert(path.as_str(), *component_id);
                }
            }
        }

        let mut installed_file_metadata = HashMap::new();
        for file in files {
            if let Some(content) = file.content.as_ref() {
                tx.execute(
                    "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        &content.sha256,
                        format!("objects/{}/{}", &content.sha256[0..2], &content.sha256[2..]),
                        i64::try_from(content.size)
                            .context("payload content size exceeds SQLite range")?,
                    ],
                )?;
            }
            let mut entry = conary_core::db::models::FileEntry::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                trove_id,
            );
            entry.component_id = path_to_component.get(file.path.as_str()).copied();
            directory_plan.reconcile_through_symlink_target(tx, file)?;
            let inserted = inner::insert_file_entry_claiming_live_root_overlap(
                tx,
                &mut entry,
                &pkg.name,
                &pkg.relation_removals,
                directory_plan.materialization_for_path(&file.path),
                directory_plan.leaf_before(&file.path),
            )?;
            directory_plan.persist_through_symlink_target(tx, file, trove_id)?;
            installed_file_metadata
                .insert(file.path.clone(), (inserted.file_id, file.cas_hash.clone()));
            if inserted.materialized {
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        changeset_id,
                        &file.path,
                        file.content.as_ref().map(|content| &content.sha256),
                        if pkg.is_upgrade { "modify" } else { "add" },
                    ],
                )?;
            }
        }
        Self::insert_config_rows(tx, pkg, trove_id, &installed_file_metadata, files)
    }
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;
