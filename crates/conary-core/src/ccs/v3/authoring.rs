// conary-core/src/ccs/v3/authoring.rs

use super::schema::*;
use crate::ccs::builder::BuildResult;
use crate::ccs::v3::PackageKindTagV3;
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

pub fn lint_manifest_for_v3_authoring(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Vec<AuthoringFinding> {
    let mut findings = Vec::new();
    if manifest.package.release.is_empty() {
        findings.push(AuthoringFinding {
            code: "m4b-missing-release",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.release"),
            message: "v3 package authoring requires package.release".to_string(),
            suggestion: "add release = \"1\" under [package]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if manifest
        .package
        .platform
        .as_ref()
        .and_then(|platform| platform.arch.as_deref())
        .is_none_or(|architecture| architecture.trim().is_empty())
    {
        findings.push(AuthoringFinding {
            code: "m4b-missing-architecture",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.platform.arch"),
            message: "v3 package authoring requires exact architecture authority".to_string(),
            suggestion: "set arch = \"noarch\" under [package.platform] for an architecture-independent Conary package, or use the exact target architecture token",
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
            message: "v3 package authoring requires exactly one non-empty default component name"
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

pub struct V3AuthoringInput<'a> {
    pub build: &'a BuildResult,
    pub local_dev: bool,
    pub debug_toml: Option<String>,
}

impl std::fmt::Debug for V3AuthoringInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("V3AuthoringInput")
            .field("local_dev", &self.local_dev)
            .field("debug_toml", &self.debug_toml.as_ref().map(|_| "<toml>"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ProjectedV3Package {
    pub authority: AuthorityDocumentV3,
    pub payloads: Vec<crate::packages::payload::PackagePayloadFile>,
    pub debug_toml: Option<String>,
}

pub fn project_build_result_to_v3(input: V3AuthoringInput<'_>) -> Result<ProjectedV3Package> {
    validate_payload_sources(input.build)?;
    let payloads = input.build.payloads.clone();
    let debug_toml = input.debug_toml.clone();
    let authority = project_build_result_authority_to_v3(input)?;
    Ok(ProjectedV3Package {
        authority,
        payloads,
        debug_toml,
    })
}

/// Project signed metadata without reassembling whole-file payload bytes.
pub fn project_build_result_authority_to_v3(
    input: V3AuthoringInput<'_>,
) -> Result<AuthorityDocumentV3> {
    let package = &input.build.manifest.package;
    let release = package.release.as_str();
    let kind = package.kind;
    if kind != PackageKindTagV3::Package {
        bail!("M4b only supports package authoring for v3 build");
    }

    let config_authority = config_authority_for_manifest(&input.build.manifest, input.build)?;
    let file_capabilities =
        file_capability_authority_for_manifest(&input.build.manifest, input.build)?;
    let files = input
        .build
        .files
        .iter()
        .map(|file| FileAuthorityV3 {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content.clone(),
            component: file.component.clone(),
            config: config_authority.get(&file.path).copied(),
            conflict: ConflictPolicyV3::Error,
        })
        .collect::<Vec<_>>();
    let mut config = input.build.manifest.config.files.clone();
    config.sort_by(|left, right| left.path().cmp(right.path()));

    let default_component = select_default_component(input.build)?;
    let mut components = input
        .build
        .components
        .iter()
        .map(|(name, component)| {
            (
                name.clone(),
                ComponentAuthorityV3 {
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
            ComponentAuthorityV3 {
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
    let authority = AuthorityDocumentV3 {
        format_version: FORMAT_VERSION_V3,
        identity: PackageIdentityV3 {
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
            kind: PackageKindTagV3::Package,
        },
        kind: PackageKindV3::Package(PackageDataV3 {
            files,
            config,
            policy: PackagePolicyV3::default(),
        }),
        provided_capabilities: project_manifest_capabilities(&input.build.manifest),
        requirements: project_requirements(&input.build.manifest),
        relations: input.build.manifest.relations.clone(),
        execution_capabilities: input.build.manifest.capabilities.clone(),
        file_capabilities,
        components,
        lifecycle,
        provenance: ProvenanceAuthorityV3 {
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

fn project_manifest_capabilities(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Vec<ProvidedCapabilityV3> {
    let scheme = manifest.package.version_scheme;
    let mut entries = Vec::new();
    entries.extend(
        manifest
            .provides
            .capabilities
            .iter()
            .map(|name| provided_capability(DependencyKindV3::Capability, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .sonames
            .iter()
            .map(|name| provided_capability(DependencyKindV3::Soname, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .binaries
            .iter()
            .map(|name| provided_capability(DependencyKindV3::Binary, name, None, scheme)),
    );
    entries.extend(
        manifest
            .provides
            .pkgconfig
            .iter()
            .map(|name| provided_capability(DependencyKindV3::PkgConfig, name, None, scheme)),
    );
    sort_and_deduplicate_capabilities(entries)
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
    kind: DependencyKindV3,
    name: &str,
    provider_version: Option<&str>,
    version_scheme: VersionScheme,
) -> ProvidedCapabilityV3 {
    ProvidedCapabilityV3 {
        kind,
        name: name.to_string(),
        provider_version: provider_version.map(str::to_string),
        version_relation: provider_version
            .map(|_| crate::repository::dependency_model::ProvideVersionRelation::Equal),
        version_scheme,
        architecture_qualifier: Default::default(),
        provenance: crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
        target: None,
        component: None,
    }
}

fn sort_and_deduplicate_capabilities(
    entries: Vec<ProvidedCapabilityV3>,
) -> Vec<ProvidedCapabilityV3> {
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

fn dependency_kind_order(kind: DependencyKindV3) -> u8 {
    match kind {
        DependencyKindV3::Package => 0,
        DependencyKindV3::Capability => 1,
        DependencyKindV3::File => 2,
        DependencyKindV3::Path => 3,
        DependencyKindV3::Binary => 4,
        DependencyKindV3::Soname => 5,
        DependencyKindV3::PkgConfig => 6,
        DependencyKindV3::PkgConfig32 => 7,
        DependencyKindV3::Comar => 8,
    }
}

fn config_authority_for_manifest(
    manifest: &crate::ccs::manifest::CcsManifest,
    build: &BuildResult,
) -> Result<BTreeMap<String, ConfigSemanticsV3>> {
    let build_paths = build
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut authority = BTreeMap::new();

    for config in &manifest.config.files {
        config
            .validate()
            .map_err(|error| anyhow::anyhow!("config path {}: {error}", config.path()))?;
        let absent_from_payload =
            config.payload() == crate::packages::config_authority::ConfigPayloadAssociation::Absent;
        if absent_from_payload == build_paths.contains(config.path()) {
            let expected = if absent_from_payload {
                "absent from"
            } else {
                "present in"
            };
            bail!(
                "config path {} must be {expected} build output for its exact semantics",
                config.path()
            );
        }
        let semantics = ConfigSemanticsV3 {
            noreplace: config.noreplace(),
            ghost: config.ghost(),
            remove_on_upgrade: config.remove_on_upgrade(),
        };
        if authority
            .insert(config.path().to_string(), semantics)
            .is_some()
        {
            bail!("config path {} is declared more than once", config.path());
        }
    }

    Ok(authority)
}

fn file_capability_authority_for_manifest(
    manifest: &crate::ccs::manifest::CcsManifest,
    build: &BuildResult,
) -> Result<Vec<crate::ccs::manifest::FileCapability>> {
    let canonical =
        super::file_capabilities::canonicalize_file_capabilities(&manifest.file_capabilities)?;
    let build_files = build
        .files
        .iter()
        .map(|file| (file.path.as_str(), &file.node.kind))
        .collect::<BTreeMap<_, _>>();
    for capability in &canonical {
        let Some(kind) = build_files.get(capability.path.as_str()) else {
            bail!(
                "file capability target {} is absent from signed build output",
                capability.path
            );
        };
        if !matches!(kind, crate::payload::PayloadNodeKind::Regular { .. }) {
            bail!(
                "file capability target {} is not a regular signed build output file",
                capability.path
            );
        }
    }
    Ok(canonical)
}

fn select_default_component(build: &BuildResult) -> Result<String> {
    let manifest_defaults = &build.manifest.components.default;
    if manifest_defaults.len() != 1 || manifest_defaults[0].trim().is_empty() {
        bail!("v3 package authoring requires exactly one named default component");
    }
    let default_component = manifest_defaults[0].clone();
    if !build.components.is_empty() && !build.components.contains_key(&default_component) {
        bail!("v3 package default component '{default_component}' is not present in build output");
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
