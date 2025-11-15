// src/main.rs

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use conary::db::models::{DeltaStats, PackageDelta};
use conary::delta::DeltaApplier;
use conary::packages::rpm::RpmPackage;
use conary::packages::traits::DependencyType;
use conary::packages::PackageFormat;
use conary::repository::{self, PackageSelector, SelectionOptions};
use conary::version::RpmVersion;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{info, warn};

/// Package format types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageFormatType {
    Rpm,
    Deb,
    Arch,
}

#[derive(Parser)]
#[command(name = "conary")]
#[command(author, version, about = "Modern package manager with atomic operations and rollback", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the Conary database
    Init {
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Install a package from file or repository
    Install {
        /// Package file path or package name
        package: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Install root directory (default: /)
        #[arg(short, long, default_value = "/")]
        root: String,
        /// Specific version to install
        #[arg(long)]
        version: Option<String>,
        /// Specific repository to use
        #[arg(long)]
        repo: Option<String>,
        /// Dry run - show what would be installed without installing
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove an installed package
    Remove {
        /// Package name to remove
        package_name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Query installed packages
    Query {
        /// Package name pattern (optional, shows all if omitted)
        pattern: Option<String>,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show changeset history
    History {
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Rollback a changeset
    Rollback {
        /// Changeset ID to rollback
        changeset_id: i64,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Install root directory (default: /)
        #[arg(short, long, default_value = "/")]
        root: String,
    },
    /// Verify installed files match their stored hashes
    Verify {
        /// Package name to verify (optional, verifies all if omitted)
        package: Option<String>,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Install root directory (default: /)
        #[arg(short, long, default_value = "/")]
        root: String,
    },
    /// Show dependencies of a package
    Depends {
        /// Package name
        package_name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show reverse dependencies (what depends on this package)
    Rdepends {
        /// Package name
        package_name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show what packages would break if this package is removed
    Whatbreaks {
        /// Package name
        package_name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Generate shell completion scripts
    Completions {
        /// Shell type (bash, zsh, fish, powershell)
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Add a new repository
    RepoAdd {
        /// Repository name
        name: String,
        /// Repository URL
        url: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Priority (higher = preferred, default: 0)
        #[arg(short, long, default_value = "0")]
        priority: i32,
        /// Disable repository after adding
        #[arg(long)]
        disabled: bool,
    },
    /// List repositories
    RepoList {
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Show all repositories (including disabled)
        #[arg(short, long)]
        all: bool,
    },
    /// Remove a repository
    RepoRemove {
        /// Repository name
        name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Enable a repository
    RepoEnable {
        /// Repository name
        name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Disable a repository
    RepoDisable {
        /// Repository name
        name: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Synchronize repository metadata
    RepoSync {
        /// Repository name (syncs all if omitted)
        name: Option<String>,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Force sync even if metadata hasn't expired
        #[arg(short, long)]
        force: bool,
    },
    /// Search for packages in repositories
    Search {
        /// Search pattern
        pattern: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Update installed packages from repositories
    Update {
        /// Package name (updates all if omitted)
        package: Option<String>,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Install root directory (default: /)
        #[arg(short, long, default_value = "/")]
        root: String,
    },
    /// Show delta update statistics
    DeltaStats {
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}

/// Detect package format from file extension and magic bytes
fn detect_package_format(path: &str) -> Result<PackageFormatType> {
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

    // RPM magic: 0xED 0xAB 0xEE 0xDB (first 4 bytes)
    if magic[0..4] == [0xED, 0xAB, 0xEE, 0xDB] {
        return Ok(PackageFormatType::Rpm);
    }

    // DEB magic: "!<arch>\n" (ar archive format)
    if magic[0..7] == *b"!<arch>" {
        return Ok(PackageFormatType::Deb);
    }

    // Arch packages are compressed tar archives
    // Check for zstd magic: 0x28 0xB5 0x2F 0xFD
    if magic[0..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return Ok(PackageFormatType::Arch);
    }

    // Check for xz magic: 0xFD 0x37 0x7A 0x58 0x5A 0x00
    if magic[0..6] == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        return Ok(PackageFormatType::Arch);
    }

    Err(anyhow::anyhow!(
        "Unable to detect package format for: {}",
        path
    ))
}

/// Install a package from a file path
///
/// This function handles the core installation logic and can be used by both
/// Install and Update commands.
///
/// # Arguments
/// * `package_path` - Path to the package file
/// * `conn` - Database connection (must be mutable for transactions)
/// * `root` - Install root directory
/// * `old_trove` - Optional existing trove to upgrade (None for fresh install)
fn install_package_from_file(
    package_path: &Path,
    conn: &mut rusqlite::Connection,
    root: &str,
    old_trove: Option<&conary::db::models::Trove>,
) -> Result<()> {
    // Auto-detect package format
    let format = detect_package_format(package_path.to_str().unwrap())?;
    info!("Detected package format: {:?}", format);

    // Parse the package based on format
    let rpm = match format {
        PackageFormatType::Rpm => RpmPackage::parse(package_path.to_str().unwrap())?,
        PackageFormatType::Deb => {
            return Err(anyhow::anyhow!("DEB format not yet implemented"));
        }
        PackageFormatType::Arch => {
            return Err(anyhow::anyhow!("Arch format not yet implemented"));
        }
    };

    info!(
        "Parsed package: {} version {} ({} files, {} dependencies)",
        rpm.name(),
        rpm.version(),
        rpm.files().len(),
        rpm.dependencies().len()
    );

    // Extract file contents from RPM
    info!("Extracting file contents from package...");
    let extracted_files = rpm.extract_file_contents()?;
    info!("Extracted {} files", extracted_files.len());

    // Initialize CAS and file deployer
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| "/var/lib/conary".to_string());
    let objects_dir = PathBuf::from(&db_dir).join("objects");
    let install_root = PathBuf::from(root);
    let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

    // Perform installation within a changeset transaction
    conary::db::transaction(conn, |tx| {
        // Create changeset for this installation
        let changeset_desc = if let Some(old) = old_trove {
            format!(
                "Upgrade {} from {} to {}",
                rpm.name(),
                old.version,
                rpm.version()
            )
        } else {
            format!("Install {}-{}", rpm.name(), rpm.version())
        };
        let mut changeset = conary::db::models::Changeset::new(changeset_desc);
        let changeset_id = changeset.insert(tx)?;

        // If upgrading, remove the old trove first
        if let Some(old) = old_trove {
            if let Some(old_id) = old.id {
                info!("Removing old version {} before upgrade", old.version);
                conary::db::models::Trove::delete(tx, old_id)?;
            }
        }

        // Convert to Trove and associate with changeset
        let mut trove = rpm.to_trove();
        trove.installed_by_changeset_id = Some(changeset_id);
        let trove_id = trove.insert(tx)?;

        // Process each file: conflict check, store in CAS, deploy, track in DB
        for file in &extracted_files {
            // Conflict detection (skip if upgrading same package)
            if deployer.file_exists(&file.path) {
                if let Some(existing) = conary::db::models::FileEntry::find_by_path(tx, &file.path)? {
                    let owner_trove = conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                    if let Some(owner) = owner_trove {
                        if owner.name != rpm.name() {
                            return Err(conary::Error::InitError(format!(
                                "File conflict: {} is owned by package {}",
                                file.path, owner.name
                            )));
                        }
                    }
                } else if old_trove.is_none() {
                    // Only error on orphans for fresh installs, not upgrades
                    return Err(conary::Error::InitError(format!(
                        "File conflict: {} exists but is not tracked by any package",
                        file.path
                    )));
                }
            }

            // Store content in CAS
            let hash = deployer.cas().store(&file.content)?;

            // Store file content metadata in database
            tx.execute(
                "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
            )?;

            // Store file metadata in database
            let mut file_entry = conary::db::models::FileEntry::new(
                file.path.clone(),
                hash.clone(),
                file.size,
                file.mode,
                trove_id,
            );
            file_entry.insert(tx)?;

            // Track in file history
            let action = if deployer.file_exists(&file.path) { "modify" } else { "add" };
            tx.execute(
                "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                [&changeset_id.to_string(), &file.path, &hash, action],
            )?;
        }

        // Store dependencies in database
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

        // Mark changeset as applied
        changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

        Ok(())
    })?;

    // Deploy files to filesystem (outside transaction for safety)
    info!("Deploying files to filesystem...");
    for file in &extracted_files {
        let hash = conary::filesystem::CasStore::compute_hash(&file.content);
        deployer.deploy_file(&file.path, &hash, file.mode as u32)?;
    }
    info!("Successfully deployed {} files", extracted_files.len());

    Ok(())
}

fn main() -> Result<()> {
    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { db_path }) => {
            info!("Initializing Conary database at: {}", db_path);
            conary::db::init(&db_path)?;
            println!("Database initialized successfully at: {}", db_path);
            Ok(())
        }
        Some(Commands::Install {
            package,
            db_path,
            root,
            version,
            repo,
            dry_run,
        }) => {
            info!("Installing package: {}", package);

            // Detect if this is a file path or package name
            let package_path = if Path::new(&package).exists() {
                // It's a file path - use directly
                info!("Installing from local file: {}", package);
                PathBuf::from(&package)
            } else {
                // It's a package name - search repositories and download
                info!("Searching repositories for package: {}", package);

                // Open database connection for repository search
                let conn = conary::db::open(&db_path)?;

                // Build selection options
                let options = SelectionOptions {
                    version: version.clone(),
                    repository: repo.clone(),
                    architecture: None, // Use system architecture
                };

                // Find best matching package
                let pkg_with_repo = PackageSelector::find_best_package(&conn, &package, &options)?;

                info!(
                    "Found package {} {} in repository {} (priority {})",
                    pkg_with_repo.package.name,
                    pkg_with_repo.package.version,
                    pkg_with_repo.repository.name,
                    pkg_with_repo.repository.priority
                );

                // Download package to temp directory
                let temp_dir = TempDir::new()?;
                let download_path = repository::download_package(&pkg_with_repo.package, temp_dir.path())?;

                info!("Downloaded package to: {}", download_path.display());
                download_path
            };

            // Auto-detect package format
            let format = detect_package_format(package_path.to_str().unwrap())?;
            info!("Detected package format: {:?}", format);

            // Parse the package based on format
            let rpm = match format {
                PackageFormatType::Rpm => RpmPackage::parse(package_path.to_str().unwrap())?,
                PackageFormatType::Deb => {
                    return Err(anyhow::anyhow!("DEB format not yet implemented"));
                }
                PackageFormatType::Arch => {
                    return Err(anyhow::anyhow!("Arch format not yet implemented"));
                }
            };

            info!(
                "Parsed package: {} version {} ({} files, {} dependencies)",
                rpm.name(),
                rpm.version(),
                rpm.files().len(),
                rpm.dependencies().len()
            );

            // Open database connection
            let mut conn = conary::db::open(&db_path)?;

            // Auto-resolve and install dependencies
            let dep_names: Vec<String> = rpm.dependencies()
                .iter()
                .map(|d| d.name.clone())
                .collect();

            if !dep_names.is_empty() {
                info!("Resolving {} dependencies...", dep_names.len());
                println!("Checking dependencies for {}...", rpm.name());

                match repository::resolve_dependencies(&conn, &dep_names) {
                    Ok(to_download) => {
                        if !to_download.is_empty() {
                            if dry_run {
                                println!("Would install {} missing dependencies:", to_download.len());
                                for (dep_name, pkg) in &to_download {
                                    println!("  {} ({})", dep_name, pkg.package.version);
                                }
                            } else {
                                println!("Installing {} missing dependencies:", to_download.len());
                                for (dep_name, pkg) in &to_download {
                                    println!("  {} ({})", dep_name, pkg.package.version);
                                }
                            }

                            // Skip download/install if dry-run
                            if !dry_run {
                                // Download all dependencies
                                let temp_dir = TempDir::new()?;
                                match repository::download_dependencies(&to_download, temp_dir.path()) {
                                    Ok(downloaded) => {
                                        // Install each dependency in order
                                        for (dep_name, dep_path) in downloaded {
                                            info!("Installing dependency: {}", dep_name);
                                            println!("Installing dependency: {}", dep_name);

                                            if let Err(e) = install_package_from_file(
                                                &dep_path,
                                                &mut conn,
                                                &root,
                                                None, // No upgrade for dependencies
                                            ) {
                                                return Err(anyhow::anyhow!(
                                                    "Failed to install dependency {}: {}",
                                                    dep_name,
                                                    e
                                                ));
                                            }
                                            println!("  ✓ Installed {}", dep_name);
                                        }
                                    }
                                    Err(e) => {
                                        return Err(anyhow::anyhow!(
                                            "Failed to download dependencies: {}",
                                            e
                                        ));
                                    }
                                }
                            }
                        } else {
                            println!("All dependencies already satisfied");
                        }
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("Dependency resolution failed: {}", e));
                    }
                }
            }

            // If dry-run, show what would be installed and exit
            if dry_run {
                println!("\nWould install package: {} version {}", rpm.name(), rpm.version());
                println!("  Architecture: {}", rpm.architecture().unwrap_or("none"));
                println!("  Files: {}", rpm.files().len());
                println!("  Dependencies: {}", rpm.dependencies().len());
                println!("\nDry run complete. No changes made.");
                return Ok(());
            }

            // Pre-transaction validation and upgrade detection
            let existing = conary::db::models::Trove::find_by_name(&conn, rpm.name())?;
            let mut old_trove_to_upgrade: Option<conary::db::models::Trove> = None;

            for trove in &existing {
                // Only compare packages with same architecture
                if trove.architecture == rpm.architecture().map(|s| s.to_string()) {
                    if trove.version == rpm.version() {
                        // Same version already installed
                        return Err(anyhow::anyhow!(
                            "Package {} version {} ({}) is already installed",
                            rpm.name(),
                            rpm.version(),
                            rpm.architecture().unwrap_or("no-arch")
                        ));
                    }

                    // Compare versions
                    match (RpmVersion::parse(&trove.version), RpmVersion::parse(rpm.version())) {
                        (Ok(existing_ver), Ok(new_ver)) => {
                            if new_ver > existing_ver {
                                // This is an upgrade
                                info!(
                                    "Upgrading {} from version {} to {}",
                                    rpm.name(),
                                    trove.version,
                                    rpm.version()
                                );
                                old_trove_to_upgrade = Some(trove.clone());
                            } else {
                                // Trying to install older version
                                return Err(anyhow::anyhow!(
                                    "Cannot downgrade package {} from version {} to {}",
                                    rpm.name(),
                                    trove.version,
                                    rpm.version()
                                ));
                            }
                        }
                        _ => {
                            // Version parsing failed - allow installation but warn
                            warn!(
                                "Could not compare versions {} and {}",
                                trove.version,
                                rpm.version()
                            );
                        }
                    }
                }
            }

            // Extract file contents from RPM
            info!("Extracting file contents from package...");
            let extracted_files = rpm.extract_file_contents()?;
            info!("Extracted {} files", extracted_files.len());

            // Initialize CAS and file deployer
            let objects_dir = std::path::Path::new(&db_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("objects");
            let install_root = std::path::PathBuf::from(&root);
            let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

            // Perform installation within a changeset transaction
            let _changeset_id = conary::db::transaction(&mut conn, |tx| {
                // Create changeset for this installation
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

                // If upgrading, remove the old trove first
                if let Some(old_trove) = old_trove_to_upgrade {
                    if let Some(old_id) = old_trove.id {
                        info!("Removing old version {} before upgrade", old_trove.version);
                        // Delete old trove (CASCADE will handle files and dependencies)
                        conary::db::models::Trove::delete(tx, old_id)?;
                    }
                }

                // Convert to Trove and associate with changeset
                let mut trove = rpm.to_trove();
                trove.installed_by_changeset_id = Some(changeset_id);
                let trove_id = trove.insert(tx)?;

                // Process each file: conflict check, store in CAS, deploy, track in DB
                for file in &extracted_files {
                    // Conflict detection
                    if deployer.file_exists(&file.path) {
                        // Check if file is tracked in database
                        if let Some(existing) = conary::db::models::FileEntry::find_by_path(tx, &file.path)? {
                            // File exists and is tracked - check ownership
                            let owner_trove = conary::db::models::Trove::find_by_id(tx, existing.trove_id)?;
                            if let Some(owner) = owner_trove
                                && owner.name != rpm.name() {
                                return Err(conary::Error::InitError(format!(
                                    "File conflict: {} is owned by package {}",
                                    file.path, owner.name
                                )));
                            }
                            // Same package owns it - this is an update, allow
                        } else {
                            // File exists but not tracked - orphan file
                            return Err(conary::Error::InitError(format!(
                                "File conflict: {} exists but is not tracked by any package",
                                file.path
                            )));
                        }
                    }

                    // Store content in CAS
                    let hash = deployer.cas().store(&file.content)?;

                    // Store file content metadata in database
                    tx.execute(
                        "INSERT OR IGNORE INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, ?3)",
                        [&hash, &format!("objects/{}/{}", &hash[0..2], &hash[2..]), &file.size.to_string()],
                    )?;

                    // Store file metadata in database
                    let mut file_entry = conary::db::models::FileEntry::new(
                        file.path.clone(),
                        hash.clone(),
                        file.size,
                        file.mode,
                        trove_id,
                    );
                    file_entry.insert(tx)?;

                    // Track in file history
                    let action = if deployer.file_exists(&file.path) { "modify" } else { "add" };
                    tx.execute(
                        "INSERT INTO file_history (changeset_id, path, sha256_hash, action) VALUES (?1, ?2, ?3, ?4)",
                        [&changeset_id.to_string(), &file.path, &hash, action],
                    )?;
                }

                // Store dependencies in database
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
                        None, // version_constraint parsing comes later
                    );
                    dep_entry.insert(tx)?;
                }

                // Mark changeset as applied
                changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

                Ok(changeset_id)
            })?;

            // Deploy files to filesystem (outside transaction for safety)
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

            // Show provenance info if available
            if let Some(source_rpm) = rpm.source_rpm() {
                println!("  Source RPM: {}", source_rpm);
            }
            if let Some(vendor) = rpm.vendor() {
                println!("  Vendor: {}", vendor);
            }

            Ok(())
        }
        Some(Commands::Remove {
            package_name,
            db_path,
        }) => {
            info!("Removing package: {}", package_name);

            // Open database connection
            let mut conn = conary::db::open(&db_path)?;

            // Find the package to remove
            let troves = conary::db::models::Trove::find_by_name(&conn, &package_name)?;

            if troves.is_empty() {
                return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
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
            let trove_id = trove.id.unwrap();

            // Check for reverse dependencies
            let resolver = conary::resolver::Resolver::new(&conn)?;
            let breaking = resolver.check_removal(&package_name)?;

            if !breaking.is_empty() {
                println!(
                    "WARNING: Removing '{}' would break the following packages:",
                    package_name
                );
                for pkg in &breaking {
                    println!("  {}", pkg);
                }
                println!("\nRefusing to remove package with dependencies.");
                println!("Use 'conary whatbreaks {}' for more information.", package_name);
                return Err(anyhow::anyhow!(
                    "Cannot remove '{}': {} packages depend on it",
                    package_name,
                    breaking.len()
                ));
            }

            // Count files before removal for reporting
            let file_count = conary::db::models::FileEntry::find_by_trove(&conn, trove_id)?.len();

            // Perform removal within a changeset transaction
            conary::db::transaction(&mut conn, |tx| {
                // Create changeset for this removal
                let mut changeset = conary::db::models::Changeset::new(format!(
                    "Remove {}-{}",
                    trove.name, trove.version
                ));
                changeset.insert(tx)?;

                // Delete the trove (files will be cascade-deleted due to foreign key)
                conary::db::models::Trove::delete(tx, trove_id)?;

                // Mark changeset as applied
                changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

                Ok(())
            })?;

            // TODO: Actually delete files from filesystem (Phase 6)

            println!("Removed package: {} version {}", trove.name, trove.version);
            println!("  Architecture: {}", trove.architecture.as_deref().unwrap_or("none"));
            println!("  Files removed: {}", file_count);

            Ok(())
        }
        Some(Commands::Query { pattern, db_path }) => {
            let conn = conary::db::open(&db_path)?;

            // Get all troves or filter by pattern
            let troves = if let Some(pattern) = pattern {
                conary::db::models::Trove::find_by_name(&conn, &pattern)?
            } else {
                // Get all troves
                let mut stmt = conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves ORDER BY name, version")?;
                let rows = stmt.query_map([], |row| {
                    Ok(conary::db::models::Trove {
                        id: Some(row.get(0)?),
                        name: row.get(1)?,
                        version: row.get(2)?,
                        trove_type: row.get::<_, String>(3)?.parse().unwrap(),
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
        Some(Commands::History { db_path }) => {
            let conn = conary::db::open(&db_path)?;

            let changesets = conary::db::models::Changeset::list_all(&conn)?;

            if changesets.is_empty() {
                println!("No changeset history.");
            } else {
                println!("Changeset history:");
                for changeset in &changesets {
                    let timestamp = changeset
                        .applied_at
                        .as_ref()
                        .or(changeset.rolled_back_at.as_ref())
                        .or(changeset.created_at.as_ref())
                        .map(|s| s.as_str())
                        .unwrap_or("pending");

                    println!(
                        "  [{}] {} - {} ({:?})",
                        changeset.id.unwrap(),
                        timestamp,
                        changeset.description,
                        changeset.status
                    );
                }
                println!("\nTotal: {} changeset(s)", changesets.len());
            }

            Ok(())
        }
        Some(Commands::Rollback {
            changeset_id,
            db_path,
            root,
        }) => {
            info!("Rolling back changeset: {}", changeset_id);

            let mut conn = conary::db::open(&db_path)?;

            // Initialize file deployer for filesystem operations
            let objects_dir = std::path::Path::new(&db_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("objects");
            let install_root = std::path::PathBuf::from(&root);
            let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

            // Find the changeset to rollback
            let changeset = conary::db::models::Changeset::find_by_id(&conn, changeset_id)?
                .ok_or_else(|| anyhow::anyhow!("Changeset {} not found", changeset_id))?;

            // Check if already rolled back
            if changeset.status == conary::db::models::ChangesetStatus::RolledBack {
                return Err(anyhow::anyhow!(
                    "Changeset {} is already rolled back",
                    changeset_id
                ));
            }

            // Check if not yet applied
            if changeset.status == conary::db::models::ChangesetStatus::Pending {
                return Err(anyhow::anyhow!(
                    "Cannot rollback pending changeset {}",
                    changeset_id
                ));
            }

            // Query file history for this changeset before the transaction
            let files_to_rollback: Vec<(String, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT path, action FROM file_history WHERE changeset_id = ?1"
                )?;
                let rows = stmt.query_map([changeset_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            // Perform rollback in a transaction
            conary::db::transaction(&mut conn, |tx| {
                // Find all troves installed by this changeset
                let troves = {
                    let mut stmt = tx.prepare(
                        "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
                         FROM troves WHERE installed_by_changeset_id = ?1",
                    )?;
                    let rows = stmt.query_map([changeset_id], |row| {
                        Ok(conary::db::models::Trove {
                            id: Some(row.get(0)?),
                            name: row.get(1)?,
                            version: row.get(2)?,
                            trove_type: row.get::<_, String>(3)?.parse().unwrap(),
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

                // Create a new changeset for the rollback operation
                let mut rollback_changeset = conary::db::models::Changeset::new(format!(
                    "Rollback of changeset {} ({})",
                    changeset_id, changeset.description
                ));
                let rollback_changeset_id = rollback_changeset.insert(tx)?;

                // Delete all troves that were installed by the original changeset
                for trove in &troves {
                    conary::db::models::Trove::delete(tx, trove.id.unwrap())?;
                    println!("Removed {} version {}", trove.name, trove.version);
                }

                // Mark the rollback changeset as applied
                rollback_changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

                // Mark the original changeset as rolled back and link to rollback changeset
                tx.execute(
                    "UPDATE changesets
                     SET status = 'rolled_back',
                         rolled_back_at = CURRENT_TIMESTAMP,
                         reversed_by_changeset_id = ?1
                     WHERE id = ?2",
                    [rollback_changeset_id, changeset_id],
                )?;

                Ok(troves.len())
            })?;

            // Remove files from filesystem (outside transaction)
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
        Some(Commands::Verify {
            package,
            db_path,
            root,
        }) => {
            info!("Verifying installed files...");

            let conn = conary::db::open(&db_path)?;

            // Initialize file deployer for verification
            let objects_dir = std::path::Path::new(&db_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("objects");
            let install_root = std::path::PathBuf::from(&root);
            let deployer = conary::filesystem::FileDeployer::new(&objects_dir, &install_root)?;

            // Get files to verify
            let files: Vec<(String, String, String)> = if let Some(pkg_name) = package {
                // Verify specific package
                let troves = conary::db::models::Trove::find_by_name(&conn, &pkg_name)?;
                if troves.is_empty() {
                    return Err(anyhow::anyhow!("Package '{}' is not installed", pkg_name));
                }

                let mut all_files = Vec::new();
                for trove in &troves {
                    let trove_files = conary::db::models::FileEntry::find_by_trove(&conn, trove.id.unwrap())?;
                    for file in trove_files {
                        all_files.push((file.path, file.sha256_hash, trove.name.clone()));
                    }
                }
                all_files
            } else {
                // Verify all installed files
                let mut stmt = conn.prepare(
                    "SELECT f.path, f.sha256_hash, t.name FROM files f
                     JOIN troves t ON f.trove_id = t.id
                     ORDER BY t.name, f.path"
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            if files.is_empty() {
                println!("No files to verify");
                return Ok(());
            }

            // Verify each file
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

            // Print summary
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
        Some(Commands::Depends {
            package_name,
            db_path,
        }) => {
            info!("Showing dependencies for package: {}", package_name);

            let conn = conary::db::open(&db_path)?;

            // Find the trove
            let troves = conary::db::models::Trove::find_by_name(&conn, &package_name)?;
            let trove = troves
                .first()
                .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;

            // Get dependencies
            let deps = conary::db::models::DependencyEntry::find_by_trove(
                &conn,
                trove.id.unwrap(),
            )?;

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
        Some(Commands::Rdepends {
            package_name,
            db_path,
        }) => {
            info!(
                "Showing reverse dependencies for package: {}",
                package_name
            );

            let conn = conary::db::open(&db_path)?;

            // Find packages that depend on this one
            let dependents =
                conary::db::models::DependencyEntry::find_dependents(&conn, &package_name)?;

            if dependents.is_empty() {
                println!(
                    "No packages depend on '{}' (or package not installed)",
                    package_name
                );
            } else {
                println!("Packages that depend on '{}':", package_name);
                for dep in dependents {
                    // Get the trove name for the dependent
                    if let Ok(Some(trove)) =
                        conary::db::models::Trove::find_by_id(&conn, dep.trove_id)
                    {
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
        Some(Commands::Whatbreaks {
            package_name,
            db_path,
        }) => {
            info!(
                "Checking what would break if '{}' is removed...",
                package_name
            );

            let conn = conary::db::open(&db_path)?;

            // Check if package exists
            let troves = conary::db::models::Trove::find_by_name(&conn, &package_name)?;
            let _trove = troves
                .first()
                .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;

            // Build dependency graph and find breaking packages
            let resolver = conary::resolver::Resolver::new(&conn)?;
            let breaking = resolver.check_removal(&package_name)?;

            if breaking.is_empty() {
                println!(
                    "Package '{}' can be safely removed (no dependencies)",
                    package_name
                );
            } else {
                println!(
                    "Removing '{}' would break the following packages:",
                    package_name
                );
                for pkg in &breaking {
                    println!("  {}", pkg);
                }
                println!("\nTotal: {} packages would be affected", breaking.len());
            }

            Ok(())
        }
        Some(Commands::Completions { shell }) => {
            info!("Generating shell completions for {:?}", shell);

            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "conary", &mut io::stdout());

            Ok(())
        }
        Some(Commands::RepoAdd {
            name,
            url,
            db_path,
            priority,
            disabled,
        }) => {
            info!("Adding repository: {} ({})", name, url);

            let conn = conary::db::open(&db_path)?;
            let repo = conary::repository::add_repository(&conn, name.clone(), url.clone(), !disabled, priority)?;

            println!("Added repository: {}", repo.name);
            println!("  URL: {}", repo.url);
            println!("  Enabled: {}", repo.enabled);
            println!("  Priority: {}", repo.priority);

            Ok(())
        }
        Some(Commands::RepoList { db_path, all }) => {
            info!("Listing repositories");

            let conn = conary::db::open(&db_path)?;
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
                    let enabled_mark = if repo.enabled { "✓" } else { "✗" };
                    let sync_status = match &repo.last_sync {
                        Some(ts) => format!("synced {}", ts),
                        None => "never synced".to_string(),
                    };
                    println!("  {} {} (priority: {}, {})", enabled_mark, repo.name, repo.priority, sync_status);
                    println!("      {}", repo.url);
                }
            }

            Ok(())
        }
        Some(Commands::RepoRemove { name, db_path }) => {
            info!("Removing repository: {}", name);

            let conn = conary::db::open(&db_path)?;
            conary::repository::remove_repository(&conn, &name)?;

            println!("Removed repository: {}", name);

            Ok(())
        }
        Some(Commands::RepoEnable { name, db_path }) => {
            info!("Enabling repository: {}", name);

            let conn = conary::db::open(&db_path)?;
            conary::repository::set_repository_enabled(&conn, &name, true)?;

            println!("Enabled repository: {}", name);

            Ok(())
        }
        Some(Commands::RepoDisable { name, db_path }) => {
            info!("Disabling repository: {}", name);

            let conn = conary::db::open(&db_path)?;
            conary::repository::set_repository_enabled(&conn, &name, false)?;

            println!("Disabled repository: {}", name);

            Ok(())
        }
        Some(Commands::RepoSync {
            name,
            db_path,
            force,
        }) => {
            info!("Synchronizing repository metadata");

            let conn = conary::db::open(&db_path)?;

            let repos_to_sync = if let Some(repo_name) = name {
                // Sync specific repository
                let repo = conary::db::models::Repository::find_by_name(&conn, &repo_name)?
                    .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repo_name))?;
                vec![repo]
            } else {
                // Sync all enabled repositories
                conary::db::models::Repository::list_enabled(&conn)?
            };

            if repos_to_sync.is_empty() {
                println!("No repositories to sync");
                return Ok(());
            }

            for mut repo in repos_to_sync {
                if !force && !conary::repository::needs_sync(&repo) {
                    println!("Repository '{}' is up to date, skipping", repo.name);
                    continue;
                }

                println!("Syncing repository: {} ...", repo.name);
                match conary::repository::sync_repository(&conn, &mut repo) {
                    Ok(count) => {
                        println!("  ✓ Synchronized {} packages", count);
                    }
                    Err(e) => {
                        println!("  ✗ Failed to sync: {}", e);
                    }
                }
            }

            Ok(())
        }
        Some(Commands::Search { pattern, db_path }) => {
            info!("Searching for packages matching: {}", pattern);

            let conn = conary::db::open(&db_path)?;
            let packages = conary::repository::search_packages(&conn, &pattern)?;

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
        Some(Commands::Update {
            package,
            db_path,
            root,
        }) => {
            info!("Checking for package updates");

            let mut conn = conary::db::open(&db_path)?;

            // Initialize paths
            let objects_dir = Path::new(&db_path)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("objects");
            let temp_dir = Path::new(&db_path)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("tmp");
            std::fs::create_dir_all(&temp_dir)?;
            let _install_root = std::path::PathBuf::from(&root);

            // Get installed packages to check for updates
            let installed_troves = if let Some(pkg_name) = package {
                conary::db::models::Trove::find_by_name(&conn, &pkg_name)?
            } else {
                // Get all installed packages
                let mut stmt = conn.prepare("SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id FROM troves ORDER BY name")?;
                let rows = stmt.query_map([], |row| {
                    Ok(conary::db::models::Trove {
                        id: Some(row.get(0)?),
                        name: row.get(1)?,
                        version: row.get(2)?,
                        trove_type: row.get::<_, String>(3)?.parse().unwrap(),
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

            // Find available updates
            let mut updates_available = Vec::new();
            for trove in &installed_troves {
                // Search for newer versions in repositories
                let repo_packages = conary::db::models::RepositoryPackage::find_by_name(&conn, &trove.name)?;

                for repo_pkg in repo_packages {
                    // Simple version comparison (exact match for now)
                    if repo_pkg.version != trove.version {
                        // Check if architecture matches
                        if repo_pkg.architecture == trove.architecture || repo_pkg.architecture.is_none() {
                            info!("Update available: {} {} -> {}", trove.name, trove.version, repo_pkg.version);
                            updates_available.push((trove.clone(), repo_pkg));
                            break; // Take first available update
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

            // Initialize delta tracking
            let mut total_bytes_saved = 0i64;
            let mut deltas_applied = 0i32;
            let mut full_downloads = 0i32;
            let mut delta_failures = 0i32;

            // Create changeset for the update operation
            let changeset_id = conary::db::transaction(&mut conn, |tx| {
                let mut changeset = conary::db::models::Changeset::new(
                    format!("Update {} package(s)", updates_available.len())
                );
                changeset.insert(tx)
            })?;

            // Process each update
            for (installed_trove, repo_pkg) in updates_available {
                println!("\nUpdating {} ...", installed_trove.name);

                // Try delta update first
                let mut delta_success = false;

                // Check if delta is available from current to new version
                if let Ok(Some(delta_info)) = PackageDelta::find_delta(
                    &conn,
                    &installed_trove.name,
                    &installed_trove.version,
                    &repo_pkg.version,
                ) {
                    println!(
                        "  Delta available: {} bytes ({:.1}% of full size)",
                        delta_info.delta_size,
                        delta_info.compression_ratio * 100.0
                    );

                    // Try to download and apply delta
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
                            // Apply delta
                            let applier = DeltaApplier::new(&objects_dir)?;

                            match applier.apply_delta(
                                &delta_info.from_hash,
                                &delta_path,
                                &delta_info.to_hash,
                            ) {
                                Ok(_) => {
                                    println!("  ✓ Delta applied successfully");
                                    delta_success = true;
                                    deltas_applied += 1;

                                    // Calculate bandwidth saved
                                    let saved = repo_pkg.size - delta_info.delta_size;
                                    total_bytes_saved += saved;
                                }
                                Err(e) => {
                                    warn!("  Delta application failed: {}", e);
                                    delta_failures += 1;
                                }
                            }

                            // Clean up delta file
                            let _ = std::fs::remove_file(delta_path);
                        }
                        Err(e) => {
                            warn!("  Delta download failed: {}", e);
                            delta_failures += 1;
                        }
                    }
                }

                // Fall back to full download if delta didn't work
                if !delta_success {
                    println!("  Downloading full package...");

                    match repository::download_package(&repo_pkg, &temp_dir) {
                        Ok(pkg_path) => {
                            println!("  ✓ Downloaded {} bytes", repo_pkg.size);
                            full_downloads += 1;

                            // Parse and install the downloaded package
                            if let Err(e) = install_package_from_file(&pkg_path, &mut conn, &root, Some(&installed_trove)) {
                                warn!("  Package installation failed: {}", e);
                                let _ = std::fs::remove_file(pkg_path);
                                continue;
                            }

                            println!("  ✓ Package installed successfully");
                            // Clean up downloaded file
                            let _ = std::fs::remove_file(pkg_path);
                        }
                        Err(e) => {
                            warn!("  Full download failed: {}", e);
                            continue;
                        }
                    }
                }
            }

            // Store delta statistics
            conary::db::transaction(&mut conn, |tx| {
                let mut stats = DeltaStats::new(changeset_id);
                stats.total_bytes_saved = total_bytes_saved;
                stats.deltas_applied = deltas_applied;
                stats.full_downloads = full_downloads;
                stats.delta_failures = delta_failures;
                stats.insert(tx)?;

                // Mark changeset as applied
                let mut changeset = conary::db::models::Changeset::find_by_id(tx, changeset_id)?
                    .ok_or_else(|| conary::Error::NotFoundError("Changeset not found".to_string()))?;
                changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

                Ok(())
            })?;

            // Print summary
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
        Some(Commands::DeltaStats { db_path }) => {
            info!("Showing delta update statistics");

            let conn = conary::db::open(&db_path)?;

            // Get total statistics across all changesets
            let total_stats = DeltaStats::get_total_stats(&conn)?;

            // Get all individual delta stats records
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

            // Print overall statistics
            println!("=== Delta Update Statistics ===\n");
            println!("Total Statistics:");
            println!("  Delta updates applied: {}", total_stats.deltas_applied);
            println!("  Full downloads: {}", total_stats.full_downloads);
            println!("  Delta failures: {}", total_stats.delta_failures);

            let total_mb = total_stats.total_bytes_saved as f64 / 1_048_576.0;
            println!("  Total bandwidth saved: {:.2} MB", total_mb);

            // Calculate success rate
            let total_updates = total_stats.deltas_applied + total_stats.full_downloads;
            if total_updates > 0 {
                let success_rate = (total_stats.deltas_applied as f64 / total_updates as f64) * 100.0;
                println!("  Delta success rate: {:.1}%", success_rate);
            }

            // Print recent delta operations
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
        None => {
            // No command provided, show help
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_from_rpm_extension() {
        // Create a temporary file with .rpm extension
        let temp_file = tempfile::NamedTempFile::with_suffix(".rpm").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write RPM magic bytes
        std::fs::write(path, [0xED, 0xAB, 0xEE, 0xDB, 0, 0, 0, 0]).unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Rpm);
    }

    #[test]
    fn test_detect_format_from_deb_extension() {
        let temp_file = tempfile::NamedTempFile::with_suffix(".deb").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write DEB magic bytes
        std::fs::write(path, b"!<arch>\n").unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Deb);
    }

    #[test]
    fn test_detect_format_from_arch_extension() {
        let temp_file = tempfile::NamedTempFile::with_suffix(".pkg.tar.zst").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write zstd magic bytes
        std::fs::write(path, [0x28, 0xB5, 0x2F, 0xFD, 0, 0, 0, 0]).unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Arch);
    }

    #[test]
    fn test_detect_format_from_rpm_magic_bytes() {
        // Test fallback to magic bytes when extension is not recognized
        let temp_file = tempfile::NamedTempFile::with_suffix(".unknown").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write RPM magic bytes
        std::fs::write(path, [0xED, 0xAB, 0xEE, 0xDB, 0, 0, 0, 0]).unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Rpm);
    }

    #[test]
    fn test_detect_format_from_deb_magic_bytes() {
        let temp_file = tempfile::NamedTempFile::with_suffix(".unknown").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write DEB magic bytes (ar archive)
        std::fs::write(path, b"!<arch>\n").unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Deb);
    }

    #[test]
    fn test_detect_format_unknown() {
        let temp_file = tempfile::NamedTempFile::with_suffix(".unknown").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write random bytes that don't match any format
        std::fs::write(path, [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]).unwrap();

        let result = detect_package_format(path);
        assert!(result.is_err());
    }

    #[test]
    fn test_package_format_type_equality() {
        assert_eq!(PackageFormatType::Rpm, PackageFormatType::Rpm);
        assert_eq!(PackageFormatType::Deb, PackageFormatType::Deb);
        assert_eq!(PackageFormatType::Arch, PackageFormatType::Arch);
        assert_ne!(PackageFormatType::Rpm, PackageFormatType::Deb);
    }
}
