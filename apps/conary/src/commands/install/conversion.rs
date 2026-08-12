// apps/conary/src/commands/install/conversion.rs

//! CCS conversion during package installation
//!
//! Handles converting native packages (RPM, DEB, Arch) to CCS format
//! during installation when --convert-to-ccs is specified.

use super::super::open_db;
use super::PackageFormatType;
use super::batch::{BatchInstaller, prepare_ccs_package_for_batch};
use super::dep_resolution;
use super::dependencies::resolved_repository_deps_from_sat_result;
use super::repository_batch::{RepositoryBatchSelection, prepare_repository_batch};
use super::{
    CcsEnvelopeAuthority, CcsTransactionInstallOptions, InstallIntent, RepositoryInstallProvenance,
    verify_ccs_package_authority,
};
use anyhow::{Context, Result};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::ccs::convert::{
    ConversionOptions, NativePackageConverter, ScriptletBundleSummary,
};
use conary_core::packages::PackageFormat;
use conary_core::repository::versioning::VersionScheme;
use conary_core::resolver::{SatPackage, SatSource};
use conary_core::scriptlet::SandboxMode;
use std::path::Path;
use tempfile::TempDir;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCcsProvider {
    name: String,
    version: String,
    package_release: Option<String>,
    architecture: Option<String>,
    version_scheme: VersionScheme,
}

impl PendingCcsProvider {
    fn from_package(ccs_pkg: &CcsPackage) -> Self {
        Self {
            name: ccs_pkg.name().to_string(),
            version: ccs_pkg.version().to_string(),
            package_release: ccs_pkg.package_release().map(str::to_string),
            architecture: ccs_pkg.architecture().map(str::to_string),
            version_scheme: ccs_pkg.version_scheme(),
        }
    }

    fn matches(&self, selected: &SatPackage) -> bool {
        self.name == selected.name
            && self.version == selected.version
            && selected_ccs_release_matches(
                self.package_release.as_deref(),
                selected.package_release.as_deref(),
            )
            && self.architecture == selected.architecture
            && self.version_scheme == selected.version_scheme
    }
}

fn selected_ccs_release_matches(ccs_release: Option<&str>, selected_release: Option<&str>) -> bool {
    match selected_release {
        Some(selected_release) => ccs_release == Some(selected_release),
        None => true,
    }
}

pub(super) fn validate_selected_repository_ccs_identity(
    ccs_pkg: &CcsPackage,
    selected: &SatPackage,
    repository_provenance: Option<&RepositoryInstallProvenance>,
) -> Result<()> {
    if selected.source != SatSource::Repository {
        anyhow::bail!(
            "verified CCS dependency '{}' was not selected from a repository row",
            selected.name
        );
    }
    let selected_row_id = selected.repo_package_id.with_context(|| {
        format!(
            "SAT-selected CCS dependency '{}-{}' has no repository package row",
            selected.name, selected.version
        )
    })?;
    let selected_repository_id = selected.repository_id.with_context(|| {
        format!(
            "SAT-selected CCS dependency '{}-{}' has no repository identity",
            selected.name, selected.version
        )
    })?;
    let provenance = repository_provenance.with_context(|| {
        format!(
            "verified CCS dependency '{}-{}' has no repository provenance",
            selected.name, selected.version
        )
    })?;

    let mut mismatches = Vec::new();
    if provenance.repository_id != selected_repository_id {
        mismatches.push(format!(
            "repository {} != {}",
            provenance.repository_id, selected_repository_id
        ));
    }
    if ccs_pkg.name() != selected.name {
        mismatches.push(format!("name '{}' != '{}'", ccs_pkg.name(), selected.name));
    }
    if ccs_pkg.version() != selected.version {
        mismatches.push(format!(
            "version '{}' != '{}'",
            ccs_pkg.version(),
            selected.version
        ));
    }
    if ccs_pkg.architecture() != selected.architecture.as_deref() {
        mismatches.push(format!(
            "architecture {:?} != {:?}",
            ccs_pkg.architecture(),
            selected.architecture
        ));
    }
    if ccs_pkg.version_scheme() != selected.version_scheme {
        mismatches.push(format!(
            "version scheme '{}' != '{}'",
            ccs_pkg.version_scheme().as_str(),
            selected.version_scheme.as_str()
        ));
    }
    if provenance.version_scheme != selected.version_scheme {
        mismatches.push(format!(
            "provenance version scheme '{}' != '{}'",
            provenance.version_scheme.as_str(),
            selected.version_scheme.as_str()
        ));
    }
    // Remi repository rows carry the exact source-native EVR in `version`,
    // while the signed CCS `release` is the conversion build release. Static
    // CCS rows that explicitly publish a release must still match it.
    if !selected_ccs_release_matches(
        ccs_pkg.package_release(),
        selected.package_release.as_deref(),
    ) {
        mismatches.push(format!(
            "package release {:?} != {:?}",
            ccs_pkg.package_release(),
            selected.package_release
        ));
    }

    if !mismatches.is_empty() {
        anyhow::bail!(
            "verified CCS dependency identity does not match SAT-selected repository row {}: {}",
            selected_row_id,
            mismatches.join(", ")
        );
    }

    Ok(())
}

