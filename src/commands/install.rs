// src/commands/install.rs
//! Package installation and removal commands

use super::progress::{InstallPhase, InstallProgress, RemovePhase, RemoveProgress};
use super::{detect_package_format, install_package_from_file, PackageFormatType};
use anyhow::{Context, Result};
use conary::components::{parse_component_spec, should_run_scriptlets, ComponentClassifier, ComponentType};
use conary::db::models::{Component, ProvideEntry, ScriptletEntry, StateEngine};
use conary::dependencies::LanguageDepDetector;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::{DependencyType, ScriptletPhase};
use conary::packages::PackageFormat;
use conary::repository::{self, DownloadOptions, PackageSelector, SelectionOptions};
use conary::resolver::{DependencyEdge, Resolver};
use conary::scriptlet::{ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode, ScriptletExecutor};
use conary::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Get the keyring directory based on db_path
fn get_keyring_dir(db_path: &str) -> PathBuf {
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| {
        Path::new(db_path)
            .parent()
            .unwrap_or(Path::new("/var/lib/conary"))
            .to_string_lossy()
            .to_string()
    });
    PathBuf::from(db_dir).join("keys")
}

/// Serializable trove metadata for rollback support
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TroveSnapshot {
    name: String,
    version: String,
    architecture: Option<String>,
    description: Option<String>,
    install_source: String,
    files: Vec<FileSnapshot>,
}

/// Serializable file metadata for rollback support
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileSnapshot {
    path: String,
    sha256_hash: String,
    size: i64,
    permissions: i32,
}

/// Result of resolving a package path
struct ResolvedPackage {
    path: PathBuf,
    /// Temp directory that must stay alive until installation completes
    _temp_dir: Option<TempDir>,
}

/// Resolve package to a local path, downloading from repository if needed
fn resolve_package_path(
    package: &str,
    db_path: &str,
    version: Option<&str>,
    repo: Option<&str>,
    progress: &InstallProgress,
) -> Result<ResolvedPackage> {
    if Path::new(package).exists() {
        info!("Installing from local file: {}", package);
        progress.set_status(&format!("Loading local file: {}", package));
        return Ok(ResolvedPackage {
            path: PathBuf::from(package),
            _temp_dir: None,
        });
    }

    info!("Searching repositories for package: {}", package);
    progress.set_status("Searching repositories...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let options = SelectionOptions {
        version: version.map(String::from),
        repository: repo.map(String::from),
        architecture: None,
    };

    let pkg_with_repo = PackageSelector::find_best_package(&conn, package, &options)
        .with_context(|| format!("Failed to find package '{}' in repositories", package))?;

    info!(
        "Found package {} {} in repository {} (priority {})",
        pkg_with_repo.package.name,
        pkg_with_repo.package.version,
        pkg_with_repo.repository.name,
        pkg_with_repo.repository.priority
    );

    let temp_dir = TempDir::new()
        .context("Failed to create temporary directory for download")?;

    // Set up GPG verification options if enabled for this repository
    let gpg_options = if pkg_with_repo.repository.gpg_check {
        let keyring_dir = get_keyring_dir(db_path);
        Some(DownloadOptions {
            gpg_check: true,
            gpg_strict: pkg_with_repo.repository.gpg_strict,
            keyring_dir,
            repository_name: pkg_with_repo.repository.name.clone(),
        })
    } else {
        None
    };

    progress.set_phase(&pkg_with_repo.package.name, InstallPhase::Downloading);
    let download_path = repository::download_package_verified(
        &pkg_with_repo.package,
        temp_dir.path(),
        gpg_options.as_ref(),
    )
    .with_context(|| format!("Failed to download package '{}'", pkg_with_repo.package.name))?;

    info!("Downloaded package to: {}", download_path.display());

    Ok(ResolvedPackage {
        path: download_path,
        _temp_dir: Some(temp_dir),
    })
}

/// Parse a package file and return the appropriate parser
fn parse_package(path: &Path, format: PackageFormatType) -> Result<Box<dyn PackageFormat>> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    let pkg: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(
            RpmPackage::parse(path_str)
                .with_context(|| format!("Failed to parse RPM package '{}'", path_str))?,
        ),
        PackageFormatType::Deb => Box::new(
            DebPackage::parse(path_str)
                .with_context(|| format!("Failed to parse DEB package '{}'", path_str))?,
        ),
        PackageFormatType::Arch => Box::new(
            ArchPackage::parse(path_str)
                .with_context(|| format!("Failed to parse Arch package '{}'", path_str))?,
        ),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        pkg.name(),
        pkg.version(),
        pkg.files().len(),
        pkg.dependencies().len()
    );

    Ok(pkg)
}

