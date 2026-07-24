// conary-core/src/ccs/convert/converter.rs
//! Legacy package to CCS format converter
//!
//! Takes `PackageMetadata` + extracted files and builds a `CcsManifest` with
//! component classification, invoking `CcsBuilder` with optional CDC chunking.

mod authority;
mod evidence;

use authority::*;
pub use evidence::ConversionError;
use evidence::*;

use crate::ccs::attestation::{
    FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary, canonical_json_hash,
    compute_build_output_identity,
};
use crate::ccs::builder::{BuildResult, CcsBuilder, write_ccs_package};
use crate::ccs::convert::adapters::{AdapterInput, AdapterRegistry};
use crate::ccs::convert::command_evidence::{
    extract_native_entry_invocations, extract_scriptlet_invocations,
};
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport,
    classification_is_complete_adapter_coverage, native_deferred_support_can_be_adapter_covered,
};
use crate::ccs::convert::legacy_provenance::LegacyProvenance;
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::convert::scriptlet_bundle::{
    ScriptletBundleInput, ScriptletBundleSummary, build_legacy_scriptlet_bundle,
};
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::ccs::manifest::{
    Capability, CcsManifest, Components, Config, FileCapability, Hooks, Package, PackageDep,
    Platform, Provides, Redirects, Requires, Suggests, SysctlHook,
};
use crate::ccs::policy::BuildPolicyConfig;
use crate::dependencies::{DependencyClass, LanguageDepDetector};
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::{
    ArchNativeScriptletMetadata, DebControlMember, NativeLifecyclePath, NativeScriptletEntry,
    NativeScriptletMetadata, NativeScriptletSupport, RpmScriptletSlot,
};
use crate::packages::traits::{DependencyType, ExtractedFile, PackageFormat};
use crate::recipe::hermetic::{
    BuildCommandRiskEntry, BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
    BuilderEnvironmentKind, DependencyLock, DivergenceReport, EcosystemPolicyReport,
    HERMETIC_EVIDENCE_SCHEMA_V1, HermeticBuildEvidence, PolicyStatus, RecipeIdentity,
    ReproducibilityRecord, SourceIdentity,
};
use crate::security::command_risk::{
    COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskReport, CommandRiskStatus, classify_shell_text,
};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Options for legacy package conversion
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Enable CDC chunking for file content (better dedup, slower)
    pub enable_chunking: bool,
    /// Output directory for the converted package
    pub output_dir: PathBuf,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            enable_chunking: true,
            output_dir: PathBuf::from("./target/ccs"),
        }
    }
}

/// Result of converting a legacy package
#[derive(Debug)]
pub struct ConversionResult {
    /// The build result from CcsBuilder
    pub build_result: BuildResult,
    /// Path to the output CCS package (if written)
    pub package_path: Option<PathBuf>,
    /// Original package format
    pub original_format: String,
    /// Original package checksum (for dedup/skip)
    pub original_checksum: String,
    /// Provenance information extracted from the legacy package
    pub legacy_provenance: Option<LegacyProvenance>,
    /// Passive scriptlet classification evidence for later migration gates.
    pub scriptlet_classification: ScriptletClassificationReport,
    /// Passive legacy scriptlet bundle embedded into the generated manifest.
    pub legacy_scriptlets: Option<LegacyScriptletBundle>,
    /// Compact passive scriptlet metadata derived from the embedded bundle.
    pub scriptlet_metadata: ScriptletBundleSummary,
}

/// Converts legacy packages (RPM/DEB/Arch) to CCS format
pub struct LegacyConverter {
    options: ConversionOptions,
    source_distro: Option<String>,
    source_release: Option<String>,
    target_profile_id: Option<String>,
    conversion_tool: String,
}

impl LegacyConverter {
    /// Create a new converter with the given options
    pub fn new(options: ConversionOptions) -> Self {
        Self {
            options,
            source_distro: None,
            source_release: None,
            target_profile_id: None,
            conversion_tool: "conary".to_string(),
        }
    }

