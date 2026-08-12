// conary-core/src/ccs/convert/converter.rs
//! Native package to CCS format converter
//!
//! Takes exact native package metadata plus extracted payloads and emits a
//! signed CCS v3 archive.

mod evidence;

pub use evidence::ConversionError;
use evidence::*;

use super::ForeignConversionInput;
use crate::ccs::attestation::{
    FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary, canonical_json_hash,
    compute_build_output_identity_from_v3,
};
use crate::ccs::builder::{
    BuildResult, ComponentData, FileEntry, write_v3_ccs_package_from_sources,
};
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
use crate::ccs::v3::PackageKindTagV3;
use crate::hash::{Hash, HashAlgorithm};
use crate::packages::payload::PackagePayloadFile;
use crate::recipe::hermetic::{
    BuildCommandRiskEntry, BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
    BuilderEnvironmentKind, DependencyLock, DivergenceReport, HERMETIC_EVIDENCE_SCHEMA,
    HermeticBuildEvidence, RecipeIdentity, ReproducibilityRecord, SourceIdentity,
};
use crate::repository::versioning::VersionScheme;
use crate::security::command_risk::{
    COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskReport, CommandRiskSeverity, classify_shell_text,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Options for native package conversion
#[derive(Debug, Clone)]
pub struct ConversionOptions {
    /// Output directory for the converted package
    pub output_dir: PathBuf,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
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
    /// Exact Ed25519 public key that authenticated the emitted CCS v3 package.
    pub signing_public_key: String,
}

/// Converts native packages (RPM/DEB/Arch) to CCS format.
pub struct NativePackageConverter {
    options: ConversionOptions,
    source_profile: Option<String>,
    source_release: Option<String>,
    conversion_tool: String,
    signing_key: Option<Arc<SigningKeyPair>>,
}

impl NativePackageConverter {
    /// Create a new converter with the given options
    pub fn new(options: ConversionOptions) -> Self {
        Self {
            options,
            source_profile: None,
            source_release: None,
            conversion_tool: "conary".to_string(),
            signing_key: None,
        }
    }

    /// Create a converter with default options
    pub fn with_defaults() -> Self {
        Self::new(ConversionOptions::default())
    }

    /// Attach exact source-profile context for native lifecycle metadata.
    pub fn with_source_profile(mut self, profile: impl Into<String>) -> Self {
        self.source_profile = Some(profile.into());
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

    /// Set the authority key required to emit a trusted CCS v3 package.
    pub fn with_signing_key(mut self, key: Arc<SigningKeyPair>) -> Self {
        self.signing_key = Some(key);
        self
    }

    /// Convert from exact reopenable payload descriptors without whole-file
    /// buffering in the conversion or CCS object-writing path.
    pub fn convert_payload(
        &self,
        metadata: &ForeignConversionInput,
        files: &[PackagePayloadFile],
        format: &str,
        checksum: &Hash,
    ) -> Result<ConversionResult, ConversionError> {
        if checksum.algorithm != HashAlgorithm::Sha256 {
            return Err(ConversionError::ManifestError(format!(
                "foreign conversion source checksum must use SHA-256, got '{}'",
                checksum.algorithm
            )));
        }
        let checksum = checksum.to_prefixed_string();
        let final_metadata = metadata.clone();
        let expected_version_scheme = match format {
            "rpm" => VersionScheme::Rpm,
            "deb" => VersionScheme::Debian,
            "arch" => VersionScheme::Arch,
            other => {
                return Err(ConversionError::ManifestError(format!(
                    "unsupported source package format '{other}'"
                )));
            }
        };
        if metadata.version_scheme() != expected_version_scheme {
            return Err(ConversionError::ManifestError(format!(
                "source package format '{format}' requires '{}' version authority, got '{}'",
                expected_version_scheme.as_str(),
                metadata.version_scheme().as_str()
            )));
        }

        // Architecture is package identity authority for every foreign
        // conversion. Validate it before format-specific lifecycle
        // projections so an optional projection (such as an RPM repository
        // declaration) cannot change the canonical identity failure.
        let architecture = metadata.architecture().ok_or_else(|| {
            ConversionError::BuildError("v3 package identity architecture is required".to_string())
        })?;

        let mut manifest = self.build_manifest(&final_metadata, &Hooks::default())?;
        let repository_enrollments = if format == "rpm" {
            crate::repository::enrollment::derive::derive_rpm_repository_enrollments(
                files,
                architecture,
                self.source_profile.as_deref(),
            )
            .map_err(|error| ConversionError::ManifestError(error.to_string()))?
        } else {
            Vec::new()
        };
        let build_risk_report = classify_foreign_build_body_risk(format, files);
        let scriptlet_risk_report = classify_foreign_scriptlet_risk(metadata);
        let conversion_evidence =
            foreign_conversion_evidence(format, &checksum, metadata, &build_risk_report);
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
            source_format: format,
            source_profile: self.source_profile.as_deref(),
            source_release: self.source_release.as_deref(),
            source_arch: metadata.architecture(),
            source_checksum: Some(&checksum),
            conversion_tool: self.conversion_tool.as_str(),
            conversion_tool_version: env!("CARGO_PKG_VERSION"),
        })
        .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        scriptlet_bundle
            .bundle
            .validate()
            .map_err(|error| ConversionError::ManifestError(error.to_string()))?;

        manifest.native_lifecycle = Some(scriptlet_bundle.bundle.clone());

        // Foreign conversion preserves exact package payload authority. It
        // does not run content-rewriting build policy or materialize a second
        // source tree.
        let mut build_result = build_streaming_result(manifest.clone(), files)?;

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
        let mut authority = crate::ccs::v3::project_build_result_authority_to_v3(
            crate::ccs::v3::V3AuthoringInput {
                build: &build_result,
                local_dev: false,
                debug_toml: None,
            },
        )
        .map_err(|error| {
            ConversionError::BuildError(format!(
                "Failed to project foreign conversion into CCS v3 authority: {error}"
            ))
        })?;
        authority.provenance.origin_class = Some("foreign-converted".to_string());
        authority.provenance.hardening_level = Some("hermetic".to_string());
        authority.provenance.build_input_identity = Some(build_input_identity);
        authority.provenance.hermetic_evidence_hash = Some(conversion_evidence_hash);
        authority.identity.debian_multi_arch = metadata.debian_multi_arch();
        authority.provided_capabilities = project_source_capabilities(metadata)?;
        authority.lifecycle.repository_enrollments = repository_enrollments;
        crate::ccs::v3::validate_authority(&authority).map_err(|error| {
            ConversionError::BuildError(format!(
                "Foreign conversion produced invalid typed provider authority: {error}"
            ))
        })?;

        let output_identity = compute_build_output_identity_from_v3(&authority).map_err(|e| {
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
            source_checksum: checksum.clone(),
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
        authority.provenance.foreign_conversion_boundary_hash = Some(boundary_hash);
        let debug_toml = toml::to_string_pretty(&build_result.manifest).map_err(|error| {
            ConversionError::ManifestError(format!(
                "Failed to serialize CCS v3 diagnostic projection: {error}"
            ))
        })?;
        authority.debug_toml_sha256 = Some(crate::hash::sha256(debug_toml.as_bytes()));
        crate::ccs::v3::validate_authority(&authority).map_err(|error| {
            ConversionError::BuildError(format!(
                "Foreign conversion produced invalid CCS v3 authority: {error}"
            ))
        })?;
        write_v3_ccs_package_from_sources(
            &authority,
            files,
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
            NativeProvenance::extract_from_path(format, &checksum, &metadata.package_path).map_err(
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
            NativeProvenance::new(format, &checksum)
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
            original_checksum: checksum,
            native_provenance,
            native_lifecycle: Some(scriptlet_bundle.bundle),
            scriptlet_metadata: scriptlet_bundle.summary,
            signing_public_key: signing_key.public_key_base64(),
        })
    }

    /// Build a CCS manifest from native package metadata
    fn build_manifest(
        &self,
        metadata: &ForeignConversionInput,
        hooks: &Hooks,
    ) -> Result<CcsManifest, ConversionError> {
        // Build platform info
        let platform = metadata.architecture().map(|arch| Platform {
            os: "linux".to_string(),
            arch: Some(arch.to_string()),
            libc: "gnu".to_string(),
            abi: None,
        });

        let manifest = CcsManifest {
            package: Package {
                name: metadata.name().to_string(),
                version: metadata.version().to_string(),
                version_scheme: metadata.version_scheme(),
                description: metadata.description.clone().unwrap_or_else(|| {
                    format!("Converted from {} package", metadata.package_path.display())
                }),
                release: "1".to_string(),
                kind: PackageKindTagV3::Package,
                debian_multi_arch: metadata.debian_multi_arch(),
                license: None,
                homepage: None,
                repository: None,
                platform,
                authors: None,
            },
            provides: Provides::default(),
            requirements: metadata.requirements.clone(),
            relations: metadata.relations.clone(),
            suggests: Suggests::default(),
            components: Components::default(),
            hooks: hooks.clone(),
            scriptlets: Default::default(),
            native_lifecycle: None,
            config: Config {
                files: metadata
                    .source_authority
                    .config_declarations()
                    .map_err(|error| ConversionError::ManifestError(error.to_string()))?,
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
}

fn build_streaming_result(
    manifest: CcsManifest,
    payloads: &[PackagePayloadFile],
) -> Result<BuildResult, ConversionError> {
    let mut files = Vec::with_capacity(payloads.len());
    let mut total_size = 0_u64;
    for payload in payloads {
        payload
            .node
            .validate_content(payload.content_authority.as_ref())
            .map_err(|error| ConversionError::BuildError(error.to_string()))?;
        if let Some(content) = &payload.content_authority {
            total_size = total_size.checked_add(content.size).ok_or_else(|| {
                ConversionError::BuildError("CCS payload size arithmetic overflow".to_string())
            })?;
        }
        files.push(FileEntry {
            path: payload.path.clone(),
            node: payload.node.clone(),
            content: payload.content_authority.clone(),
            component: "runtime".to_string(),
            chunks: None,
        });
    }
    let component_hash = crate::hash::sha256_prefixed(
        &crate::ccs::attestation::canonical_json_bytes(&files)
            .map_err(|error| ConversionError::BuildError(error.to_string()))?,
    );
    let components = if files.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: component_hash,
                size: total_size,
            },
        )])
    };
    Ok(BuildResult {
        manifest,
        components,
        files,
        payloads: payloads.to_vec(),
        total_size,
        chunked: false,
        chunk_stats: None,
    })
}

