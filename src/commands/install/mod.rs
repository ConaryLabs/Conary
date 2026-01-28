// src/commands/install/mod.rs
//! Package installation commands

mod batch;
mod conversion;
mod dependencies;
mod execute;
mod prepare;
mod resolve;
mod scriptlets;

pub use batch::{prepare_package_for_batch, BatchInstaller};

pub use prepare::{ComponentSelection, UpgradeCheck};

use conversion::{install_converted_ccs, try_convert_to_ccs, ConversionResult};
use dependencies::build_dependency_edges;
use execute::{convert_extracted_files, get_files_to_remove};
use prepare::{check_upgrade_status, parse_package};
use resolve::{check_provides_dependencies, resolve_package_path, ResolvedSourceType};
use scriptlets::{
    build_execution_mode, get_old_package_scriptlets, run_old_post_remove, run_old_pre_remove,
    run_post_install, run_pre_install, to_scriptlet_format,
};

use super::create_state_snapshot;
use super::progress::{InstallPhase, InstallProgress};
use super::{detect_package_format, PackageFormatType};
use anyhow::{Context, Result};
use conary::components::{parse_component_spec, should_run_scriptlets, ComponentClassifier, ComponentType};
use conary::db::models::{Changeset, ChangesetStatus, Component, ProvideEntry, ScriptletEntry};
use conary::db::paths::keyring_dir;
use conary::dependencies::LanguageDepDetector;
use conary::packages::traits::DependencyType;
use conary::repository;
use conary::resolver::Resolver;
use conary::scriptlet::SandboxMode;
use conary::transaction::{
    PackageInfo, TransactionConfig,
    TransactionEngine, TransactionOperations,
};
use conary::version::RpmVersion;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Install a package
///
/// Uses the unified resolution flow with per-package routing strategies.
/// Packages can be resolved from binary repos, on-demand converters, or recipes
/// based on their routing table entries.
///
/// # Arguments
/// * `package` - Package name or path
/// * `db_path` - Path to the database
/// * `root` - Filesystem root for installation
/// * `version` - Specific version to install (optional)
/// * `repo` - Specific repository to use (optional)
/// * `dry_run` - Preview without installing
/// * `no_deps` - Skip dependency resolution
/// * `no_scripts` - Skip scriptlet execution
/// * `selection_reason` - Human-readable reason for installation (e.g., "Installed via @server")
/// * `sandbox_mode` - Sandbox mode for scriptlet execution
/// * `allow_downgrade` - Allow installing older versions
/// * `convert_to_ccs` - Convert legacy packages to CCS format during install
#[allow(clippy::too_many_arguments)]
pub fn cmd_install(
    package: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
    repo: Option<String>,
    dry_run: bool,
    no_deps: bool,
    no_scripts: bool,
    selection_reason: Option<&str>,
    sandbox_mode: SandboxMode,
    allow_downgrade: bool,
    convert_to_ccs: bool,
    no_capture: bool,
) -> Result<()> {
    // Parse component spec from package argument (e.g., "nginx:devel" or "nginx:all")
    let (package_name, component_selection) = if let Some((pkg, comp)) = parse_component_spec(package) {
        let selection = if comp == "all" {
            ComponentSelection::All
        } else if let Some(comp_type) = ComponentType::parse(&comp) {
            ComponentSelection::Specific(vec![comp_type])
        } else {
            return Err(anyhow::anyhow!(
                "Unknown component '{}'. Valid components: runtime, lib, devel, doc, config, all",
                comp
            ));
        };
        (pkg, selection)
    } else {
        // No component spec - install defaults only
        (package.to_string(), ComponentSelection::Defaults)
    };

    info!("Installing package: {} (components: {})", package_name, component_selection.display());

    // Check if the package is already installed as a dependency - if so, promote it
    // This must happen before we try to download, as we may not need to do anything else
    {
        let conn = conary::db::open(db_path)
            .context("Failed to open package database for promotion check")?;

        if let Some(existing) = conary::db::models::Trove::find_one_by_name(&conn, &package_name)?
            && existing.install_reason == conary::db::models::InstallReason::Dependency
        {
            // Check if we're requesting a specific version that differs
            let needs_version_change = version.as_ref().is_some_and(|v| v != &existing.version);

            // Promote to explicit
            let reason = selection_reason.unwrap_or("Explicitly installed by user");
            conary::db::models::Trove::promote_to_explicit(&conn, &package_name, Some(reason))?;
            println!("Promoted {} from dependency to explicit", package_name);

            // If same version (or no version specified), we're done
            if !needs_version_change {
                println!("{} {} is already installed", package_name, existing.version);
                return Ok(());
            }
            // Otherwise continue with version upgrade
            info!("Continuing with version change: {} -> {:?}", existing.version, version);
        }
    }

    // Create progress tracker for single package installation
    let progress = InstallProgress::single("Installing");
    progress.set_phase(&package_name, InstallPhase::Downloading);

    // Resolve package path (download if needed)
    let resolved = match resolve_package_path(
        &package_name,
        db_path,
        version.as_deref(),
        repo.as_deref(),
        &progress,
    ) {
        Ok(r) => r,
        Err(e) => {
            // Check if this is an "already installed" response from the resolver
            let err_str = e.to_string();
            if let Some(rest) = err_str.strip_prefix("ALREADY_INSTALLED:") {
                let parts: Vec<&str> = rest.splitn(2, ':').collect();
                let (name, ver) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    (&package_name as &str, "unknown")
                };
                println!("{} {} is already installed (skipping download)", name, ver);
                return Ok(());
            }
            return Err(e);
        }
    };

    // If resolved from Remi, it's already CCS format - install directly
    if resolved.source_type == ResolvedSourceType::Remi {
        info!("Package from Remi is already CCS format, installing directly");
        let ccs_path = resolved.path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid CCS path (non-UTF8)"))?;
        return install_converted_ccs(ccs_path, db_path, root, dry_run, sandbox_mode, no_deps);
    }

    let path_str = resolved.path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    // Check if it's a CCS package by extension (from update command or local file)
    if path_str.ends_with(".ccs") {
        info!("Detected CCS package from path extension, installing directly");
        return install_converted_ccs(path_str, db_path, root, dry_run, sandbox_mode, no_deps);
    }

    // Detect format and parse legacy packages
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

    progress.set_phase(package, InstallPhase::Parsing);
    let pkg = parse_package(&resolved.path, format)?;

    // Convert to CCS format if requested (only for legacy packages)
    if convert_to_ccs {
        progress.set_status(&format!("Converting {} to CCS format...", pkg.name()));

        match try_convert_to_ccs(pkg.as_ref(), &resolved.path, format, db_path, !no_capture)? {
            ConversionResult::Converted { ccs_path, temp_dir: _temp_dir } => {
                // Install via CCS path (temp_dir kept alive until install completes)
                return install_converted_ccs(&ccs_path, db_path, root, dry_run, sandbox_mode, no_deps);
            }
            ConversionResult::Skipped => {
                // Already converted - fall through to regular install path
            }
        }
    }

    let mut conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    // Build dependency edges from the package
    let package_version = RpmVersion::parse(pkg.version())
        .with_context(|| format!("Failed to parse version '{}' for package '{}'", pkg.version(), pkg.name()))?;
    let dependency_edges = build_dependency_edges(pkg.as_ref());

    if no_deps && !dependency_edges.is_empty() {
        info!("Skipping dependency check (--no-deps specified)");
        println!(
            "Skipping {} dependencies (--no-deps specified)",
            dependency_edges.len()
        );
    } else if !dependency_edges.is_empty() {
        progress.set_phase(pkg.name(), InstallPhase::ResolvingDeps);
        info!(
            "Resolving {} dependencies with constraint validation...",
            dependency_edges.len()
        );
        println!("Checking dependencies for {}...", pkg.name());

        // Build resolver from current system state
        let mut resolver = Resolver::new(&conn)
            .context("Failed to initialize dependency resolver")?;

        // Resolve with the new package
        let plan = resolver.resolve_install(
            pkg.name().to_string(),
            package_version.clone(),
            dependency_edges,
        ).with_context(|| format!("Failed to resolve dependencies for '{}'", pkg.name()))?;

        // Check for conflicts (fail on any conflict)
        if !plan.conflicts.is_empty() {
            eprintln!("\nDependency conflicts detected:");
            for conflict in &plan.conflicts {
                eprintln!("  {}", conflict);
            }
            return Err(anyhow::anyhow!(
                "Cannot install {}: {} dependency conflict(s) detected",
                pkg.name(),
                plan.conflicts.len()
            ));
        }

        // Handle missing dependencies
        if !plan.missing.is_empty() {
            info!("Found {} missing dependencies", plan.missing.len());

            // Try to find missing deps in repositories
            let missing_names: Vec<String> = plan.missing.iter().map(|m| m.name.clone()).collect();

            match repository::resolve_dependencies_transitive(&conn, &missing_names, 10) {
                Ok(to_download) => {
                    if !to_download.is_empty() {
                        if dry_run {
                            println!("Would install {} missing dependencies:", to_download.len());
                        } else {
                            println!("Installing {} missing dependencies:", to_download.len());
                        }
                        for (dep_name, pkg) in &to_download {
                            println!("  {} ({})", dep_name, pkg.package.version);
                        }

                        if !dry_run {
                            progress.set_phase(pkg.name(), InstallPhase::InstallingDeps);
                            let temp_dir = TempDir::new()?;
                            let keyring_dir = keyring_dir(db_path);
                            match repository::download_dependencies(&to_download, temp_dir.path(), Some(&keyring_dir)) {
                                Ok(downloaded) => {
                                    // Use batch installer for atomic dependency installation
                                    // This ensures all dependencies are installed in a single
                                    // transaction - if any fails, all are rolled back.
                                    let parent_name = pkg.name().to_string();
                                    let mut prepared_packages = Vec::with_capacity(downloaded.len());

                                    // Prepare all dependencies
                                    for (dep_name, dep_path) in &downloaded {
                                        progress.set_status(&format!("Preparing dependency: {}", dep_name));
                                        info!("Preparing dependency: {}", dep_name);
                                        let reason = format!("Required by {}", parent_name);
                                        match prepare_package_for_batch(
                                            dep_path,
                                            db_path,
                                            &reason,
                                            allow_downgrade,
                                        ) {
                                            Ok(prepared) => {
                                                prepared_packages.push(prepared);
                                            }
                                            Err(e) => {
                                                // Check for "already installed" error
                                                let err_str = e.to_string();
                                                if err_str.contains("already installed") {
                                                    info!("Dependency {} already installed, skipping", dep_name);
                                                    continue;
                                                }
                                                return Err(anyhow::anyhow!(
                                                    "Failed to prepare dependency {}: {}",
                                                    dep_name,
                                                    e
                                                ));
                                            }
                                        }
                                    }

                                    // Install all dependencies atomically
                                    if !prepared_packages.is_empty() {
                                        progress.set_status(&format!(
                                            "Installing {} dependencies atomically...",
                                            prepared_packages.len()
                                        ));
                                        info!(
                                            "Installing {} dependencies atomically",
                                            prepared_packages.len()
                                        );

                                        let installer = BatchInstaller::new(
                                            db_path,
                                            root,
                                            sandbox_mode,
                                            no_scripts,
                                        );

                                        if let Err(e) = installer.install_batch(prepared_packages) {
                                            return Err(anyhow::anyhow!(
                                                "Failed to install dependencies atomically: {}",
                                                e
                                            ));
                                        }

                                        println!("  [OK] Installed {} dependencies atomically", downloaded.len());
                                    }
                                }
                                Err(e) => {
                                    return Err(anyhow::anyhow!(
                                        "Failed to download dependencies: {}",
                                        e
                                    ))
                                }
                            }
                        }
                    } else {
                        // Dependencies not found in Conary repos - check provides table
                        let (satisfied, unsatisfied) =
                            check_provides_dependencies(&conn, &plan.missing);

                        if !satisfied.is_empty() {
                            println!(
                                "\nDependencies satisfied by tracked packages ({}):",
                                satisfied.len()
                            );
                            for (name, provider, version) in &satisfied {
                                if let Some(v) = version {
                                    println!("  {} -> {} ({})", name, provider, v);
                                } else {
                                    println!("  {} -> {}", name, provider);
                                }
                            }
                        }

                        if !unsatisfied.is_empty() {
                            println!("\nMissing dependencies:");
                            for missing in &unsatisfied {
                                println!(
                                    "  {} {} (required by: {})",
                                    missing.name,
                                    missing.constraint,
                                    missing.required_by.join(", ")
                                );
                            }
                            println!("\nHint: Run 'conary adopt-system' to track all installed packages");
                            return Err(anyhow::anyhow!(
                                "Cannot install {}: {} unresolvable dependencies",
                                pkg.name(),
                                unsatisfied.len()
                            ));
                        }

                        // All dependencies satisfied by tracked packages
                        println!("All dependencies satisfied by tracked packages");
                    }
                }
                Err(e) => {
                    debug!("Repository lookup failed: {}", e);
                    // Check provides table for dependencies
                    let (satisfied, unsatisfied) = check_provides_dependencies(&conn, &plan.missing);

                    if !satisfied.is_empty() {
                        println!(
                            "\nDependencies satisfied by tracked packages ({}):",
                            satisfied.len()
                        );
                        for (name, provider, version) in &satisfied {
                            if let Some(v) = version {
                                println!("  {} -> {} ({})", name, provider, v);
                            } else {
                                println!("  {} -> {}", name, provider);
                            }
                        }
                    }

                    if !unsatisfied.is_empty() {
                        println!("\nMissing dependencies:");
                        for missing in &unsatisfied {
                            println!(
                                "  {} {} (required by: {})",
                                missing.name,
                                missing.constraint,
                                missing.required_by.join(", ")
                            );
                        }
                        println!("\nHint: Run 'conary adopt-system' to track all installed packages");
                        return Err(anyhow::anyhow!(
                            "Cannot install {}: {} unresolvable dependencies",
                            pkg.name(),
                            unsatisfied.len()
                        ));
                    }

                    // All dependencies satisfied by tracked packages
                    println!("All dependencies satisfied by tracked packages");
                }
            }
        } else {
            println!("All dependencies already satisfied");
        }
    }

    if dry_run {
        // For dry run, classify files to show component info
        let dry_run_paths: Vec<String> = pkg.files().iter().map(|f| f.path.clone()).collect();
        let dry_run_classified = ComponentClassifier::classify_all(&dry_run_paths);
        let dry_run_available: Vec<_> = dry_run_classified.keys().collect();
        let dry_run_selected: Vec<_> = dry_run_available
            .iter()
            .filter(|c| component_selection.should_install(***c))
            .collect();
        let dry_run_skipped: Vec<_> = dry_run_available
            .iter()
            .filter(|c| !component_selection.should_install(***c))
            .collect();

        let selected_file_count: usize = dry_run_classified
            .iter()
            .filter(|(c, _)| component_selection.should_install(**c))
            .map(|(_, files)| files.len())
            .sum();

        println!(
            "\nWould install package: {} version {}",
            pkg.name(),
            pkg.version()
        );
        println!(
            "  Architecture: {}",
            pkg.architecture().unwrap_or("none")
        );
        println!(
            "  Components to install: {} ({} files)",
            dry_run_selected.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
            selected_file_count
        );
        if !dry_run_skipped.is_empty() {
            println!(
                "  Components skipped: {} (use {}:all to include)",
                dry_run_skipped.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
                pkg.name()
            );
        }
        println!("  Dependencies: {}", pkg.dependencies().len());
        println!("\nDry run complete. No changes made.");
        return Ok(());
    }

    // Pre-transaction validation - check if already installed or needs upgrade
    let old_trove_to_upgrade = match check_upgrade_status(&conn, pkg.as_ref(), allow_downgrade)? {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(trove),
    };

    // Extract and install
    progress.set_phase(pkg.name(), InstallPhase::Extracting);
    info!("Extracting file contents from package...");
    let extracted_files = pkg.extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;
    info!("Extracted {} files", extracted_files.len());

    // Classify files into components
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let all_classified = ComponentClassifier::classify_all(&file_paths);

    // Show what components are available in the package
    let available_components: Vec<ComponentType> = all_classified.keys().copied().collect();
    info!(
        "Package contains {} component types: {:?}",
        available_components.len(),
        available_components.iter().map(|c| c.as_str()).collect::<Vec<_>>()
    );

    // Filter to only selected components
    let classified: HashMap<ComponentType, Vec<String>> = all_classified
        .into_iter()
        .filter(|(comp_type, _)| component_selection.should_install(*comp_type))
        .collect();

    // Build set of paths for selected components
    let selected_paths: std::collections::HashSet<&str> = classified
        .values()
        .flatten()
        .map(|s| s.as_str())
        .collect();

    // Filter extracted files to only include selected components
    let extracted_files: Vec<_> = extracted_files
        .into_iter()
        .filter(|f| selected_paths.contains(f.path.as_str()))
        .collect();

    let installed_component_types: Vec<ComponentType> = classified.keys().copied().collect();

    // Show what we're actually installing
    let skipped_components: Vec<_> = available_components
        .iter()
        .filter(|c| !component_selection.should_install(**c))
        .map(|c| c.as_str())
        .collect();

    if !skipped_components.is_empty() {
        info!(
            "Skipping non-default components: {:?} (use package:all to install everything)",
            skipped_components
        );
    }

    info!(
        "Installing {} files from {} component(s): {:?}",
        extracted_files.len(),
        classified.len(),
        installed_component_types.iter().map(|c| c.as_str()).collect::<Vec<_>>()
    );

    // Detect language-specific provides from installed files
    // Do this before the transaction so we can display the count in the summary
    let installed_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let language_provides = LanguageDepDetector::detect_all_provides(&installed_paths);
    if !language_provides.is_empty() {
        info!(
            "Detected {} language-specific provides: {:?}",
            language_provides.len(),
            language_provides.iter().take(5).map(|d| d.to_dep_string()).collect::<Vec<_>>()
        );
    }

    // Determine package format and execution mode for scriptlet execution
    let scriptlet_format = to_scriptlet_format(format);
    let execution_mode = build_execution_mode(
        old_trove_to_upgrade.as_ref().map(|t| t.version.as_str())
    );

    // Execute pre-install scriptlet (before any changes)
    // Scriptlets only run when :runtime or :lib is being installed
    let scriptlets = pkg.scriptlets();
    let run_scriptlets = should_run_scriptlets(&installed_component_types);
    if !no_scripts && !scriptlets.is_empty() && run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PreScript);
        run_pre_install(
            Path::new(root),
            pkg.name(),
            pkg.version(),
            &scriptlets,
            scriptlet_format,
            &execution_mode,
            sandbox_mode,
        )?;
    } else if !no_scripts && !scriptlets.is_empty() && !run_scriptlets {
        info!(
            "Skipping scriptlets: no :runtime or :lib component being installed (components: {:?})",
            installed_component_types.iter().map(|c| c.as_str()).collect::<Vec<_>>()
        );
    }

    // Query old package's scriptlets BEFORE we delete it from DB
    // We need these for running pre-remove and post-remove during upgrade
    let old_trove_id = old_trove_to_upgrade.as_ref().and_then(|t| t.id);
    let old_package_scriptlets = get_old_package_scriptlets(&conn, old_trove_id)?;

    // For RPM/DEB upgrades: run old package's pre-remove scriptlet
    if !no_scripts
        && let Some(ref old_trove) = old_trove_to_upgrade
    {
        run_old_pre_remove(
            Path::new(root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &old_package_scriptlets,
            scriptlet_format,
            sandbox_mode,
        )?;
    }

    // Track if this is an upgrade
    let is_upgrade = old_trove_to_upgrade.is_some();

    // === TRANSACTION ENGINE INTEGRATION ===
    // Create transaction engine for crash-safe atomic operations
    let db_path_buf = PathBuf::from(db_path);
    let tx_config = TransactionConfig::new(PathBuf::from(root), db_path_buf.clone());
    let engine = TransactionEngine::new(tx_config)
        .context("Failed to create transaction engine")?;

    // Recover any incomplete transactions from previous crashes
    let recovery_outcomes = engine.recover(&mut conn)
        .context("Failed to recover incomplete transactions")?;
    for outcome in &recovery_outcomes {
        info!("Recovery outcome: {:?}", outcome);
    }

    // Begin new transaction
    let tx_description = if let Some(ref old_trove) = old_trove_to_upgrade {
        format!("Upgrade {} from {} to {}", pkg.name(), old_trove.version, pkg.version())
    } else {
        format!("Install {}-{}", pkg.name(), pkg.version())
    };
    let mut txn = engine.begin(&tx_description)
        .context("Failed to begin transaction")?;

    info!("Started transaction {} for {}", txn.uuid(), tx_description);

    // Convert extracted files to transaction format
    let tx_files = convert_extracted_files(&extracted_files);

    // Get files to remove for upgrades
    let files_to_remove = if let Some(ref old_trove) = old_trove_to_upgrade
        && let Some(old_id) = old_trove.id
    {
        let new_paths: std::collections::HashSet<&str> =
            extracted_files.iter().map(|f| f.path.as_str()).collect();
        get_files_to_remove(&conn, old_id, &new_paths)?
    } else {
        Vec::new()
    };

    // Plan transaction operations
    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let operations = TransactionOperations {
        package: PackageInfo {
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
            release: None,
            arch: pkg.architecture().map(|s| s.to_string()),
        },
        files_to_add: tx_files.clone(),
        files_to_remove,
        is_upgrade,
        old_package: old_trove_to_upgrade.as_ref().map(|t| PackageInfo {
            name: t.name.clone(),
            version: t.version.clone(),
            release: None,
            arch: t.architecture.clone(),
        }),
    };

    let plan = txn.plan_operations(operations, &conn)
        .context("Failed to plan transaction")?;

    // Extract plan data before further mutations (to avoid borrow checker issues)
    let plan_conflicts = plan.conflicts.clone();
    let plan_files_to_stage = plan.files_to_stage.clone();
    let plan_files_to_backup_len = plan.files_to_backup.len();
    let plan_dirs_to_create_len = plan.dirs_to_create.len();

    // Check for conflicts
    if !plan_conflicts.is_empty() {
        let conflict_msgs: Vec<String> = plan_conflicts.iter().map(|c| format!("{:?}", c)).collect();
        txn.abort().context("Failed to abort transaction after conflicts")?;
        return Err(anyhow::anyhow!(
            "File conflicts detected:\n  {}",
            conflict_msgs.join("\n  ")
        ));
    }

    info!("Transaction plan: {} files to stage, {} files to backup, {} dirs to create",
          plan_files_to_stage.len(), plan_files_to_backup_len, plan_dirs_to_create_len);

    // Prepare: store content in CAS
    txn.prepare(&tx_files)
        .context("Failed to prepare transaction (CAS storage)")?;

    // Backup existing files
    txn.backup_files()
        .context("Failed to backup existing files")?;

    // Stage new files from CAS
    txn.stage_files()
        .context("Failed to stage files")?;

    // Apply filesystem changes (atomic renames)
    let fs_result = txn.apply_filesystem()
        .context("Failed to apply filesystem changes")?;

    info!("Filesystem changes: {} added, {} replaced, {} removed",
          fs_result.files_added, fs_result.files_replaced, fs_result.files_removed);

    // Write DB commit intent for crash recovery correlation
    txn.write_db_commit_intent()
        .context("Failed to write DB commit intent")?;

    // Build file hashes from staged files for DB insertion
    // Create a lookup from path to size using extracted files
    let size_lookup: HashMap<String, i64> = extracted_files
        .iter()
        .map(|f| (f.path.clone(), f.size))
        .collect();
    let file_hashes: Vec<(String, String, i64, i32)> = plan_files_to_stage
        .iter()
        .map(|s| {
            let path_str = s.path.display().to_string();
            let size = size_lookup.get(&path_str).copied().unwrap_or(0);
            (path_str, s.hash.clone(), size, s.mode as i32)
        })
        .collect();

    // DB transaction with tx_uuid for crash recovery
    let tx_uuid = txn.uuid().to_string();
    let db_result = conary::db::transaction(&mut conn, |tx| {
        // Create changeset with tx_uuid for crash recovery
        let mut changeset = Changeset::with_tx_uuid(tx_description.clone(), tx_uuid.clone());
        let changeset_id = changeset.insert(tx)?;

        if let Some(old_trove) = old_trove_to_upgrade.as_ref()
            && let Some(old_id) = old_trove.id
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = pkg.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);

        // Set custom selection reason if provided (e.g., from collection install)
        if let Some(reason) = selection_reason {
            trove.selection_reason = Some(reason.to_string());
        }

        let trove_id = trove.insert(tx)?;

        // Create components and build path-to-component-id mapping
        let mut component_ids: HashMap<ComponentType, i64> = HashMap::new();
        for comp_type in classified.keys() {
            let mut component = Component::from_type(trove_id, *comp_type);
            component.description = Some(format!("{} files", comp_type.as_str()));
            let comp_id = component.insert(tx)?;
            component_ids.insert(*comp_type, comp_id);
            info!("Created component :{} (id={})", comp_type.as_str(), comp_id);
        }

        // Build path-to-component-id lookup for efficient file insertion
        let mut path_to_component: HashMap<&str, i64> = HashMap::new();
        for (comp_type, files) in &classified {
            if let Some(&comp_id) = component_ids.get(comp_type) {
                for path in files {
                    path_to_component.insert(path.as_str(), comp_id);
                }
            }
        }

        for (path, hash, size, mode) in &file_hashes {
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &size.to_string()],
            )?;

            // Look up the component ID for this file
            let component_id = path_to_component.get(path.as_str()).copied();

            let mut file_entry = conary::db::models::FileEntry::new(
                path.clone(),
                hash.clone(),
                *size,
                *mode,
                trove_id,
            );
            file_entry.component_id = component_id;
            file_entry.insert(tx)?;

            // Record in history
            let action = if is_upgrade { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), path, hash, action],
            )?;
        }

        for dep in pkg.dependencies() {
            let dep_type_str = match dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Build => "build",
                DependencyType::Optional => "optional",
            };
            let mut dep_entry = conary::db::models::DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                None, // depends_on_version is for resolved version, not constraint
                dep_type_str.to_string(),
                dep.version.clone(), // Store the version constraint
            );
            dep_entry.insert(tx)?;
        }

        // Store scriptlets for later removal (always, even if --no-scripts)
        let format_str = match format {
            PackageFormatType::Rpm => "rpm",
            PackageFormatType::Deb => "deb",
            PackageFormatType::Arch => "arch",
        };
        for scriptlet in &scriptlets {
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

        // Store language-specific provides (python, perl, ruby, etc.)
        for lang_dep in &language_provides {
            let mut provide = ProvideEntry::new(
                trove_id,
                lang_dep.to_dep_string(),
                lang_dep.version_constraint.clone(),
            );
            provide.insert_or_ignore(tx)?;
        }

        // Also store the package name itself as a provide
        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok((changeset_id, trove_id))
    });

    // Handle DB transaction result
    let (changeset_id, trove_id) = match db_result {
        Ok((cs_id, tr_id)) => {
            // Record successful DB commit in transaction journal
            txn.record_db_commit(cs_id, tr_id)
                .context("Failed to record DB commit")?;
            (cs_id, tr_id)
        }
        Err(e) => {
            // DB failed - abort transaction (will restore backups)
            if let Err(abort_err) = txn.abort() {
                warn!("Failed to abort transaction after DB failure: {}", abort_err);
            }
            return Err(anyhow::anyhow!("Database transaction failed: {}", e));
        }
    };

    // Suppress unused variable warning
    let _ = trove_id;

    // For RPM/DEB upgrades: run old package's post-remove scriptlet
    if !no_scripts
        && let Some(ref old_trove) = old_trove_to_upgrade
    {
        run_old_post_remove(
            Path::new(root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &old_package_scriptlets,
            scriptlet_format,
            sandbox_mode,
        );
    }

    // Execute post-install scriptlet (after files are deployed)
    if !no_scripts && !scriptlets.is_empty() && run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PostScript);
        run_post_install(
            Path::new(root),
            pkg.name(),
            pkg.version(),
            &scriptlets,
            scriptlet_format,
            &execution_mode,
            sandbox_mode,
        );
    }

    // Mark post-scripts complete in transaction
    txn.mark_post_scripts_complete()
        .context("Failed to mark post-scripts complete")?;

    // Execute triggers based on installed files
    progress.set_phase(pkg.name(), InstallPhase::Triggers);
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let trigger_executor = conary::trigger::TriggerExecutor::new(&conn, Path::new(root));

    // Record which triggers need to run
    let triggered = trigger_executor.record_triggers(changeset_id, &file_paths)
        .unwrap_or_else(|e| {
            warn!("Failed to record triggers: {}", e);
            Vec::new()
        });

    if !triggered.is_empty() {
        info!("Recorded {} trigger(s) for execution", triggered.len());
        // Execute triggers
        match trigger_executor.execute_pending(changeset_id) {
            Ok(results) => {
                if results.total() > 0 {
                    info!(
                        "Triggers: {} succeeded, {} failed, {} skipped",
                        results.succeeded, results.failed, results.skipped
                    );
                    if !results.all_succeeded() {
                        for error in &results.errors {
                            warn!("Trigger error: {}", error);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Trigger execution failed: {}", e);
            }
        }
    }

    progress.finish(&format!("Installed {} {}", pkg.name(), pkg.version()));

    // Show what components were available vs installed
    let skipped_info = if !skipped_components.is_empty() {
        format!(" (skipped: {})", skipped_components.join(", "))
    } else {
        String::new()
    };

    println!(
        "Installed package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!(
        "  Architecture: {}",
        pkg.architecture().unwrap_or("none")
    );
    println!("  Files installed: {}", extracted_files.len());
    println!(
        "  Components: {}{}",
        installed_component_types
            .iter()
            .map(|c| format!(":{}", c.as_str()))
            .collect::<Vec<_>>()
            .join(", "),
        skipped_info
    );
    println!("  Dependencies: {}", pkg.dependencies().len());
    if !language_provides.is_empty() {
        println!("  Provides: {} (language-specific capabilities)", language_provides.len());
    }

    // Finish transaction (cleanup working directory, archive journal)
    let tx_result = txn.finish()
        .context("Failed to finish transaction")?;
    info!("Transaction {} completed in {}ms", tx_result.tx_uuid, tx_result.duration_ms);

    // Create state snapshot after successful install
    create_state_snapshot(&conn, changeset_id, &format!("Install {}", pkg.name()))?;

    Ok(())
}

