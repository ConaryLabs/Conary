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
mod ccs;
mod config;
mod execution;
mod ordering;
mod preparation;
mod promises;
mod relations;
mod witness_universe;

use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::inner;
use super::native_events::{NativeInstallInput, PreparedNativeTransaction};
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::{
    InstallIntent, InstallSemantics, NativeLifecycleInstallState, RepositoryInstallProvenance,
    build_execution_mode, detect_package_format,
};
use anyhow::{Context, Result};
pub(crate) use ccs::prepare_ccs_package_for_batch;
use conary_core::components::ComponentType;
use conary_core::db::models::{
    Changeset, InstallReason, InstalledCcsRemoveHook, InstalledFileCapability,
    InstalledNativeLifecycleBundle, InstalledRequirementGroup, PackageTransactionStaging,
    StagedAnchorDisposition, StagedPayloadRow, Trove,
};
use conary_core::filesystem::CasStore;
use conary_core::packages::config_authority::SourceConfigDeclaration;
use conary_core::packages::payload::PackagePayloadFile;
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
    /// Exact install semantics recovered from the source artifact or CCS
    /// lifecycle authority.
    pub(crate) semantics: InstallSemantics,
    /// Monotonic signed CCS build release, absent for a source-native artifact.
    pub package_release: Option<String>,
    /// Exact Debian Multi-Arch behavior, when source-owned.
    pub debian_multi_arch: Option<conary_core::repository::dependency_model::DebianMultiArch>,
    /// Architecture
    pub architecture: Option<String>,
    /// Package description
    pub description: Option<String>,
    /// Exact payload descriptors with independently reopenable content sources.
    pub extracted_files: Vec<PackagePayloadFile>,
    /// Pre-mutation repository authority derived from authenticated native
    /// payloads or carried by signed CCS lifecycle authority.
    pub repository_enrollments:
        Vec<conary_core::repository::enrollment::PackageRepositoryEnrollmentIntent>,
    /// Exact positive requirement groups declared by the package.
    pub requirements: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    /// Exact source-native capabilities declared by the package.
    pub provides: Vec<conary_core::repository::dependency_model::ProvidedCapability>,
    /// Exact source-native negative/replacement relation groups.
    pub relations: Vec<conary_core::repository::dependency_model::RepositoryRequirementGroup>,
    /// Installed packages selected by exact relation evaluation immediately
    /// before the batch transaction.
    pub relation_removals: Vec<conary_core::transaction::PackageRelationRemoval>,
    /// Installed packages whose configured state must be lowered before this
    /// exact incoming transaction element.
    pub relation_deconfigurations: Vec<conary_core::transaction::PackageRelationDeconfiguration>,
    /// Configuration files declared by the native package metadata
    pub config_declarations: Vec<SourceConfigDeclaration>,
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
    /// Exact CCS component names selected for this transaction.
    pub installed_component_names: Option<Vec<String>>,
    /// Exact CCS component ownership by normalized payload path.
    pub component_names_by_path: Option<HashMap<String, String>>,
    /// Repository metadata for packages resolved from synced repository rows.
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    /// Exact source identity explicitly supplied for a local artifact.
    pub requested_source_identity: Option<String>,
    /// Bundle replay plans accepted during preflight, carried through commit.
    pub native_lifecycle_state: NativeLifecycleInstallState,
    /// CCS-only lifecycle and capability authority.
    pub(crate) ccs: Option<PreparedCcsMetadata>,
}

