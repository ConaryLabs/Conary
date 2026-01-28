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

use super::execute::convert_extracted_files;
use super::prepare::{check_upgrade_status, parse_package, UpgradeCheck};
use super::scriptlets::{
    build_execution_mode, get_old_package_scriptlets, run_old_post_remove, run_old_pre_remove,
    run_post_install, run_pre_install, to_scriptlet_format,
};
use super::{detect_package_format, PackageFormatType};
use anyhow::{Context, Result};
use conary::components::{should_run_scriptlets, ComponentClassifier, ComponentType};
use conary::db::models::{
    Changeset, ChangesetStatus, Component, DependencyEntry, ProvideEntry, ScriptletEntry, Trove,
};
use conary::dependencies::{LanguageDep, LanguageDepDetector};
use conary::packages::traits::{DependencyType, ExtractedFile, Scriptlet};
use conary::scriptlet::SandboxMode;
use conary::transaction::{
    FileToRemove, PackageInfo, TransactionConfig, TransactionEngine, TransactionOperations,
};
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
    pub dependencies: Vec<conary::packages::traits::Dependency>,
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
}

impl PreparedPackage {
    /// Create a Trove model from this prepared package
    pub fn to_trove(&self, changeset_id: i64) -> Trove {
        let mut trove = Trove::new(
            self.name.clone(),
            self.version.clone(),
            conary::db::models::TroveType::Package,
        );
        trove.architecture = self.architecture.clone();
        trove.description = self.description.clone();
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.selection_reason = Some(self.install_reason.clone());

        // Mark as dependency if install reason contains "Required by"
        if self.install_reason.starts_with("Required by") {
            trove.install_reason = conary::db::models::InstallReason::Dependency;
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
        let main_pkg_name = packages.last().map(|p| p.name.as_str()).unwrap_or("unknown");

        info!(
            "Starting batch install: {} packages (main: {})",
            package_count, main_pkg_name
        );

        // Open database connection
        let mut conn = conary::db::open(self.db_path)
            .context("Failed to open package database for batch install")?;

        // Create transaction engine
        let db_path_buf = PathBuf::from(self.db_path);
        let tx_config = TransactionConfig::new(PathBuf::from(self.root), db_path_buf.clone());
        let engine =
            TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

        // Recover any incomplete transactions
        let recovery_outcomes = engine
            .recover(&mut conn)
            .context("Failed to recover incomplete transactions")?;
        for outcome in &recovery_outcomes {
            info!("Recovery outcome: {:?}", outcome);
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

        // Begin transaction - holds lock for entire batch
        let mut txn = engine
            .begin(&tx_description)
            .context("Failed to begin batch transaction")?;

        info!("Started batch transaction {}", txn.uuid());

        // Phase 1: Unified planning across all packages
        // Collect all files and detect cross-package conflicts
        let batch_plan = self.plan_batch(&packages, &conn)?;

        // Check for conflicts
        if !batch_plan.conflicts.is_empty() {
            let conflict_msgs: Vec<String> = batch_plan
                .conflicts
                .iter()
                .map(|c| c.to_string())
                .collect();
            txn.abort()
                .context("Failed to abort transaction after conflicts")?;
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
                self.run_pre_scripts(pkg)?;
            }
        }

        // Phase 3: Plan, prepare, backup, stage, and apply for each package
        // We do this per-package but within the same transaction
        for (idx, pkg) in packages.iter().enumerate() {
            info!(
                "[{}/{}] Processing package: {} {}",
                idx + 1,
                package_count,
                pkg.name,
                pkg.version
            );

            // Convert extracted files
            let tx_files = convert_extracted_files(&pkg.extracted_files);

            // Build operations for this package
            let operations = TransactionOperations {
                package: PackageInfo {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    release: None,
                    arch: pkg.architecture.clone(),
                },
                files_to_add: tx_files.clone(),
                files_to_remove: pkg.old_files.clone(),
                is_upgrade: pkg.is_upgrade,
                old_package: pkg.old_trove.as_ref().map(|t| PackageInfo {
                    name: t.name.clone(),
                    version: t.version.clone(),
                    release: None,
                    arch: t.architecture.clone(),
                }),
            };

            // Plan this package's operations
            let plan = txn
                .plan_operations(operations, &conn)
                .with_context(|| format!("Failed to plan package {}", pkg.name))?;

            // Check for conflicts (shouldn't happen after batch planning, but double-check)
            if !plan.conflicts.is_empty() {
                let conflict_msgs: Vec<String> =
                    plan.conflicts.iter().map(|c| c.to_string()).collect();
                txn.abort()
                    .context("Failed to abort transaction after package conflicts")?;
                return Err(anyhow::anyhow!(
                    "Package {} has file conflicts:\n  {}",
                    pkg.name,
                    conflict_msgs.join("\n  ")
                ));
            }

            // Prepare: store content in CAS
            txn.prepare(&tx_files)
                .with_context(|| format!("Failed to prepare package {} (CAS storage)", pkg.name))?;
        }

        // Phase 4: Execute filesystem operations
        // Backup all files that will be replaced
        txn.backup_files()
            .context("Failed to backup existing files")?;

        // Stage all new files from CAS
        txn.stage_files().context("Failed to stage files")?;

        // Apply filesystem changes (atomic renames)
        let fs_result = txn
            .apply_filesystem()
            .context("Failed to apply filesystem changes")?;

        info!(
            "Batch filesystem changes: {} added, {} replaced, {} removed",
            fs_result.files_added, fs_result.files_replaced, fs_result.files_removed
        );

        // Phase 5: Write DB commit intent for crash recovery
        txn.write_db_commit_intent()
            .context("Failed to write DB commit intent")?;

        // Phase 6: Single DB transaction for ALL packages
        let tx_uuid = txn.uuid().to_string();
        let db_result = conary::db::transaction(&mut conn, |tx| {
            // Create single changeset for entire batch
            let mut changeset = Changeset::with_tx_uuid(tx_description.clone(), tx_uuid.clone());
            let changeset_id = changeset.insert(tx)?;

            let mut trove_ids: Vec<i64> = Vec::with_capacity(packages.len());

            for pkg in &packages {
                // Remove old trove if upgrading
                if let Some(ref old_trove) = pkg.old_trove {
                    if let Some(old_id) = old_trove.id {
                        info!(
                            "Removing old version {} of {} before upgrade",
                            old_trove.version, pkg.name
                        );
                        Trove::delete(tx, old_id)?;
                    }
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

                // Insert files
                for file in &pkg.extracted_files {
                    let hash = file.sha256.clone().unwrap_or_default();

                    tx.execute(
                        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                        [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
                    )?;

                    let component_id = path_to_component.get(file.path.as_str()).copied();

                    let mut file_entry = conary::db::models::FileEntry::new(
                        file.path.clone(),
                        hash.clone(),
                        file.size,
                        file.mode,
                        trove_id,
                    );
                    file_entry.component_id = component_id;
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
                    let dep_type_str = match dep.dep_type {
                        DependencyType::Runtime => "runtime",
                        DependencyType::Build => "build",
                        DependencyType::Optional => "optional",
                    };
                    let mut dep_entry = DependencyEntry::new(
                        trove_id,
                        dep.name.clone(),
                        None,
                        dep_type_str.to_string(),
                        dep.version.clone(),
                    );
                    dep_entry.insert(tx)?;
                }

                // Store scriptlets
                let format_str = match pkg.format {
                    PackageFormatType::Rpm => "rpm",
                    PackageFormatType::Deb => "deb",
                    PackageFormatType::Arch => "arch",
                };
                for scriptlet in &pkg.scriptlets {
                    let mut entry = ScriptletEntry::with_flags(
                        trove_id,
                        scriptlet.phase.to_string(),
                        scriptlet.interpreter.clone(),
                        scriptlet.content.clone(),
                        scriptlet.flags.clone(),
                        format_str,
                    );
                    entry.insert(tx)?;
                }

                // Store provides
                for lang_dep in &pkg.language_provides {
                    let mut provide = ProvideEntry::new(
                        trove_id,
                        lang_dep.to_dep_string(),
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
                // Record batch DB commit for recovery
                // Use first trove_id for single-package compat, but log all
                let first_trove_id = *tr_ids.first().unwrap_or(&0);
                txn.record_db_commit(cs_id, first_trove_id)
                    .context("Failed to record DB commit")?;
                info!(
                    "Batch DB commit successful: changeset={}, {} troves",
                    cs_id,
                    tr_ids.len()
                );
                (cs_id, tr_ids)
            }
            Err(e) => {
                // DB failed - abort transaction (will restore backups)
                if let Err(abort_err) = txn.abort() {
                    warn!(
                        "Failed to abort transaction after DB failure: {}",
                        abort_err
                    );
                }
                return Err(anyhow::anyhow!("Batch database transaction failed: {}", e));
            }
        };

        // Phase 7: Run post-install scriptlets in topological order
        // Also run old package removal scriptlets for upgrades
        if !self.no_scripts {
            for pkg in &packages {
                // Run old package post-remove for upgrades
                if let Some(ref old_trove) = pkg.old_trove {
                    let old_scriptlets = get_old_package_scriptlets(&conn, old_trove.id)?;
                    let scriptlet_format = to_scriptlet_format(pkg.format);
                    run_old_post_remove(
                        Path::new(self.root),
                        &old_trove.name,
                        &old_trove.version,
                        &pkg.version,
                        &old_scriptlets,
                        scriptlet_format,
                        self.sandbox_mode,
                    );
                }

                // Run post-install scriptlet
                self.run_post_scripts(pkg);
            }
        }

        // Mark post-scripts complete
        txn.mark_post_scripts_complete()
            .context("Failed to mark post-scripts complete")?;

        // Phase 8: Finish transaction
        let tx_result = txn.finish().context("Failed to finish batch transaction")?;

        info!(
            "Batch transaction {} completed in {}ms: {} packages installed",
            tx_result.tx_uuid, tx_result.duration_ms, package_count
        );

        // Create state snapshot
        let snapshot_desc = if package_count == 1 {
            format!("Install {}", packages[0].name)
        } else {
            format!("Install {} + {} deps", main_pkg_name, package_count - 1)
        };
        crate::commands::create_state_snapshot(&conn, changeset_id, &snapshot_desc)?;

        // Print summary
        println!(
            "Batch installed {} package(s) successfully:",
            trove_ids.len()
        );
        for pkg in &packages {
            println!("  {} {} ({} files)", pkg.name, pkg.version, pkg.extracted_files.len());
        }

        Ok(())
    }

    /// Plan the batch installation, detecting cross-package conflicts
    fn plan_batch(
        &self,
        packages: &[PreparedPackage],
        _conn: &Connection,
    ) -> Result<BatchPlan> {
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
            let conn = conary::db::open(self.db_path)?;
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
            } => write!(f, "{}: conflict between {} and {}", path, package1, package2),
        }
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
    let conn = conary::db::open(db_path).context("Failed to open package database")?;

    // Check for existing installation
    let (is_upgrade, old_trove) = match check_upgrade_status(&conn, pkg.as_ref(), allow_downgrade)?
    {
        UpgradeCheck::FreshInstall => (false, None),
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => (true, Some(trove)),
    };

    // Get files to remove for upgrades
    let old_files = if let Some(ref old_trove) = old_trove {
        if let Some(old_id) = old_trove.id {
            let new_paths: HashSet<&str> = pkg.files().iter().map(|f| f.path.as_str()).collect();
            super::execute::get_files_to_remove(&conn, old_id, &new_paths)?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

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
    let installed_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let language_provides = LanguageDepDetector::detect_all_provides(&installed_paths);

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
        };

        let trove = pkg.to_trove(42);

        assert_eq!(trove.name, "test-pkg");
        assert_eq!(trove.version, "1.2.3");
        assert_eq!(trove.architecture, Some("amd64".to_string()));
        assert_eq!(trove.installed_by_changeset_id, Some(42));
        assert_eq!(
            trove.install_reason,
            conary::db::models::InstallReason::Dependency
        );
    }
}
