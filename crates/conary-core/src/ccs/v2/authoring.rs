// conary-core/src/ccs/v2/authoring.rs

use super::schema::*;
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::v2::PackageKindTagV2;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthoringFindingBucket {
    Contract,
    PublicationReadiness,
    ProfileDeferred,
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
    if manifest
        .package
        .release
        .as_deref()
        .is_none_or(str::is_empty)
    {
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
    if manifest.package.kind.is_none() {
        findings.push(AuthoringFinding {
            code: "m4b-missing-kind",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.kind"),
            message: "v2 package authoring requires package.kind".to_string(),
            suggestion: "add kind = \"package\" under [package]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if manifest.hooks.has_script_hooks()
        || manifest.hooks.has_service_hooks()
        || manifest.hooks.has_declarative_hooks()
    {
        findings.push(AuthoringFinding {
            code: "m4b-profile-deferred-lifecycle",
            bucket: AuthoringFindingBucket::ProfileDeferred,
            severity: AuthoringFindingSeverity::Warning,
            field: Some("hooks"),
            message: "lifecycle declarations need M4d target-profile facts before v2 build"
                .to_string(),
            suggestion: "remove lifecycle declarations for the M4b minimal-file path",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if !manifest.requires.packages.is_empty() || !manifest.requires.capabilities.is_empty() {
        findings.push(AuthoringFinding {
            code: "m4b-profile-deferred-dependencies",
            bucket: AuthoringFindingBucket::ProfileDeferred,
            severity: AuthoringFindingSeverity::Warning,
            field: Some("requires"),
            message: "dependencies need database/profile support before v2 build".to_string(),
            suggestion: "remove [requires] entries for the M4b minimal-file path",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    // PublicationReadiness and Style buckets are part of the stable diagnostic
    // shape, but M4b's first implementation only emits concrete
    // contract/profile-deferred findings.
    findings
}

#[derive(Debug)]
pub struct V2AuthoringInput<'a> {
    pub build: &'a BuildResult,
    pub local_dev: bool,
    pub debug_toml: Option<String>,
}

#[derive(Debug)]
pub struct ProjectedV2Package {
    pub authority: AuthorityDocumentV2,
    pub payloads_by_path: BTreeMap<String, Vec<u8>>,
    pub debug_toml: Option<String>,
}

pub fn project_build_result_to_v2(input: V2AuthoringInput<'_>) -> Result<ProjectedV2Package> {
    let package = &input.build.manifest.package;
    let release = package
        .release
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("v2 package authoring requires package.release")?;
    let kind = package
        .kind
        .context("v2 package authoring requires package.kind")?;
    if kind != PackageKindTagV2::Package {
        bail!("M4b only supports package authoring for v2 build");
    }

    let payloads_by_path = payloads_by_path(input.build)?;
    let files = input
        .build
        .files
        .iter()
        .map(|file| FileAuthorityV2 {
            path: file.path.clone(),
            sha256: file.hash.clone(),
            size: file.size,
            file_type: match file.file_type {
                FileType::Regular => FileTypeV2::Regular,
                FileType::Symlink => FileTypeV2::Symlink,
                FileType::Directory => FileTypeV2::Directory,
            },
            mode: file.mode,
            owner: "root".to_string(),
            group: "root".to_string(),
            component: file.component.clone(),
            symlink_target: file.target.clone(),
            config: None,
            conflict: ConflictPolicyV2::Error,
        })
        .collect::<Vec<_>>();

    let default_component = select_default_component(input.build)?;
    let components = input
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

    let build_input_identity =
        crate::hash::sha256(format!("{}:{}:{}", package.name, package.version, release).as_bytes());
    let evidence_hash = crate::hash::sha256(
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
    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: package.name.clone(),
            version: package.version.clone(),
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
            config: Vec::new(),
            policy: PackagePolicyV2::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components,
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("host".to_string()),
            build_input_identity: Some(build_input_identity),
            hermetic_evidence_hash: Some(evidence_hash),
            foreign_conversion_boundary_hash: None,
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

fn select_default_component(build: &BuildResult) -> Result<String> {
    let manifest_defaults = build
        .manifest
        .components
        .default
        .iter()
        .filter(|name| build.components.contains_key(name.as_str()))
        .collect::<Vec<_>>();

    if let Some(name) = manifest_defaults.first() {
        return Ok((*name).clone());
    }
    if build.components.len() == 1 {
        return Ok(build
            .components
            .keys()
            .next()
            .expect("one component")
            .clone());
    }
    bail!("v2 package authoring requires one default component present in build output");
}

fn payloads_by_path(build: &BuildResult) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut payloads = BTreeMap::new();
    for file in &build.files {
        if file.file_type != FileType::Regular {
            continue;
        }
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
                    .get(&file.hash)
                    .with_context(|| format!("missing payload blob for {}", file.path))?
                    .clone()
            };
        if crate::hash::sha256(&bytes) != file.hash {
            bail!("payload bytes for {} do not match builder hash", file.path);
        }
        payloads.insert(file.path.clone(), bytes);
    }
    Ok(payloads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::test_support;

    #[test]
    fn projection_requires_release_and_kind_for_v2_package_authoring() {
        let build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");

        let error = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: None,
        })
        .unwrap_err();

        assert!(
            error.to_string().contains("release"),
            "expected release diagnostic, got {error}"
        );
    }

    #[test]
    fn projection_builds_complete_local_dev_package_authority() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
        })
        .unwrap();

        assert_eq!(projected.authority.identity.name, "hello");
        assert_eq!(projected.authority.identity.release, "1");
        assert_eq!(
            projected.authority.provenance.hardening_level.as_deref(),
            Some("host")
        );
        assert!(projected.authority.components["runtime"].default);
        assert!(projected.payloads_by_path.contains_key("/hello"));
    }

    #[test]
    fn projection_keeps_host_hardening_for_release_key_signing_path() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: false,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
        })
        .unwrap();

        assert_eq!(
            projected.authority.provenance.hardening_level.as_deref(),
            Some("host")
        );
    }

    #[test]
    fn lint_manifest_reports_missing_release_and_kind() {
        let manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        let findings = lint_manifest_for_v2_authoring(&manifest);

        assert!(findings.iter().any(|f| f.field == Some("package.release")));
        assert!(findings.iter().any(|f| f.field == Some("package.kind")));
        assert!(findings.iter().all(|f| f.blocks_build));
    }

    #[test]
    fn lint_manifest_marks_lifecycle_as_profile_deferred() {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.release = Some("1".to_string());
        manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        manifest.hooks.services.push(crate::ccs::manifest::Service {
            name: "hello.service".to_string(),
            action: crate::ccs::manifest::ServiceAction::Restart,
            reversible: None,
        });

        let findings = lint_manifest_for_v2_authoring(&manifest);
        assert!(
            findings
                .iter()
                .any(|f| { f.bucket == AuthoringFindingBucket::ProfileDeferred && f.blocks_build })
        );
    }

    #[test]
    fn lint_manifest_blocks_unresolved_dependencies_for_m4b() {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.release = Some("1".to_string());
        manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        manifest
            .requires
            .packages
            .push(crate::ccs::manifest::PackageDep {
                name: "openssl".to_string(),
                version: Some(">=3.0".to_string()),
            });

        let findings = lint_manifest_for_v2_authoring(&manifest);
        assert!(
            findings
                .iter()
                .any(|f| { f.code == "m4b-profile-deferred-dependencies" && f.blocks_build })
        );
    }
}