/// Result of checking for existing package installation
enum UpgradeCheck {
    /// Fresh install - no existing package
    FreshInstall,
    /// Upgrade from an older version (boxed to reduce enum size)
    Upgrade(Box<conary::db::models::Trove>),
}

/// Check if package is already installed and determine upgrade status
fn check_upgrade_status(
    conn: &Connection,
    pkg: &dyn PackageFormat,
) -> Result<UpgradeCheck> {
    let existing = conary::db::models::Trove::find_by_name(conn, pkg.name())?;

    for trove in &existing {
        if trove.architecture == pkg.architecture().map(|s: &str| s.to_string()) {
            if trove.version == pkg.version() {
                return Err(anyhow::anyhow!(
                    "Package {} version {} ({}) is already installed",
                    pkg.name(),
                    pkg.version(),
                    pkg.architecture().unwrap_or("no-arch")
                ));
            }

            match (
                RpmVersion::parse(&trove.version),
                RpmVersion::parse(pkg.version()),
            ) {
                (Ok(existing_ver), Ok(new_ver)) => {
                    if new_ver > existing_ver {
                        info!(
                            "Upgrading {} from version {} to {}",
                            pkg.name(),
                            trove.version,
                            pkg.version()
                        );
                        return Ok(UpgradeCheck::Upgrade(Box::new(trove.clone())));
                    } else {
                        return Err(anyhow::anyhow!(
                            "Cannot downgrade package {} from version {} to {}",
                            pkg.name(),
                            trove.version,
                            pkg.version()
                        ));
                    }
                }
                _ => warn!(
                    "Could not compare versions {} and {}",
                    trove.version,
                    pkg.version()
                ),
            }
        }
    }

    Ok(UpgradeCheck::FreshInstall)
}

/// Deploy files to filesystem with rollback capability
///
/// Returns the list of (path, hash, size, mode) for all deployed files
fn deploy_files(
    deployer: &conary::filesystem::FileDeployer,
    extracted_files: &[conary::packages::traits::ExtractedFile],
    is_upgrade: bool,
    conn: &Connection,
    pkg_name: &str,
) -> Result<Vec<(String, String, i64, i32)>> {
    // Phase 1: Check file conflicts BEFORE any changes
    for file in extracted_files {
        if deployer.file_exists(&file.path) {
            if let Some(existing) =
                conary::db::models::FileEntry::find_by_path(conn, &file.path)?
            {
                let owner_trove =
                    conary::db::models::Trove::find_by_id(conn, existing.trove_id)?;
                if let Some(owner) = owner_trove
                    && owner.name != pkg_name
                {
                    return Err(anyhow::anyhow!(
                        "File conflict: {} is owned by package {}",
                        file.path, owner.name
                    ));
                }
            } else if !is_upgrade {
                return Err(anyhow::anyhow!(
                    "File conflict: {} exists but is not tracked by any package",
                    file.path
                ));
            }
        }
    }

    // Phase 2: Store content in CAS and pre-compute hashes
    let mut file_hashes: Vec<(String, String, i64, i32)> = Vec::with_capacity(extracted_files.len());
    for file in extracted_files {
        let hash = deployer.cas().store(&file.content)?;
        file_hashes.push((file.path.clone(), hash, file.size, file.mode));
    }

    // Phase 3: Deploy files, tracking what we've deployed for rollback
    let mut deployed_files: Vec<String> = Vec::with_capacity(extracted_files.len());
    let deploy_result: Result<()> = (|| {
        for (path, hash, _size, mode) in &file_hashes {
            deployer.deploy_file(path, hash, *mode as u32)?;
            deployed_files.push(path.clone());
        }
        Ok(())
    })();

    // If deployment failed, rollback deployed files
    if let Err(e) = deploy_result {
        warn!(
            "File deployment failed, rolling back {} deployed files",
            deployed_files.len()
        );
        for path in &deployed_files {
            if let Err(remove_err) = deployer.remove_file(path) {
                warn!("Failed to rollback file {}: {}", path, remove_err);
            }
        }
        return Err(anyhow::anyhow!("File deployment failed: {}", e));
    }

    info!("Successfully deployed {} files", deployed_files.len());
    Ok(file_hashes)
}

