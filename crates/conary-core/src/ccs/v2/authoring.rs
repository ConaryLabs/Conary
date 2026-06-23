// conary-core/src/ccs/v2/authoring.rs

use super::schema::*;
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::v2::PackageKindTagV2;
use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};
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

#[derive(Clone, Copy)]
pub struct V2AuthoringTargetProfile<'a> {
    pub public_id: &'a str,
    pub query: &'a dyn TargetProfileQuery,
}

impl std::fmt::Debug for V2AuthoringTargetProfile<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("V2AuthoringTargetProfile")
            .field("public_id", &self.public_id)
            .finish()
    }
}

pub fn lint_manifest_for_v2_authoring(
    manifest: &crate::ccs::manifest::CcsManifest,
    target_profile: Option<V2AuthoringTargetProfile<'_>>,
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
        match target_profile {
            Some(profile) => lint_lifecycle_with_profile(manifest, profile, &mut findings),
            None => findings.push(AuthoringFinding {
                code: "m4e-target-profile-required",
                bucket: AuthoringFindingBucket::Profile,
                severity: AuthoringFindingSeverity::Error,
                field: Some("hooks"),
                message: "lifecycle declarations require --target-profile for v2 authoring"
                    .to_string(),
                suggestion: "pass --target-profile fedora-44, --target-profile ubuntu-26.04, or --target-profile arch",
                blocks_build: true,
                blocks_local_test: true,
                blocks_publish: true,
            }),
        }
    }
    if !manifest.requires.packages.is_empty() || !manifest.requires.capabilities.is_empty() {
        findings.push(AuthoringFinding {
            code: "m4e-dependencies-unsupported",
            bucket: AuthoringFindingBucket::Profile,
            severity: AuthoringFindingSeverity::Error,
            field: Some("requires"),
            message: "v2 dependency authoring is not supported yet".to_string(),
            suggestion: "remove [requires] entries until v2 dependency authoring is implemented",
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
    pub target_profile: Option<V2AuthoringTargetProfile<'a>>,
}

impl std::fmt::Debug for V2AuthoringInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("V2AuthoringInput")
            .field("local_dev", &self.local_dev)
            .field("debug_toml", &self.debug_toml.as_ref().map(|_| "<toml>"))
            .field(
                "target_profile",
                &self.target_profile.map(|profile| profile.public_id),
            )
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
    let config_policies = config_policies_for_manifest(&input.build.manifest, input.build)?;
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
    let lifecycle = project_lifecycle(&input.build.manifest);
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
            config,
            policy: PackagePolicyV2::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components,
        lifecycle,
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

    if lifecycle_is_empty(&authority.lifecycle) {
        super::validation::validate_authority(&authority)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    } else {
        let profile = input
            .target_profile
            .context("m4e-target-profile-required: lifecycle authority requires target profile")?;
        super::validation::validate_authority_with_profile(&authority, profile.query)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    Ok(ProjectedV2Package {
        authority,
        payloads_by_path,
        debug_toml: input.debug_toml,
    })
}

fn project_lifecycle(manifest: &crate::ccs::manifest::CcsManifest) -> LifecycleAuthorityV2 {
    LifecycleAuthorityV2 {
        services: manifest
            .hooks
            .services
            .iter()
            .map(|service| service.name.clone())
            .chain(
                manifest
                    .hooks
                    .systemd
                    .iter()
                    .map(|service| service.unit.clone()),
            )
            .collect(),
        tmpfiles: manifest
            .hooks
            .tmpfiles
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        sysctl: manifest
            .hooks
            .sysctl
            .iter()
            .map(|entry| entry.key.clone())
            .collect(),
        users: manifest
            .hooks
            .users
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        groups: manifest
            .hooks
            .groups
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        directories: manifest
            .hooks
            .directories
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        alternatives: manifest
            .hooks
            .alternatives
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
    }
}

fn lifecycle_is_empty(lifecycle: &LifecycleAuthorityV2) -> bool {
    lifecycle.users.is_empty()
        && lifecycle.groups.is_empty()
        && lifecycle.directories.is_empty()
        && lifecycle.services.is_empty()
        && lifecycle.tmpfiles.is_empty()
        && lifecycle.sysctl.is_empty()
        && lifecycle.alternatives.is_empty()
}

fn lint_lifecycle_with_profile(
    manifest: &crate::ccs::manifest::CcsManifest,
    profile: V2AuthoringTargetProfile<'_>,
    findings: &mut Vec<AuthoringFinding>,
) {
    if manifest.hooks.has_script_hooks() {
        push_lifecycle_unsupported(
            findings,
            "hooks",
            format!(
                "script lifecycle hooks are not supported for target profile {}",
                profile.public_id
            ),
        );
    }

    for service in &manifest.hooks.services {
        if profile.query.service_status(&service.name) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.services",
                format!(
                    "service {} is not supported by target profile {}",
                    service.name, profile.public_id
                ),
            );
        }
    }

    for unit in &manifest.hooks.systemd {
        if profile.query.service_status(&unit.unit) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.systemd",
                format!(
                    "systemd unit {} is not supported by target profile {}",
                    unit.unit, profile.public_id
                ),
            );
        }
    }

    for entry in &manifest.hooks.tmpfiles {
        if profile.query.tmpfiles_status(&entry.path) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.tmpfiles",
                format!(
                    "tmpfiles entry {} is not supported by target profile {}",
                    entry.path, profile.public_id
                ),
            );
        }
    }

    for entry in &manifest.hooks.sysctl {
        if profile.query.sysctl_status(&entry.key) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.sysctl",
                format!(
                    "sysctl key {} is not supported by target profile {}",
                    entry.key, profile.public_id
                ),
            );
        }
    }

    for user in &manifest.hooks.users {
        if profile.query.user_status(&user.name) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.users",
                format!(
                    "user {} is not supported by target profile {}",
                    user.name, profile.public_id
                ),
            );
        }
    }

    for group in &manifest.hooks.groups {
        if profile.query.group_status(&group.name) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.groups",
                format!(
                    "group {} is not supported by target profile {}",
                    group.name, profile.public_id
                ),
            );
        }
    }

    for directory in &manifest.hooks.directories {
        if profile.query.directory_status(&directory.path) == ProfileConstraintStatus::Unsupported {
            push_lifecycle_unsupported(
                findings,
                "hooks.directories",
                format!(
                    "directory {} is not supported by target profile {}",
                    directory.path, profile.public_id
                ),
            );
        }
    }

    for alternative in &manifest.hooks.alternatives {
        if profile.query.alternative_status(&alternative.name)
            == ProfileConstraintStatus::Unsupported
        {
            push_lifecycle_unsupported(
                findings,
                "hooks.alternatives",
                format!(
                    "alternative {} is not supported by target profile {}",
                    alternative.name, profile.public_id
                ),
            );
        }
    }
}

