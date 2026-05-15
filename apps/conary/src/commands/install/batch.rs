// src/commands/install/batch.rs

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
//! 5. **Scriptlet ordering** - Pre-scripts in topo order before FS changes, post-scripts after

use super::super::open_db;
// convert_extracted_files no longer needed -- CAS storage is done directly
use super::inner;
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::scriptlets::{
    build_execution_mode, get_old_package_scriptlets, run_old_post_remove, run_old_pre_remove,
    run_post_install, run_pre_install, to_scriptlet_format,
};
use super::{InstallSemantics, PackageExecutionPath, PackageFormatType, detect_package_format};
use anyhow::{Context, Result};
use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, DependencyEntry, ProvideEntry, ScriptletEntry, Trove,
};
use conary_core::dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
use conary_core::packages::traits::{ExtractedFile, Scriptlet};
use conary_core::scriptlet::SandboxMode;
use conary_core::transaction::{FileToRemove, TransactionConfig, TransactionEngine};
use rusqlite::{Connection, Transaction};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Metadata about a file to be installed (without content)
///
/// Used for memory-efficient batch operations where we track file info
/// without holding content in memory.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Part of public API for future streaming implementation
#[allow(clippy::struct_field_names)]
pub struct FileMetadata {
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mode: u32,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
}

/// A package prepared for batch installation
///
/// CRITICAL: This struct stores paths to extracted content on disk, NOT raw bytes.
/// This prevents OOM when installing many packages with large files.
#[derive(Debug)]
#[allow(dead_code)] // old_files retained for composefs-native upgrade tracking
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
    /// Files extracted from the package (with content for now, will be streamed later)
    pub extracted_files: Vec<ExtractedFile>,
    /// Dependencies declared by the package
    pub dependencies: Vec<conary_core::packages::traits::Dependency>,
    /// Scriptlets from the package
    pub scriptlets: Vec<Scriptlet>,
    /// Why this package is being installed
    pub install_reason: String,
    /// Whether this is an upgrade of an existing package
    pub is_upgrade: bool,
    /// Old trove being upgraded (if any)
    pub old_trove: Option<Box<Trove>>,
    /// Files to remove from old version (for upgrades)
    pub old_files: Vec<FileToRemove>,
    /// Which components are being installed
    pub installed_components: Vec<ComponentType>,
    /// Classified files by component type
    pub classified_files: HashMap<ComponentType, Vec<String>>,
    /// Language-specific provides detected from files
    pub language_provides: Vec<LanguageDep>,
    /// Cached scriptlets from old package (for upgrades), queried before DB commit
    /// to avoid cascade-delete losing them
    pub cached_old_scriptlets: Vec<ScriptletEntry>,
}

impl PreparedPackage {
    /// Create a Trove model from this prepared package
    pub fn to_trove(&self, changeset_id: i64) -> Trove {
        let mut trove = Trove::new(
            self.name.clone(),
            self.version.clone(),
            conary_core::db::models::TroveType::Package,
        );
        trove.architecture = self.architecture.clone();
        trove.description = self.description.clone();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.selection_reason = Some(self.install_reason.clone());

        // Mark as dependency if install reason contains "Required by"
        if self.install_reason.starts_with("Required by") {
            trove.install_reason = conary_core::db::models::InstallReason::Dependency;
        }

        trove
    }
}

/// Batch installer for atomic multi-package installation
///
/// # Usage
///
/// ```ignore
/// let installer = BatchInstaller::new(db_path, root, sandbox_mode, no_scripts);
/// installer.install_batch(packages)?;
/// ```
pub struct BatchInstaller<'a> {
    db_path: &'a str,
    root: &'a str,
    sandbox_mode: SandboxMode,
    no_scripts: bool,
    preflighted_execution_path: Option<PackageExecutionPath>,
}

impl<'a> BatchInstaller<'a> {
    /// Create a new batch installer
    pub fn new(
        db_path: &'a str,
        root: &'a str,
        sandbox_mode: SandboxMode,
        no_scripts: bool,
    ) -> Self {
        Self {
            db_path,
            root,
            sandbox_mode,
            no_scripts,
            preflighted_execution_path: None,
        }
    }

