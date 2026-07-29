// conary-core/src/ccs/package/v2_projection.rs

//! Structural projection of native CCS v2 authority into package-install data.

use crate::ccs::attestation::{BuildAttestationEnvelope, ForeignConversionBoundary};
use crate::ccs::builder::FileEntry;
use crate::ccs::manifest::CcsManifest;
use crate::ccs::v2::AuthorityDocumentV2;
use crate::ccs::v2::schema::{DependencyKindV2, PackageKindV2};
use crate::error::{Error, Result};
use crate::packages::traits::{ConfigFileInfo, ProvidedCapability};
use crate::repository::dependency_model::RepositoryCapabilityKind;

pub(super) fn install_manifest_from_v2(
    authority: &AuthorityDocumentV2,
    build_attestation: Option<BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<ForeignConversionBoundary>,
) -> Result<CcsManifest> {
    let mut manifest = crate::ccs::v2::project_manifest_identity(
        authority,
        format!("CCS v2 {}", authority.identity.name),
    );

    manifest.requirements = authority.requirements.clone();
    manifest.relations = authority.relations.clone();
    manifest.capabilities = authority.capabilities.clone();
    manifest.file_capabilities = authority.file_capabilities.clone();
    manifest.config.files = config_files_from_v2_authority(authority)?;

    crate::ccs::v2::lifecycle::apply_authority_to_manifest(&authority.lifecycle, &mut manifest)
        .map_err(Error::ParseError)?;

    let provenance = manifest.provenance.get_or_insert_with(Default::default);
    provenance.origin_class = authority.provenance.origin_class.clone();
    provenance.hardening_level = authority.provenance.hardening_level.clone();
    provenance.build_attestation = build_attestation;
    provenance.foreign_conversion_boundary = foreign_conversion_boundary;
    Ok(manifest)
}

pub(super) fn files_from_v2_authority(authority: &AuthorityDocumentV2) -> Result<Vec<FileEntry>> {
    let PackageKindV2::Package(data) = &authority.kind else {
        return Err(Error::ParseError(
            "group and redirect v2 packages are not installable in M4a".to_string(),
        ));
    };

    Ok(data
        .files
        .iter()
        .map(|file| FileEntry {
            path: file.path.clone(),
            node: file.node.clone(),
            content: file.content.clone(),
            component: file.component.clone(),
            chunks: None,
        })
        .collect())
}

pub(super) fn config_files_from_v2_authority(
    authority: &AuthorityDocumentV2,
) -> Result<Vec<ConfigFileInfo>> {
    let PackageKindV2::Package(data) = &authority.kind else {
        return Err(Error::ParseError(
            "group and redirect v2 packages do not carry config authority".to_string(),
        ));
    };

    Ok(data
        .config
        .iter()
        .map(|config| ConfigFileInfo {
            path: config.path.clone(),
            noreplace: config.semantics.noreplace,
            ghost: config.semantics.ghost,
            remove_on_upgrade: config.semantics.remove_on_upgrade,
        })
        .collect())
}

pub(super) fn provides_from_v2_authority(
    authority: &AuthorityDocumentV2,
) -> Vec<ProvidedCapability> {
    authority
        .provides
        .iter()
        .map(|provide| ProvidedCapability {
            kind: capability_kind(provide.kind),
            name: provide.name.clone(),
            version: provide.provider_version.clone(),
            version_relation: provide.version_relation,
            version_scheme: provide.version_scheme,
            architecture_qualifier: provide.architecture_qualifier.clone(),
        })
        .collect()
}

const fn capability_kind(kind: DependencyKindV2) -> RepositoryCapabilityKind {
    match kind {
        DependencyKindV2::Package => RepositoryCapabilityKind::PackageName,
        DependencyKindV2::Capability => RepositoryCapabilityKind::Virtual,
        DependencyKindV2::File => RepositoryCapabilityKind::File,
        DependencyKindV2::Path => RepositoryCapabilityKind::Path,
        DependencyKindV2::Binary => RepositoryCapabilityKind::Binary,
        DependencyKindV2::Soname => RepositoryCapabilityKind::Soname,
        DependencyKindV2::PkgConfig => RepositoryCapabilityKind::PkgConfig,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v2::schema::{
        LifecycleDirectoryV2, LifecycleScriptExecutionV2, LifecycleScriptV2,
    };

    #[test]
    fn verified_v2_lifecycle_is_visible_to_install_hook_execution() {
        let mut authority = AuthorityDocumentV2::package_for_tests("lifecycle-install");
        authority.lifecycle.directories.push(LifecycleDirectoryV2 {
            path: "/var/lib/lifecycle-install".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: Some(true),
        });
        authority.lifecycle.post_install = Some(LifecycleScriptV2 {
            interpreter: "/bin/sh".to_string(),
            body: "printf installed".to_string(),
            capabilities: Vec::new(),
            reversible: Some(false),
            execution: LifecycleScriptExecutionV2::SandboxedTargetRoot,
        });

        let manifest = install_manifest_from_v2(&authority, None, None).unwrap();
        assert_eq!(
            manifest.hooks.post_install.as_ref().unwrap().script,
            "printf installed"
        );

        let root = tempfile::tempdir().unwrap();
        let mut executor = crate::ccs::HookExecutor::new(root.path(), Default::default());
        executor.execute_pre_hooks(&manifest.hooks).unwrap();
        assert!(root.path().join("var/lib/lifecycle-install").is_dir());
    }

    #[test]
    fn verified_v2_identity_is_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV2::package_for_tests("identity-projection");
        authority.identity.release = "7".to_string();
        authority.identity.architecture = Some("aarch64".to_string());
        authority.identity.platform = Some("linux".to_string());

        let manifest = install_manifest_from_v2(&authority, None, None).unwrap();

        assert_eq!(manifest.package.release, "7");
        assert_eq!(manifest.package.kind, authority.identity.kind);
        let platform = manifest.package.platform.as_ref().unwrap();
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.arch.as_deref(), Some("aarch64"));
    }

    #[test]
    fn verified_v2_package_capabilities_are_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV2::package_for_tests("capability-projection");
        let declaration = crate::capability::CapabilityDeclaration {
            rationale: Some("needs repository access".to_string()),
            network: crate::capability::NetworkCapabilities {
                connect_tcp: vec![443],
                ..Default::default()
            },
            ..Default::default()
        };
        authority.capabilities = Some(declaration.clone());

        let manifest = install_manifest_from_v2(&authority, None, None).unwrap();

        assert_eq!(manifest.capabilities, Some(declaration));
    }

    #[test]
    fn verified_v2_file_capabilities_are_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV2::package_for_tests("file-capability-projection");
        let declaration = crate::ccs::manifest::FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        };
        authority.file_capabilities = vec![declaration.clone()];

        let manifest = install_manifest_from_v2(&authority, None, None).unwrap();

        assert_eq!(manifest.file_capabilities, vec![declaration]);
    }
}