fn push_lifecycle_unsupported(
    findings: &mut Vec<AuthoringFinding>,
    field: &'static str,
    message: String,
) {
    findings.push(AuthoringFinding {
        code: "m4e-lifecycle-unsupported",
        bucket: AuthoringFindingBucket::Profile,
        severity: AuthoringFindingSeverity::Error,
        field: Some(field),
        message,
        suggestion: "remove the lifecycle declaration or choose a target profile that supports it",
        blocks_build: true,
        blocks_local_test: true,
        blocks_publish: true,
    });
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
    use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

    struct FakeTargetProfile {
        accepted_services: Vec<&'static str>,
    }

    impl FakeTargetProfile {
        fn accepting_services(accepted_services: Vec<&'static str>) -> Self {
            Self { accepted_services }
        }
    }

    impl TargetProfileQuery for FakeTargetProfile {
        fn service_status(&self, service: &str) -> ProfileConstraintStatus {
            if self.accepted_services.iter().any(|entry| entry == &service) {
                ProfileConstraintStatus::Accepted
            } else {
                ProfileConstraintStatus::Unsupported
            }
        }

        fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn user_status(&self, _user: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn group_status(&self, _group: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }

        fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Unsupported
        }
    }

    struct AcceptAllProfile;

    impl TargetProfileQuery for AcceptAllProfile {
        fn service_status(&self, _service: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn tmpfiles_status(&self, _entry: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn user_status(&self, _user: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn group_status(&self, _group: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn directory_status(&self, _directory: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }

        fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus {
            ProfileConstraintStatus::Accepted
        }
    }

    #[test]
    fn projection_requires_release_and_kind_for_v2_package_authoring() {
        let build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");

        let error = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: None,
            target_profile: None,
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
            target_profile: None,
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
    fn projection_reconstructs_payload_from_chunk_blobs() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);

        let chunks = [b"hel".to_vec(), b"lo\n".to_vec()];
        let chunk_hashes = chunks
            .iter()
            .map(|chunk| crate::hash::sha256(chunk))
            .collect::<Vec<_>>();
        build.blobs.clear();
        for (hash, bytes) in chunk_hashes.iter().zip(chunks) {
            build.blobs.insert(hash.clone(), bytes);
        }
        build.files[0].chunks = Some(chunk_hashes.clone());
        build.components.get_mut("runtime").unwrap().files[0].chunks = Some(chunk_hashes);
        build.chunked = true;

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: None,
        })
        .unwrap();

        assert_eq!(projected.payloads_by_path["/hello"], b"hello\n");
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
            target_profile: None,
        })
        .unwrap();

        assert_eq!(
            projected.authority.provenance.hardening_level.as_deref(),
            Some("host")
        );
    }

    #[test]
    fn projection_marks_noreplace_config_files() {
        let mut build = test_support::single_file_build_result_at(
            "demo",
            "0.1.0",
            "/etc/conary-example/config.toml",
            b"message = \"hello\"\n",
        );
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];
        build.manifest.config.noreplace = true;

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: None,
        })
        .unwrap();

        let package = match &projected.authority.kind {
            PackageKindV2::Package(package) => package,
            other => panic!("expected package authority, got {other:?}"),
        };
        assert_eq!(package.files[0].config, Some(ConfigPolicyV2::NoReplace));
        assert_eq!(package.config[0].path, "/etc/conary-example/config.toml");
        assert_eq!(package.config[0].policy, ConfigPolicyV2::NoReplace);
    }

    #[test]
    fn projection_marks_replace_config_when_noreplace_is_false() {
        let mut build = test_support::single_file_build_result_at(
            "demo",
            "0.1.0",
            "/etc/conary-example/config.toml",
            b"message = \"hello\"\n",
        );
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];
        build.manifest.config.noreplace = false;

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: None,
        })
        .unwrap();

        let package = match &projected.authority.kind {
            PackageKindV2::Package(package) => package,
            other => panic!("expected package authority, got {other:?}"),
        };
        assert_eq!(package.files[0].config, Some(ConfigPolicyV2::Replace));
        assert_eq!(package.config[0].policy, ConfigPolicyV2::Replace);
    }

    #[test]
    fn projection_rejects_config_path_absent_from_payload() {
        let mut build = test_support::minimal_file_build_result("demo", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];

        let error = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: None,
        })
        .unwrap_err();

        assert!(error.to_string().contains("config path"));
    }

    #[test]
    fn projection_rejects_relative_config_paths() {
        let mut build = test_support::single_file_build_result_at(
            "demo",
            "0.1.0",
            "etc/conary-example/config.toml",
            b"message = \"hello\"\n",
        );
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        build.manifest.config.files = vec!["etc/conary-example/config.toml".to_string()];

        let error = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: None,
        })
        .unwrap_err();

        assert!(error.to_string().contains("config path"));
        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn lint_manifest_reports_missing_release_and_kind() {
        let manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        let findings = lint_manifest_for_v2_authoring(&manifest, None);

        assert!(findings.iter().any(|f| f.field == Some("package.release")));
        assert!(findings.iter().any(|f| f.field == Some("package.kind")));
        assert!(findings.iter().all(|f| f.blocks_build));
    }

    #[test]
    fn lint_manifest_requires_target_profile_for_lifecycle() {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.release = Some("1".to_string());
        manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        manifest.hooks.services.push(crate::ccs::manifest::Service {
            name: "hello.service".to_string(),
            action: crate::ccs::manifest::ServiceAction::Restart,
            reversible: None,
        });

        let findings = lint_manifest_for_v2_authoring(&manifest, None);
        assert!(
            findings
                .iter()
                .any(|f| { f.code == "m4e-target-profile-required" && f.blocks_build })
        );
    }

    #[test]
    fn lint_manifest_allows_lifecycle_with_matching_target_profile() {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.release = Some("1".to_string());
        manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        manifest.hooks.services.push(crate::ccs::manifest::Service {
            name: "hello.service".to_string(),
            action: crate::ccs::manifest::ServiceAction::Restart,
            reversible: None,
        });
        let profile = FakeTargetProfile::accepting_services(vec!["hello.service"]);

        let findings = lint_manifest_for_v2_authoring(
            &manifest,
            Some(V2AuthoringTargetProfile {
                public_id: "test-profile",
                query: &profile as &dyn TargetProfileQuery,
            }),
        );

        assert!(findings.iter().all(|finding| !finding.blocks_build));
        assert!(
            findings
                .iter()
                .all(|finding| finding.code != "m4e-target-profile-required")
        );
    }

    #[test]
    fn lint_manifest_reports_unsupported_lifecycle_for_target_profile() {
        let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
        manifest.package.release = Some("1".to_string());
        manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        manifest.hooks.services.push(crate::ccs::manifest::Service {
            name: "other.service".to_string(),
            action: crate::ccs::manifest::ServiceAction::Restart,
            reversible: None,
        });
        let profile = FakeTargetProfile::accepting_services(vec!["hello.service"]);

        let findings = lint_manifest_for_v2_authoring(
            &manifest,
            Some(V2AuthoringTargetProfile {
                public_id: "test-profile",
                query: &profile as &dyn TargetProfileQuery,
            }),
        );

        assert!(findings.iter().any(|finding| {
            finding.code == "m4e-lifecycle-unsupported"
                && finding.message.contains("other.service")
                && finding.blocks_build
        }));
    }

    #[test]
    fn projection_input_accepts_trait_object_target_profile() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        let profile = FakeTargetProfile::accepting_services(vec!["hello.service"]);

        project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: Some(V2AuthoringTargetProfile {
                public_id: "test-profile",
                query: &profile as &dyn TargetProfileQuery,
            }),
        })
        .unwrap();
    }

    #[test]
    fn projection_maps_declarative_lifecycle_hooks_to_signed_authority() {
        let mut build = test_support::single_file_build_result_at(
            "conary-example",
            "0.1.0",
            "/usr/bin/conary-example",
            b"#!/bin/sh\necho conary-example\n",
        );
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
        build
            .manifest
            .hooks
            .services
            .push(crate::ccs::manifest::Service {
                name: "conary-example.service".to_string(),
                action: crate::ccs::manifest::ServiceAction::Restart,
                reversible: None,
            });
        build
            .manifest
            .hooks
            .tmpfiles
            .push(crate::ccs::manifest::TmpfilesHook {
                entry_type: "d".to_string(),
                path: "/var/lib/conary-example".to_string(),
                mode: "0755".to_string(),
                owner: "conary-example".to_string(),
                group: "conary-example".to_string(),
                reversible: None,
            });
        build
            .manifest
            .hooks
            .users
            .push(crate::ccs::manifest::UserHook {
                name: "conary-example".to_string(),
                system: true,
                home: None,
                shell: None,
                group: Some("conary-example".to_string()),
                reversible: None,
            });
        build
            .manifest
            .hooks
            .groups
            .push(crate::ccs::manifest::GroupHook {
                name: "conary-example".to_string(),
                system: true,
                reversible: None,
            });
        build
            .manifest
            .hooks
            .directories
            .push(crate::ccs::manifest::DirectoryHook {
                path: "/var/lib/conary-example".to_string(),
                mode: "0755".to_string(),
                owner: "conary-example".to_string(),
                group: "conary-example".to_string(),
                cleanup: None,
                reversible: None,
            });
        let profile = AcceptAllProfile;

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
            target_profile: Some(V2AuthoringTargetProfile {
                public_id: "accept-all",
                query: &profile as &dyn TargetProfileQuery,
            }),
        })
        .unwrap();

        assert_eq!(
            projected.authority.lifecycle.services,
            vec!["conary-example.service"]
        );
        assert_eq!(
            projected.authority.lifecycle.tmpfiles,
            vec!["/var/lib/conary-example"]
        );
        assert_eq!(projected.authority.lifecycle.users, vec!["conary-example"]);
        assert_eq!(projected.authority.lifecycle.groups, vec!["conary-example"]);
        assert_eq!(
            projected.authority.lifecycle.directories,
            vec!["/var/lib/conary-example"]
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

        let findings = lint_manifest_for_v2_authoring(&manifest, None);
        assert!(
            findings
                .iter()
                .any(|f| { f.code == "m4e-dependencies-unsupported" && f.blocks_build })
        );
    }
}