pub(super) fn validate_selected_repository_native_identity(
    artifact_name: &str,
    artifact_version: &str,
    artifact_package_release: Option<&str>,
    artifact_architecture: Option<&str>,
    artifact_version_scheme: VersionScheme,
    selected: &SatPackage,
    repository_provenance: &RepositoryInstallProvenance,
) -> Result<()> {
    if selected.source != SatSource::Repository {
        anyhow::bail!(
            "native dependency '{}' was not selected from a repository row",
            selected.name
        );
    }
    let selected_row_id = selected.repo_package_id.with_context(|| {
        format!(
            "SAT-selected native dependency '{}-{}' has no repository package row",
            selected.name, selected.version
        )
    })?;
    let selected_repository_id = selected.repository_id.with_context(|| {
        format!(
            "SAT-selected native dependency '{}-{}' has no repository identity",
            selected.name, selected.version
        )
    })?;

    let mut mismatches = Vec::new();
    if repository_provenance.repository_id != selected_repository_id {
        mismatches.push(format!(
            "repository {} != {}",
            repository_provenance.repository_id, selected_repository_id
        ));
    }
    if artifact_name != selected.name {
        mismatches.push(format!("name '{artifact_name}' != '{}'", selected.name));
    }
    if artifact_version != selected.version {
        mismatches.push(format!(
            "version '{artifact_version}' != '{}'",
            selected.version
        ));
    }
    if artifact_package_release != selected.package_release.as_deref() {
        mismatches.push(format!(
            "package release {artifact_package_release:?} != {:?}",
            selected.package_release
        ));
    }
    if artifact_architecture != selected.architecture.as_deref() {
        mismatches.push(format!(
            "architecture {artifact_architecture:?} != {:?}",
            selected.architecture
        ));
    }
    if artifact_version_scheme != selected.version_scheme {
        mismatches.push(format!(
            "version scheme '{}' != '{}'",
            artifact_version_scheme.as_str(),
            selected.version_scheme.as_str()
        ));
    }
    if repository_provenance.version_scheme != selected.version_scheme {
        mismatches.push(format!(
            "provenance version scheme '{}' != '{}'",
            repository_provenance.version_scheme.as_str(),
            selected.version_scheme.as_str()
        ));
    }

    if !mismatches.is_empty() {
        anyhow::bail!(
            "native dependency identity does not match SAT-selected repository row {}: {}",
            selected_row_id,
            mismatches.join(", ")
        );
    }
    Ok(())
}

