// src/commands/install.rs
//! Package installation and removal commands

use super::{detect_package_format, install_package_from_file, PackageFormatType};
use anyhow::Result;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::DependencyType;
use conary::packages::PackageFormat;
use conary::repository::{self, PackageSelector, SelectionOptions};
use conary::version::RpmVersion;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{info, warn};

/// Install a package
pub fn cmd_install(
    package: &str,
    db_path: &str,
    root: &str,
    version: Option<String>,
    repo: Option<String>,
    dry_run: bool,
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

    let rpm = match format {
        PackageFormatType::Rpm => RpmPackage::parse(path_str)?,
        PackageFormatType::Deb => return Err(anyhow::anyhow!("DEB format not yet implemented")),
        PackageFormatType::Arch => return Err(anyhow::anyhow!("Arch format not yet implemented")),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        rpm.name(),
        rpm.version(),
        rpm.files().len(),
        rpm.dependencies().len()
    );

    let mut conn = conary::db::open(db_path)?;

    // Resolve dependencies
    let dep_names: Vec<String> = rpm.dependencies().iter().map(|d| d.name.clone()).collect();

    if !dep_names.is_empty() {
        info!(
            "Resolving {} dependencies transitively...",
            dep_names.len()
        );
        println!("Checking dependencies for {}...", rpm.name());

        match repository::resolve_dependencies_transitive(&conn, &dep_names, 10) {
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
                    println!("All dependencies already satisfied");
                }
            }
            Err(e) => return Err(anyhow::anyhow!("Dependency resolution failed: {}", e)),
        }
    }

    if dry_run {
        println!(
            "\nWould install package: {} version {}",
            rpm.name(),
            rpm.version()
        );
        println!(
            "  Architecture: {}",
            rpm.architecture().unwrap_or("none")
        );
        println!("  Files: {}", rpm.files().len());
        println!("  Dependencies: {}", rpm.dependencies().len());
        println!("\nDry run complete. No changes made.");
        return Ok(());
    }

    // Pre-transaction validation
    let existing = conary::db::models::Trove::find_by_name(&conn, rpm.name())?;
    let mut old_trove_to_upgrade: Option<conary::db::models::Trove> = None;

    for trove in &existing {
        if trove.architecture == rpm.architecture().map(|s: &str| s.to_string()) {
            if trove.version == rpm.version() {
                return Err(anyhow::anyhow!(
                    "Package {} version {} ({}) is already installed",
                    rpm.name(),
                    rpm.version(),
                    rpm.architecture().unwrap_or("no-arch")
                ));
            }

            match (
                RpmVersion::parse(&trove.version),
                RpmVersion::parse(rpm.version()),
            ) {
                (Ok(existing_ver), Ok(new_ver)) => {
                    if new_ver > existing_ver {
                        info!(
                            "Upgrading {} from version {} to {}",
                            rpm.name(),
                            trove.version,
                            rpm.version()
                        );
                        old_trove_to_upgrade = Some(trove.clone());
                    } else {
                        return Err(anyhow::anyhow!(
                            "Cannot downgrade package {} from version {} to {}",
                            rpm.name(),
                            trove.version,
                            rpm.version()
                        ));
                    }
                }
                _ => warn!(
                    "Could not compare versions {} and {}",
                    trove.version,
                    rpm.version()
                ),
            }
        }
    }

    // Extract and install
    info!("Extracting file contents from package...");
    let extracted_files = rpm.extract_file_contents()?;
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
                rpm.name(),
                old_trove.version,
                rpm.version()
            )
        } else {
            format!("Install {}-{}", rpm.name(), rpm.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
        let changeset_id = changeset.insert(tx)?;

        if let Some(old_trove) = old_trove_to_upgrade.as_ref()
            && let Some(old_id) = old_trove.id
        {
            info!("Removing old version {} before upgrade", old_trove.version);
            conary::db::models::Trove::delete(tx, old_id)?;
        }

        let mut trove = rpm.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        for file in &extracted_files {
            if deployer.file_exists(&file.path) {
                if let Some(existing) =
                    conary::db::models::FileEntry::find_by_path(tx, &file.path)?
                {
                    let owner_trove =
                        conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove
                        && owner.name != rpm.name()
                    {
                        return Err(conary::Error::InitError(format!(
                            "File conflict: {} is owned by package {}",
                            file.path, owner.name
                        )));
                    }
                } else {
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

        for dep in rpm.dependencies() {
            let dep_type_str = match dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Build => "build",
                DependencyType::Optional => "optional",
            };
            let mut dep_entry = conary::db::models::DependencyEntry::new(
                trove_id,
                dep.name.clone(),
                dep.version.clone(),
                dep_type_str.to_string(),
                None,
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
        rpm.name(),
        rpm.version()
    );
    println!(
        "  Architecture: {}",
        rpm.architecture().unwrap_or("none")
    );
    println!("  Files: {}", rpm.files().len());
    println!("  Dependencies: {}", rpm.dependencies().len());

    if let Some(source_rpm) = rpm.source_rpm() {
        println!("  Source RPM: {}", source_rpm);
    }
    if let Some(vendor) = rpm.vendor() {
        println!("  Vendor: {}", vendor);
    }

    Ok(())
}

/// Remove an installed package
pub fn cmd_remove(package_name: &str, db_path: &str) -> Result<()> {
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

    let file_count = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?.len();

    conary::db::transaction(&mut conn, |tx| {
        let mut changeset =
            conary::db::models::Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
        changeset.insert(tx)?;
        conary::db::models::Trove::delete(tx, trove_id)?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })?;

    // TODO: Actually delete files from filesystem (Phase 6)

    println!(
        "Removed package: {} version {}",
        trove.name, trove.version
    );
    println!(
        "  Architecture: {}",
        trove.architecture.as_deref().unwrap_or("none")
    );
    println!("  Files removed: {}", file_count);

    Ok(())
}