/// Rollback deployed files on failure
fn rollback_deployed_files(deployer: &conary::filesystem::FileDeployer, files: &[(String, String, i64, i32)]) {
    warn!("Rolling back {} deployed files", files.len());
    for (path, _, _, _) in files {
        if let Err(e) = deployer.remove_file(path) {
            warn!("Failed to rollback file {}: {}", path, e);
        }
    }
}

/// Represents which components to install
#[derive(Debug, Clone)]
enum ComponentSelection {
    /// Install only default components (runtime, lib, config)
    Defaults,
    /// Install all components
    All,
    /// Install specific component(s)
    Specific(Vec<ComponentType>),
}

impl ComponentSelection {
    /// Check if a component type should be installed
    fn should_install(&self, comp_type: ComponentType) -> bool {
        match self {
            Self::All => true,
            Self::Defaults => comp_type.is_default(),
            Self::Specific(types) => types.contains(&comp_type),
        }
    }

    /// Get a display string for the selection
    fn display(&self) -> String {
        match self {
            Self::All => "all".to_string(),
            Self::Defaults => "defaults (runtime, lib, config)".to_string(),
            Self::Specific(types) => types.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "),
        }
    }
}

/// Install a package
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

    // Create progress tracker for single package installation
    let progress = InstallProgress::single("Installing");
    progress.set_phase(&package_name, InstallPhase::Downloading);

    // Resolve package path (download if needed)
    let resolved = resolve_package_path(
        &package_name,
        db_path,
        version.as_deref(),
        repo.as_deref(),
        &progress,
    )?;

    // Detect format and parse
    let path_str = resolved.path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

    progress.set_phase(package, InstallPhase::Parsing);
    let pkg = parse_package(&resolved.path, format)?;

    let mut conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    // Build dependency edges from the package
    let package_version = RpmVersion::parse(pkg.version())
        .with_context(|| format!("Failed to parse version '{}' for package '{}'", pkg.version(), pkg.name()))?;
    let dependency_edges: Vec<DependencyEdge> = pkg
        .dependencies()
        .iter()
        .filter(|d| d.dep_type == DependencyType::Runtime)
        .map(|d| {
            let constraint = d
                .version
                .as_ref()
                .and_then(|v| VersionConstraint::parse(v).ok())
                .unwrap_or(VersionConstraint::Any);
            DependencyEdge {
                from: pkg.name().to_string(),
                to: d.name.clone(),
                constraint,
                dep_type: "runtime".to_string(),
                kind: "package".to_string(),
            }
        })
        .collect();

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
                            let keyring_dir = get_keyring_dir(db_path);
                            match repository::download_dependencies(&to_download, temp_dir.path(), Some(&keyring_dir)) {
                                Ok(downloaded) => {
                                    // Capture parent package name for selection reason
                                    let parent_name = pkg.name().to_string();
                                    for (dep_name, dep_path) in downloaded {
                                        progress.set_status(&format!("Installing dependency: {}", dep_name));
                                        info!("Installing dependency: {}", dep_name);
                                        println!("Installing dependency: {}", dep_name);
                                        let reason = format!("Required by {}", parent_name);
                                        if let Err(e) =
                                            install_package_from_file(&dep_path, &mut conn, root, db_path, None, Some(&reason))
                                        {
                                            return Err(anyhow::anyhow!(
                                                "Failed to install dependency {}: {}",
                                                dep_name,
                                                e
                                            ));
                                        }
                                        println!("  [OK] Installed {}", dep_name);
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
    let old_trove_to_upgrade = match check_upgrade_status(&conn, pkg.as_ref())? {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::Upgrade(trove) => Some(trove),
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

    // Determine package format for scriptlet execution
    let scriptlet_format = match format {
        PackageFormatType::Rpm => ScriptletPackageFormat::Rpm,
        PackageFormatType::Deb => ScriptletPackageFormat::Deb,
        PackageFormatType::Arch => ScriptletPackageFormat::Arch,
    };

    // Determine execution mode
    let execution_mode = if let Some(old_trove) = &old_trove_to_upgrade {
        ExecutionMode::Upgrade {
            old_version: old_trove.version.clone(),
        }
    } else {
        ExecutionMode::Install
    };

    // Execute pre-install scriptlet (before any changes)
    // Scriptlets only run when :runtime or :lib is being installed
    let scriptlets = pkg.scriptlets();
    let run_scriptlets = should_run_scriptlets(&installed_component_types);
    if !no_scripts && !scriptlets.is_empty() && run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            pkg.name(),
            pkg.version(),
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

        // For Arch packages during upgrade, use PreUpgrade; for RPM/DEB always use PreInstall
        // (RPM/DEB distinguish via $1 argument, Arch uses different functions)
        let pre_phase = if scriptlet_format == ScriptletPackageFormat::Arch
            && matches!(execution_mode, ExecutionMode::Upgrade { .. })
        {
            ScriptletPhase::PreUpgrade
        } else {
            ScriptletPhase::PreInstall
        };

        // For Arch: if pre_upgrade is missing, do nothing (it's intentional)
        // For RPM/DEB: pre_install handles both cases via $1 argument
        if let Some(pre) = scriptlets.iter().find(|s| s.phase == pre_phase) {
            info!("Running {} scriptlet...", pre.phase);
            executor.execute(pre, &execution_mode)?;
        }
    } else if !no_scripts && !scriptlets.is_empty() && !run_scriptlets {
        info!(
            "Skipping scriptlets: no :runtime or :lib component being installed (components: {:?})",
            installed_component_types.iter().map(|c| c.as_str()).collect::<Vec<_>>()
        );
    }

    // Query old package's scriptlets BEFORE we delete it from DB
    // We need these for running pre-remove and post-remove during upgrade
    let old_package_scriptlets: Vec<ScriptletEntry> = if let Some(ref old_trove) = old_trove_to_upgrade
        && let Some(old_id) = old_trove.id
    {
        ScriptletEntry::find_by_trove(&conn, old_id)?
    } else {
        Vec::new()
    };

    // For RPM/DEB upgrades: run old package's pre-remove scriptlet
    // For Arch: skip entirely (Arch does NOT run removal scripts during upgrade)
    if !no_scripts
        && !old_package_scriptlets.is_empty()
        && scriptlet_format != ScriptletPackageFormat::Arch
        && let Some(ref old_trove) = old_trove_to_upgrade
    {
        let old_executor = ScriptletExecutor::new(
            Path::new(root),
            &old_trove.name,
            &old_trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);
        let upgrade_removal_mode = ExecutionMode::UpgradeRemoval {
            new_version: pkg.version().to_string(),
        };

        if let Some(pre_remove) = old_package_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            info!("Running old package pre-remove scriptlet (upgrade)...");
            old_executor.execute_entry(pre_remove, &upgrade_removal_mode)?;
        }
    }

    // Set up file deployer
    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    // Track if this is an upgrade
    let is_upgrade = old_trove_to_upgrade.is_some();

    // Deploy files with conflict checking and rollback capability
    progress.set_phase(pkg.name(), InstallPhase::Deploying);
    let file_hashes = deploy_files(&deployer, &extracted_files, is_upgrade, &conn, pkg.name())?;

    // Phase 3: DB transaction - only runs after files are successfully deployed
    let db_result = conary::db::transaction(&mut conn, |tx| {
        let changeset_desc = if let Some(ref old_trove) = old_trove_to_upgrade {
            format!(
                "Upgrade {} from {} to {}",
                pkg.name(),
                old_trove.version,
                pkg.version()
            )
        } else {
            format!("Install {}-{}", pkg.name(), pkg.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
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

            // Record in history (we know files exist since we just deployed them)
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
        // These were detected earlier and enable dependency resolution against language ecosystem packages
        for lang_dep in &language_provides {
            let mut provide = ProvideEntry::new(
                trove_id,
                lang_dep.to_dep_string(),
                lang_dep.version_constraint.clone(),
            );
            // Use insert_or_ignore to avoid duplicates
            provide.insert_or_ignore(tx)?;
        }

        // Also store the package name itself as a provide (for package-level deps)
        let mut pkg_provide = ProvideEntry::new(
            trove_id,
            pkg.name().to_string(),
            Some(pkg.version().to_string()),
        );
        pkg_provide.insert_or_ignore(tx)?;

        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(changeset_id)
    });

    // If DB transaction failed, rollback deployed files
    let changeset_id = match db_result {
        Ok(id) => id,
        Err(e) => {
            rollback_deployed_files(&deployer, &file_hashes);
            return Err(anyhow::anyhow!("Database transaction failed: {}", e));
        }
    };

    // For RPM/DEB upgrades: run old package's post-remove scriptlet
    // For Arch: skip entirely (Arch does NOT run removal scripts during upgrade)
    if !no_scripts
        && !old_package_scriptlets.is_empty()
        && scriptlet_format != ScriptletPackageFormat::Arch
        && let Some(ref old_trove) = old_trove_to_upgrade
    {
        let old_executor = ScriptletExecutor::new(
            Path::new(root),
            &old_trove.name,
            &old_trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);
        let upgrade_removal_mode = ExecutionMode::UpgradeRemoval {
            new_version: pkg.version().to_string(),
        };

        if let Some(post_remove) = old_package_scriptlets.iter().find(|s| s.phase == "post-remove") {
            info!("Running old package post-remove scriptlet (upgrade)...");
            // Post-remove failure during upgrade is not fatal - files are already replaced
            if let Err(e) = old_executor.execute_entry(post_remove, &upgrade_removal_mode) {
                warn!("Old package post-remove scriptlet failed: {}. Continuing anyway.", e);
                eprintln!("WARNING: Old package post-remove scriptlet failed: {}", e);
            }
        }
    }

    // Execute post-install scriptlet (after files are deployed)
    // Scriptlets only run when :runtime or :lib is being installed
    if !no_scripts && !scriptlets.is_empty() && run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PostScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            pkg.name(),
            pkg.version(),
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

        // For Arch packages during upgrade, use PostUpgrade; for RPM/DEB always use PostInstall
        let post_phase = if scriptlet_format == ScriptletPackageFormat::Arch
            && matches!(execution_mode, ExecutionMode::Upgrade { .. })
        {
            ScriptletPhase::PostUpgrade
        } else {
            ScriptletPhase::PostInstall
        };

        // For Arch: if post_upgrade is missing, do nothing (it's intentional)
        // For RPM/DEB: post_install handles both cases via $1 argument
        if let Some(post) = scriptlets.iter().find(|s| s.phase == post_phase) {
            info!("Running {} scriptlet...", post.phase);
            if let Err(e) = executor.execute(post, &execution_mode) {
                // Post-install failure is serious but files are already deployed
                // Log warning but don't fail the install
                warn!("{} scriptlet failed: {}. Package files are installed.", post.phase, e);
                eprintln!("WARNING: {} scriptlet failed: {}", post.phase, e);
            }
        }
    }

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

    // Create state snapshot after successful install
    create_state_snapshot(&conn, changeset_id, &format!("Install {}", pkg.name()))?;

    Ok(())
}

/// Remove an installed package
pub fn cmd_remove(package_name: &str, db_path: &str, root: &str, no_scripts: bool, sandbox_mode: SandboxMode) -> Result<()> {
    info!("Removing package: {}", package_name);

    // Create progress tracker for removal
    let progress = RemoveProgress::new(package_name);

    let mut conn = conary::db::open(db_path)
        .context("Failed to open package database")?;
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)
        .with_context(|| format!("Failed to query package '{}'", package_name))?;

    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    if troves.len() > 1 {
        println!("Multiple versions of '{}' found:", package_name);
        for trove in &troves {
            println!("  - version {}", trove.version);
        }
        return Err(anyhow::anyhow!(
            "Please specify version (future enhancement)"
        ));
    }

    let trove = &troves[0];
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    // Check if package is pinned
    if trove.pinned {
        return Err(anyhow::anyhow!(
            "Package '{}' is pinned and cannot be removed. Use 'conary unpin {}' first.",
            package_name,
            package_name
        ));
    }

    let resolver = conary::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

    if !breaking.is_empty() {
        println!(
            "WARNING: Removing '{}' would break the following packages:",
            package_name
        );
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nRefusing to remove package with dependencies.");
        println!(
            "Use 'conary whatbreaks {}' for more information.",
            package_name
        );
        return Err(anyhow::anyhow!(
            "Cannot remove '{}': {} packages depend on it",
            package_name,
            breaking.len()
        ));
    }

    // Get files BEFORE deleting the trove (cascade delete will remove file records)
    let files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
    let _file_count = files.len(); // Used for snapshot, not display

    // Get stored scriptlets BEFORE deleting the trove
    let stored_scriptlets = ScriptletEntry::find_by_trove(&conn, trove_id)?;

    // Determine package format from stored scriptlets (default to RPM if no scriptlets)
    let scriptlet_format = stored_scriptlets
        .first()
        .and_then(|s| ScriptletPackageFormat::parse(&s.package_format))
        .unwrap_or(ScriptletPackageFormat::Rpm);

    // Execute pre-remove scriptlet (before any changes)
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PreScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

        if let Some(pre) = stored_scriptlets.iter().find(|s| s.phase == "pre-remove") {
            info!("Running pre-remove scriptlet...");
            executor.execute_entry(pre, &ExecutionMode::Remove)?;
        }
    }

    // Create snapshot of trove for rollback support
    let snapshot = TroveSnapshot {
        name: trove.name.clone(),
        version: trove.version.clone(),
        architecture: trove.architecture.clone(),
        description: trove.description.clone(),
        install_source: trove.install_source.as_str().to_string(),
        files: files
            .iter()
            .map(|f| FileSnapshot {
                path: f.path.clone(),
                sha256_hash: f.sha256_hash.clone(),
                size: f.size,
                permissions: f.permissions,
            })
            .collect(),
    };
    let snapshot_json = serde_json::to_string(&snapshot)?;

    // Set up file deployer for actual filesystem operations
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| "/var/lib/conary".to_string());
    let objects_dir = PathBuf::from(&db_dir).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    progress.set_phase(RemovePhase::UpdatingDb);
    let remove_changeset_id = conary::db::transaction(&mut conn, |tx| {
        let mut changeset =
            conary::db::models::Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
        let changeset_id = changeset.insert(tx)?;

        // Store snapshot metadata for rollback
        tx.execute(
            "UPDATE changesets SET metadata = ?1 WHERE id = ?2",
            [&snapshot_json, &changeset_id.to_string()],
        )?;

        // Record file removals in history before deleting
        for file in &files {
            // Check if hash is valid format (64 hex chars) and exists in file_contents
            // Adopted files may have placeholder hashes or real hashes not in the content store
            let use_hash = if file.sha256_hash.len() == 64
                && file.sha256_hash.chars().all(|c| c.is_ascii_hexdigit())
            {
                // Check if this hash actually exists in file_contents (FK constraint)
                let hash_exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM file_contents WHERE sha256_hash = ?1)",
                    [&file.sha256_hash],
                    |row| row.get(0),
                )?;
                if hash_exists {
                    Some(file.sha256_hash.as_str())
                } else {
                    None // Hash not in content store (adopted file)
                }
            } else {
                None // Placeholder hash
            };

            // Always record file removal, but only include hash if it exists in file_contents
            match use_hash {
                Some(hash) => {
                    tx.execute(
                        "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                        [&changeset_id.to_string(), &file.path, hash, "delete"],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, NULL, ?3)",
                        [&changeset_id.to_string(), &file.path, "delete"],
                    )?;
                }
            }
        }

        conary::db::models::Trove::delete(tx, trove_id)?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    // Separate files and directories
    // Directories typically have mode starting with 040xxx (directory bit)
    // or path ending with /
    let (directories, regular_files): (Vec<_>, Vec<_>) = files.iter().partition(|f| {
        f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000
    });

    // Remove regular files first
    progress.set_phase(RemovePhase::RemovingFiles);
    let mut removed_count = 0;
    let mut failed_count = 0;
    for file in &regular_files {
        match deployer.remove_file(&file.path) {
            Ok(()) => {
                removed_count += 1;
                info!("Removed file: {}", file.path);
            }
            Err(e) => {
                warn!("Failed to remove file {}: {}", file.path, e);
                failed_count += 1;
            }
        }
    }

    // Sort directories by path length (deepest first) to remove children before parents
    let mut sorted_dirs: Vec<_> = directories.iter().collect();
    sorted_dirs.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

    // Remove directories (only if empty)
    progress.set_phase(RemovePhase::RemovingDirs);
    let mut dirs_removed = 0;
    for dir in sorted_dirs {
        let dir_path = dir.path.trim_end_matches('/');
        match deployer.remove_directory(dir_path) {
            Ok(true) => {
                dirs_removed += 1;
                info!("Removed directory: {}", dir_path);
            }
            Ok(false) => {
                debug!("Directory not empty or already removed: {}", dir_path);
            }
            Err(e) => {
                warn!("Failed to remove directory {}: {}", dir_path, e);
            }
        }
    }

    // Execute post-remove scriptlet (best effort - warn on failure, don't abort)
    if !no_scripts && !stored_scriptlets.is_empty() {
        progress.set_phase(RemovePhase::PostScript);
        let executor = ScriptletExecutor::new(
            Path::new(root),
            &trove.name,
            &trove.version,
            scriptlet_format,
        ).with_sandbox_mode(sandbox_mode);

        if let Some(post) = stored_scriptlets.iter().find(|s| s.phase == "post-remove") {
            info!("Running post-remove scriptlet...");
            if let Err(e) = executor.execute_entry(post, &ExecutionMode::Remove) {
                // Post-remove failure is not critical - files are already removed
                warn!("Post-remove scriptlet failed: {}. Package files already removed.", e);
                eprintln!("WARNING: Post-remove scriptlet failed: {}", e);
            }
        }
    }

    progress.finish(&format!("Removed {} {}", trove.name, trove.version));

    println!(
        "Removed package: {} version {}",
        trove.name, trove.version
    );
    println!(
        "  Architecture: {}",
        trove.architecture.as_deref().unwrap_or("none")
    );
    println!(
        "  Files removed: {}/{}",
        removed_count,
        regular_files.len()
    );
    if dirs_removed > 0 {
        println!("  Directories removed: {}", dirs_removed);
    }
    if failed_count > 0 {
        println!("  Files failed to remove: {}", failed_count);
    }

    // Create state snapshot after successful remove
    create_state_snapshot(&conn, remove_changeset_id, &format!("Remove {}", trove.name))?;

    Ok(())
}