    /// Create a converter with default options
    pub fn with_defaults() -> Self {
        Self::new(ConversionOptions::default())
    }

    /// Attach source distro context for passive scriptlet bundle metadata.
    pub fn with_source_distro(mut self, distro: impl Into<String>) -> Self {
        self.source_distro = Some(distro.into());
        self
    }

    /// Attach source release context for passive scriptlet bundle metadata.
    pub fn with_source_release(mut self, release: impl Into<String>) -> Self {
        self.source_release = Some(release.into());
        self
    }

    /// Attach target profile context for passive scriptlet bundle policy review.
    pub fn with_target_profile_id(mut self, profile_id: impl Into<String>) -> Self {
        self.target_profile_id = Some(profile_id.into());
        self
    }

    /// Override the conversion tool name recorded in passive scriptlet bundles.
    pub fn with_conversion_tool(mut self, tool: impl Into<String>) -> Self {
        self.conversion_tool = tool.into();
        self
    }

    /// Convert a legacy package to CCS format
    ///
    /// # Arguments
    /// * `metadata` - Package metadata from the legacy parser
    /// * `files` - Extracted file contents
    /// * `format` - Original format ("rpm", "deb", "arch")
    /// * `checksum` - Original package file checksum
    ///
    /// # Returns
    /// A `ConversionResult` containing the CCS build result and typed lifecycle evidence
    pub fn convert(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> Result<ConversionResult, ConversionError> {
        let scriptlet_classification = classify_scriptlets(metadata, files);
        let final_metadata = metadata.clone();
        let mut final_files = files.to_vec();

        // Step 1: Materialize only complete typed adapter effects as manifest hooks.
        let adapter_hooks = manifest_hooks_from_complete_adapter_effects(&scriptlet_classification);
        let setuid_mode_updates =
            setuid_mode_updates_from_complete_adapter_effects(&scriptlet_classification);
        let file_capability_updates =
            file_capability_updates_from_complete_adapter_effects(&scriptlet_classification)?;
        apply_setuid_mode_updates(&mut final_files, &setuid_mode_updates)?;
        let mut manifest_hooks = Hooks::default();
        manifest_hooks.sysctl.extend(adapter_hooks.sysctl.clone());

        // Step 2: Build CCS manifest from metadata.
        let mut manifest = self.build_manifest(&final_metadata, &final_files, &manifest_hooks)?;
        apply_setuid_policy_allowlist(&mut manifest, &setuid_mode_updates);
        apply_file_capability_authority(&mut manifest, &file_capability_updates);
        let build_risk_report = classify_foreign_build_body_risk(format, files);
        let scriptlet_risk_report = classify_foreign_scriptlet_risk(metadata);
        let conversion_evidence =
            foreign_conversion_evidence(format, checksum, metadata, &build_risk_report);
        let provenance = manifest.provenance.get_or_insert_with(Default::default);
        provenance.origin_class = Some("foreign-converted".to_string());
        provenance.hardening_level = Some("hermetic".to_string());
        provenance.hermetic_evidence = Some(conversion_evidence);

        let scriptlet_bundle = build_legacy_scriptlet_bundle(ScriptletBundleInput {
            source_metadata: metadata,
            final_metadata: &final_metadata,
            source_files: files,
            final_files: &final_files,
            source_format: format,
            source_distro: self.source_distro.as_deref(),
            source_release: self.source_release.as_deref(),
            source_arch: metadata.architecture.as_deref(),
            source_checksum: Some(checksum),
            classification: &scriptlet_classification,
            target_profile_id: self.target_profile_id.as_deref(),
            conversion_tool: self.conversion_tool.as_str(),
            conversion_tool_version: env!("CARGO_PKG_VERSION"),
        })
        .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        scriptlet_bundle
            .bundle
            .validate()
            .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        manifest.legacy_scriptlets = Some(scriptlet_bundle.bundle.clone());

        // Step 3: Create temporary directory with file structure.
        let temp_dir = TempDir::new()
            .map_err(|e| ConversionError::IoError(format!("Failed to create temp dir: {}", e)))?;

        // Write files to temp directory
        self.write_files_to_temp(&final_files, temp_dir.path())?;

        // Write manifest
        let manifest_path = temp_dir.path().join("ccs.toml");
        let manifest_toml = toml::to_string_pretty(&manifest).map_err(|e| {
            ConversionError::ManifestError(format!("Failed to serialize manifest: {}", e))
        })?;
        std::fs::write(&manifest_path, manifest_toml)
            .map_err(|e| ConversionError::IoError(format!("Failed to write manifest: {}", e)))?;

        // Step 4: Build CCS package using CcsBuilder.
        let mut builder = CcsBuilder::new(manifest.clone(), temp_dir.path());

        if self.options.enable_chunking {
            builder = builder.with_chunking();
        }

        let mut build_result = builder
            .build()
            .map_err(|e| ConversionError::BuildError(format!("CCS build failed: {}", e)))?;

        // Step 5: Write the package file.
        std::fs::create_dir_all(&self.options.output_dir)
            .map_err(|e| ConversionError::IoError(format!("Failed to create output dir: {}", e)))?;

        let package_filename = format!(
            "{}-{}.ccs",
            build_result.manifest.package.name, build_result.manifest.package.version
        );
        let package_path = self.options.output_dir.join(&package_filename);

        write_ccs_package(&build_result, &package_path)
            .map_err(|e| ConversionError::BuildError(format!("Failed to write package: {}", e)))?;

        let parsed_package =
            crate::ccs::CcsPackage::parse(package_path.to_str().ok_or_else(|| {
                ConversionError::BuildError(format!(
                    "Converted package path is not valid UTF-8: {}",
                    package_path.display()
                ))
            })?)
            .map_err(|e| {
                ConversionError::BuildError(format!(
                    "Failed to parse converted package for boundary identity: {}",
                    e
                ))
            })?;
        let output_identity = compute_build_output_identity(&parsed_package).map_err(|e| {
            ConversionError::BuildError(format!(
                "Failed to compute foreign conversion output identity: {}",
                e
            ))
        })?;
        let provenance = build_result
            .manifest
            .provenance
            .as_mut()
            .expect("conversion provenance was initialized before build");
        let build_risk_report_hash = canonical_json_hash(&build_risk_report).map_err(|e| {
            ConversionError::BuildError(format!(
                "Failed to hash conversion command-risk report: {}",
                e
            ))
        })?;
        let scriptlet_risk_report_hash =
            canonical_json_hash(&scriptlet_risk_report).map_err(|e| {
                ConversionError::BuildError(format!(
                    "Failed to hash foreign scriptlet risk report: {}",
                    e
                ))
            })?;
        provenance.foreign_conversion_boundary = Some(ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: format.to_string(),
            source_checksum: checksum.to_string(),
            output_identity,
            build_risk_report_hash: Some(build_risk_report_hash),
            build_risk_report: Some(build_risk_report),
            scriptlet_risk_report_hash: Some(scriptlet_risk_report_hash),
            scriptlet_risk_report: Some(scriptlet_risk_report),
            diagnostics: Vec::new(),
        });
        write_ccs_package(&build_result, &package_path).map_err(|e| {
            ConversionError::BuildError(format!(
                "Failed to rewrite package with conversion boundary: {}",
                e
            ))
        })?;

        // Step 6: Extract source-package provenance information.
        let legacy_provenance = if metadata.package_path.exists() {
            let prov =
                LegacyProvenance::extract_from_path(format, checksum, &metadata.package_path);
            if prov.has_content() {
                tracing::info!(
                    "Extracted provenance from {} package: {}",
                    format,
                    prov.summary()
                );
                Some(prov)
            } else {
                tracing::debug!("No meaningful provenance extracted from {} package", format);
                Some(prov) // Still include for audit trail
            }
        } else {
            tracing::debug!(
                "Package path does not exist for provenance extraction: {:?}",
                metadata.package_path
            );
            None
        };

        Ok(ConversionResult {
            build_result,
            package_path: Some(package_path),
            original_format: format.to_string(),
            original_checksum: checksum.to_string(),
            legacy_provenance,
            scriptlet_classification,
            legacy_scriptlets: Some(scriptlet_bundle.bundle),
            scriptlet_metadata: scriptlet_bundle.summary,
        })
    }

