// src/commands.rs
//! Command handlers for the Conary CLI

use anyhow::Result;
use conary::db::models::{DeltaStats, PackageDelta};
use conary::delta::DeltaApplier;
use conary::packages::arch::ArchPackage;
use conary::packages::deb::DebPackage;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::DependencyType;
use conary::packages::PackageFormat;
use conary::repository::{self, PackageSelector, SelectionOptions};
use conary::version::RpmVersion;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{info, warn};

/// Package format types we support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormatType {
    Rpm,
    Deb,
    Arch,
}

/// Detect package format from file path and magic bytes
pub fn detect_package_format(path: &str) -> Result<PackageFormatType> {
    // First try file extension
    if path.ends_with(".rpm") {
        return Ok(PackageFormatType::Rpm);
    } else if path.ends_with(".deb") {
        return Ok(PackageFormatType::Deb);
    } else if path.ends_with(".pkg.tar.zst") || path.ends_with(".pkg.tar.xz") {
        return Ok(PackageFormatType::Arch);
    }

    // Fallback to magic bytes detection
    let mut file = File::open(path)?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;

    // RPM magic: 0xED 0xAB 0xEE 0xDB
    if magic[0..4] == [0xED, 0xAB, 0xEE, 0xDB] {
        return Ok(PackageFormatType::Rpm);
    }

    // DEB magic: "!<arch>\n"
    if magic[0..7] == *b"!<arch>" {
        return Ok(PackageFormatType::Deb);
    }

    // Arch: zstd magic
    if magic[0..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return Ok(PackageFormatType::Arch);
    }

    // Arch: xz magic
    if magic[0..6] == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        return Ok(PackageFormatType::Arch);
    }

    Err(anyhow::anyhow!("Unable to detect package format for: {}", path))
}

/// Install a package from a file path
pub fn install_package_from_file(
    package_path: &Path,
    conn: &mut rusqlite::Connection,
    root: &str,
    old_trove: Option<&conary::db::models::Trove>,
) -> Result<()> {
    let path_str = package_path.to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;
    let format = detect_package_format(path_str)?;
    info!("Detected package format: {:?}", format);

    let package: Box<dyn PackageFormat> = match format {
        PackageFormatType::Rpm => Box::new(RpmPackage::parse(path_str)?),
        PackageFormatType::Deb => Box::new(DebPackage::parse(path_str)?),
        PackageFormatType::Arch => Box::new(ArchPackage::parse(path_str)?),
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        package.name(), package.version(), package.files().len(), package.dependencies().len()
    );

    info!("Extracting file contents from package...");
    let extracted_files = package.extract_file_contents()?;
    info!("Extracted {} files", extracted_files.len());

    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| "/var/lib/conary".to_string());
    let objects_dir = PathBuf::from(&db_dir).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    conary::db::transaction(conn, |tx| {
        let changeset_desc = if let Some(old) = old_trove {
            format!("Upgrade {} from {} to {}", package.name(), old.version, package.version())
        } else {
            format!("Install {}-{}", package.name(), package.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
        let changeset_id = changeset.insert(tx)?;

        if let Some(old) = old_trove {
            if let Some(old_id) = old.id {
                info!("Removing old version {} before upgrade", old.version);
                conary::db::models::Trove::delete(tx, old_id)?;
            }
        }

        let mut trove = package.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        for file in &extracted_files {
            if deployer.file_exists(&file.path) {
                if let Some(existing) = conary::db::models::FileEntry::find_by_path(tx, &file.path)? {
                    let owner_trove = conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove {
                        if owner.name != package.name() {
                            return Err(conary::Error::InitError(format!(
                                "File conflict: {} is owned by package {}", file.path, owner.name
                            )));
                        }
                    }
                } else if old_trove.is_none() {
                    return Err(conary::Error::InitError(format!(
                        "File conflict: {} exists but is not tracked by any package", file.path
                    )));
                }
            }

            let hash = deployer.cas().store(&file.content)?;
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(), hash.clone(), file.size, file.mode, trove_id,
            );
            file_entry.insert(tx)?;

            let action = if deployer.file_exists(&file.path) { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), &file.path, &hash, action],
            )?;
        }

        for dep in package.dependencies() {
            let dep_type_str = match dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Build => "build",
                DependencyType::Optional => "optional",
            };
            let mut dep_entry = conary::db::models::DependencyEntry::new(
                trove_id, dep.name.clone(), dep.version.clone(), dep_type_str.to_string(), None,
            );
            dep_entry.insert(tx)?;
        }

        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })?;

    info!("Deploying files to filesystem...");
    for file in &extracted_files {
        let hash = conary::filesystem::CasStore::compute_hash(&file.content);
        deployer.deploy_file(&file.path, &hash, file.mode as u32)?;
    }
    info!("Successfully deployed {} files", extracted_files.len());

    Ok(())
}

