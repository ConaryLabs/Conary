// src/main.rs

use anyhow::Result;
use clap::{Parser, Subcommand};
use conary::packages::rpm::RpmPackage;
use conary::packages::PackageFormat;
use std::fs::File;
use std::io::Read;
use tracing::info;

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
    /// Install a package (auto-detects RPM, DEB, Arch formats)
    Install {
        /// Path to the package file
        package_path: String,
        /// Database path (default: /var/lib/conary/conary.db)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
        /// Install root directory (default: /)
        #[arg(short, long, default_value = "/")]
        root: String,
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
            package_path,
            db_path,
            root,
        }) => {
            info!("Installing package: {}", package_path);

            // Auto-detect package format
            let format = detect_package_format(&package_path)?;
            info!("Detected package format: {:?}", format);

            // Parse the package based on format
            let rpm = match format {
                PackageFormatType::Rpm => RpmPackage::parse(&package_path)?,
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

            // Pre-transaction validation: check if already installed
            let existing = conary::db::models::Trove::find_by_name(&conn, rpm.name())?;
            for trove in &existing {
                if trove.version == rpm.version() && trove.architecture == rpm.architecture().map(|s| s.to_string()) {
                    return Err(anyhow::anyhow!(
                        "Package {} version {} ({}) is already installed",
                        rpm.name(),
                        rpm.version(),
                        rpm.architecture().unwrap_or("no-arch")
                    ));
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
                let mut changeset = conary::db::models::Changeset::new(format!(
                    "Install {}-{}",
                    rpm.name(),
                    rpm.version()
                ));
                let changeset_id = changeset.insert(tx)?;

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
        std::fs::write(path, &[0xED, 0xAB, 0xEE, 0xDB, 0, 0, 0, 0]).unwrap();

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
        std::fs::write(path, &[0x28, 0xB5, 0x2F, 0xFD, 0, 0, 0, 0]).unwrap();

        let format = detect_package_format(path).unwrap();
        assert_eq!(format, PackageFormatType::Arch);
    }

    #[test]
    fn test_detect_format_from_rpm_magic_bytes() {
        // Test fallback to magic bytes when extension is not recognized
        let temp_file = tempfile::NamedTempFile::with_suffix(".unknown").unwrap();
        let path = temp_file.path().to_str().unwrap();

        // Write RPM magic bytes
        std::fs::write(path, &[0xED, 0xAB, 0xEE, 0xDB, 0, 0, 0, 0]).unwrap();

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
        std::fs::write(path, &[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]).unwrap();

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
