// src/commands/install.rs
//! Package installation and removal commands

use super::{detect_package_format, install_package_from_file, PackageFormatType};
use anyhow::Result;
use conary::db::models::ProvideEntry;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::DependencyType;
use conary::packages::PackageFormat;
use conary::repository::{self, PackageSelector, SelectionOptions};
use conary::resolver::{DependencyEdge, Resolver};
use conary::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

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

/// Install a package
pub fn cmd_install(
    package: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
    repo: Option<String>,
    dry_run: bool,
    no_deps: bool,
) -> Result<()> {
    info!("Installing package: {}", package);

    let package_path = if Path::new(package).exists() {
        info!("Installing from local file: {}", package);
        PathBuf::from(package)
    } else {
        info!("Searching repositories for package: {}", package);
        let conn = conary::db::open(db_path)?;

        let options = SelectionOptions {
            version: version.clone(),
            repository: repo.clone(),
            architecture: None,
        };

        let pkg_with_repo = PackageSelector::find_best_package(&conn, package, &options)?;
        info!(
            "Found package {} {} in repository {} (priority {})",
            pkg_with_repo.package.name,
            pkg_with_repo.package.version,
            pkg_with_repo.repository.name,
            pkg_with_repo.repository.priority
        );

        let temp_dir = TempDir::new()?;
        let download_path =
            repository::download_package(&pkg_with_repo.package, temp_dir.path())?;
        info!("Downloaded package to: {}", download_path.display());
        download_path
    };

    let path_str = package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;
    let format = detect_package_format(path_str)?;
    info!("Detected package format: {:?}", format);

    // Parse package using the appropriate format parser
    let pkg: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(path_str)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(path_str)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(path_str)?),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        pkg.name(),
        pkg.version(),
        pkg.files().len(),
        pkg.dependencies().len()
    );

    let mut conn = conary::db::open(db_path)?;

    // Build dependency edges from the package
    let package_version = RpmVersion::parse(pkg.version())?;
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
        info!(
            "Resolving {} dependencies with constraint validation...",
            dependency_edges.len()
        );
        println!("Checking dependencies for {}...", pkg.name());

        // Build resolver from current system state
        let mut resolver = Resolver::new(&conn)?;

        // Resolve with the new package
        let plan = resolver.resolve_install(
            pkg.name().to_string(),
            package_version.clone(),
            dependency_edges,
        )?;

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
                            let temp_dir = TempDir::new()?;
                            match repository::download_dependencies(&to_download, temp_dir.path()) {
                                Ok(downloaded) => {
                                    for (dep_name, dep_path) in downloaded {
                                        info!("Installing dependency: {}", dep_name);
                                        println!("Installing dependency: {}", dep_name);
                                        if let Err(e) =
                                            install_package_from_file(&dep_path, &mut conn, root, None)
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
        println!(
            "\nWould install package: {} version {}",
            pkg.name(),
            pkg.version()
        );
        println!(
            "  Architecture: {}",
            pkg.architecture().unwrap_or("none")
        );
        println!("  Files: {}", pkg.files().len());
        println!("  Dependencies: {}", pkg.dependencies().len());
        println!("\nDry run complete. No changes made.");
        return Ok(());
    }

    // Pre-transaction validation
    let existing = conary::db::models::Trove::find_by_name(&conn, pkg.name())?;
    let mut old_trove_to_upgrade: Option<conary::db::models::Trove> = None;

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
                        old_trove_to_upgrade = Some(trove.clone());
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

    // Extract and install
    info!("Extracting file contents from package...");
    let extracted_files = pkg.extract_file_contents()?;
    info!("Extracted {} files", extracted_files.len());

    let objects_dir = Path::new(db_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let _changeset_id = conary::db::transaction(&mut conn, |tx| {
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
        let trove_id = trove.insert(tx)?;

        // Track if this is an upgrade (old trove was deleted above)
        let is_upgrade = old_trove_to_upgrade.is_some();

        for file in &extracted_files {
            if deployer.file_exists(&file.path) {
                if let Some(existing) =
                    conary::db::models::FileEntry::find_by_path(tx, &file.path)?
                {
                    let owner_trove =
                        conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove
                        && owner.name != pkg.name()
                    {
                        return Err(conary::Error::InitError(format!(
                            "File conflict: {} is owned by package {}",
                            file.path, owner.name
                        )));
                    }
                } else if !is_upgrade {
                    // Only error on untracked files for fresh installs
                    // For upgrades, the old files were deleted with the old trove
                    return Err(conary::Error::InitError(format!(
                        "File conflict: {} exists but is not tracked by any package",
                        file.path
                    )));
                }
            }

            let hash = deployer.cas().store(&file.content)?;
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(),
                hash.clone(),
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.insert(tx)?;

            let action = if deployer.file_exists(&file.path) {
                "modify"
            } else {
                "add"
            };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), &file.path, &hash, action],
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

        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    info!("Deploying files to filesystem...");
    for file in &extracted_files {
        let hash = conary::filesystem::CasStore::compute_hash(&file.content);
        deployer.deploy_file(&file.path, &hash, file.mode as u32)?;
    }
    info!("Successfully deployed {} files", extracted_files.len());

    println!(
        "Installed package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!(
        "  Architecture: {}",
        pkg.architecture().unwrap_or("none")
    );
    println!("  Files: {}", pkg.files().len());
    println!("  Dependencies: {}", pkg.dependencies().len());

    Ok(())
}

/// Remove an installed package
pub fn cmd_remove(package_name: &str, db_path: &str, root: &str) -> Result<()> {
    info!("Removing package: {}", package_name);

    let mut conn = conary::db::open(db_path)?;
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;

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

    conary::db::transaction(&mut conn, |tx| {
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
        Ok(())
    })?;

    // Separate files and directories
    // Directories typically have mode starting with 040xxx (directory bit)
    // or path ending with /
    let (directories, regular_files): (Vec<_>, Vec<_>) = files.iter().partition(|f| {
        f.path.ends_with('/') || (f.permissions & 0o170000) == 0o040000
    });

    // Remove regular files first
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
    } else if capability.starts_with("perl-") {
        // perl-Foo-Bar -> perl(Foo::Bar)
        let module = &capability[5..].replace('-', "::");
        variations.push(format!("perl({})", module));
    }

    // Python module variations
    if capability.starts_with("python3-") {
        let module = &capability[8..];
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

    // Package name might be used directly
    // Try stripping version suffixes: foo-1.0 -> foo
    if let Some(pos) = capability.rfind('-') {
        let potential_name = &capability[..pos];
        if !potential_name.is_empty() && capability[pos+1..].chars().next().map_or(false, |c| c.is_ascii_digit()) {
            variations.push(potential_name.to_string());
        }
    }

    variations
}
