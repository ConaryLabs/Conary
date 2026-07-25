// src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting native packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::super::open_db;
use super::PackageFormatType;
use super::batch::{BatchInstaller, prepare_package_for_batch};
use super::dep_resolution;
use super::dependencies::missing_repository_deps_from_sat_result;
use super::{
    CcsTransactionInstallOptions, RepositoryInstallProvenance,
    repository_install_provenance_from_package, verify_ccs_package_authority,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::{
    ConversionOptions, NativePackageConverter, ScriptletBundleSummary,
};
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::common::PackageMetadata;
use conary_core::repository;
use conary_core::repository::versioning::VersionScheme;
use conary_core::resolver::MissingDependency;
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use std::collections::HashMap;
use std::path::Path;
use tempfile::TempDir;
use tracing::info;

/// Bounded dependency expansion for live CCS installs.
///
/// Kernel installs need to reach the initramfs toolchain without allowing a
/// full unbounded distro dependency takeover:
/// kernel -> kernel-core -> dracut -> cpio.
pub const DEFAULT_CCS_DEPENDENCY_PASSES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCcsProvider {
    version_scheme: VersionScheme,
    provides: Vec<PendingProvide>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingProvide {
    name: String,
    version: Option<String>,
}

impl PendingCcsProvider {
    fn from_package(ccs_pkg: &CcsPackage) -> Self {
        let mut provides = vec![PendingProvide {
            name: ccs_pkg.name().to_string(),
            version: Some(ccs_pkg.version().to_string()),
        }];
        provides.extend(ccs_pkg.provides().iter().map(|provide| PendingProvide {
            name: provide.name.clone(),
            version: provide.version.clone(),
        }));
        provides.sort();
        provides.dedup();

        Self {
            version_scheme: ccs_pkg.manifest().package.version_scheme,
            provides,
        }
    }
}

fn pending_provider_directly_satisfies(
    provider: &PendingCcsProvider,
    dep: &MissingDependency,
) -> Result<bool> {
    for provided in &provider.provides {
        if provided.name != dep.name {
            continue;
        }
        let matched = match provided.version.as_deref() {
            Some(version) => dep_resolution::version_satisfies_constraint(
                provider.version_scheme,
                version,
                &dep.constraint,
            )?,
            None => matches!(dep.constraint, VersionConstraint::Any),
        };
        if matched {
            return Ok(true);
        }
    }
    Ok(false)
}

fn pending_provider_satisfies_dependency(
    pending_providers: &[PendingCcsProvider],
    dep: &MissingDependency,
) -> Result<bool> {
    for provider in pending_providers {
        if pending_provider_directly_satisfies(provider, dep)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn build_dependency_requests(
    to_install: &[dep_resolution::ResolvedDep],
) -> Vec<(String, VersionConstraint)> {
    to_install
        .iter()
        .map(|dependency| (dependency.name.clone(), dependency.constraint.clone()))
        .collect()
}

/// Result of attempting CCS conversion
pub enum ConversionResult {
    /// Package was converted, install via CCS path
    Converted {
        ccs_path: String,
        temp_dir: TempDir,
        pending_record: PendingInstalledConversion,
    },
    /// Conversion skipped (already converted or not needed)
    Skipped,
}

/// Conversion evidence that becomes installed state only after CCS commit.
pub struct PendingInstalledConversion {
    original_format: String,
    original_checksum: String,
    extracted_provenance_json: Option<String>,
    scriptlet_summary: ScriptletBundleSummary,
}

impl PendingInstalledConversion {
    pub fn persist(self, db_path: &str, trove_id: i64) -> Result<()> {
        let conn = open_db(db_path)?;
        let mut converted = conary_core::db::models::ConvertedPackage::new_installed(
            trove_id,
            self.original_format,
            self.original_checksum,
        );
        converted.set_scriptlet_metadata(&self.scriptlet_summary)?;
        converted.extracted_provenance_json = self.extracted_provenance_json;
        converted.insert(&conn)?;
        Ok(())
    }
}

pub struct ConvertedCcsInstallOptions<'a> {
    pub ccs_path: &'a str,
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub sandbox_mode: SandboxMode,
    pub no_deps: bool,
    pub allow_downgrade: bool,
    pub yes: bool,
    pub dependency_passes_remaining: usize,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
}

/// Attempt to convert a native package to CCS format
///
/// Returns `ConversionResult::Converted` if conversion succeeded and installation
/// should proceed via the CCS installer, or `ConversionResult::Skipped` if
/// conversion was skipped (e.g., already converted).
pub async fn try_convert_to_ccs(
    pkg: &dyn PackageFormat,
    package_path: &Path,
    format: PackageFormatType,
    db_path: &str,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let package_bytes = std::fs::read(package_path).with_context(|| {
        format!(
            "Failed to read package file for checksum: {}",
            package_path.display()
        )
    })?;
    let original_checksum = conary_core::hash::sha256_prefixed(&package_bytes);

    // Determine format string
    let format_str = match format {
        PackageFormatType::Rpm => "rpm",
        PackageFormatType::Deb => "deb",
        PackageFormatType::Arch => "arch",
    };

    // Open database early to check for existing conversion
    let conn = open_db(db_path)?;

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
        version_scheme: pkg.version_scheme(),
        architecture: pkg.architecture().map(|s| s.to_string()),
        description: pkg.description().map(|s| s.to_string()),
        files: pkg.files().to_vec(),
        requirements: pkg.requirements().to_vec(),
        provides: pkg.provides().to_vec(),
        relations: pkg.relations().to_vec(),
        diagnostic_scriptlet_evidence: Vec::new(),
        native_scriptlet_abi: pkg.native_scriptlet_abi().to_vec(),
        config_files: Vec::new(),
    };

    // Create temp directory for CCS output
    let ccs_temp = TempDir::new().context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        enable_chunking: true,
        output_dir: ccs_temp.path().to_path_buf(),
    };

    let conversion_key = std::sync::Arc::new(crate::commands::ccs::load_or_create_local_dev_key()?);
    let converter = NativePackageConverter::new(options).with_signing_key(conversion_key);
    let conversion_result = converter
        .convert(&metadata, &extracted, format_str, &original_checksum)
        .with_context(|| format!("Failed to convert {} to CCS format", pkg.name()))?;

    // Get the package path
    let ccs_package_path = conversion_result
        .package_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Conversion succeeded but no package path returned"))?;
    let converted_ccs_path = ccs_package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Converted CCS path is not valid UTF-8"))?;
    let verification = conary_core::ccs::verify::verify_package(
        ccs_package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![conversion_result.signing_public_key.clone()]),
    )
    .context("Failed to verify converted CCS package authority")?;
    let converted_ccs_pkg = CcsPackage::from_verified_archive(converted_ccs_path, &verification)
        .context("Failed to construct converted CCS package for capability policy")?;
    crate::commands::ccs::validate_ccs_capability_declaration(&converted_ccs_pkg)?;

    info!(
        "Converted {} to CCS format: {} (scriptlet_fidelity: {})",
        pkg.name(),
        ccs_package_path.display(),
        conversion_result.scriptlet_metadata.scriptlet_fidelity
    );

    // Serialize extracted provenance to JSON for audit trail
    let provenance_json = conversion_result
        .native_provenance
        .as_ref()
        .map(|provenance| provenance.to_json())
        .transpose()
        .context("Failed to serialize native package provenance")?;

    if let Some(ref prov) = conversion_result.native_provenance
        && prov.has_content()
    {
        info!("Provenance extracted: {}", prov.summary());
    }

    let pending_record = PendingInstalledConversion {
        original_format: conversion_result.original_format.clone(),
        original_checksum: conversion_result.original_checksum.clone(),
        extracted_provenance_json: provenance_json,
        scriptlet_summary: conversion_result.scriptlet_metadata.clone(),
    };

    let ccs_path = ccs_package_path.to_string_lossy().to_string();
    Ok(ConversionResult::Converted {
        ccs_path,
        temp_dir: ccs_temp,
        pending_record,
    })
}

