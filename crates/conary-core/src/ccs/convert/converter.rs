// conary-core/src/ccs/convert/converter.rs
//! Native package to CCS format converter
//!
//! Takes exact native package metadata plus extracted payloads and builds a
//! `CcsManifest`, invoking `CcsBuilder` with optional CDC chunking.

mod evidence;

pub use evidence::ConversionError;
use evidence::*;

use crate::ccs::attestation::{
    FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary, canonical_json_hash,
    compute_build_output_identity_from_v2,
};
use crate::ccs::builder::{BuildResult, CcsBuilder, write_v2_ccs_package};
use crate::ccs::convert::native_provenance::NativeProvenance;
use crate::ccs::convert::scriptlet_bundle::{
    ScriptletBundleInput, ScriptletBundleSummary, build_native_lifecycle_bundle,
};
use crate::ccs::manifest::{
    CcsManifest, Components, Config, Hooks, Package, Platform, Provides, Redirects, Suggests,
};
use crate::ccs::native_lifecycle::NativeLifecycleBundle;
use crate::ccs::policy::BuildPolicyConfig;
use crate::ccs::signing::SigningKeyPair;
use crate::ccs::v2::PackageKindTagV2;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{DependencyType, ExtractedFile};
use crate::payload::PayloadNodeKind;
use crate::recipe::hermetic::{
    BuildCommandRiskEntry, BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
    BuilderEnvironmentKind, DependencyLock, DivergenceReport, HERMETIC_EVIDENCE_SCHEMA,
    HermeticBuildEvidence, RecipeIdentity, ReproducibilityRecord, SourceIdentity,
};
use crate::repository::versioning::VersionScheme;
use crate::security::command_risk::{
    COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskReport, CommandRiskSeverity, classify_shell_text,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

/// Options for native package conversion
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

/// Result of converting a native package
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
    /// Provenance information extracted from the source package
    pub native_provenance: Option<NativeProvenance>,
    /// Exact native lifecycle bundle embedded into the generated manifest.
    pub native_lifecycle: Option<NativeLifecycleBundle>,
    /// Compact lifecycle metadata derived from the embedded bundle.
    pub scriptlet_metadata: ScriptletBundleSummary,
    /// Exact Ed25519 public key that authenticated the emitted CCS v2 package.
    pub signing_public_key: String,
}

/// Converts native packages (RPM/DEB/Arch) to CCS format.
pub struct NativePackageConverter {
    options: ConversionOptions,
    source_distro: Option<String>,
    source_release: Option<String>,
    conversion_tool: String,
    signing_key: Option<Arc<SigningKeyPair>>,
}

