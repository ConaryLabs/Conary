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

            // Perform installation within a changeset transaction
            conary::db::transaction(&mut conn, |tx| {
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

                // Store file metadata in database
                for file in rpm.files() {
                    let mut file_entry = conary::db::models::FileEntry::new(
                        file.path.clone(),
                        file.sha256.clone().unwrap_or_default(),
                        file.size,
                        file.mode,
                        trove_id,
                    );
                    file_entry.insert(tx)?;
                }

                // Mark changeset as applied
                changeset.update_status(tx, conary::db::models::ChangesetStatus::Applied)?;

                Ok(())
            })?;

            // TODO: Actually deploy files to filesystem (Phase 2 of this feature)

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
        }) => {
            info!("Rolling back changeset: {}", changeset_id);

            let mut conn = conary::db::open(&db_path)?;

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

            println!("Rollback complete. Changeset {} has been reversed.", changeset_id);

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