    pub(super) fn with_preflighted_execution_path(
        mut self,
        execution_path: PackageExecutionPath,
    ) -> Self {
        self.preflighted_execution_path = Some(execution_path);
        self
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
        if packages.is_empty() {
            return Ok(());
        }

        let package_count = packages.len();
        let main_pkg_name = packages
            .last()
            .map(|p| p.name.as_str())
            .unwrap_or("unknown");

        info!(
            "Starting batch install: {} packages (main: {})",
            package_count, main_pkg_name
        );

        // Open database connection
        let conn = open_db(self.db_path)?;

        let execution_path = match self.preflighted_execution_path {
            Some(execution_path) => execution_path,
            None => super::prepare_install_environment_before_scriptlets(
                &conn,
                self.db_path,
                self.root,
            )?,
        };

        // Create composefs-native transaction engine
        let db_path_buf = PathBuf::from(self.db_path);
        let tx_config = TransactionConfig::from_paths(PathBuf::from(self.root), db_path_buf);
        let mut engine =
            TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

        // Recover any incomplete transactions (only if generations exist)
        if execution_path == PackageExecutionPath::GenerationAware
            && engine.config().generations_dir.exists()
        {
            engine
                .recover(&conn)
                .context("Failed to recover incomplete transactions")?;
        }

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

        // Acquire transaction lock for entire batch
        engine
            .begin()
            .context("Failed to begin batch transaction")?;

        info!("Started batch transaction for {}", tx_description);

        // Phase 1: Unified planning across all packages
        // Collect all files and detect cross-package conflicts
        let batch_plan = self.plan_batch(&packages, &conn)?;

        // Check for conflicts
        if !batch_plan.conflicts.is_empty() {
            let conflict_msgs: Vec<String> =
                batch_plan.conflicts.iter().map(|c| c.to_string()).collect();
            engine.release_lock();
            return Err(anyhow::anyhow!(
                "Batch install conflicts detected:\n  {}",
                conflict_msgs.join("\n  ")
            ));
        }

        info!(
            "Batch plan: {} total files across {} packages",
            batch_plan.total_files, package_count
        );

        if execution_path == PackageExecutionPath::MutableLiveRoot {
            self.preflight_live_root_file_ownership_for_batch(&conn, &packages)?;
        }

        // Phase 2: Run pre-install scriptlets in topological order (dependencies first)
        if !self.no_scripts {
            for pkg in &packages {
                if let Err(e) = self.run_pre_scripts(pkg) {
                    warn!("Pre-install scriptlet failed for {}: {}", pkg.name, e);
                    engine.release_lock();
                    return Err(anyhow::anyhow!(
                        "Pre-install scriptlet failed for '{}': {}. Transaction aborted.",
                        pkg.name,
                        e
                    ));
                }
            }
        }

        // Phase 3: Store all package files in CAS, capturing the
        // authoritative hash returned by the store for each file.
        let stored_files_by_pkg = self.store_batch_files_in_cas(&engine, &packages)?;

        info!("Batch CAS storage complete: {} packages", package_count);

        // Phase 4: Single DB transaction for ALL packages
        let (changeset_id, trove_ids) = if execution_path == PackageExecutionPath::MutableLiveRoot {
            let mutable_result = (|| -> Result<(i64, Vec<i64>)> {
                self.preflight_live_root_file_ownership_for_batch(&conn, &packages)?;
                let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(
                    PathBuf::from(self.db_path),
                );
                crate::commands::live_root::recover_pending_journals_with_changesets(
                    runtime_root.root(),
                    Path::new(self.root),
                    &conn,
                )?;
                let live_files = super::live_root_files_from_stored_files(
                    engine.cas(),
                    &stored_files_by_pkg
                        .iter()
                        .flat_map(|files| files.iter().cloned())
                        .collect::<Vec<_>>(),
                )?;
                let tx_uuid = uuid::Uuid::new_v4().to_string();
                let mut live_tx = crate::commands::LiveRootTransaction::begin(
                    runtime_root.root(),
                    Path::new(self.root),
                    tx_uuid.clone(),
                    tx_description.clone(),
                )?;
                live_tx.apply_install_files(&live_files)?;

                let tx = conn.unchecked_transaction()?;
                let db_result = Self::insert_batch_db_rows(
                    &tx,
                    &packages,
                    &stored_files_by_pkg,
                    &tx_description,
                    Some(tx_uuid),
                );
                let (changeset_id, trove_ids) = match db_result {
                    Ok(result) => result,
                    Err(error) => {
                        drop(tx);
                        live_tx.rollback()?;
                        return Err(error);
                    }
                };
                if let Err(error) = tx.commit() {
                    if let Err(rollback_error) = live_tx.rollback() {
                        return Err(error)
                            .context(format!("Failed to rollback live root: {rollback_error}"));
                    }
                    return Err(error.into());
                }
                live_tx.commit()?;
                Ok((changeset_id, trove_ids))
            })();
            match mutable_result {
                Ok(result) => result,
                Err(error) => {
                    engine.release_lock();
                    return Err(error);
                }
            }
        } else {
            let tx = conn.unchecked_transaction()?;
            let db_result = Self::insert_batch_db_rows(
                &tx,
                &packages,
                &stored_files_by_pkg,
                &tx_description,
                None,
            );
            match db_result {
                Ok((cs_id, tr_ids)) => {
                    if let Err(error) = tx.commit() {
                        engine.release_lock();
                        return Err(error.into());
                    }
                    info!(
                        "Batch DB commit successful: changeset={}, {} troves",
                        cs_id,
                        tr_ids.len()
                    );
                    (cs_id, tr_ids)
                }
                Err(e) => {
                    drop(tx);
                    engine.release_lock();
                    return Err(anyhow::anyhow!("Batch database transaction failed: {}", e));
                }
            }
        };

        // Phase 7: Run post-install scriptlets in topological order
        // Also run old package removal scriptlets for upgrades
        if !self.no_scripts {
            for pkg in &packages {
                // Run old package post-remove for upgrades
                if let Some(ref old_trove) = pkg.old_trove {
                    // Use cached scriptlets (queried before DB commit deleted the old trove)
                    let scriptlet_format = to_scriptlet_format(pkg.format);
                    run_old_post_remove(
                        Path::new(self.root),
                        &old_trove.name,
                        &old_trove.version,
                        &pkg.version,
                        &pkg.cached_old_scriptlets,
                        scriptlet_format,
                        self.sandbox_mode,
                    );
                }

                // Run post-install scriptlet
                self.run_post_scripts(pkg);
            }
        }

        // Phase 6: Execute triggers for all installed files
        let all_file_paths: Vec<String> = packages
            .iter()
            .flat_map(|pkg| pkg.extracted_files.iter().map(|f| f.path.clone()))
            .collect();

        super::run_triggers(&conn, Path::new(self.root), changeset_id, &all_file_paths);

        if execution_path == PackageExecutionPath::GenerationAware {
            let rebuild_result = crate::commands::composefs_ops::rebuild_and_mount(
                &conn,
                self.db_path,
                &format!("Batch install: {}", main_pkg_name),
                None,
            );
            if let Err(error) = rebuild_result
                && let Err(metadata_error) =
                    Self::record_generation_rebuild_failure(&conn, changeset_id, error)
            {
                engine.release_lock();
                return Err(metadata_error);
            }
        }

        // Release transaction lock
        engine.release_lock();

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

    fn record_generation_rebuild_failure(
        conn: &Connection,
        changeset_id: i64,
        error: anyhow::Error,
    ) -> Result<()> {
        crate::commands::append_deferred_follow_up_metadata(
            conn,
            changeset_id,
            crate::commands::DeferredFollowUp {
                kind: "generation_rebuild".to_string(),
                status: "failed".to_string(),
                message: error.to_string(),
                retry_command: Some(
                    "conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\""
                        .to_string(),
                ),
            },
        )?;
        warn!(
            changeset_id,
            "Package mutation completed, but generation rebuild was deferred: {}", error
        );
        eprintln!(
            "WARNING: package mutation completed, but generation rebuild was deferred: {error}"
        );
        Ok(())
    }

    fn preflight_live_root_file_ownership_for_batch(
        &self,
        conn: &Connection,
        packages: &[PreparedPackage],
    ) -> Result<()> {
        for pkg in packages {
            inner::preflight_live_root_file_ownership(
                conn,
                pkg.extracted_files.iter().map(|file| file.path.as_str()),
                &pkg.name,
            )?;
        }
        Ok(())
    }

    fn store_batch_files_in_cas(
        &self,
        engine: &TransactionEngine,
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

            let mut stored_files = Vec::with_capacity(pkg.extracted_files.len());
            for file in &pkg.extracted_files {
                let hash = if let Some(target) = file.symlink_target.as_deref() {
                    engine.cas().store_symlink(target).with_context(|| {
                        format!(
                            "Failed to store symlink {} from {} in CAS",
                            file.path, pkg.name
                        )
                    })?
                } else {
                    engine.cas().store(&file.content).with_context(|| {
                        format!("Failed to store {} from {} in CAS", file.path, pkg.name)
                    })?
                };
                stored_files.push(inner::StoredInstallFile {
                    path: file.path.clone(),
                    hash,
                    size: file.size,
                    mode: file.mode,
                    symlink_target: file.symlink_target.clone(),
                });
            }
            stored_files_by_pkg.push(stored_files);
        }
        Ok(stored_files_by_pkg)
    }

