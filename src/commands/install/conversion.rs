// src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting legacy packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::PackageFormatType;
use anyhow::{Context, Result};
use conary::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary::packages::common::PackageMetadata;
use conary::packages::PackageFormat;
use conary::scriptlet::SandboxMode;
use sha2::{Digest, Sha256};
use std::path::Path;
use tempfile::TempDir;
use tracing::{info, warn};

/// Result of attempting CCS conversion
pub enum ConversionResult {
    /// Package was converted, install via CCS path
    Converted {
        ccs_path: String,
        temp_dir: TempDir,
    },
    /// Conversion skipped (already converted or not needed)
    Skipped,
}

/// Attempt to convert a legacy package to CCS format
///
/// Returns `ConversionResult::Converted` if conversion succeeded and installation
/// should proceed via the CCS installer, or `ConversionResult::Skipped` if
/// conversion was skipped (e.g., already converted).
pub fn try_convert_to_ccs(
    pkg: &dyn PackageFormat,
    package_path: &Path,
    format: PackageFormatType,
    db_path: &str,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let package_bytes = std::fs::read(package_path)
        .with_context(|| format!("Failed to read package file for checksum: {}", package_path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&package_bytes);
    let hash_result = hasher.finalize();
    let original_checksum = format!("sha256:{:x}", hash_result);

    // Determine format string
    let format_str = match format {
        PackageFormatType::Rpm => "rpm",
        PackageFormatType::Deb => "deb",
        PackageFormatType::Arch => "arch",
    };

    // Open database early to check for existing conversion
    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    // Check if already converted (skip re-conversion)
    if let Some(existing) = conary::db::models::ConvertedPackage::find_by_checksum(
        &conn,
        &original_checksum,
    )? {
        if existing.needs_reconversion() {
            info!("Re-converting {} (algorithm upgraded)", pkg.name());
            conary::db::models::ConvertedPackage::delete_by_checksum(
                &conn,
                &original_checksum,
            )?;
        } else {
            // Already converted and up to date
            info!("Package {} already converted, using regular install path", pkg.name());
            println!("Note: {} was previously converted - using standard install", pkg.name());
            return Ok(ConversionResult::Skipped);
        }
    }

    // Extract files for conversion
    let extracted = pkg.extract_file_contents()
        .with_context(|| format!("Failed to extract files for conversion: {}", pkg.name()))?;

    // Build PackageMetadata from the package
    let metadata = PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: pkg.name().to_string(),
        version: pkg.version().to_string(),
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        files: pkg.files().to_vec(),
        dependencies: pkg.dependencies().to_vec(),
        scriptlets: pkg.scriptlets().to_vec(),
        config_files: Vec::new(),
    };

    // Create temp directory for CCS output
    let ccs_temp = TempDir::new()
        .context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        enable_chunking: true,
        output_dir: ccs_temp.path().to_path_buf(),
        auto_classify: true,
        min_fidelity: FidelityLevel::Partial,
    };

    let converter = LegacyConverter::new(options);
    let conversion_result = converter.convert(&metadata, &extracted, format_str, &original_checksum)
        .with_context(|| format!("Failed to convert {} to CCS format", pkg.name()))?;

    // Warn if fidelity is below High
    if conversion_result.fidelity.level < FidelityLevel::High {
        warn!(
            "Conversion fidelity is {}: complex scripts may not be fully analyzed",
            conversion_result.fidelity.level
        );
        eprintln!(
            "WARNING: Conversion fidelity is {} - complex legacy scripts may not be fully analyzed",
            conversion_result.fidelity.level
        );
    }

    // Get the package path
    let ccs_package_path = conversion_result.package_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Conversion succeeded but no package path returned"))?;

    info!(
        "Converted {} to CCS format: {} (fidelity: {})",
        pkg.name(),
        ccs_package_path.display(),
        conversion_result.fidelity.level
    );

    // Serialize hooks to JSON for storage
    let hooks_json = serde_json::to_string(&conversion_result.detected_hooks)
        .unwrap_or_else(|_| "{}".to_string());

    // Create conversion record
    let mut converted_pkg = conary::db::models::ConvertedPackage::new(
        conversion_result.original_format.clone(),
        conversion_result.original_checksum.clone(),
        conversion_result.fidelity.level.to_string(),
    );
    converted_pkg.detected_hooks = Some(hooks_json);
    converted_pkg.insert(&conn)?;

    let ccs_path = ccs_package_path.to_string_lossy().to_string();
    Ok(ConversionResult::Converted {
        ccs_path,
        temp_dir: ccs_temp,
    })
}

/// Install a converted CCS package
///
/// This is a wrapper that calls the CCS installer with appropriate options.
pub fn install_converted_ccs(
    ccs_path: &str,
    db_path: &str,
    root: &str,
    dry_run: bool,
    sandbox_mode: SandboxMode,
    no_deps: bool,
) -> Result<()> {
    println!("Installing converted CCS package...");
    super::super::ccs::cmd_ccs_install(
        ccs_path,
        db_path,
        root,
        dry_run,
        true,  // allow_unsigned - converted packages aren't signed yet
        None,  // policy
        None,  // components - install all
        sandbox_mode,
        no_deps,
    )
}