/// Check if missing dependencies are satisfied by packages in the provides table
///
/// This is a self-contained approach that doesn't query the host package manager.
/// Instead, it checks if any tracked package provides the required capability.
///
/// Returns a tuple of:
/// - satisfied: Vec of (dep_name, provider_name, version)
/// - unsatisfied: Vec of MissingDependency (cloned)
#[allow(clippy::type_complexity)]
fn check_provides_dependencies(
    conn: &Connection,
    missing: &[conary::resolver::MissingDependency],
) -> (
    Vec<(String, String, Option<String>)>,
    Vec<conary::resolver::MissingDependency>,
) {
    let mut satisfied = Vec::new();
    let mut unsatisfied = Vec::new();

    for dep in missing {
        // Check if this capability is provided by any tracked package
        match ProvideEntry::find_satisfying_provider(conn, &dep.name) {
            Ok(Some((provider, version))) => {
                satisfied.push((dep.name.clone(), provider, Some(version)));
            }
            Ok(None) => {
                // Try some common variations for cross-distro compatibility
                let variations = generate_capability_variations(&dep.name);
                let mut found = false;

                for variation in &variations {
                    if let Ok(Some((provider, version))) = ProvideEntry::find_satisfying_provider(conn, variation) {
                        satisfied.push((dep.name.clone(), provider, Some(version)));
                        found = true;
                        break;
                    }
                }

                if !found {
                    unsatisfied.push(dep.clone());
                }
            }
            Err(e) => {
                debug!("Error checking provides for {}: {}", dep.name, e);
                unsatisfied.push(dep.clone());
            }
        }
    }

    (satisfied, unsatisfied)
}