/// Install a converted CCS package
///
/// This is a wrapper that calls the CCS installer with appropriate options.
pub async fn install_converted_ccs(opts: ConvertedCcsInstallOptions<'_>) -> Result<Option<i64>> {
    install_converted_ccs_with_pending(opts, Vec::new(), false).await
}

async fn install_converted_ccs_with_pending(
    opts: ConvertedCcsInstallOptions<'_>,
    pending_providers: Vec<PendingCcsProvider>,
    defer_generation: bool,
) -> Result<Option<i64>> {
    let ConvertedCcsInstallOptions {
        ccs_path,
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        allow_downgrade,
        yes,
        dependency_passes_remaining,
        repository_provenance,
    } = opts;

    let verified =
        verify_ccs_package_authority(db_path, Path::new(ccs_path), repository_provenance.as_ref())?;

    let ccs_pkg = CcsPackage::from_verified_archive(ccs_path, &verified)
        .context("Failed to construct verified CCS package")?;
    crate::commands::ccs::validate_ccs_capability_declaration(&ccs_pkg)?;

    if !no_deps {
        let conn = open_db(db_path)?;
        let effective_policy = repository::load_effective_policy(
            &conn,
            repository::resolution_policy::RequestScope::Any,
        )?;
        let dependency_selection = repository::SelectionOptions {
            policy: Some(effective_policy.resolution),
            is_root: false,
            primary_flavor: effective_policy.primary_flavor,
            ..Default::default()
        };
        let mut scoped_pending_providers = pending_providers;
        scoped_pending_providers.push(PendingCcsProvider::from_package(&ccs_pkg));
        let sat_result = conary_core::resolver::solve_requirement_groups_with_policy(
            &conn,
            ccs_pkg.requirements(),
            ccs_pkg.version_scheme(),
            dependency_selection
                .policy
                .as_ref()
                .context("converted CCS dependency selection is missing its exact policy")?,
        )
        .with_context(|| {
            format!(
                "Failed to solve exact requirements for converted package '{}'",
                ccs_pkg.name()
            )
        })?;
        if let Some(conflict) = sat_result.conflict_message {
            anyhow::bail!(
                "Cannot install converted package '{}': {conflict}",
                ccs_pkg.name()
            );
        }
        let missing = missing_repository_deps_from_sat_result(&sat_result, ccs_pkg.name())?;

        if !missing.is_empty() {
            let mut pending_satisfied = Vec::new();
            let mut still_unresolved_missing = Vec::new();
            for dep in missing {
                if pending_provider_satisfies_dependency(&scoped_pending_providers, &dep)? {
                    pending_satisfied.push(dep);
                } else {
                    still_unresolved_missing.push(dep);
                }
            }
            for dep in &pending_satisfied {
                info!(
                    "Dependency {} already satisfied by pending CCS transaction provider",
                    dep.name
                );
            }
            let unresolved_missing = still_unresolved_missing;

            let dep_plan = dep_resolution::plan_repository_dependencies(
                &conn,
                &unresolved_missing,
                &dependency_selection,
            )?;

            if !dep_plan.to_install.is_empty() {
                let dep_requests = build_dependency_requests(&dep_plan.to_install);

                if dry_run {
                    repository::resolve_dependency_requests(
                        &conn,
                        &dep_requests,
                        &dependency_selection,
                    )
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
                            dep_plan.to_install.len()
                        );
                        use std::io::Write;
                        std::io::stdout().flush()?;

                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        let input = input.trim().to_lowercase();
                        if input == "n" || input == "no" {
                            println!("Cancelled.");
                            return Ok(None);
                        }
                    }

                    let to_download = repository::resolve_dependency_requests(
                        &conn,
                        &dep_requests,
                        &dependency_selection,
                    )?;
                    if !to_download.is_empty() {
                        let temp_dir = TempDir::new()?;
                        let keyring_dir = keyring_dir(db_path);
                        let mut provenance_by_dep = HashMap::new();
                        for (dep_name, pkg_with_repo) in &to_download {
                            provenance_by_dep.insert(
                                dep_name.clone(),
                                repository_install_provenance_from_package(
                                    &pkg_with_repo.package,
                                    &pkg_with_repo.repository,
                                )?,
                            );
                        }
                        let downloaded = repository::download_dependencies(
                            &to_download,
                            temp_dir.path(),
                            &keyring_dir,
                        )
                        .await?;
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
                                // Allow one nested dependency pass for
                                // downloaded CCS dependencies. Kernel meta
                                // packages need this for kernel-core -> dracut,
                                // but grandchildren remain non-recursive inside
                                // that child call so dependency expansion stays
                                // bounded.
                                let nested_dependency_passes =
                                    dependency_passes_remaining.saturating_sub(1);
                                let child_pending_providers = scoped_pending_providers.clone();
                                let install_result = Box::pin(install_converted_ccs_with_pending(
                                    ConvertedCcsInstallOptions {
                                        ccs_path: dep_ccs_path,
                                        db_path,
                                        root,
                                        dry_run,
                                        sandbox_mode,
                                        no_deps: dependency_passes_remaining == 0,
                                        allow_downgrade,
                                        yes,
                                        dependency_passes_remaining: nested_dependency_passes,
                                        repository_provenance: provenance_by_dep
                                            .get(dep_name)
                                            .cloned(),
                                    },
                                    child_pending_providers,
                                    true,
                                ))
                                .await;
                                install_result.with_context(|| {
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
                                Ok(Some(prepared)) => {
                                    let mut prepared = prepared;
                                    prepared.repository_provenance =
                                        provenance_by_dep.get(dep_name).cloned();
                                    prepared_packages.push(prepared);
                                }
                                Ok(None) => {
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
                            let installer = BatchInstaller::new(db_path, sandbox_mode);
                            installer.install_batch(prepared_packages)?;
                        }
                    }
                }
            }

            if !dep_plan.unresolvable.is_empty() {
                let mut detail_lines = Vec::new();
                for dep in &dep_plan.unresolvable {
                    detail_lines.push(format!(
                        "  {} {} (required by: {})",
                        dep.name,
                        dep.constraint,
                        dep.required_by.join(", "),
                    ));
                }
                return Err(anyhow::anyhow!(
                    "Cannot install {}: {} requirements have no installed or repository provider:\n{}",
                    ccs_pkg.name(),
                    dep_plan.unresolvable.len(),
                    detail_lines.join("\n"),
                ));
            }
        }
    }

    println!("Installing converted CCS package...");
    let mut conn = open_db(db_path)?;
    let result = super::install_ccs_package_transactionally(
        &mut conn,
        &ccs_pkg,
        CcsTransactionInstallOptions {
            db_path,
            root,
            dry_run,
            defer_generation,
            quiet: false,
            sandbox_mode,
            allow_downgrade,
            reinstall: false,
            selection_reason: None,
            selected_manifest_components: None,
            repository_provenance,
        },
    )?;
    Ok(result.trove_id)
}

#[cfg(test)]
mod tests;