    fn insert_batch_db_rows(
        tx: &Transaction<'_>,
        packages: &[PreparedPackage],
        stored_files_by_pkg: &[Vec<inner::StoredInstallFile>],
        tx_description: &str,
        tx_uuid: Option<String>,
    ) -> Result<(i64, Vec<i64>)> {
        let mut changeset = match tx_uuid {
            Some(tx_uuid) => Changeset::with_tx_uuid(tx_description.to_string(), tx_uuid),
            None => Changeset::new(tx_description.to_string()),
        };
        let changeset_id = changeset.insert(tx)?;

        let mut trove_ids: Vec<i64> = Vec::with_capacity(packages.len());

        for (pkg_idx, pkg) in packages.iter().enumerate() {
            if let Some(ref old_trove) = pkg.old_trove
                && let Some(old_id) = old_trove.id
            {
                info!(
                    "Removing old version {} of {} before upgrade",
                    old_trove.version, pkg.name
                );
                Trove::delete(tx, old_id)?;
            }

            let mut trove = pkg.to_trove(changeset_id);
            let trove_id = trove.insert(tx)?;
            trove_ids.push(trove_id);

            let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
            for comp_type in pkg.installed_components.iter() {
                let mut component = Component::from_type(trove_id, *comp_type);
                component.description = Some(format!("{} files", comp_type.as_str()));
                let comp_id = component.insert(tx)?;
                component_ids.insert(*comp_type, comp_id);
            }

            let mut path_to_component: HashMap<&str, i64> = HashMap::new();
            for (comp_type, files) in &pkg.classified_files {
                if let Some(&comp_id) = component_ids.get(comp_type) {
                    for path in files {
                        path_to_component.insert(path.as_str(), comp_id);
                    }
                }
            }

            for file in &stored_files_by_pkg[pkg_idx] {
                let hash = &file.hash;
                if hash.len() < 3 {
                    warn!(
                        "Skipping file_contents insert for '{}': hash too short ('{}')",
                        file.path, hash
                    );
                    continue;
                }

                tx.execute(
                    "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                    [
                        hash,
                        &format!("objects/{}/{}", &hash[0..2], &hash[2..]),
                        &file.size.to_string(),
                    ],
                )?;

                let component_id = path_to_component.get(file.path.as_str()).copied();

                let mut file_entry = conary_core::db::models::FileEntry::new(
                    file.path.clone(),
                    hash.clone(),
                    file.size,
                    file.mode,
                    trove_id,
                );
                file_entry.component_id = component_id;
                file_entry.symlink_target = file.symlink_target.clone();
                inner::insert_file_entry_claiming_live_root_overlap(
                    tx,
                    &mut file_entry,
                    &pkg.name,
                )?;

                let action = if pkg.is_upgrade { "modify" } else { "add" };
                tx.execute(
                    "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                    [&changeset_id.to_string(), &file.path, hash, action],
                )?;
            }

            for dep in &pkg.dependencies {
                let mut dep_entry = DependencyEntry::new(
                    trove_id,
                    dep.name.clone(),
                    None,
                    dep.dep_type.as_str().to_string(),
                    dep.version.clone(),
                );
                dep_entry.insert(tx)?;
            }

            for scriptlet in &pkg.scriptlets {
                let mut entry = ScriptletEntry::with_flags(
                    trove_id,
                    scriptlet.phase.to_string(),
                    scriptlet.interpreter.clone(),
                    scriptlet.content.clone(),
                    scriptlet.flags.clone(),
                    pkg.format.as_str(),
                );
                entry.insert(tx)?;
            }

            for lang_dep in &pkg.language_provides {
                let kind = match lang_dep.class {
                    DependencyClass::Package => "package",
                    _ => lang_dep.class.prefix(),
                };
                let mut provide = ProvideEntry::new_typed(
                    trove_id,
                    kind,
                    lang_dep.name.clone(),
                    lang_dep.version_constraint.clone(),
                );
                provide.insert_or_ignore(tx)?;
            }

            let mut pkg_provide =
                ProvideEntry::new(trove_id, pkg.name.clone(), Some(pkg.version.clone()));
            pkg_provide.insert_or_ignore(tx)?;

            if let Some(old_trove) = pkg.old_trove.as_ref() {
                super::mark_upgraded_parent_deriveds_stale(
                    tx,
                    &pkg.name,
                    Some(old_trove.version.as_str()),
                    &pkg.version,
                );
            }

            debug!(
                "Inserted trove {} (id={}) with {} files",
                pkg.name,
                trove_id,
                pkg.extracted_files.len()
            );
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok((changeset_id, trove_ids))
    }