fn project_source_capabilities(
    metadata: &ForeignConversionInput,
) -> Result<Vec<crate::ccs::v3::ProvidedCapabilityV3>, ConversionError> {
    use crate::ccs::v3::schema::DependencyKindV3;
    use crate::repository::dependency_model::RepositoryCapabilityKind;

    let mut signed = Vec::new();
    for provide in metadata
        .source_authority
        .declared_capabilities()
        .map_err(|error| ConversionError::BuildError(error.to_string()))?
    {
        let projected = crate::ccs::v3::ProvidedCapabilityV3 {
            kind: match provide.kind {
                RepositoryCapabilityKind::PackageName => DependencyKindV3::Package,
                RepositoryCapabilityKind::Generic | RepositoryCapabilityKind::Virtual => {
                    DependencyKindV3::Capability
                }
                RepositoryCapabilityKind::File => DependencyKindV3::File,
                RepositoryCapabilityKind::Path => DependencyKindV3::Path,
                RepositoryCapabilityKind::Binary => DependencyKindV3::Binary,
                RepositoryCapabilityKind::Soname => DependencyKindV3::Soname,
                RepositoryCapabilityKind::PkgConfig => DependencyKindV3::PkgConfig,
            },
            name: provide.name.clone(),
            provider_version: provide.version.clone(),
            version_relation: provide.version_relation,
            version_scheme: provide.version_scheme,
            architecture_qualifier: provide.architecture_qualifier.clone(),
            provenance: provide.provenance.clone(),
            target: None,
            component: None,
        };
        if !signed.contains(&projected) {
            signed.push(projected);
        }
    }
    Ok(signed)
}

#[cfg(test)]
mod tests;