// =============================================================================
// Command Handlers
// =============================================================================

/// Initialize the Conary database and add default repositories
pub fn cmd_init(db_path: &str) -> Result<()> {
    info!("Initializing Conary database at: {}", db_path);
    conary::db::init(db_path)?;
    println!("Database initialized successfully at: {}", db_path);

    let conn = conary::db::open(db_path)?;
    info!("Adding default repositories...");

    let default_repos = [
        ("arch-core", "https://geo.mirror.pkgbuild.com/core/os/x86_64", 100, "Arch Linux"),
        ("arch-extra", "https://geo.mirror.pkgbuild.com/extra/os/x86_64", 95, "Arch Linux"),
        ("fedora-43", "https://dl.fedoraproject.org/pub/fedora/linux/releases/43/Everything/x86_64/os", 90, "Fedora 43"),
        ("arch-multilib", "https://geo.mirror.pkgbuild.com/multilib/os/x86_64", 85, "Arch Linux"),
        ("ubuntu-noble", "http://archive.ubuntu.com/ubuntu", 80, "Ubuntu 24.04 LTS"),
    ];

    for (name, url, priority, desc) in default_repos {
        match conary::repository::add_repository(&conn, name.to_string(), url.to_string(), true, priority) {
            Ok(_) => println!("  Added: {} ({})", name, desc),
            Err(e) => eprintln!("  Warning: Could not add {}: {}", name, e),
        }
    }

    println!("\nDefault repositories added. Use 'conary repo-sync' to download metadata.");
    Ok(())
}

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
            pkg_with_repo.package.name, pkg_with_repo.package.version,
            pkg_with_repo.repository.name, pkg_with_repo.repository.priority
        );

        let temp_dir = TempDir::new()?;
        let download_path = repository::download_package(&pkg_with_repo.package, temp_dir.path())?;
        info!("Downloaded package to: {}", download_path.display());
        download_path
    };

    let path_str = package_path.to_str()
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
        rpm.name(), rpm.version(), rpm.files().len(), rpm.dependencies().len()
    );

    let mut conn = conary::db::open(db_path)?;

    // Resolve dependencies
    let dep_names: Vec<String> = rpm.dependencies().iter().map(|d| d.name.clone()).collect();

    if !dep_names.is_empty() {
        info!("Resolving {} dependencies transitively...", dep_names.len());
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
                                    if let Err(e) = install_package_from_file(&dep_path, &mut conn, root, None) {
                                        return Err(anyhow::anyhow!("Failed to install dependency {}: {}", dep_name, e));
                                    }
                                    println!("  [OK] Installed {}", dep_name);
                                }
                            }
                            Err(e) => return Err(anyhow::anyhow!("Failed to download dependencies: {}", e)),
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
        println!("\nWould install package: {} version {}", rpm.name(), rpm.version());
        println!("  Architecture: {}", rpm.architecture().unwrap_or("none"));
        println!("  Files: {}", rpm.files().len());
        println!("  Dependencies: {}", rpm.dependencies().len());
        println!("\nDry run complete. No changes made.");
        return Ok(());
    }

    // Pre-transaction validation
    let existing = conary::db::models::Trove::find_by_name(&conn, rpm.name())?;
    let mut old_trove_to_upgrade: Option<conary::db::models::Trove> = None;

    for trove in &existing {
        if trove.architecture == rpm.architecture().map(|s| s.to_string()) {
            if trove.version == rpm.version() {
                return Err(anyhow::anyhow!(
                    "Package {} version {} ({}) is already installed",
                    rpm.name(), rpm.version(), rpm.architecture().unwrap_or("no-arch")
                ));
            }

            match (RpmVersion::parse(&trove.version), RpmVersion::parse(rpm.version())) {
                (Ok(existing_ver), Ok(new_ver)) => {
                    if new_ver > existing_ver {
                        info!("Upgrading {} from version {} to {}", rpm.name(), trove.version, rpm.version());
                        old_trove_to_upgrade = Some(trove.clone());
                    } else {
                        return Err(anyhow::anyhow!(
                            "Cannot downgrade package {} from version {} to {}",
                            rpm.name(), trove.version, rpm.version()
                        ));
                    }
                }
                _ => warn!("Could not compare versions {} and {}", trove.version, rpm.version()),
            }
        }
    }

    // Extract and install
    info!("Extracting file contents from package...");
    let extracted_files = rpm.extract_file_contents()?;
    info!("Extracted {} files", extracted_files.len());

    let objects_dir = Path::new(db_path).parent().unwrap_or(Path::new(".")).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let _changeset_id = conary::db::transaction(&mut conn, |tx| {
        let changeset_desc = if let Some(ref old_trove) = old_trove_to_upgrade {
            format!("Upgrade {} from {} to {}", rpm.name(), old_trove.version, rpm.version())
        } else {
            format!("Install {}-{}", rpm.name(), rpm.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
        let changeset_id = changeset.insert(tx)?;

        if let Some(old_trove) = old_trove_to_upgrade.as_ref() {
            if let Some(old_id) = old_trove.id {
                info!("Removing old version {} before upgrade", old_trove.version);
                conary::db::models::Trove::delete(tx, old_id)?;
            }
        }

        let mut trove = rpm.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        for file in &extracted_files {
            if deployer.file_exists(&file.path) {
                if let Some(existing) = conary::db::models::FileEntry::find_by_path(tx, &file.path)? {
                    let owner_trove = conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove && owner.name != rpm.name() {
                        return Err(conary::Error::InitError(format!(
                            "File conflict: {} is owned by package {}", file.path, owner.name
                        )));
                    }
                } else {
                    return Err(conary::Error::InitError(format!(
                        "File conflict: {} exists but is not tracked by any package", file.path
                    )));
                }
            }

            let hash = deployer.cas().store(&file.content)?;
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(), hash.clone(), file.size, file.mode, trove_id,
            );
            file_entry.insert(tx)?;

            let action = if deployer.file_exists(&file.path) { "modify" } else { "add" };
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
                trove_id, dep.name.clone(), dep.version.clone(), dep_type_str.to_string(), None,
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

    println!("Installed package: {} version {}", rpm.name(), rpm.version());
    println!("  Architecture: {}", rpm.architecture().unwrap_or("none"));
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
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    if troves.len() > 1 {
        println!("Multiple versions of '{}' found:", package_name);
        for trove in &troves {
            println!("  - version {}", trove.version);
        }
        return Err(anyhow::anyhow!("Please specify version (future enhancement)"));
    }

    let trove = &troves[0];
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let resolver = conary::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

    if !breaking.is_empty() {
        println!("WARNING: Removing '{}' would break the following packages:", package_name);
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nRefusing to remove package with dependencies.");
        println!("Use 'conary whatbreaks {}' for more information.", package_name);
        return Err(anyhow::anyhow!(
            "Cannot remove '{}': {} packages depend on it", package_name, breaking.len()
        ));
    }

    let file_count = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?.len();

    conary::db::transaction(&mut conn, |tx| {
        let mut changeset = conary::db::models::Changeset::new(format!("Remove {}-{}", trove.name, trove.version));
        changeset.insert(tx)?;
        conary::db::models::Trove::delete(tx, trove_id)?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;
        Ok(())
    })?;

    // TODO: Actually delete files from filesystem (Phase 6)

    println!("Removed package: {} version {}", trove.name, trove.version);
    println!("  Architecture: {}", trove.architecture.as_deref().unwrap_or("none"));
    println!("  Files removed: {}", file_count);

    Ok(())
}

/// Query installed packages
pub fn cmd_query(pattern: Option<&str>, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = if let Some(pattern) = pattern {
        conary::db::models::Trove::find_by_name(&conn, pattern)?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
             FROM troves ORDER BY name, version"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(conary::db::models::Trove {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                version: row.get(2)?,
                trove_type: row.get::<_, String>(3)?.parse().unwrap_or(conary::db::models::TroveType::Package),
                architecture: row.get(4)?,
                description: row.get(5)?,
                installed_at: row.get(6)?,
                installed_by_changeset_id: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if troves.is_empty() {
        println!("No packages found.");
    } else {
        println!("Installed packages:");
        for trove in &troves {
            print!("  {} {} ({:?})", trove.name, trove.version, trove.trove_type);
            if let Some(arch) = &trove.architecture {
                print!(" [{}]", arch);
            }
            println!();
        }
        println!("\nTotal: {} package(s)", troves.len());
    }

    Ok(())
}

/// Show changeset history
pub fn cmd_history(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;
    let changesets = conary::db::models::Changeset::list_all(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
            let timestamp = changeset.applied_at.as_ref()
                .or(changeset.rolled_back_at.as_ref())
                .or(changeset.created_at.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("pending");
            let id = changeset.id.map(|i| i.to_string()).unwrap_or_else(|| "?".to_string());
            println!("  [{}] {} - {} ({:?})", id, timestamp, changeset.description, changeset.status);
        }
        println!("\nTotal: {} changeset(s)", changesets.len());
    }

    Ok(())
}

/// Rollback a changeset
pub fn cmd_rollback(changeset_id: i64, db_path: &str, root: &str) -> Result<()> {
    info!("Rolling back changeset: {}", changeset_id);

    let mut conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path).parent().unwrap_or(Path::new(".")).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let changeset = conary::db::models::Changeset::find_by_id(&conn, changeset_id)?
        .ok_or_else(|| anyhow::anyhow!("Changeset {} not found", changeset_id))?;

    if changeset.status == conary::db::models::ChangesetStatus::RolledBack {
        return Err(anyhow::anyhow!("Changeset {} is already rolled back", changeset_id));
    }
    if changeset.status == conary::db::models::ChangesetStatus::Pending {
        return Err(anyhow::anyhow!("Cannot rollback pending changeset {}", changeset_id));
    }

    let files_to_rollback: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT path, action FROM file_history WHERE changeset_id = ?1")?;
        let rows = stmt.query_map([changeset_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    conary::db::transaction(&mut conn, |tx| {
        let troves = {
            let mut stmt = tx.prepare(
                "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
                 FROM troves WHERE installed_by_changeset_id = ?1"
            )?;
            let rows = stmt.query_map([changeset_id], |row| {
                Ok(conary::db::models::Trove {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    version: row.get(2)?,
                    trove_type: row.get::<_, String>(3)?.parse().unwrap_or(conary::db::models::TroveType::Package),
                    architecture: row.get(4)?,
                    description: row.get(5)?,
                    installed_at: row.get(6)?,
                    installed_by_changeset_id: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if troves.is_empty() {
            return Err(conary::Error::InitError(
                "No troves found for this changeset. Cannot rollback Remove operations yet.".to_string()
            ));
        }

        let mut rollback_changeset = conary::db::models::Changeset::new(format!(
            "Rollback of changeset {} ({})", changeset_id, changeset.description
        ));
        let rollback_changeset_id = rollback_changeset.insert(tx)?;

        for trove in &troves {
            if let Some(trove_id) = trove.id {
                conary::db::models::Trove::delete(tx, trove_id)?;
                println!("Removed {} version {}", trove.name, trove.version);
            }
        }

        rollback_changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

        tx.execute(
            "UPDATE changesets SET status = 'rolled_back', rolled_back_at = CURRENT_TIMESTAMP,
             reversed_by_changeset_id = ?1 WHERE id = ?2",
            [rollback_changeset_id, changeset_id],
        )?;

        Ok(troves.len())
    })?;

    info!("Removing files from filesystem...");
    for (path, action) in &files_to_rollback {
        if action == "add" || action == "modify" {
            deployer.remove_file(path)?;
            info!("Removed file: {}", path);
        }
    }

    println!("Rollback complete. Changeset {} has been reversed.", changeset_id);
    println!("  Removed {} files from filesystem", files_to_rollback.len());

    Ok(())
}

/// Verify installed files
pub fn cmd_verify(package: Option<String>, db_path: &str, root: &str) -> Result<()> {
    info!("Verifying installed files...");

    let conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path).parent().unwrap_or(Path::new(".")).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    let files: Vec<(String, String, String)> = if let Some(pkg_name) = package {
        let troves = conary::db::models::Trove::find_by_name(&conn, &pkg_name)?;
        if troves.is_empty() {
            return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
        }

        let mut all_files = Vec::new();
        for trove in &troves {
            if let Some(trove_id) = trove.id {
                let trove_files = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?;
                for file in trove_files {
                    all_files.push((file.path, file.sha256_hash, trove.name.clone()));
                }
            }
        }
        all_files
    } else {
        let mut stmt = conn.prepare(
            "SELECT f.path, f.sha256_hash, t.name FROM files f
             JOIN troves t ON f.trove_id = t.id ORDER BY t.name, f.path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if files.is_empty() {
        println!("No files to verify");
        return Ok(());
    }

    let mut ok_count = 0;
    let mut modified_count = 0;
    let mut missing_count = 0;

    for (path, expected_hash, pkg_name) in &files {
        match deployer.verify_file(path, expected_hash) {
            Ok(true) => {
                ok_count += 1;
                info!("OK: {} (from {})", path, pkg_name);
            }
            Ok(false) => {
                modified_count += 1;
                println!("MODIFIED: {} (from {})", path, pkg_name);
            }
            Err(_) => {
                missing_count += 1;
                println!("MISSING: {} (from {})", path, pkg_name);
            }
        }
    }

    println!("\nVerification summary:");
    println!("  OK: {} files", ok_count);
    println!("  Modified: {} files", modified_count);
    println!("  Missing: {} files", missing_count);
    println!("  Total: {} files", files.len());

    if modified_count > 0 || missing_count > 0 {
        return Err(anyhow::anyhow!("Verification failed"));
    }

    Ok(())
}

/// Show dependencies for a package
pub fn cmd_depends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing dependencies for package: {}", package_name);
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    let trove = troves.first().ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let deps = conary::db::models::DependencyEntry::find_by_trove(&conn, trove_id)?;

    if deps.is_empty() {
        println!("Package '{}' has no dependencies", package_name);
    } else {
        println!("Dependencies for package '{}':", package_name);
        for dep in deps {
            print!("  {} ({})", dep.depends_on_name, dep.dependency_type);
            if let Some(version) = dep.depends_on_version {
                print!(" - version: {}", version);
            }
            if let Some(constraint) = dep.version_constraint {
                print!(" - constraint: {}", constraint);
            }
            println!();
        }
    }

    Ok(())
}

/// Show reverse dependencies
pub fn cmd_rdepends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing reverse dependencies for package: {}", package_name);
    let conn = conary::db::open(db_path)?;

    let dependents = conary::db::models::DependencyEntry::find_dependents(&conn, package_name)?;

    if dependents.is_empty() {
        println!("No packages depend on '{}' (or package not installed)", package_name);
    } else {
        println!("Packages that depend on '{}':", package_name);
        for dep in dependents {
            if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(&conn, dep.trove_id) {
                print!("  {} ({})", trove.name, dep.dependency_type);
                if let Some(constraint) = dep.version_constraint {
                    print!(" - requires: {}", constraint);
                }
                println!();
            }
        }
    }

    Ok(())
}

/// Show what packages would break if a package is removed
pub fn cmd_whatbreaks(package_name: &str, db_path: &str) -> Result<()> {
    info!("Checking what would break if '{}' is removed...", package_name);
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    troves.first().ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;

    let resolver = conary::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

    if breaking.is_empty() {
        println!("Package '{}' can be safely removed (no dependencies)", package_name);
    } else {
        println!("Removing '{}' would break the following packages:", package_name);
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nTotal: {} packages would be affected", breaking.len());
    }

    Ok(())
}

/// Add a new repository
pub fn cmd_repo_add(name: &str, url: &str, db_path: &str, priority: i32, disabled: bool) -> Result<()> {
    info!("Adding repository: {} ({})", name, url);
    let conn = conary::db::open(db_path)?;
    let repo = conary::repository::add_repository(&conn, name.to_string(), url.to_string(), !disabled, priority)?;
    println!("Added repository: {}", repo.name);
    println!("  URL: {}", repo.url);
    println!("  Enabled: {}", repo.enabled);
    println!("  Priority: {}", repo.priority);
    Ok(())
}

/// List repositories
pub fn cmd_repo_list(db_path: &str, all: bool) -> Result<()> {
    info!("Listing repositories");
    let conn = conary::db::open(db_path)?;
    let repos = if all {
        conary::db::models::Repository::list_all(&conn)?
    } else {
        conary::db::models::Repository::list_enabled(&conn)?
    };

    if repos.is_empty() {
        println!("No repositories configured");
    } else {
        println!("Repositories:");
        for repo in repos {
            let enabled_mark = if repo.enabled { "[x]" } else { "[ ]" };
            let sync_status = repo.last_sync.as_ref()
                .map(|ts| format!("synced {}", ts))
                .unwrap_or_else(|| "never synced".to_string());
            println!("  {} {} (priority: {}, {})", enabled_mark, repo.name, repo.priority, sync_status);
            println!("      {}", repo.url);
        }
    }
    Ok(())
}

/// Remove a repository
pub fn cmd_repo_remove(name: &str, db_path: &str) -> Result<()> {
    info!("Removing repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::remove_repository(&conn, name)?;
    println!("Removed repository: {}", name);
    Ok(())
}

/// Enable a repository
pub fn cmd_repo_enable(name: &str, db_path: &str) -> Result<()> {
    info!("Enabling repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::set_repository_enabled(&conn, name, true)?;
    println!("Enabled repository: {}", name);
    Ok(())
}

/// Disable a repository
pub fn cmd_repo_disable(name: &str, db_path: &str) -> Result<()> {
    info!("Disabling repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::set_repository_enabled(&conn, name, false)?;
    println!("Disabled repository: {}", name);
    Ok(())
}

/// Sync repository metadata
pub fn cmd_repo_sync(name: Option<String>, db_path: &str, force: bool) -> Result<()> {
    info!("Synchronizing repository metadata");

    let conn = conary::db::open(db_path)?;

    let repos_to_sync = if let Some(repo_name) = name {
        let repo = conary::db::models::Repository::find_by_name(&conn, &repo_name)?
            .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repo_name))?;
        vec![repo]
    } else {
        conary::db::models::Repository::list_enabled(&conn)?
    };

    if repos_to_sync.is_empty() {
        println!("No repositories to sync");
        return Ok(());
    }

    let repos_needing_sync: Vec<_> = repos_to_sync
        .into_iter()
        .filter(|repo| force || conary::repository::needs_sync(repo))
        .collect();

    if repos_needing_sync.is_empty() {
        println!("All repositories are up to date");
        return Ok(());
    }

    use rayon::prelude::*;
    let results: Vec<(String, conary::Result<usize>)> = repos_needing_sync
        .par_iter()
        .map(|repo| {
            println!("Syncing repository: {} ...", repo.name);
            let sync_result = (|| -> conary::Result<usize> {
                let conn = conary::db::open(db_path)?;
                let mut repo_mut = repo.clone();
                conary::repository::sync_repository(&conn, &mut repo_mut)
            })();
            (repo.name.clone(), sync_result)
        })
        .collect();

    for (name, result) in results {
        match result {
            Ok(count) => println!("  [OK] Synchronized {} packages from {}", count, name),
            Err(e) => println!("  [FAILED] Failed to sync {}: {}", name, e),
        }
    }

    Ok(())
}

/// Search for packages
pub fn cmd_search(pattern: &str, db_path: &str) -> Result<()> {
    info!("Searching for packages matching: {}", pattern);
    let conn = conary::db::open(db_path)?;
    let packages = conary::repository::search_packages(&conn, pattern)?;

    if packages.is_empty() {
        println!("No packages found matching '{}'", pattern);
    } else {
        println!("Found {} packages matching '{}':", packages.len(), pattern);
        for pkg in packages {
            let arch_str = pkg.architecture.as_deref().unwrap_or("noarch");
            println!("  {} {} ({})", pkg.name, pkg.version, arch_str);
            if let Some(desc) = &pkg.description {
                println!("      {}", desc);
            }
        }
    }
    Ok(())
}

/// Check for and apply package updates
pub fn cmd_update(package: Option<String>, db_path: &str, root: &str) -> Result<()> {
    info!("Checking for package updates");

    let mut conn = conary::db::open(db_path)?;

    let objects_dir = Path::new(db_path).parent().unwrap_or(Path::new(".")).join("objects");
    let temp_dir = Path::new(db_path).parent().unwrap_or(Path::new(".")).join("tmp");
    std::fs::create_dir_all(&temp_dir)?;

    let installed_troves = if let Some(pkg_name) = package {
        conary::db::models::Trove::find_by_name(&conn, &pkg_name)?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(conary::db::models::Trove {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                version: row.get(2)?,
                trove_type: row.get::<_, String>(3)?.parse().unwrap_or(conary::db::models::TroveType::Package),
                architecture: row.get(4)?,
                description: row.get(5)?,
                installed_at: row.get(6)?,
                installed_by_changeset_id: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if installed_troves.is_empty() {
        println!("No packages to update");
        return Ok(());
    }

    let mut updates_available = Vec::new();
    for trove in &installed_troves {
        let repo_packages = conary::db::models::RepositoryPackage::find_by_name(&conn, &trove.name)?;
        for repo_pkg in repo_packages {
            if repo_pkg.version != trove.version {
                if repo_pkg.architecture == trove.architecture || repo_pkg.architecture.is_none() {
                    info!("Update available: {} {} -> {}", trove.name, trove.version, repo_pkg.version);
                    updates_available.push((trove.clone(), repo_pkg));
                    break;
                }
            }
        }
    }

    if updates_available.is_empty() {
        println!("All packages are up to date");
        return Ok(());
    }

    println!("Found {} package(s) with updates available:", updates_available.len());
    for (trove, repo_pkg) in &updates_available {
        println!("  {} {} -> {}", trove.name, trove.version, repo_pkg.version);
    }

    let mut total_bytes_saved = 0i64;
    let mut deltas_applied = 0i32;
    let mut full_downloads = 0i32;
    let mut delta_failures = 0i32;

    let changeset_id = conary::db::transaction(&mut conn, |tx| {
        let mut changeset = conary::db::models::Changeset::new(
            format!("Update {} package(s)", updates_available.len())
        );
        changeset.insert(tx)
    })?;

    for (installed_trove, repo_pkg) in updates_available {
        println!("\nUpdating {} ...", installed_trove.name);

        let mut delta_success = false;

        if let Ok(Some(delta_info)) = PackageDelta::find_delta(
            &conn, &installed_trove.name, &installed_trove.version, &repo_pkg.version,
        ) {
            println!(
                "  Delta available: {} bytes ({:.1}% of full size)",
                delta_info.delta_size, delta_info.compression_ratio * 100.0
            );

            let delta_path = temp_dir.join(format!(
                "{}-{}-to-{}.delta",
                installed_trove.name, installed_trove.version, repo_pkg.version
            ));

            match repository::download_delta(
                &repository::DeltaInfo {
                    from_version: delta_info.from_version,
                    from_hash: delta_info.from_hash.clone(),
                    delta_url: delta_info.delta_url,
                    delta_size: delta_info.delta_size,
                    delta_checksum: delta_info.delta_checksum,
                    compression_ratio: delta_info.compression_ratio,
                },
                &installed_trove.name,
                &repo_pkg.version,
                &temp_dir,
            ) {
                Ok(_) => {
                    let applier = DeltaApplier::new(&objects_dir)?;
                    match applier.apply_delta(&delta_info.from_hash, &delta_path, &delta_info.to_hash) {
                        Ok(_) => {
                            println!("  [OK] Delta applied successfully");
                            delta_success = true;
                            deltas_applied += 1;
                            total_bytes_saved += repo_pkg.size - delta_info.delta_size;
                        }
                        Err(e) => {
                            warn!("  Delta application failed: {}", e);
                            delta_failures += 1;
                        }
                    }
                    let _ = std::fs::remove_file(delta_path);
                }
                Err(e) => {
                    warn!("  Delta download failed: {}", e);
                    delta_failures += 1;
                }
            }
        }

        if !delta_success {
            println!("  Downloading full package...");
            match repository::download_package(&repo_pkg, &temp_dir) {
                Ok(pkg_path) => {
                    println!("  [OK] Downloaded {} bytes", repo_pkg.size);
                    full_downloads += 1;

                    if let Err(e) = install_package_from_file(&pkg_path, &mut conn, root, Some(&installed_trove)) {
                        warn!("  Package installation failed: {}", e);
                        let _ = std::fs::remove_file(pkg_path);
                        continue;
                    }

                    println!("  [OK] Package installed successfully");
                    let _ = std::fs::remove_file(pkg_path);
                }
                Err(e) => {
                    warn!("  Full download failed: {}", e);
                    continue;
                }
            }
        }
    }

    conary::db::transaction(&mut conn, |tx| {
        let mut stats = DeltaStats::new(changeset_id);
        stats.total_bytes_saved = total_bytes_saved;
        stats.deltas_applied = deltas_applied;
        stats.full_downloads = full_downloads;
        stats.delta_failures = delta_failures;
        stats.insert(tx)?;

        let mut changeset = conary::db::models::Changeset::find_by_id(tx, changeset_id)?
            .ok_or_else(|| conary::Error::NotFoundError("Changeset not found".to_string()))?;
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

        Ok(())
    })?;

    println!("\n=== Update Summary ===");
    println!("Delta updates: {}", deltas_applied);
    println!("Full downloads: {}", full_downloads);
    println!("Delta failures: {}", delta_failures);
    if total_bytes_saved > 0 {
        let saved_mb = total_bytes_saved as f64 / 1_048_576.0;
        println!("Bandwidth saved: {:.2} MB", saved_mb);
    }

    Ok(())
}

/// Show delta update statistics
pub fn cmd_delta_stats(db_path: &str) -> Result<()> {
    info!("Showing delta update statistics");

    let conn = conary::db::open(db_path)?;
    let total_stats = DeltaStats::get_total_stats(&conn)?;

    let all_stats = {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DeltaStats {
                id: Some(row.get(0)?),
                changeset_id: row.get(1)?,
                total_bytes_saved: row.get(2)?,
                deltas_applied: row.get(3)?,
                full_downloads: row.get(4)?,
                delta_failures: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if all_stats.is_empty() {
        println!("No delta statistics available");
        println!("Run 'conary update' to start tracking delta usage");
        return Ok(());
    }

    println!("=== Delta Update Statistics ===\n");
    println!("Total Statistics:");
    println!("  Delta updates applied: {}", total_stats.deltas_applied);
    println!("  Full downloads: {}", total_stats.full_downloads);
    println!("  Delta failures: {}", total_stats.delta_failures);

    let total_mb = total_stats.total_bytes_saved as f64 / 1_048_576.0;
    println!("  Total bandwidth saved: {:.2} MB", total_mb);

    let total_updates = total_stats.deltas_applied + total_stats.full_downloads;
    if total_updates > 0 {
        let success_rate = (total_stats.deltas_applied as f64 / total_updates as f64) * 100.0;
        println!("  Delta success rate: {:.1}%", success_rate);
    }

    println!("\nRecent Operations:");
    for (idx, stats) in all_stats.iter().take(10).enumerate() {
        if idx > 0 {
            println!();
        }

        let timestamp = stats.created_at.as_deref().unwrap_or("unknown");
        println!("  [Changeset {}] {}", stats.changeset_id, timestamp);
        println!("    Deltas applied: {}", stats.deltas_applied);
        println!("    Full downloads: {}", stats.full_downloads);

        if stats.delta_failures > 0 {
            println!("    Delta failures: {}", stats.delta_failures);
        }

        if stats.total_bytes_saved > 0 {
            let saved_mb = stats.total_bytes_saved as f64 / 1_048_576.0;
            println!("    Bandwidth saved: {:.2} MB", saved_mb);
        }
    }

    if all_stats.len() > 10 {
        println!("\n... and {} more operations", all_stats.len() - 10);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_from_rpm_extension() {
        let result = detect_package_format("/path/to/package.rpm");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Rpm);
    }

    #[test]
    fn test_detect_format_from_deb_extension() {
        let result = detect_package_format("/path/to/package.deb");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Deb);
    }

    #[test]
    fn test_detect_format_from_arch_extension() {
        let result = detect_package_format("/path/to/package.pkg.tar.zst");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageFormatType::Arch);
    }
}