    /// Plan the batch installation, detecting cross-package conflicts
    fn plan_batch(&self, packages: &[PreparedPackage], _conn: &Connection) -> Result<BatchPlan> {
        let mut all_paths: HashSet<String> = HashSet::new();
        let mut conflicts: Vec<BatchConflict> = Vec::new();
        let mut total_files = 0;

        // Check for cross-package file conflicts
        for pkg in packages {
            for file in &pkg.extracted_files {
                if all_paths.contains(&file.path) {
                    // Find which package already claims this path
                    for other_pkg in packages {
                        if other_pkg.name == pkg.name {
                            continue;
                        }
                        if other_pkg
                            .extracted_files
                            .iter()
                            .any(|f| f.path == file.path)
                        {
                            conflicts.push(BatchConflict::CrossPackageConflict {
                                path: file.path.clone(),
                                package1: other_pkg.name.clone(),
                                package2: pkg.name.clone(),
                            });
                            break;
                        }
                    }
                } else {
                    all_paths.insert(file.path.clone());
                }
                total_files += 1;
            }
        }

        Ok(BatchPlan {
            total_files,
            conflicts,
        })
    }

    /// Run pre-install scriptlets for a package
    fn run_pre_scripts(&self, pkg: &PreparedPackage) -> Result<()> {
        if pkg.scriptlets.is_empty() || !should_run_scriptlets(&pkg.installed_components) {
            return Ok(());
        }

        let scriptlet_format = to_scriptlet_format(pkg.format);
        let execution_mode =
            build_execution_mode(pkg.old_trove.as_ref().map(|t| t.version.as_str()));

        // For upgrades, run old package pre-remove first
        if let Some(ref old_trove) = pkg.old_trove {
            let conn = open_db(self.db_path)?;
            let old_scriptlets = get_old_package_scriptlets(&conn, old_trove.id)?;
            run_old_pre_remove(
                Path::new(self.root),
                &old_trove.name,
                &old_trove.version,
                &pkg.version,
                &old_scriptlets,
                scriptlet_format,
                self.sandbox_mode,
            )?;
        }

        run_pre_install(
            Path::new(self.root),
            &pkg.name,
            &pkg.version,
            &pkg.scriptlets,
            scriptlet_format,
            &execution_mode,
            self.sandbox_mode,
        )
    }

