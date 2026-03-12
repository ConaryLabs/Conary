// src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting legacy packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::PackageFormatType;
use super::batch::{BatchInstaller, prepare_package_for_batch};
use super::dep_mode::DepMode;
use super::dep_resolution;
use super::resolve::check_provides_dependencies;
use anyhow::{Context, Result};
use conary_core::capability::inference::InferenceOptions;
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{ConversionOptions, FidelityLevel, LegacyConverter};
use conary_core::db::models::generate_capability_variations;
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::common::PackageMetadata;
use conary_core::repository;
use conary_core::resolver::MissingDependency;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use sha2::{Digest, Sha256};
use std::path::Path;
use tempfile::TempDir;
use tracing::{info, warn};

fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    let provided: std::collections::HashSet<String> = std::iter::once(ccs_pkg.name().to_string())
        .chain(ccs_pkg.manifest().provides.capabilities.iter().cloned())
        .chain(ccs_pkg.manifest().provides.sonames.iter().cloned())
        .chain(ccs_pkg.manifest().provides.binaries.iter().cloned())
        .chain(ccs_pkg.manifest().provides.pkgconfig.iter().cloned())
        .collect();

    if provided.contains(dep_name) {
        return true;
    }

    for variation in generate_capability_variations(dep_name) {
        if provided.contains(&variation) {
            return true;
        }
    }

    false
}

/// Result of attempting CCS conversion
pub enum ConversionResult {
    /// Package was converted, install via CCS path
    Converted { ccs_path: String, temp_dir: TempDir },
    /// Conversion skipped (already converted or not needed)
    Skipped,
}

