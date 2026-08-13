// conary-core/src/ccs/package/v3_projection.rs

//! Structural projection of native CCS v3 authority into package-install data.

use crate::ccs::attestation::{BuildAttestationEnvelope, ForeignConversionBoundary};
use crate::ccs::builder::FileEntry;
use crate::ccs::manifest::CcsManifest;
use crate::ccs::v3::AuthorityDocumentV3;
use crate::ccs::v3::schema::{DependencyKindV3, PackageKindV3};
use crate::error::{Error, Result};
use crate::packages::config_authority::SourceConfigDeclaration;
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation, ProvidedCapability,
    RepositoryCapabilityKind,
};

pub(super) fn install_manifest_from_v3(
    authority: &AuthorityDocumentV3,
    build_attestation: Option<BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<ForeignConversionBoundary>,
) -> Result<CcsManifest> {
    let mut manifest = crate::ccs::v3::project_manifest_identity(
        authority,
        format!("CCS v3 {}", authority.identity.name),
    );

    manifest.requirements = authority.requirements.clone();
    manifest.relations = authority.relations.clone();
    manifest.capabilities = authority.execution_capabilities.clone();
    manifest.file_capabilities = authority.file_capabilities.clone();
    manifest.config.files = config_declarations_from_v3_authority(authority)?;

    crate::ccs::v3::lifecycle::apply_authority_to_manifest(&authority.lifecycle, &mut manifest)
        .map_err(Error::ParseError)?;

    let provenance = manifest.provenance.get_or_insert_with(Default::default);
    provenance.origin_class = authority.provenance.origin_class.clone();
    provenance.hardening_level = authority.provenance.hardening_level.clone();
    provenance.build_attestation = build_attestation;
    provenance.foreign_conversion_boundary = foreign_conversion_boundary;
    Ok(manifest)
}

pub(super) fn files_from_v3_authority(authority: &AuthorityDocumentV3) -> Result<Vec<FileEntry>> {
    let PackageKindV3::Package(data) = &authority.kind else {
        return Err(Error::ParseError(
            "group and redirect v3 packages are not installable in M4a".to_string(),
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

pub(super) fn config_declarations_from_v3_authority(
    authority: &AuthorityDocumentV3,
) -> Result<Vec<SourceConfigDeclaration>> {
    let PackageKindV3::Package(data) = &authority.kind else {
        return Err(Error::ParseError(
            "group and redirect v3 packages do not carry config authority".to_string(),
        ));
    };

    Ok(data.config.clone())
}

pub(super) fn capabilities_from_v3_authority(
    authority: &AuthorityDocumentV3,
) -> Vec<ProvidedCapability> {
    let mut capabilities = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: authority.identity.name.clone(),
        version: Some(authority.identity.version.clone()),
        version_relation: Some(ProvideVersionRelation::Equal),
        version_scheme: authority.identity.version_scheme,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    capabilities.extend(
        authority
            .provided_capabilities
            .iter()
            .map(|provide| ProvidedCapability {
                kind: capability_kind(provide.kind),
                name: provide.name.clone(),
                version: provide.provider_version.clone(),
                version_relation: provide.version_relation,
                version_scheme: provide.version_scheme,
                architecture_qualifier: provide.architecture_qualifier.clone(),
                provenance: provide.provenance.clone(),
            }),
    );
    capabilities
}

const fn capability_kind(kind: DependencyKindV3) -> RepositoryCapabilityKind {
    match kind {
        DependencyKindV3::Package => RepositoryCapabilityKind::PackageName,
        DependencyKindV3::Capability => RepositoryCapabilityKind::Virtual,
        DependencyKindV3::File => RepositoryCapabilityKind::File,
        DependencyKindV3::Path => RepositoryCapabilityKind::Path,
        DependencyKindV3::Binary => RepositoryCapabilityKind::Binary,
        DependencyKindV3::Soname => RepositoryCapabilityKind::Soname,
        DependencyKindV3::PkgConfig => RepositoryCapabilityKind::PkgConfig,
        DependencyKindV3::PkgConfig32 => RepositoryCapabilityKind::PkgConfig32,
        DependencyKindV3::Comar => RepositoryCapabilityKind::Comar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v3::schema::{
        LifecycleDirectoryV3, LifecycleScriptExecutionV3, LifecycleScriptV3,
    };

    #[test]
    fn verified_v3_lifecycle_is_visible_to_install_hook_execution() {
        let mut authority = AuthorityDocumentV3::package_for_tests("lifecycle-install");
        authority.lifecycle.directories.push(LifecycleDirectoryV3 {
            path: "/var/lib/lifecycle-install".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: Some(true),
        });
        authority.lifecycle.post_install = Some(LifecycleScriptV3 {
            interpreter: "/bin/sh".to_string(),
            body: "printf installed".to_string(),
            capabilities: Vec::new(),
            reversible: Some(false),
            execution: LifecycleScriptExecutionV3::SandboxedTargetRoot,
        });

        let manifest = install_manifest_from_v3(&authority, None, None).unwrap();
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
    fn verified_v3_identity_is_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV3::package_for_tests("identity-projection");
        authority.identity.release = "7".to_string();
        authority.identity.architecture = Some("aarch64".to_string());
        authority.identity.platform = Some("linux".to_string());

        let manifest = install_manifest_from_v3(&authority, None, None).unwrap();

        assert_eq!(manifest.package.release, "7");
        assert_eq!(manifest.package.kind, authority.identity.kind);
        let platform = manifest.package.platform.as_ref().unwrap();
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.arch.as_deref(), Some("aarch64"));
    }

    #[test]
    fn verified_v3_package_capabilities_are_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV3::package_for_tests("capability-projection");
        let declaration = crate::capability::CapabilityDeclaration {
            rationale: Some("needs repository access".to_string()),
            network: crate::capability::NetworkCapabilities {
                connect_tcp: vec![443],
                ..Default::default()
            },
            ..Default::default()
        };
        authority.execution_capabilities = Some(declaration.clone());

        let manifest = install_manifest_from_v3(&authority, None, None).unwrap();

        assert_eq!(manifest.capabilities, Some(declaration));
    }

    #[test]
    fn verified_v3_file_capabilities_are_preserved_in_the_install_projection() {
        let mut authority = AuthorityDocumentV3::package_for_tests("file-capability-projection");
        let declaration = crate::ccs::manifest::FileCapability {
            path: "/usr/bin/hello".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        };
        authority.file_capabilities = vec![declaration.clone()];

        let manifest = install_manifest_from_v3(&authority, None, None).unwrap();

        assert_eq!(manifest.file_capabilities, vec![declaration]);
    }
}