impl NativePackageConverter {
    /// Create a new converter with the given options
    pub fn new(options: ConversionOptions) -> Self {
        Self {
            options,
            source_distro: None,
            source_release: None,
            conversion_tool: "conary".to_string(),
            signing_key: None,
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

    /// Override the conversion tool name recorded in passive scriptlet bundles.
    pub fn with_conversion_tool(mut self, tool: impl Into<String>) -> Self {
        self.conversion_tool = tool.into();
        self
    }

    /// Set the authority key required to emit a trusted CCS v2 package.
    pub fn with_signing_key(mut self, key: Arc<SigningKeyPair>) -> Self {
        self.signing_key = Some(key);
        self
    }

    /// Convert a native package to CCS format
    ///
    /// # Arguments
    /// * `metadata` - Package metadata from the native parser
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
        let final_metadata = metadata.clone();
        let final_files = files.to_vec();

        let mut manifest = self.build_manifest(&final_metadata, &Hooks::default())?;
        manifest.package.version_scheme = match format {
            "rpm" => VersionScheme::Rpm,
            "deb" => VersionScheme::Debian,
            "arch" => VersionScheme::Arch,
            other => {
                return Err(ConversionError::ManifestError(format!(
                    "unsupported source package format '{other}'"
                )));
            }
        };
        let build_risk_report = classify_foreign_build_body_risk(format, files);
        let scriptlet_risk_report = classify_foreign_scriptlet_risk(metadata);
        let conversion_evidence =
            foreign_conversion_evidence(format, checksum, metadata, &build_risk_report);
        let conversion_evidence_hash =
            canonical_json_hash(&conversion_evidence).map_err(|error| {
                ConversionError::ManifestError(format!(
                    "Failed to hash exact foreign conversion evidence: {error}"
                ))
            })?;
        let build_input_identity =
            canonical_json_hash(&conversion_evidence.build_input).map_err(|error| {
                ConversionError::ManifestError(format!(
                    "Failed to hash foreign conversion input identity: {error}"
                ))
            })?;
        let provenance = manifest.provenance.get_or_insert_with(Default::default);
        provenance.origin_class = Some("foreign-converted".to_string());
        provenance.hardening_level = Some("hermetic".to_string());
        provenance.hermetic_evidence = Some(conversion_evidence);

        let scriptlet_bundle = build_native_lifecycle_bundle(ScriptletBundleInput {
            source_metadata: metadata,
            final_metadata: &final_metadata,
            source_files: files,
            final_files: &final_files,
            source_format: format,
            source_distro: self.source_distro.as_deref(),
            source_release: self.source_release.as_deref(),
            source_arch: metadata.architecture.as_deref(),
            source_checksum: Some(checksum),
            conversion_tool: self.conversion_tool.as_str(),
            conversion_tool_version: env!("CARGO_PKG_VERSION"),
        })
        .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        scriptlet_bundle
            .bundle
            .validate()
            .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        manifest.native_lifecycle = Some(scriptlet_bundle.bundle.clone());

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
        let mut builder =
            CcsBuilder::new(manifest.clone(), temp_dir.path()).with_package_entries(&final_files);

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

        let signing_key = self.signing_key.as_ref().ok_or_else(|| {
            ConversionError::BuildError(
                "Foreign conversion requires an explicit CCS authority signing key".to_string(),
            )
        })?;
        let mut projected =
            crate::ccs::v2::project_build_result_to_v2(crate::ccs::v2::V2AuthoringInput {
                build: &build_result,
                local_dev: false,
                debug_toml: None,
            })
            .map_err(|error| {
                ConversionError::BuildError(format!(
                    "Failed to project foreign conversion into CCS v2 authority: {error}"
                ))
            })?;
        projected.authority.provenance.origin_class = Some("foreign-converted".to_string());
        projected.authority.provenance.hardening_level = Some("hermetic".to_string());
        projected.authority.provenance.build_input_identity = Some(build_input_identity);
        projected.authority.provenance.hermetic_evidence_hash = Some(conversion_evidence_hash);

        let output_identity =
            compute_build_output_identity_from_v2(&projected.authority).map_err(|e| {
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
        let boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: format.to_string(),
            source_checksum: checksum.to_string(),
            output_identity,
            build_risk_report_hash: Some(build_risk_report_hash),
            build_risk_report: Some(build_risk_report),
            scriptlet_risk_report_hash: Some(scriptlet_risk_report_hash),
            scriptlet_risk_report: Some(scriptlet_risk_report),
            diagnostics: Vec::new(),
        };
        let boundary_hash = canonical_json_hash(&boundary).map_err(|error| {
            ConversionError::BuildError(format!(
                "Failed to hash foreign conversion boundary: {error}"
            ))
        })?;
        provenance.foreign_conversion_boundary = Some(boundary.clone());
        projected
            .authority
            .provenance
            .foreign_conversion_boundary_hash = Some(boundary_hash);
        let debug_toml = toml::to_string_pretty(&build_result.manifest).map_err(|error| {
            ConversionError::ManifestError(format!(
                "Failed to serialize CCS v2 diagnostic projection: {error}"
            ))
        })?;
        projected.authority.debug_toml_sha256 = Some(crate::hash::sha256(debug_toml.as_bytes()));
        crate::ccs::v2::validate_authority(&projected.authority).map_err(|error| {
            ConversionError::BuildError(format!(
                "Foreign conversion produced invalid CCS v2 authority: {error}"
            ))
        })?;
        write_v2_ccs_package(
            &projected.authority,
            &projected.payloads_by_path,
            &package_path,
            signing_key,
            Some(&debug_toml),
            None,
            Some(&boundary),
        )
        .map_err(|e| {
            ConversionError::BuildError(format!(
                "Failed to write signed current CCS package with conversion boundary: {}",
                e
            ))
        })?;
        crate::ccs::verify::verify_package(
            &package_path,
            &crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
        )
        .map_err(|error| {
            ConversionError::BuildError(format!(
                "Converted CCS package failed immediate authority verification: {error:#}"
            ))
        })?;

        // Step 6: Extract source-package provenance information.
        let artifact_exists = metadata.package_path.try_exists().map_err(|error| {
            ConversionError::IoError(format!(
                "Failed to inspect native package provenance path {:?}: {error}",
                metadata.package_path
            ))
        })?;
        let provenance = if artifact_exists {
            NativeProvenance::extract_from_path(format, checksum, &metadata.package_path).map_err(
                |error| {
                    ConversionError::IoError(format!(
                        "Failed to extract native package provenance: {error:#}"
                    ))
                },
            )?
        } else {
            tracing::debug!(
                "Native package artifact is unavailable for provenance inspection: {:?}",
                metadata.package_path
            );
            NativeProvenance::new(format, checksum)
        };
        if provenance.has_content() {
            tracing::info!(
                "Extracted provenance from {} package: {}",
                format,
                provenance.summary()
            );
        } else {
            tracing::debug!("No optional provenance fields found in {} package", format);
        }
        let native_provenance = Some(provenance);

        Ok(ConversionResult {
            build_result,
            package_path: Some(package_path),
            original_format: format.to_string(),
            original_checksum: checksum.to_string(),
            native_provenance,
            native_lifecycle: Some(scriptlet_bundle.bundle),
            scriptlet_metadata: scriptlet_bundle.summary,
            signing_public_key: signing_key.public_key_base64(),
        })
    }

    /// Build a CCS manifest from native package metadata
    fn build_manifest(
        &self,
        metadata: &PackageMetadata,
        hooks: &Hooks,
    ) -> Result<CcsManifest, ConversionError> {
        // Build platform info
        let platform = metadata.architecture.as_ref().map(|arch| Platform {
            os: "linux".to_string(),
            arch: Some(arch.clone()),
            libc: "gnu".to_string(),
            abi: None,
        });

        // Build config file list
        let config_files: Vec<String> = metadata
            .config_files
            .iter()
            .map(|c| c.path.clone())
            .collect();

        let mut provides = Provides::default();
        merge_native_provides(&mut provides, &metadata.provides);

        let manifest = CcsManifest {
            package: Package {
                name: metadata.name.clone(),
                version: metadata.version.clone(),
                version_scheme: metadata.version_scheme,
                description: metadata.description.clone().unwrap_or_else(|| {
                    format!("Converted from {} package", metadata.package_path.display())
                }),
                release: "1".to_string(),
                kind: PackageKindTagV2::Package,
                license: None,
                homepage: None,
                repository: None,
                platform,
                authors: None,
            },
            provides,
            requirements: metadata.requirements.clone(),
            relations: metadata.relations.clone(),
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: hooks.clone(),
            scriptlets: Default::default(),
            native_lifecycle: None,
            config: Config {
                files: config_files,
                noreplace: true,
            },
            build: None,
            native_export: None,
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

            match &file.node.kind {
                PayloadNodeKind::Symlink { target } => {
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
                }
                PayloadNodeKind::Directory => {
                    std::fs::create_dir_all(&full_path).map_err(|e| {
                        ConversionError::IoError(format!(
                            "Failed to create directory {}: {}",
                            file.path, e
                        ))
                    })?;
                }
                PayloadNodeKind::Regular { .. } => {
                    std::fs::write(&full_path, &file.content).map_err(|e| {
                        ConversionError::IoError(format!(
                            "Failed to write file {}: {}",
                            file.path, e
                        ))
                    })?;
                }
                PayloadNodeKind::Hardlink { .. }
                | PayloadNodeKind::BlockDevice { .. }
                | PayloadNodeKind::CharacterDevice { .. }
                | PayloadNodeKind::Fifo
                | PayloadNodeKind::Socket => {}
            }

            if matches!(
                file.node.kind,
                PayloadNodeKind::Regular { .. } | PayloadNodeKind::Directory
            ) {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &full_path,
                        std::fs::Permissions::from_mode(file.node.mode),
                    )
                    .map_err(|error| {
                        ConversionError::IoError(format!(
                            "Failed to set exact mode on {}: {error}",
                            file.path
                        ))
                    })?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