/// Result of attempting CCS conversion
pub enum ConversionResult {
    /// Package was converted, install via CCS path
    Converted {
        ccs_path: String,
        temp_dir: TempDir,
        pending_record: PendingInstalledConversion,
        signing_public_key: String,
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
    pub(crate) fn original_checksum(&self) -> &str {
        &self.original_checksum
    }

    pub(crate) fn into_record(
        self,
        trove_id: i64,
    ) -> Result<conary_core::db::models::ConvertedPackage> {
        let mut converted = conary_core::db::models::ConvertedPackage::new_installed(
            trove_id,
            self.original_format,
            self.original_checksum,
        );
        converted.set_scriptlet_metadata(&self.scriptlet_summary)?;
        converted.extracted_provenance_json = self.extracted_provenance_json;
        Ok(converted)
    }

    pub fn persist(self, db_path: &str, trove_id: i64) -> Result<()> {
        let conn = open_db(db_path)?;
        let mut converted = self.into_record(trove_id)?;
        converted.insert(&conn)?;
        Ok(())
    }
}

/// Fully verified output of the canonical native-package conversion pipeline.
///
/// The temporary directory owns `ccs_path`. Callers must either consume the
/// artifact while this value is alive or copy it into same-directory staging
/// before publishing it durably.
pub(crate) struct NativeCcsConversion {
    pub(crate) ccs_path: std::path::PathBuf,
    pub(crate) temp_dir: TempDir,
    pub(crate) pending_record: PendingInstalledConversion,
    pub(crate) signing_public_key: String,
}

pub struct CcsArtifactInstallOptions<'a> {
    pub ccs_path: &'a str,
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub sandbox_mode: SandboxMode,
    pub no_deps: bool,
    pub allow_downgrade: bool,
    pub intent: InstallIntent,
    pub yes: bool,
    pub envelope_authority: CcsEnvelopeAuthority,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    /// Exact transaction source policy established by explicit scope,
    /// persisted pin, or selected root repository provenance.
    pub resolution_policy: conary_core::repository::resolution_policy::ResolutionPolicy,
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
    source_profile: Option<&str>,
) -> Result<ConversionResult> {
    info!("Converting {} to CCS format...", pkg.name());

    // Compute checksum of original package for deduplication
    let mut package_file = std::fs::File::open(package_path).with_context(|| {
        format!(
            "Failed to open package file for checksum: {}",
            package_path.display()
        )
    })?;
    let original_checksum =
        conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut package_file)
            .with_context(|| {
                format!(
                    "Failed to stream package checksum: {}",
                    package_path.display()
                )
            })?;
    let original_checksum_text = original_checksum.to_prefixed_string();

    // Open database early to check for existing conversion
    let conn = open_db(db_path)?;

    // Check if already converted (skip re-conversion)
    if let Some(existing) = conary_core::db::models::ConvertedPackage::find_installed_by_checksum(
        &conn,
        &original_checksum_text,
    )? {
        if existing.needs_reconversion() {
            info!("Re-converting {} (algorithm upgraded)", pkg.name());
            conary_core::db::models::ConvertedPackage::delete_installed_by_checksum(
                &conn,
                &original_checksum_text,
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

    let converted = convert_native_package_to_ccs_with_checksum(
        pkg,
        package_path,
        format,
        source_profile,
        &original_checksum,
    )?;
    let ccs_path = converted.ccs_path.to_string_lossy().to_string();
    Ok(ConversionResult::Converted {
        ccs_path,
        temp_dir: converted.temp_dir,
        pending_record: converted.pending_record,
        signing_public_key: converted.signing_public_key,
    })
}

/// Convert one exact native artifact through the same parser/lifecycle/CCS
/// pipeline used by direct installation.
///
/// This function performs no database mutation and returns only after the
/// signed CCS archive has passed strict verification and capability-policy
/// validation.
pub(crate) fn convert_native_package_to_ccs(
    pkg: &dyn PackageFormat,
    package_path: &Path,
    format: PackageFormatType,
    source_profile: Option<&str>,
) -> Result<NativeCcsConversion> {
    let mut package_file = std::fs::File::open(package_path).with_context(|| {
        format!(
            "Failed to open package file for checksum: {}",
            package_path.display()
        )
    })?;
    let original_checksum =
        conary_core::hash::hash_reader(conary_core::hash::HashAlgorithm::Sha256, &mut package_file)
            .with_context(|| {
                format!(
                    "Failed to stream package checksum: {}",
                    package_path.display()
                )
            })?;
    convert_native_package_to_ccs_with_checksum(
        pkg,
        package_path,
        format,
        source_profile,
        &original_checksum,
    )
}

fn convert_native_package_to_ccs_with_checksum(
    pkg: &dyn PackageFormat,
    package_path: &Path,
    format: PackageFormatType,
    source_profile: Option<&str>,
    original_checksum: &conary_core::hash::Hash,
) -> Result<NativeCcsConversion> {
    let format_str = match format {
        PackageFormatType::Rpm => "rpm",
        PackageFormatType::Deb => "deb",
        PackageFormatType::Arch => "arch",
    };
    // Extract files for conversion
    let extracted = pkg
        .package_payload()
        .map(conary_core::packages::payload::PackagePayload::into_files)
        .with_context(|| format!("Failed to extract files for conversion: {}", pkg.name()))?;

    let metadata = ForeignConversionInput::from_package(package_path.to_path_buf(), pkg)?;

    // Create temp directory for CCS output
    let ccs_temp = TempDir::new().context("Failed to create temp directory for CCS conversion")?;

    let options = ConversionOptions {
        output_dir: ccs_temp.path().to_path_buf(),
    };

    let conversion_key = std::sync::Arc::new(crate::commands::ccs::load_or_create_local_dev_key()?);
    let mut converter = NativePackageConverter::new(options).with_signing_key(conversion_key);
    if let Some(profile) = source_profile {
        converter = converter.with_source_profile(profile);
    }
    let conversion_result = converter
        .convert_payload(&metadata, &extracted, format_str, original_checksum)
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

    Ok(NativeCcsConversion {
        ccs_path: ccs_package_path.to_path_buf(),
        temp_dir: ccs_temp,
        pending_record,
        signing_public_key: conversion_result.signing_public_key.clone(),
    })
}

/// Install one verified CCS artifact.
///
/// The artifact may be Conary-native or converted from another package format.
pub async fn install_ccs_artifact(opts: CcsArtifactInstallOptions<'_>) -> Result<Option<i64>> {
    let CcsArtifactInstallOptions {
        ccs_path,
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        allow_downgrade,
        intent,
        yes,
        envelope_authority,
        repository_provenance,
        resolution_policy,
    } = opts;

    let verified = verify_ccs_package_authority(
        db_path,
        Path::new(ccs_path),
        &envelope_authority,
        repository_provenance.as_ref(),
    )?;

    let ccs_pkg = CcsPackage::from_verified_archive(ccs_path, &verified)
        .context("Failed to construct verified CCS package")?;
    crate::commands::ccs::validate_ccs_capability_declaration(&ccs_pkg)?;

    let mut selected_dependencies = Vec::new();
    if !no_deps && !ccs_pkg.requirements().is_empty() {
        let conn = open_db(db_path)?;
        let sat_result = conary_core::resolver::solve_requirement_groups_with_policy(
            &conn,
            ccs_pkg.requirements(),
            ccs_pkg.version_scheme(),
            &resolution_policy,
        )
        .with_context(|| {
            format!(
                "Failed to solve exact requirements for CCS package '{}'",
                ccs_pkg.name()
            )
        })?;
        if let Some(conflict) = sat_result.conflict_message {
            anyhow::bail!(
                "Cannot install CCS package '{}': {conflict}",
                ccs_pkg.name()
            );
        }
        let pending_root = PendingCcsProvider::from_package(&ccs_pkg);
        selected_dependencies =
            resolved_repository_deps_from_sat_result(&sat_result, ccs_pkg.name())
                .into_iter()
                .filter(|dependency| !pending_root.matches(&dependency.package))
                .collect();
    }

    if selected_dependencies.is_empty() || dry_run {
        if dry_run && !selected_dependencies.is_empty() {
            let conn = open_db(db_path)?;
            dep_resolution::exact_repository_downloads(&conn, &selected_dependencies)
                .with_context(|| {
                    format!(
                        "SAT-selected dependency identity drifted before dry-run for '{}'",
                        ccs_pkg.name()
                    )
                })?;
        }
        println!("Installing CCS package...");
        let mut conn = open_db(db_path)?;
        let result = super::install_ccs_package_transactionally(
            &mut conn,
            &ccs_pkg,
            CcsTransactionInstallOptions {
                db_path,
                root,
                dry_run,
                defer_generation: false,
                quiet: false,
                sandbox_mode,
                allow_downgrade,
                intent,
                reinstall: false,
                selection_reason: None,
                selected_manifest_components: None,
                repository_provenance,
            },
        )?;
        return Ok(result.trove_id);
    }

    if !yes {
        println!();
        print!(
            "Proceed with {} dependency changes? [Y/n] ",
            selected_dependencies.len()
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

    let selections = selected_dependencies
        .into_iter()
        .map(|selected| RepositoryBatchSelection {
            selected,
            install_reason: conary_core::db::models::InstallReason::Dependency,
            selection_reason: format!("Required by {}", ccs_pkg.name()),
            allow_downgrade,
            intent,
        })
        .collect();
    let mut prepared = prepare_repository_batch(db_path, selections).await?;
    prepared.push(prepare_ccs_package_for_batch(
        &ccs_pkg,
        db_path,
        conary_core::db::models::InstallReason::Explicit,
        "Explicit package request",
        allow_downgrade,
        intent,
        repository_provenance,
    )?);

    println!("Installing CCS package...");
    let result = prepared.install_with_result(BatchInstaller::new(db_path, sandbox_mode))?;
    Ok(Some(result.exact_trove_id(&ccs_pkg)?))
}

#[cfg(test)]
mod tests;