    /// Run post-install scriptlets for a package
    fn run_post_scripts(&self, pkg: &PreparedPackage) {
        if pkg.scriptlets.is_empty() || !should_run_scriptlets(&pkg.installed_components) {
            return;
        }

        let scriptlet_format = to_scriptlet_format(pkg.format);
        let execution_mode =
            build_execution_mode(pkg.old_trove.as_ref().map(|t| t.version.as_str()));

        run_post_install(
            Path::new(self.root),
            &pkg.name,
            &pkg.version,
            &pkg.scriptlets,
            scriptlet_format,
            &execution_mode,
            self.sandbox_mode,
        );
    }
}

/// Result of batch planning
struct BatchPlan {
    total_files: usize,
    conflicts: Vec<BatchConflict>,
}

/// Conflict detected during batch planning
#[derive(Debug)]
enum BatchConflict {
    /// Two packages in the batch both try to install the same file
    CrossPackageConflict {
        path: String,
        package1: String,
        package2: String,
    },
}

impl fmt::Display for BatchConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchConflict::CrossPackageConflict {
                path,
                package1,
                package2,
            } => write!(
                f,
                "{}: conflict between {} and {}",
                path, package1, package2
            ),
        }
    }
}

fn get_old_files_for_upgrade(
    conn: &Connection,
    old_trove: Option<&Trove>,
    new_files: &[conary_core::packages::traits::PackageFile],
) -> Result<Vec<FileToRemove>> {
    if let Some(old_trove) = old_trove
        && let Some(old_id) = old_trove.id
    {
        let new_paths: HashSet<&str> = new_files.iter().map(|f| f.path.as_str()).collect();
        super::execute::get_files_to_remove(conn, old_id, &new_paths)
    } else {
        Ok(Vec::new())
    }
}

/// Prepare a package for batch installation
///
/// This extracts the package, parses metadata, and checks for upgrades.
/// The returned `PreparedPackage` can be passed to `BatchInstaller::install_batch()`.
pub fn prepare_package_for_batch(
    package_path: &Path,
    db_path: &str,
    install_reason: &str,
    allow_downgrade: bool,
) -> Result<PreparedPackage> {
    // Detect format
    let path_str = package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;

    // Parse package
    let pkg = parse_package(package_path, format)?;

    // Open database
    let conn = open_db(db_path)?;

    // Check for existing installation
    let (is_upgrade, old_trove) = match check_upgrade_status(
        &conn,
        pkg.as_ref(),
        &InstallSemantics::legacy(format),
        allow_downgrade,
    )? {
        UpgradeCheck::FreshInstall => (false, None),
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => (true, Some(trove)),
    };

    let old_files = get_old_files_for_upgrade(&conn, old_trove.as_deref(), pkg.files())?;

    // Extract files
    info!("Extracting files from {}...", pkg.name());
    let extracted_files = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;

    // Classify files into components
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified_files = ComponentClassifier::classify_all(&file_paths);
    let installed_components: Vec<ComponentType> = classified_files.keys().copied().collect();

    // Detect language provides
    let language_provides = LanguageDepDetector::detect_all_provides(&file_paths);

    // Cache old package scriptlets before DB commit (cascade delete would lose them)
    let old_trove_id = old_trove.as_ref().and_then(|t| t.id);
    let cached_old_scriptlets = get_old_package_scriptlets(&conn, old_trove_id)?;

    Ok(PreparedPackage {
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
        format,
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        extracted_files,
        dependencies: pkg.dependencies().to_vec(),
        scriptlets: pkg.scriptlets().to_vec(),
        install_reason: install_reason.to_string(),
        is_upgrade,
        old_trove,
        old_files,
        installed_components,
        classified_files,
        language_provides,
        cached_old_scriptlets,
    })
}

