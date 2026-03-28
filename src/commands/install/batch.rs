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
use super::prepare::{UpgradeCheck, check_upgrade_status, parse_package};
use super::scriptlets::{
    build_execution_mode, get_old_package_scriptlets, run_old_post_remove, run_old_pre_remove,
    run_post_install, run_pre_install, to_scriptlet_format,
};
use super::{PackageFormatType, detect_package_format};
use anyhow::{Context, Result};
use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
use conary_core::db::models::{
    Changeset, ChangesetStatus, Component, DependencyEntry, ProvideEntry, ScriptletEntry, Trove,
};
use conary_core::dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
use conary_core::packages::traits::{ExtractedFile, Scriptlet};
use conary_core::scriptlet::SandboxMode;
use conary_core::transaction::{FileToRemove, TransactionConfig, TransactionEngine};
use rusqlite::Connection;
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
        let mut conn = open_db(self.db_path)?;

        // Create composefs-native transaction engine
        let db_path_buf = PathBuf::from(self.db_path);
        let tx_config = TransactionConfig::from_paths(PathBuf::from(self.root), db_path_buf);
        let mut engine =
            TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

        // Recover any incomplete transactions (only if generations exist)
        if engine.config().generations_dir.exists() {
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
        // Keyed by (package index, file index) so Phase 4 can look them up.
        let mut cas_hashes: HashMap<(usize, usize), String> = HashMap::new();
        for (pkg_idx, pkg) in packages.iter().enumerate() {
            info!(
                "[{}/{}] Storing files in CAS: {} {}",
                pkg_idx + 1,
                package_count,
                pkg.name,
                pkg.version
            );

            for (file_idx, file) in pkg.extracted_files.iter().enumerate() {
                let hash = engine.cas().store(&file.content).with_context(|| {
                    format!("Failed to store {} from {} in CAS", file.path, pkg.name)
                })?;
                cas_hashes.insert((pkg_idx, file_idx), hash);
            }
        }

        info!("Batch CAS storage complete: {} packages", package_count);

        // Phase 4: Single DB transaction for ALL packages
        let db_result = conary_core::db::transaction(&mut conn, |tx| {
            // Create single changeset for entire batch
            let mut changeset = Changeset::new(tx_description.clone());
            let changeset_id = changeset.insert(tx)?;

            let mut trove_ids: Vec<i64> = Vec::with_capacity(packages.len());

            for (pkg_idx, pkg) in packages.iter().enumerate() {
                // Remove old trove if upgrading
                if let Some(ref old_trove) = pkg.old_trove
                    && let Some(old_id) = old_trove.id
                {
                    info!(
                        "Removing old version {} of {} before upgrade",
                        old_trove.version, pkg.name
                    );
                    Trove::delete(tx, old_id)?;
                }

                // Insert new trove
                let mut trove = pkg.to_trove(changeset_id);
                let trove_id = trove.insert(tx)?;
                trove_ids.push(trove_id);

                // Create components
                let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
                for comp_type in pkg.installed_components.iter() {
                    let mut component = Component::from_type(trove_id, *comp_type);
                    component.description = Some(format!("{} files", comp_type.as_str()));
                    let comp_id = component.insert(tx)?;
                    component_ids.insert(*comp_type, comp_id);
                }

                // Build path-to-component-id lookup
                let mut path_to_component: HashMap<&str, i64> = HashMap::new();
                for (comp_type, files) in &pkg.classified_files {
                    if let Some(&comp_id) = component_ids.get(comp_type) {
                        for path in files {
                            path_to_component.insert(path.as_str(), comp_id);
                        }
                    }
                }

                // Insert files -- use the authoritative CAS hash from Phase 3,
                // falling back to the embedded sha256 only as a last resort.
                for (file_idx, file) in pkg.extracted_files.iter().enumerate() {
                    let hash = cas_hashes
                        .get(&(pkg_idx, file_idx))
                        .cloned()
                        .or_else(|| file.sha256.clone())
                        .unwrap_or_default();

                    if hash.len() < 3 {
                        warn!(
                            "Skipping file_contents insert for '{}': hash too short ('{}')",
                            file.path, hash
                        );
                        continue;
                    }

                    tx.execute(
                        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                        [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
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
                    file_entry.insert(tx)?;

                    // Record in history
                    let action = if pkg.is_upgrade { "modify" } else { "add" };
                    tx.execute(
                        "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                        [&changeset_id.to_string(), &file.path, &hash, action],
                    )?;
                }

                // Insert dependencies
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

                // Store scriptlets
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

                // Store provides
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

                // Store package name as provide
                let mut pkg_provide =
                    ProvideEntry::new(trove_id, pkg.name.clone(), Some(pkg.version.clone()));
                pkg_provide.insert_or_ignore(tx)?;

                debug!(
                    "Inserted trove {} (id={}) with {} files",
                    pkg.name,
                    trove_id,
                    pkg.extracted_files.len()
                );
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok((changeset_id, trove_ids))
        });

        // Handle DB result
        let (changeset_id, trove_ids) = match db_result {
            Ok((cs_id, tr_ids)) => {
                info!(
                    "Batch DB commit successful: changeset={}, {} troves",
                    cs_id,
                    tr_ids.len()
                );
                (cs_id, tr_ids)
            }
            Err(e) => {
                // DB failed - release lock and bail
                engine.release_lock();
                return Err(anyhow::anyhow!("Batch database transaction failed: {}", e));
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

        // Phase 7: Build EROFS image and mount new generation
        let _gen_num = crate::commands::composefs_ops::rebuild_and_mount(
            &conn,
            &format!("Batch install: {}", main_pkg_name),
            None,
            std::path::Path::new("/conary"),
        )?;

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
    let (is_upgrade, old_trove) =
        match check_upgrade_status(&conn, pkg.as_ref(), format, allow_downgrade)? {
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
    let (is_upgrade, old_trove) = match check_upgrade_status(&conn, pkg, format, allow_downgrade)? {
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
}
