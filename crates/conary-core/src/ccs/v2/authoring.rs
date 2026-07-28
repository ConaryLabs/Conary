// conary-core/src/ccs/v2/authoring.rs

use super::schema::*;
use crate::ccs::builder::BuildResult;
use crate::ccs::v2::PackageKindTagV2;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use crate::repository::versioning::VersionScheme;
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthoringFindingBucket {
    Contract,
    PublicationReadiness,
    Profile,
    Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthoringFindingSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthoringFinding {
    pub code: &'static str,
    pub bucket: AuthoringFindingBucket,
    pub severity: AuthoringFindingSeverity,
    pub field: Option<&'static str>,
    pub message: String,
    pub suggestion: &'static str,
    pub blocks_build: bool,
    pub blocks_local_test: bool,
    pub blocks_publish: bool,
}

pub fn lint_manifest_for_v2_authoring(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Vec<AuthoringFinding> {
    let mut findings = Vec::new();
    if manifest.package.release.is_empty() {
        findings.push(AuthoringFinding {
            code: "m4b-missing-release",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.release"),
            message: "v2 package authoring requires package.release".to_string(),
            suggestion: "add release = \"1\" under [package]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if manifest.components.default.len() != 1 || manifest.components.default[0].trim().is_empty() {
        findings.push(AuthoringFinding {
            code: "m4b-default-component",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("components.default"),
            message: "v2 package authoring requires exactly one non-empty default component name"
                .to_string(),
            suggestion: "set [components].default = [\"runtime\"]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    // PublicationReadiness and Style buckets are part of the stable diagnostic
    // shape, but M4b's first implementation only emits concrete
    // contract/profile findings.
    findings
}

pub struct V2AuthoringInput<'a> {
    pub build: &'a BuildResult,
    pub local_dev: bool,
    pub debug_toml: Option<String>,
}

impl std::fmt::Debug for V2AuthoringInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("V2AuthoringInput")
            .field("local_dev", &self.local_dev)
            .field("debug_toml", &self.debug_toml.as_ref().map(|_| "<toml>"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ProjectedV2Package {
    pub authority: AuthorityDocumentV2,
    pub payloads: Vec<crate::packages::payload::PackagePayloadFile>,
    pub debug_toml: Option<String>,
}

pub fn project_build_result_to_v2(input: V2AuthoringInput<'_>) -> Result<ProjectedV2Package> {
    validate_payload_sources(input.build)?;
    let payloads = input.build.payloads.clone();
    let debug_toml = input.debug_toml.clone();
    let authority = project_build_result_authority_to_v2(input)?;
    Ok(ProjectedV2Package {
        authority,
        payloads,
        debug_toml,
    })
}

/// Project signed metadata without reassembling whole-file payload bytes.
pub fn project_build_result_authority_to_v2(
    input: V2AuthoringInput<'_>,
) -> Result<AuthorityDocumentV2> {
    let package = &input.build.manifest.package;
    let release = package.release.as_str();
    let kind = package.kind;
    if kind != PackageKindTagV2::Package {
        bail!("M4b only supports package authoring for v2 build");
    }

    let config_authority = config_authority_for_manifest(&input.build.manifest, input.build)?;
    let files = input
        .build
        .files
        .iter()
        .map(|file| FileAuthorityV2 {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content.clone(),
            component: file.component.clone(),
            config: config_authority.get(&file.path).copied(),
            conflict: ConflictPolicyV2::Error,
        })
        .collect::<Vec<_>>();
    let config = config_authority
        .iter()
        .map(|(path, semantics)| ConfigAuthorityV2 {
            path: path.clone(),
            semantics: *semantics,
        })
        .collect::<Vec<_>>();

    let default_component = select_default_component(input.build)?;
    let mut components = input
        .build
        .components
        .iter()
        .map(|(name, component)| {
            (
                name.clone(),
                ComponentAuthorityV2 {
                    name: name.clone(),
                    default: name == &default_component,
                    file_count: component.files.len() as u32,
                    total_size: component.size,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    if components.is_empty() {
        components.insert(
            default_component.clone(),
            ComponentAuthorityV2 {
                name: default_component,
                default: true,
                file_count: 0,
                total_size: 0,
            },
        );
    }

    let fallback_build_input_identity =
        crate::hash::sha256(format!("{}:{}:{}", package.name, package.version, release).as_bytes());
    let fallback_evidence_hash = crate::hash::sha256(
        serde_json::json!({
            "mode": if input.local_dev { "local-dev" } else { "signed" },
            "package": package.name,
            "version": package.version,
            "release": release,
            "file_count": files.len(),
        })
        .to_string()
        .as_bytes(),
    );

    // M4b uses the existing host file scan for both local-dev and explicit-key
    // signing. Do not claim hermetic hardening until a later slice routes
    // through a hermetic builder.
    let lifecycle = super::lifecycle::authority_from_manifest(&input.build.manifest);
    let manifest_provenance = input.build.manifest.provenance.as_ref();
    let build_input_identity = manifest_provenance
        .and_then(|provenance| provenance.hermetic_evidence.as_ref())
        .map(|evidence| crate::ccs::attestation::canonical_json_hash(&evidence.build_input))
        .transpose()?
        .unwrap_or(fallback_build_input_identity);
    let evidence_hash = manifest_provenance
        .and_then(|provenance| provenance.hermetic_evidence.as_ref())
        .map(crate::ccs::attestation::canonical_json_hash)
        .transpose()?
        .unwrap_or(fallback_evidence_hash);
    let boundary_hash = manifest_provenance
        .and_then(|provenance| provenance.foreign_conversion_boundary.as_ref())
        .map(crate::ccs::attestation::canonical_json_hash)
        .transpose()?;
    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: package.name.clone(),
            version: package.version.clone(),
            version_scheme: package.version_scheme,
            release: release.to_string(),
            architecture: package
                .platform
                .as_ref()
                .and_then(|platform| platform.arch.clone()),
            debian_multi_arch: package.debian_multi_arch,
            platform: package
                .platform
                .as_ref()
                .map(|platform| platform.os.clone()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files,
            config,
            policy: PackagePolicyV2::default(),
        }),
        provides: project_provides(&input.build.manifest),
        requirements: project_requirements(&input.build.manifest),
        relations: input.build.manifest.relations.clone(),
        capabilities: input.build.manifest.capabilities.clone(),
        components,
        lifecycle,
        provenance: ProvenanceAuthorityV2 {
            origin_class: manifest_provenance
                .and_then(|provenance| provenance.origin_class.clone())
                .or_else(|| Some("native-built".to_string())),
            hardening_level: manifest_provenance
                .and_then(|provenance| provenance.hardening_level.clone())
                .or_else(|| Some("host".to_string())),
            build_input_identity: Some(build_input_identity),
            hermetic_evidence_hash: Some(evidence_hash),
            foreign_conversion_boundary_hash: boundary_hash,
        },
        debug_toml_sha256: input
            .debug_toml
            .as_ref()
            .map(|toml| crate::hash::sha256(toml.as_bytes())),
    };

    super::validation::validate_authority(&authority)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(authority)
}

fn project_provides(manifest: &crate::ccs::manifest::CcsManifest) -> Vec<ProvidedCapabilityV2> {
    let scheme = manifest.package.version_scheme;
    let mut entries = vec![provided_capability(
        DependencyKindV2::Package,
        &manifest.package.name,
        Some(&manifest.package.version),
        scheme,
    )];
    entries.extend(
        manifest
            .provides
            .capabilities
            .iter()
            .map(|name| provided_capability(DependencyKindV2::Capability, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .sonames
            .iter()
            .map(|name| provided_capability(DependencyKindV2::Soname, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .binaries
            .iter()
            .map(|name| provided_capability(DependencyKindV2::Binary, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .pkgconfig
            .iter()
            .map(|name| provided_capability(DependencyKindV2::PkgConfig, name, None, scheme)),
    );
    sort_and_deduplicate_provides(entries)
}

pub(super) fn project_requirements(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Vec<RepositoryRequirementGroup> {
    let mut keyed = BTreeMap::new();
    for requirement in manifest.requirements.clone() {
        let key = serde_json::to_string(&requirement)
            .expect("typed CCS requirement is JSON serializable");
        keyed.entry(key).or_insert(requirement);
    }
    keyed.into_values().collect()
}

fn provided_capability(
    kind: DependencyKindV2,
    name: &str,
    provider_version: Option<&str>,
    version_scheme: VersionScheme,
) -> ProvidedCapabilityV2 {
    ProvidedCapabilityV2 {
        kind,
        name: name.to_string(),
        provider_version: provider_version.map(str::to_string),
        version_relation: provider_version
            .map(|_| crate::repository::dependency_model::ProvideVersionRelation::Equal),
        version_scheme,
        architecture_qualifier: Default::default(),
        target: None,
        component: None,
    }
}

fn sort_and_deduplicate_provides(entries: Vec<ProvidedCapabilityV2>) -> Vec<ProvidedCapabilityV2> {
    let mut keyed = BTreeMap::new();
    for entry in entries {
        let key = (
            dependency_kind_order(entry.kind),
            entry.name.clone(),
            entry.provider_version.clone(),
            entry.version_relation,
            entry.version_scheme.as_str(),
            serde_json::to_string(&entry.architecture_qualifier)
                .expect("provider architecture qualifier is JSON serializable"),
            entry.target.clone(),
            entry.component.clone(),
        );
        keyed.entry(key).or_insert(entry);
    }
    keyed.into_values().collect()
}

fn dependency_kind_order(kind: DependencyKindV2) -> u8 {
    match kind {
        DependencyKindV2::Package => 0,
        DependencyKindV2::Capability => 1,
        DependencyKindV2::File => 2,
        DependencyKindV2::Path => 3,
        DependencyKindV2::Binary => 4,
        DependencyKindV2::Soname => 5,
        DependencyKindV2::PkgConfig => 6,
    }
}

fn config_authority_for_manifest(
    manifest: &crate::ccs::manifest::CcsManifest,
    build: &BuildResult,
) -> Result<BTreeMap<String, ConfigSemanticsV2>> {
    let build_paths = build
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut authority = BTreeMap::new();

    for config in &manifest.config.files {
        let absent_from_payload = config.ghost || config.remove_on_upgrade;
        if !config.path.starts_with('/') {
            bail!("config path {} must be absolute", config.path);
        }
        if absent_from_payload == build_paths.contains(config.path.as_str()) {
            let expected = if absent_from_payload {
                "absent from"
            } else {
                "present in"
            };
            bail!(
                "config path {} must be {expected} build output for its exact semantics",
                config.path
            );
        }
        let semantics = ConfigSemanticsV2 {
            noreplace: config.noreplace,
            ghost: config.ghost,
            remove_on_upgrade: config.remove_on_upgrade,
        };
        if authority.insert(config.path.clone(), semantics).is_some() {
            bail!("config path {} is declared more than once", config.path);
        }
    }

    Ok(authority)
}

fn select_default_component(build: &BuildResult) -> Result<String> {
    let manifest_defaults = &build.manifest.components.default;
    if manifest_defaults.len() != 1 || manifest_defaults[0].trim().is_empty() {
        bail!("v2 package authoring requires exactly one named default component");
    }
    let default_component = manifest_defaults[0].clone();
    if !build.components.is_empty() && !build.components.contains_key(&default_component) {
        bail!("v2 package default component '{default_component}' is not present in build output");
    }
    Ok(default_component)
}

fn validate_payload_sources(build: &BuildResult) -> Result<()> {
    let mut seen = BTreeSet::new();
    for payload in &build.payloads {
        if !seen.insert(payload.path.as_str()) {
            bail!(
                "payload source path {} appears more than once",
                payload.path
            );
        }
    }
    for file in &build.files {
        let payload = build.payload(&file.path)?;
        if payload.node != file.node || payload.content_authority != file.content {
            bail!(
                "payload descriptor authority disagrees with build entry {}",
                file.path
            );
        }
        if file.node.kind.is_regular() != payload.source().is_some() {
            bail!(
                "payload source presence disagrees with build entry {}",
                file.path
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "authoring/tests.rs"]
mod tests;