pub(crate) struct PreparedPackageSourceAuthority<'a> {
    pub(crate) repository_provenance: Option<RepositoryInstallProvenance>,
    pub(crate) requested_source_identity: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedCcsMetadata {
    hooks: conary_core::ccs::manifest::Hooks,
    capabilities: Option<conary_core::capability::CapabilityDeclaration>,
    file_capabilities: Vec<conary_core::ccs::manifest::FileCapability>,
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
        let version_scheme = self.semantics.version_scheme;
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
        trove.package_release = self.package_release.clone();
        trove.debian_multi_arch = self.debian_multi_arch;
        trove.description = self.description.clone();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.install_reason = self.install_reason.clone();
        trove.selection_reason = Some(self.selection_reason.clone());
        if let Some(provenance) = self.repository_provenance.as_ref() {
            trove.install_source = conary_core::db::models::InstallSource::Repository;
            trove.installed_from_repository_id = Some(provenance.repository_id);
            trove.source_profile = provenance.source_profile.clone();
        } else if let Some(source_identity) = self.requested_source_identity.as_ref() {
            trove.source_profile = Some(source_identity.clone());
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

pub(crate) struct BatchInstallResult {
    installed: Vec<BatchInstalledPackage>,
}

struct BatchInstalledPackage {
    name: String,
    version: String,
    package_release: Option<String>,
    architecture: Option<String>,
    trove_id: i64,
}

impl BatchInstallResult {
    pub(crate) fn exact_trove_id(
        &self,
        package: &dyn conary_core::packages::PackageFormat,
    ) -> Result<i64> {
        let matches = self
            .installed
            .iter()
            .filter(|installed| {
                installed.name == package.name()
                    && installed.version == package.version()
                    && installed.package_release.as_deref() == package.package_release()
                    && installed.architecture.as_deref() == package.architecture()
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [installed] => Ok(installed.trove_id),
            [] => anyhow::bail!(
                "atomic package transaction did not return installed identity {}-{} ({})",
                package.name(),
                package.version(),
                package.architecture().unwrap_or("no-arch")
            ),
            _ => anyhow::bail!(
                "atomic package transaction returned duplicate installed identity {}-{} ({})",
                package.name(),
                package.version(),
                package.architecture().unwrap_or("no-arch")
            ),
        }
    }
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
    pub fn install_batch(self, packages: Vec<PreparedPackage>) -> Result<()> {
        self.install_batch_with_result(packages).map(|_| ())
    }

    /// Run the read-only dependency ordering and relation validation shared
    /// with [`Self::install_batch`] without materializing a selected root or
    /// executing lifecycle and payload mutation.
    pub(crate) fn validate_batch(self, mut packages: Vec<PreparedPackage>) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }
        let conn = open_db(self.db_path)?;
        self.validate_batch_transaction(&conn, &mut packages)?;
        Ok(())
    }

    fn validate_batch_transaction(
        &self,
        conn: &Connection,
        packages: &mut Vec<PreparedPackage>,
    ) -> Result<promises::PromiseWitnessPlan> {
        let promise_plan = ordering::order_packages_for_transaction(conn, packages)?;
        self.plan_package_relations_for_batch(conn, packages)?;
        Ok(promise_plan)
    }

    pub(crate) fn install_batch_with_result(
        self,
        mut packages: Vec<PreparedPackage>,
    ) -> Result<BatchInstallResult> {
        if packages.is_empty() {
            return Ok(BatchInstallResult {
                installed: Vec::new(),
            });
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

        // Every fact this transaction certifies is read from installed state:
        // requirement satisfaction against the end-state universe, promise
        // reliance, and the negative relation effects of the incoming set. All
        // of it is read with the runtime mutation lock already held. Certifying
        // first and locking second would let another transaction remove a
        // provider this certification relied on, and the database would commit
        // an end state missing a capability the batch was admitted on.
        let locked_root =
            crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(self.db_path)?;
        let mut promise_plan = self.validate_batch_transaction(&conn, &mut packages)?;
        let mut selected_root = locked_root.prepare(&conn, &tx_description)?;
        let selected_path = selected_root.selected_root().to_path_buf();
        for package in &mut packages {
            package.normalize_ccs_for_selected_root(&selected_path)?;
            super::transaction::preflight_generation_file_capabilities(
                package
                    .ccs
                    .as_ref()
                    .map(|ccs| ccs.file_capabilities.as_slice()),
                false,
                &package.extracted_files,
            )?;
        }
        let native_inputs = packages
            .iter()
            .map(|package| NativeInstallInput {
                package_name: &package.name,
                package_version: &package.version,
                package_arch: package.architecture.as_deref(),
                version_scheme: package.semantics.version_scheme,
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
        let native_execution_mode = build_execution_mode(None);
        native_transaction.preflight(&selected_path, &native_execution_mode)?;
        let preflighted_ccs_removal_hooks =
            ccs_removal_hook_plan.preflight(&selected_path, self.sandbox_mode)?;
        let mut ccs_hook_executors = ccs::prepare_hook_executors(&conn, &packages, &selected_path)?;
        let rollback_root = selected_root.capture_rollback_authority()?;
        let cas = selected_root.cas().clone();

        info!("Started batch transaction for {}", tx_description);

        // Phase 1: Unified planning across all packages after selected-root
        // normalization. Collect all files and detect cross-package conflicts.
        let batch_plan = self.plan_batch(&packages, &conn)?;
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
        ccs::execute_pre_hooks(&packages, &mut ccs_hook_executors)?;

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
            &mut ccs_hook_executors,
            &mut promise_plan,
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

        let installed = packages
            .iter()
            .zip(trove_ids)
            .map(|(package, trove_id)| BatchInstalledPackage {
                name: package.name.clone(),
                version: package.version.clone(),
                package_release: package.package_release.clone(),
                architecture: package.architecture.clone(),
                trove_id,
            })
            .collect();
        Ok(BatchInstallResult { installed })
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
        rollback_root: conary_core::generation::root_manifest::SelectedRootSnapshot,
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
        staging: &mut PackageTransactionStaging<'_>,
        changeset_id: i64,
        pkg: &PreparedPackage,
        trove_id: i64,
        files: &[inner::ResolvedInstallFile],
        directory_plan: &super::shared_directory::DirectoryInstallPlan,
    ) -> Result<()> {
        staging.clear()?;
        let mut path_to_component = HashMap::<&str, String>::new();
        if let (Some(component_names), Some(component_names_by_path)) = (
            pkg.installed_component_names.as_ref(),
            pkg.component_names_by_path.as_ref(),
        ) {
            for component_name in component_names {
                staging.stage_component(
                    trove_id,
                    component_name,
                    Some(&format!("{component_name} files")),
                    true,
                )?;
            }
            for (path, component_name) in component_names_by_path {
                if component_names.iter().any(|name| name == component_name) {
                    path_to_component.insert(path.as_str(), component_name.clone());
                }
            }
        } else {
            for comp_type in &pkg.installed_components {
                staging.stage_component(
                    trove_id,
                    comp_type.as_str(),
                    Some(&format!("{} files", comp_type.as_str())),
                    true,
                )?;
            }
            for (comp_type, paths) in &pkg.classified_files {
                if pkg.installed_components.contains(comp_type) {
                    for path in paths {
                        path_to_component.insert(path.as_str(), comp_type.as_str().to_string());
                    }
                }
            }
        }

        for removal in &pkg.relation_removals {
            if removal.mode
                == conary_core::repository::dependency_model::PackageRelationRemovalMode::OwnershipTransfer
                && removal
                    .ownership_transfer_packages
                    .iter()
                    .any(|incoming| incoming == &pkg.name)
            {
                staging.allow_replacement_of(removal.trove_id)?;
            }
        }

        for file in files {
            let entry = conary_core::db::models::FileEntry::new(
                file.path.clone(),
                file.node.clone(),
                file.content.clone(),
                trove_id,
            )
            .with_claim_policy(pkg.semantics.payload_sharing_policy());
            directory_plan.reconcile_through_symlink_target(tx, file)?;
            let (disposition, materialization_target_path) = match directory_plan.path(&file.path) {
                Some(super::shared_directory::DirectoryPathPlan::PreserveCompatiblePayload) => {
                    (StagedAnchorDisposition::Share, None)
                }
                Some(super::shared_directory::DirectoryPathPlan::PreserveLeaf { .. }) => {
                    (StagedAnchorDisposition::PreserveSelectedRoot, None)
                }
                Some(super::shared_directory::DirectoryPathPlan::ApplyDirectoryMetadata {
                    ..
                }) => (StagedAnchorDisposition::ApplyDirectory, None),
                Some(super::shared_directory::DirectoryPathPlan::ApplyThroughSymlink {
                    resolved_target_path,
                    ..
                }) => (
                    StagedAnchorDisposition::PreserveSelectedRoot,
                    Some(resolved_target_path.clone()),
                ),
                Some(super::shared_directory::DirectoryPathPlan::CreateOrReplace) | None => {
                    (StagedAnchorDisposition::Replace, None)
                }
            };
            staging.stage_payload(&StagedPayloadRow {
                entry,
                package_name: pkg.name.clone(),
                component_name: path_to_component.get(file.path.as_str()).cloned(),
                directory_materialization: directory_plan.materialization_for_path(&file.path),
                disposition,
                selected_root_node: directory_plan.leaf_before(&file.path).cloned(),
                materialization_target_path,
                history: Some((changeset_id, if pkg.is_upgrade { "modify" } else { "add" })),
            })?;
        }
        Self::stage_config_rows(tx, staging, pkg, trove_id, files)?;
        let installed_file_metadata = staging.validate_and_reconcile()?;
        if let Some(ccs) = pkg.ccs.as_ref() {
            let files_by_path = files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect::<HashMap<_, _>>();
            let selected_capabilities = ccs
                .file_capabilities
                .iter()
                .filter(|capability| installed_file_metadata.contains_key(&capability.path))
                .map(|capability| {
                    let file = files_by_path
                        .get(capability.path.as_str())
                        .with_context(|| {
                            format!(
                                "file capability target {} for {} is missing stored file metadata",
                                capability.path, pkg.name
                            )
                        })?;
                    if !super::file_capabilities::is_regular_file_capability_payload(
                        &file.node.source.kind,
                    ) {
                        anyhow::bail!(
                            "file capability target {} for {} is not a regular installed file",
                            capability.path,
                            pkg.name
                        );
                    }
                    Ok(capability.clone())
                })
                .collect::<Result<Vec<_>>>()?;
            InstalledFileCapability::replace_for_trove(tx, trove_id, &selected_capabilities)
                .with_context(|| {
                    format!("Failed to persist CCS file capabilities for {}", pkg.name)
                })?;
            if let Some(hook) = ccs.hooks.pre_remove.as_ref() {
                InstalledCcsRemoveHook::new(trove_id, hook.script.clone(), hook.reversible)
                    .insert_or_replace(tx)?;
            }
            if let Some(capabilities) = ccs.capabilities.as_ref() {
                conary_core::capability::store_capabilities(tx, trove_id, capabilities)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;