/// Prepare a package for batch installation from an already-parsed package
///
/// This is useful when the package has already been parsed (e.g., in cmd_install)
/// and we want to use BatchInstaller for atomicity.
///
/// # Arguments
/// * `pkg` - Already parsed package
/// * `format` - Package format type
/// * `db_path` - Path to the database
/// * `install_reason` - Why this package is being installed
/// * `allow_downgrade` - Whether to allow downgrades
/// * `component_filter` - Optional filter for which components to install
#[allow(dead_code)] // Available for future unification of install paths
pub fn prepare_from_parsed(
    pkg: &dyn conary_core::packages::PackageFormat,
    format: PackageFormatType,
    db_path: &str,
    install_reason: &str,
    allow_downgrade: bool,
    component_filter: Option<&[ComponentType]>,
) -> Result<PreparedPackage> {
    let conn = open_db(db_path)?;

    // Check for existing installation
    let (is_upgrade, old_trove) = match check_upgrade_status(
        &conn,
        pkg,
        &InstallSemantics::legacy(format),
        allow_downgrade,
    )? {
        UpgradeCheck::FreshInstall => (false, None),
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => (true, Some(trove)),
    };

    let old_files = get_old_files_for_upgrade(&conn, old_trove.as_deref(), pkg.files())?;

    // Extract files
    info!("Extracting files from {}...", pkg.name());
    let all_extracted_files = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;

    // Classify files into components
    let file_paths: Vec<String> = all_extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified_files = ComponentClassifier::classify_all(&file_paths);

    // Filter by components if specified
    let (extracted_files, installed_components) = if let Some(filter) = component_filter {
        let filter_set: HashSet<_> = filter.iter().collect();
        let filtered: Vec<_> = all_extracted_files
            .into_iter()
            .filter(|f| {
                let comp = ComponentClassifier::classify(Path::new(&f.path));
                filter_set.contains(&comp)
            })
            .collect();
        let comps: Vec<_> = filter.to_vec();
        (filtered, comps)
    } else {
        let comps: Vec<ComponentType> = classified_files.keys().copied().collect();
        (all_extracted_files, comps)
    };

    // Detect language provides
    let installed_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let language_provides = LanguageDepDetector::detect_all_provides(&installed_paths);

    // Cache old package scriptlets before DB commit (cascade delete would lose them)
    let old_trove_id = old_trove.as_ref().and_then(|t| t.id);
    let cached_old_scriptlets = get_old_package_scriptlets(&conn, old_trove_id)?;

    Ok(PreparedPackage {
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
        format,
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        extracted_files,
        dependencies: pkg.dependencies().to_vec(),
        scriptlets: pkg.scriptlets().to_vec(),
        install_reason: install_reason.to_string(),
        is_upgrade,
        old_trove,
        old_files,
        installed_components,
        classified_files,
        language_provides,
        cached_old_scriptlets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Changeset, ChangesetStatus, FileEntry, Trove, TroveType};

    #[test]
    fn test_batch_plan_detects_cross_package_conflict() {
        // Create two packages that both try to install /usr/bin/foo
        let pkg1 = PreparedPackage {
            name: "pkg1".to_string(),
            version: "1.0".to_string(),
            format: PackageFormatType::Rpm,
            architecture: Some("x86_64".to_string()),
            description: None,
            extracted_files: vec![ExtractedFile {
                path: "/usr/bin/foo".to_string(),
                content: b"pkg1 content".to_vec(),
                size: 12,
                mode: 0o755,
                sha256: None,
                symlink_target: None,
            }],
            dependencies: Vec::new(),
            scriptlets: Vec::new(),
            install_reason: "Test".to_string(),
            is_upgrade: false,
            old_trove: None,
            old_files: Vec::new(),
            installed_components: vec![ComponentType::Runtime],
            classified_files: HashMap::new(),
            language_provides: Vec::new(),
            cached_old_scriptlets: Vec::new(),
        };

        let pkg2 = PreparedPackage {
            name: "pkg2".to_string(),
            version: "1.0".to_string(),
            format: PackageFormatType::Rpm,
            architecture: Some("x86_64".to_string()),
            description: None,
            extracted_files: vec![ExtractedFile {
                path: "/usr/bin/foo".to_string(), // Same path!
                content: b"pkg2 content".to_vec(),
                size: 12,
                mode: 0o755,
                sha256: None,
                symlink_target: None,
            }],
            dependencies: Vec::new(),
            scriptlets: Vec::new(),
            install_reason: "Test".to_string(),
            is_upgrade: false,
            old_trove: None,
            old_files: Vec::new(),
            installed_components: vec![ComponentType::Runtime],
            classified_files: HashMap::new(),
            language_provides: Vec::new(),
            cached_old_scriptlets: Vec::new(),
        };

        let installer = BatchInstaller::new("/tmp/test.db", "/", SandboxMode::None, true);
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let plan = installer.plan_batch(&[pkg1, pkg2], &conn).unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            BatchConflict::CrossPackageConflict {
                path,
                package1,
                package2,
            } => {
                assert_eq!(path, "/usr/bin/foo");
                assert!(package1 == "pkg1" || package1 == "pkg2");
                assert!(package2 == "pkg1" || package2 == "pkg2");
                assert_ne!(package1, package2);
            }
        }
    }

    #[test]
    fn test_prepared_package_to_trove() {
        let pkg = PreparedPackage {
            name: "test-pkg".to_string(),
            version: "1.2.3".to_string(),
            format: PackageFormatType::Deb,
            architecture: Some("amd64".to_string()),
            description: Some("Test package".to_string()),
            extracted_files: Vec::new(),
            dependencies: Vec::new(),
            scriptlets: Vec::new(),
            install_reason: "Required by nginx".to_string(),
            is_upgrade: false,
            old_trove: None,
            old_files: Vec::new(),
            installed_components: Vec::new(),
            classified_files: HashMap::new(),
            language_provides: Vec::new(),
            cached_old_scriptlets: Vec::new(),
        };

        let trove = pkg.to_trove(42);

        assert_eq!(trove.name, "test-pkg");
        assert_eq!(trove.version, "1.2.3");
        assert_eq!(trove.architecture, Some("amd64".to_string()));
        assert_eq!(trove.installed_by_changeset_id, Some(42));
        assert_eq!(
            trove.install_reason,
            conary_core::db::models::InstallReason::Dependency
        );
    }

    #[test]
    fn batch_install_preflights_before_pre_scripts() {
        let source = include_str!("batch.rs");
        let install_batch_start = source
            .find("pub fn install_batch")
            .expect("install_batch should exist");
        let plan_batch_start = source[install_batch_start..]
            .find("fn plan_batch")
            .expect("plan_batch boundary should exist");
        let install_batch_source =
            &source[install_batch_start..install_batch_start + plan_batch_start];

        let preflight_pos = install_batch_source
            .find("prepare_install_environment_before_scriptlets")
            .expect("install_batch should preflight before scriptlets");
        let scripts_pos = install_batch_source
            .find("self.run_pre_scripts(pkg)")
            .expect("install_batch should run pre-install scripts");

        assert!(
            preflight_pos < scripts_pos,
            "batch installs must validate generation state before dependency pre-install scriptlets"
        );
    }

    #[test]
    fn generation_rebuild_failure_records_deferred_follow_up_for_applied_batch() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut changeset = Changeset::new("Batch install: fixture".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        BatchInstaller::record_generation_rebuild_failure(
            &conn,
            changeset_id,
            anyhow::anyhow!("composefs build failed"),
        )
        .unwrap();

        let changeset = Changeset::find_by_id(&conn, changeset_id)
            .unwrap()
            .expect("changeset should exist");
        assert_eq!(changeset.status, ChangesetStatus::Applied);
        let deferred = crate::commands::deferred_follow_up(changeset.metadata.as_deref());
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].kind, "generation_rebuild");
        assert_eq!(deferred[0].status, "failed");
        assert!(deferred[0].message.contains("composefs build failed"));
        assert!(
            deferred[0]
                .retry_command
                .as_deref()
                .unwrap()
                .contains("system generation build")
        );
    }

    fn prepared_test_package(
        name: &str,
        path: &str,
        content: &[u8],
        scriptlets: Vec<Scriptlet>,
    ) -> PreparedPackage {
        PreparedPackage {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            format: PackageFormatType::Rpm,
            architecture: Some("x86_64".to_string()),
            description: None,
            extracted_files: vec![ExtractedFile {
                path: path.to_string(),
                content: content.to_vec(),
                size: content.len() as i64,
                mode: 0o100755,
                sha256: None,
                symlink_target: None,
            }],
            dependencies: Vec::new(),
            scriptlets,
            install_reason: "Test".to_string(),
            is_upgrade: false,
            old_trove: None,
            old_files: Vec::new(),
            installed_components: vec![ComponentType::Runtime],
            classified_files: HashMap::from([(ComponentType::Runtime, vec![path.to_string()])]),
            language_provides: Vec::new(),
            cached_old_scriptlets: Vec::new(),
        }
    }

    fn prepared_test_symlink_package(name: &str, path: &str, target: &str) -> PreparedPackage {
        let mut package = prepared_test_package(name, path, &[], vec![]);
        package.extracted_files[0].size = target.len() as i64;
        package.extracted_files[0].mode = 0o120777;
        package.extracted_files[0].symlink_target = Some(target.to_string());
        package
    }

    #[test]
    fn no_generation_batch_install_materializes_live_root_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();

        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();
        let package = prepared_test_package(
            "batch-fixture",
            "/usr/bin/batch-fixture",
            b"batch-live",
            vec![],
        );
        let installer =
            BatchInstaller::new(&db_path_string, &root_string, SandboxMode::Always, true)
                .with_preflighted_execution_path(PackageExecutionPath::MutableLiveRoot);

        installer.install_batch(vec![package]).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("usr/bin/batch-fixture")).unwrap(),
            "batch-live"
        );
        let conn = conary_core::db::open(&db_path).unwrap();
        let file = FileEntry::find_by_path(&conn, "/usr/bin/batch-fixture")
            .unwrap()
            .expect("batch file should be recorded in DB");
        let owner = Trove::find_by_id(&conn, file.trove_id)
            .unwrap()
            .expect("batch file owner should exist");
        assert_eq!(owner.name, "batch-fixture");
        let changesets = conary_core::db::models::Changeset::list_all(&conn).unwrap();
        assert_eq!(changesets.len(), 1);
        assert_eq!(changesets[0].status, ChangesetStatus::Applied);
        let journal_dir = temp.path().join("live-root-journals");
        assert!(!journal_dir.exists() || std::fs::read_dir(&journal_dir).unwrap().next().is_none());
    }

    #[test]
    fn no_generation_batch_install_materializes_live_root_symlink_from_cas() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        std::fs::create_dir_all(&root).unwrap();
        conary_core::db::init(&db_path).unwrap();

        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();
        let package = prepared_test_symlink_package(
            "batch-link-fixture",
            "/usr/bin/batch-link",
            "batch-target",
        );
        let installer =
            BatchInstaller::new(&db_path_string, &root_string, SandboxMode::Always, true)
                .with_preflighted_execution_path(PackageExecutionPath::MutableLiveRoot);

        installer.install_batch(vec![package]).unwrap();

        assert_eq!(
            std::fs::read_link(root.join("usr/bin/batch-link")).unwrap(),
            PathBuf::from("batch-target")
        );
        let conn = conary_core::db::open(&db_path).unwrap();
        let file = FileEntry::find_by_path(&conn, "/usr/bin/batch-link")
            .unwrap()
            .expect("batch symlink should be recorded in DB");
        assert_eq!(file.symlink_target.as_deref(), Some("batch-target"));
    }

    #[test]
    fn no_generation_batch_install_conflict_preflight_runs_before_scripts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let db_path = temp.path().join("conary.db");
        let live_file = root.join("usr/bin/batch-fixture");
        let marker = root.join("batch-pre-scriptlet-ran");
        std::fs::create_dir_all(live_file.parent().unwrap()).unwrap();
        std::fs::write(&live_file, "owned elsewhere").unwrap();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut other_trove = Trove::new(
            "other-owner".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let other_trove_id = other_trove.insert(&conn).unwrap();
        let mut existing = FileEntry::new(
            "/usr/bin/batch-fixture".to_string(),
            "other-hash".to_string(),
            15,
            0o100755,
            other_trove_id,
        );
        existing.insert(&conn).unwrap();

        let package = prepared_test_package(
            "batch-fixture",
            "/usr/bin/batch-fixture",
            b"replacement",
            vec![Scriptlet {
                phase: conary_core::packages::traits::ScriptletPhase::PreInstall,
                interpreter: "/bin/sh".to_string(),
                content: format!("touch {}", marker.display()),
                flags: None,
            }],
        );
        let db_path_string = db_path.to_string_lossy().into_owned();
        let root_string = root.to_string_lossy().into_owned();
        let installer =
            BatchInstaller::new(&db_path_string, &root_string, SandboxMode::Always, false)
                .with_preflighted_execution_path(PackageExecutionPath::MutableLiveRoot);

        let error = installer.install_batch(vec![package]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Path /usr/bin/batch-fixture is already tracked by package other-owner"),
            "{error:#}"
        );
        assert!(!marker.exists(), "pre-install scriptlet must not run");
        assert_eq!(
            std::fs::read_to_string(live_file).unwrap(),
            "owned elsewhere"
        );
    }
}
