// conary-core/src/ccs/v2/authoring.rs

use super::schema::*;
use crate::ccs::builder::BuildResult;
use crate::ccs::v2::PackageKindTagV2;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use anyhow::{Context, Result, bail};
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
    pub payloads_by_path: BTreeMap<String, Vec<u8>>,
    pub debug_toml: Option<String>,
}

pub fn project_build_result_to_v2(input: V2AuthoringInput<'_>) -> Result<ProjectedV2Package> {
    let package = &input.build.manifest.package;
    let release = package.release.as_str();
    let kind = package.kind;
    if kind != PackageKindTagV2::Package {
        bail!("M4b only supports package authoring for v2 build");
    }

    let payloads_by_path = payloads_by_path(input.build)?;
    let config_policies = config_policies_for_manifest(&input.build.manifest, input.build)?;
    let files = input
        .build
        .files
        .iter()
        .map(|file| FileAuthorityV2 {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content.clone(),
            component: file.component.clone(),
            config: config_policies.get(&file.path).copied(),
            conflict: ConflictPolicyV2::Error,
        })
        .collect::<Vec<_>>();
    let config = config_policies
        .iter()
        .map(|(path, policy)| ConfigAuthorityV2 {
            path: path.clone(),
            policy: *policy,
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
    Ok(ProjectedV2Package {
        authority,
        payloads_by_path,
        debug_toml: input.debug_toml,
    })
}

fn project_provides(manifest: &crate::ccs::manifest::CcsManifest) -> Vec<DependencyEntryV2> {
    let mut entries = Vec::new();
    entries.extend(
        manifest
            .provides
            .capabilities
            .iter()
            .map(|name| dependency_entry(DependencyKindV2::Capability, name, None)),
    );
    entries.extend(
        manifest
            .provides
            .sonames
            .iter()
            .map(|name| dependency_entry(DependencyKindV2::Soname, name, None)),
    );
    entries.extend(
        manifest
            .provides
            .binaries
            .iter()
            .map(|name| dependency_entry(DependencyKindV2::Binary, name, None)),
    );
    entries.extend(
        manifest
            .provides
            .pkgconfig
            .iter()
            .map(|name| dependency_entry(DependencyKindV2::PkgConfig, name, None)),
    );
    sort_and_deduplicate_dependencies(entries)
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

fn dependency_entry(
    kind: DependencyKindV2,
    name: &str,
    version_constraint: Option<&str>,
) -> DependencyEntryV2 {
    DependencyEntryV2 {
        kind,
        name: name.to_string(),
        version_constraint: version_constraint.map(str::to_string),
        target: None,
        component: None,
    }
}

fn sort_and_deduplicate_dependencies(entries: Vec<DependencyEntryV2>) -> Vec<DependencyEntryV2> {
    let mut keyed = BTreeMap::new();
    for entry in entries {
        let key = (
            dependency_kind_order(entry.kind),
            entry.name.clone(),
            entry.version_constraint.clone(),
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

fn config_policies_for_manifest(
    manifest: &crate::ccs::manifest::CcsManifest,
    build: &BuildResult,
) -> Result<BTreeMap<String, ConfigPolicyV2>> {
    let policy = config_policy_for_manifest(manifest);
    let build_paths = build
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut policies = BTreeMap::new();

    for path in &manifest.config.files {
        if !path.starts_with('/') || !build_paths.contains(path.as_str()) {
            bail!("config path {path} must be absolute and present in build output");
        }
        policies.insert(path.clone(), policy);
    }

    Ok(policies)
}

fn config_policy_for_manifest(manifest: &crate::ccs::manifest::CcsManifest) -> ConfigPolicyV2 {
    if manifest.config.noreplace {
        ConfigPolicyV2::NoReplace
    } else {
        ConfigPolicyV2::Replace
    }
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

fn payloads_by_path(build: &BuildResult) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut payloads = BTreeMap::new();
    for file in &build.files {
        if !file.node.kind.is_regular() {
            continue;
        }
        let content = file.content.as_ref().with_context(|| {
            format!(
                "regular payload node {} has no content authority",
                file.path
            )
        })?;
        let bytes =
            if let Some(chunks) = &file.chunks {
                let mut bytes = Vec::new();
                for chunk_hash in chunks {
                    bytes.extend(build.blobs.get(chunk_hash).with_context(|| {
                        format!("missing chunk {chunk_hash} for {}", file.path)
                    })?);
                }
                bytes
            } else {
                build
                    .blobs
                    .get(&content.sha256)
                    .with_context(|| format!("missing payload blob for {}", file.path))?
                    .clone()
            };
        if crate::hash::sha256(&bytes) != content.sha256 || bytes.len() as u64 != content.size {
            bail!("payload bytes for {} do not match builder hash", file.path);
        }
        payloads.insert(file.path.clone(), bytes);
    }
    Ok(payloads)
}

#[cfg(test)]
#[path = "authoring/tests.rs"]
mod tests;