/// Generate common variations of a capability name for cross-distro matching
///
/// For example:
/// - perl(Text::CharWidth) might also be: perl-Text-CharWidth
/// - libc.so.6 might also be: glibc, libc6
fn generate_capability_variations(capability: &str) -> Vec<String> {
    let mut variations = Vec::new();

    // Perl module variations: perl(Foo::Bar) <-> perl-Foo-Bar
    if capability.starts_with("perl(") && capability.ends_with(')') {
        let module = &capability[5..capability.len()-1];
        // perl(Foo::Bar) -> perl-Foo-Bar
        variations.push(format!("perl-{}", module.replace("::", "-")));
        // Also try lowercase
        variations.push(format!("perl-{}", module.replace("::", "-").to_lowercase()));
    } else if let Some(rest) = capability.strip_prefix("perl-") {
        // perl-Foo-Bar -> perl(Foo::Bar)
        let module = rest.replace('-', "::");
        variations.push(format!("perl({})", module));
    }

    // Python module variations
    if let Some(module) = capability.strip_prefix("python3-") {
        variations.push(format!("python3dist({})", module));
        variations.push(format!("python({})", module));
    } else if capability.starts_with("python3dist(") {
        let module = &capability[12..capability.len()-1];
        variations.push(format!("python3-{}", module));
    }

    // Library variations
    if capability.ends_with(".so") || capability.contains(".so.") {
        // libc.so.6 -> glibc, libc6
        if capability.starts_with("libc.so") {
            variations.push("glibc".to_string());
            variations.push("libc6".to_string());
        }
        // Extract library name: libfoo.so.1 -> libfoo, foo
        if let Some(base) = capability.split(".so").next() {
            variations.push(base.to_string());
            if let Some(name) = base.strip_prefix("lib") {
                variations.push(name.to_string());
            }
        }
    }

    // Debian :any suffix (architecture-independent)
    // perl:any -> perl
    if let Some(base) = capability.strip_suffix(":any") {
        variations.push(base.to_string());
    }

    // Debian perl library naming: libfoo-bar-perl -> perl-Foo-Bar, perl(Foo::Bar)
    if capability.starts_with("lib") && capability.ends_with("-perl") {
        // libtext-charwidth-perl -> text-charwidth -> Text::CharWidth
        let middle = &capability[3..capability.len()-5]; // strip "lib" and "-perl"
        // Convert to title case with :: separators
        let module_name: String = middle
            .split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("::");
        variations.push(format!("perl({})", module_name));
        variations.push(format!("perl-{}", middle.split('-').map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }).collect::<Vec<_>>().join("-")));
    }

    // Package name might be used directly
    // Try stripping version suffixes: foo-1.0 -> foo
    if let Some(pos) = capability.rfind('-') {
        let potential_name = &capability[..pos];
        if !potential_name.is_empty() && capability[pos+1..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            variations.push(potential_name.to_string());
        }
    }

    variations
}