pub struct ConvertedCcsInstallOptions<'a> {
    pub ccs_path: &'a str,
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub sandbox_mode: SandboxMode,
    pub no_deps: bool,
    pub no_scripts: bool,
    pub allow_downgrade: bool,
    pub dep_mode: DepMode,
    pub yes: bool,
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
    capture_scriptlets: bool,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let package_bytes = std::fs::read(package_path).with_context(|| {
        format!(
            "Failed to read package file for checksum: {}",
            package_path.display()
        )
    })?;
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
    let conn = conary_core::db::open(db_path).context("Failed to open package database")?;

    // Check if already converted (skip re-conversion)
    if let Some(existing) =
        conary_core::db::models::ConvertedPackage::find_by_checksum(&conn, &original_checksum)?
    {
        if existing.needs_reconversion() {
            info!("Re-converting {} (algorithm upgraded)", pkg.name());
            conary_core::db::models::ConvertedPackage::delete_by_checksum(
                &conn,
                &original_checksum,
            )?;
        } else {
            // Already converted and up to date
            info!(
                "Package {} already converted, using regular install path",
                pkg.name()
            );
            println!(
                "Note: {} was previously converted - using standard install",
                pkg.name()
            );
            return Ok(ConversionResult::Skipped);
        }
    }

    // Extract files for conversion
    let extracted = pkg
        .extract_file_contents()
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
    let ccs_temp = TempDir::new().context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        enable_chunking: true,
        output_dir: ccs_temp.path().to_path_buf(),
        auto_classify: true,
        min_fidelity: FidelityLevel::Partial,
        capture_scriptlets,
        enable_inference: true,
        inference_options: InferenceOptions::fast(),
    };

    let converter = LegacyConverter::new(options);
    let conversion_result = converter
        .convert(&metadata, &extracted, format_str, &original_checksum)
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
    let ccs_package_path = conversion_result
        .package_path
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

    // Serialize inferred capabilities to JSON for audit trail
    let inferred_caps_json = conversion_result
        .inferred_capabilities
        .as_ref()
        .and_then(|caps| serde_json::to_string(caps).ok());

    // Serialize extracted provenance to JSON for audit trail
    let provenance_json = conversion_result
        .legacy_provenance
        .as_ref()
        .and_then(|prov| prov.to_json().ok());

    if let Some(ref prov) = conversion_result.legacy_provenance
        && prov.has_content()
    {
        info!("Provenance extracted: {}", prov.summary());
    }

    // Create conversion record
    let mut converted_pkg = conary_core::db::models::ConvertedPackage::new(
        conversion_result.original_format.clone(),
        conversion_result.original_checksum.clone(),
        conversion_result.fidelity.level.to_string(),
    );
    converted_pkg.detected_hooks = Some(hooks_json);
    converted_pkg.inferred_caps_json = inferred_caps_json;
    converted_pkg.extracted_provenance_json = provenance_json;
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
pub fn install_converted_ccs(opts: ConvertedCcsInstallOptions<'_>) -> Result<()> {
    let ConvertedCcsInstallOptions {
        ccs_path,
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        no_scripts,
        allow_downgrade,
        dep_mode,
        yes,
    } = opts;

    if !no_deps {
        let conn = conary_core::db::open(db_path).context("Failed to open package database")?;
        let ccs_pkg =
            CcsPackage::parse(ccs_path).context("Failed to parse converted CCS package")?;
        let missing: Vec<MissingDependency> = ccs_pkg
            .dependencies()
            .iter()
            .filter(|dep| !package_self_provides(&ccs_pkg, &dep.name))
            .filter(|dep| !dep.name.starts_with("rpmlib(") && !dep.name.starts_with('/'))
            .map(|dep| MissingDependency {
                name: dep.name.clone(),
                constraint: dep
                    .version
                    .as_ref()
                    .and_then(|v| VersionConstraint::parse(v).ok())
                    .unwrap_or(VersionConstraint::Any),
                required_by: vec![ccs_pkg.name().to_string()],
            })
            .collect();

        if !missing.is_empty() {
            let (tracked_satisfied, unresolved_missing) =
                check_provides_dependencies(&conn, &missing);

            for (dep_name, provider, _version) in &tracked_satisfied {
                info!(
                    "Dependency {} already satisfied by tracked provider {}",
                    dep_name, provider
                );
            }

            let dep_plan =
                dep_resolution::resolve_missing_deps(&conn, &unresolved_missing, dep_mode);

            if !dep_plan.to_adopt.is_empty() && !dry_run {
                crate::commands::adopt::cmd_adopt(&dep_plan.to_adopt, db_path, false)?;
            }

            if !dep_plan.to_install.is_empty() {
                let dep_names: Vec<String> =
                    dep_plan.to_install.iter().map(|d| d.name.clone()).collect();

                if dry_run {
                    repository::resolve_dependencies_transitive(&conn, &dep_names, 10)
                        .with_context(|| {
                            format!(
                                "Failed to resolve dependencies from repositories for '{}'",
                                ccs_pkg.name()
                            )
                        })?;
                } else {
                    if !yes {
                        println!();
                        print!(
                            "Proceed with {} dependency changes? [Y/n] ",
                            dep_plan.to_install.len() + dep_plan.to_adopt.len()
                        );
                        use std::io::Write;
                        std::io::stdout().flush()?;

                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        let input = input.trim().to_lowercase();
                        if input == "n" || input == "no" {
                            println!("Cancelled.");
                            return Ok(());
                        }
                    }

                    let to_download =
                        repository::resolve_dependencies_transitive(&conn, &dep_names, 10)?;
                    if !to_download.is_empty() {
                        let temp_dir = TempDir::new()?;
                        let keyring_dir = keyring_dir(db_path);
                        let downloaded = repository::download_dependencies(
                            &to_download,
                            temp_dir.path(),
                            Some(&keyring_dir),
                        )?;
                        let parent_name = ccs_pkg.name().to_string();
                        let mut prepared_packages = Vec::with_capacity(downloaded.len());

                        for (dep_name, dep_path) in &downloaded {
                            if dep_path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("ccs"))
                            {
                                let dep_ccs_path = dep_path.to_str().ok_or_else(|| {
                                    anyhow::anyhow!("Invalid CCS path (non-UTF8)")
                                })?;
                                install_converted_ccs(ConvertedCcsInstallOptions {
                                    ccs_path: dep_ccs_path,
                                    db_path,
                                    root,
                                    dry_run,
                                    sandbox_mode,
                                    no_deps,
                                    no_scripts,
                                    allow_downgrade,
                                    dep_mode,
                                    yes,
                                })
                                .with_context(|| {
                                    format!("Failed to install CCS dependency {}", dep_name)
                                })?;
                                continue;
                            }

                            let reason = format!("Required by {}", parent_name);
                            match prepare_package_for_batch(
                                dep_path,
                                db_path,
                                &reason,
                                allow_downgrade,
                            ) {
                                Ok(prepared) => prepared_packages.push(prepared),
                                Err(e) if e.to_string().contains("already installed") => {
                                    info!("Dependency {} already installed, skipping", dep_name);
                                }
                                Err(e) => {
                                    return Err(anyhow::anyhow!(
                                        "Failed to prepare dependency {}: {}",
                                        dep_name,
                                        e
                                    ));
                                }
                            }
                        }

                        if !prepared_packages.is_empty() {
                            let installer =
                                BatchInstaller::new(db_path, root, sandbox_mode, no_scripts);
                            installer.install_batch(prepared_packages)?;
                        }
                    }
                }
            }

            if !dep_plan.unresolvable.is_empty() {
                let (_satisfied, still_missing) =
                    check_provides_dependencies(&conn, &dep_plan.unresolvable);
                if !still_missing.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Cannot install {}: {} unresolvable dependencies",
                        ccs_pkg.name(),
                        still_missing.len()
                    ));
                }
            }
        }
    }

    println!("Installing converted CCS package...");
    super::super::ccs::cmd_ccs_install(
        ccs_path,
        db_path,
        root,
        dry_run,
        true, // allow_unsigned - converted packages aren't signed yet
        None, // policy
        None, // components - install all
        sandbox_mode,
        no_deps,
    )
}
