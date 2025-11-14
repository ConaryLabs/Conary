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