/// Create a state snapshot after a successful operation
fn create_state_snapshot(conn: &Connection, changeset_id: i64, summary: &str) -> Result<()> {
    let engine = StateEngine::new(conn);
    match engine.create_snapshot(summary, None, Some(changeset_id)) {
        Ok(state) => {
            info!("Created state {} ({})", state.state_number, summary);
        }
        Err(e) => {
            warn!("Failed to create state snapshot: {}", e);
            // Don't fail the operation if snapshot creation fails
        }
    }
    Ok(())
}

/// Remove orphaned packages (installed as dependencies but no longer needed)
///
/// Finds packages that were installed as dependencies of other packages,
/// but are no longer required by any installed package.
pub fn cmd_autoremove(db_path: &str, root: &str, dry_run: bool, no_scripts: bool, sandbox_mode: SandboxMode) -> Result<()> {
    info!("Finding orphaned packages...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let orphans = conary::db::models::Trove::find_orphans(&conn)?;

    if orphans.is_empty() {
        println!("No orphaned packages found.");
        return Ok(());
    }

    println!("Found {} orphaned package(s):", orphans.len());
    for trove in &orphans {
        print!("  {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }

    if dry_run {
        println!("\nDry run - no packages will be removed.");
        println!("Run without --dry-run to remove these packages.");
        return Ok(());
    }

    println!("\nRemoving {} orphaned package(s)...", orphans.len());

    // Remove each orphaned package
    let mut removed_count = 0;
    let mut failed_count = 0;

    for trove in &orphans {
        println!("\nRemoving {} {}...", trove.name, trove.version);
        match cmd_remove(&trove.name, db_path, root, no_scripts, sandbox_mode) {
            Ok(()) => {
                removed_count += 1;
            }
            Err(e) => {
                eprintln!("  Failed to remove {}: {}", trove.name, e);
                failed_count += 1;
            }
        }
    }

    println!("\nAutoremove complete:");
    println!("  Removed: {} package(s)", removed_count);
    if failed_count > 0 {
        println!("  Failed: {} package(s)", failed_count);
    }

    Ok(())
}