    /// Build a CCS manifest from legacy package metadata
    fn build_manifest(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        hooks: &Hooks,
    ) -> Result<CcsManifest, ConversionError> {
        // Build platform info
        let platform = metadata.architecture.as_ref().map(|arch| Platform {
            os: "linux".to_string(),
            arch: Some(arch.clone()),
            libc: "gnu".to_string(),
            abi: None,
        });

        // Convert dependencies to capabilities and packages
        let mut capabilities = Vec::new();
        let mut packages = Vec::new();

        for dep in &metadata.dependencies {
            if dep.dep_type == DependencyType::Runtime {
                if let Some(ref ver) = dep.version {
                    capabilities.push(Capability::Versioned {
                        name: dep.name.clone(),
                        version: ver.clone(),
                    });
                } else {
                    packages.push(PackageDep {
                        name: dep.name.clone(),
                        version: None,
                    });
                }
            }
        }

        // Build config file list
        let config_files: Vec<String> = metadata
            .config_files
            .iter()
            .map(|c| c.path.clone())
            .collect();

        let mut provides = derive_provides(files);
        merge_native_provides(&mut provides, &metadata.provides);

        let manifest = CcsManifest {
            package: Package {
                name: metadata.name.clone(),
                version: metadata.version.clone(),
                description: metadata.description.clone().unwrap_or_else(|| {
                    format!("Converted from {} package", metadata.package_path.display())
                }),
                release: None,
                kind: None,
                license: None,
                homepage: None,
                repository: None,
                platform,
                authors: None,
            },
            provides,
            requires: Requires {
                capabilities,
                packages,
            },
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: hooks.clone(),
            scriptlets: Default::default(),
            legacy_scriptlets: None,
            config: Config {
                files: config_files,
                noreplace: true,
            },
            build: None,
            legacy: None,
            policy: BuildPolicyConfig::default(),
            file_capabilities: Vec::new(),
            provenance: None,
            capabilities: None,
            redirects: Redirects::default(),
        };

        Ok(manifest)
    }

    /// Write extracted files to a temporary directory
    fn write_files_to_temp(
        &self,
        files: &[ExtractedFile],
        temp_dir: &Path,
    ) -> Result<(), ConversionError> {
        for file in files {
            // Use safe_join to prevent path traversal from untrusted package paths
            let full_path = crate::filesystem::safe_join(temp_dir, &file.path).map_err(|e| {
                ConversionError::IoError(format!("Unsafe path '{}': {}", file.path, e))
            })?;

            // Create parent directories
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ConversionError::IoError(format!("Failed to create directory: {}", e))
                })?;
            }

            let is_symlink = file.symlink_target.is_some() || (file.mode & 0o170000) == 0o120000;
            if is_symlink {
                let target = file.symlink_target.as_deref().ok_or_else(|| {
                    ConversionError::IoError(format!(
                        "Symlink file {} is missing its target",
                        file.path
                    ))
                })?;

                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &full_path).map_err(|e| {
                    ConversionError::IoError(format!(
                        "Failed to create symlink {} -> {}: {}",
                        file.path, target, e
                    ))
                })?;

                #[cfg(not(unix))]
                {
                    let _ = target;
                    return Err(ConversionError::IoError(format!(
                        "Symlink file {} is not supported on this platform",
                        file.path
                    )));
                }

                continue;
            } else {
                // Write file content
                std::fs::write(&full_path, &file.content).map_err(|e| {
                    ConversionError::IoError(format!("Failed to write file {}: {}", file.path, e))
                })?;

                // Set permissions (best effort on Unix)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &full_path,
                        std::fs::Permissions::from_mode(file.mode as u32),
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
