// conary-core/src/ccs/package/v2_projection.rs

//! Structural projection of native CCS v2 authority into package-install data.

use crate::ccs::attestation::{BuildAttestationEnvelope, ForeignConversionBoundary};
use crate::ccs::builder::FileEntry;
use crate::ccs::manifest::{CcsManifest, Platform};
use crate::ccs::v2::AuthorityDocumentV2;
use crate::ccs::v2::schema::{DependencyKindV2, PackageKindV2};
use crate::error::{Error, Result};
use crate::packages::traits::{Dependency, DependencyType};

pub(super) fn install_manifest_from_v2(
    authority: &AuthorityDocumentV2,
    build_attestation: Option<BuildAttestationEnvelope>,
    foreign_conversion_boundary: Option<ForeignConversionBoundary>,
) -> Result<CcsManifest> {
    let mut manifest =
        CcsManifest::new_minimal(&authority.identity.name, &authority.identity.version);
    manifest.package.version_scheme = authority.identity.version_scheme;
    manifest.package.release = authority.identity.release.clone();
    manifest.package.kind = authority.identity.kind;
    manifest.package.platform =
        if authority.identity.platform.is_some() || authority.identity.architecture.is_some() {
            Some(Platform {
                os: authority.identity.platform.clone().unwrap_or_default(),
                arch: authority.identity.architecture.clone(),
                ..Platform::default()
            })
        } else {
            None
        };
    manifest.package.description = format!("CCS v2 {}", authority.identity.name);

    for provide in &authority.provides {
        match provide.kind {
            DependencyKindV2::Soname => manifest.provides.sonames.push(provide.name.clone()),
            DependencyKindV2::Binary => manifest.provides.binaries.push(provide.name.clone()),
            DependencyKindV2::PkgConfig => manifest.provides.pkgconfig.push(provide.name.clone()),
            DependencyKindV2::Package
            | DependencyKindV2::Capability
            | DependencyKindV2::File
            | DependencyKindV2::Path => {
                manifest.provides.capabilities.push(provide.name.clone());
            }
        }
    }

    manifest.requirements = authority.requirements.clone();
    manifest.relations = authority.relations.clone();

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

pub(super) fn provides_from_v2_authority(authority: &AuthorityDocumentV2) -> Vec<Dependency> {
    authority
        .provides
        .iter()
        .filter_map(|provide| {
            dependency_identity(provide.kind, &provide.name).map(|name| Dependency {
                name,
                version: provide.version_constraint.clone(),
                dep_type: DependencyType::Runtime,
                description: None,
            })
        })
        .collect()
}

fn dependency_identity(kind: DependencyKindV2, name: &str) -> Option<String> {
    match kind {
        DependencyKindV2::Package
        | DependencyKindV2::Capability
        | DependencyKindV2::File
        | DependencyKindV2::Path => Some(name.to_string()),
        DependencyKindV2::Binary => Some(format!("binary({name})")),
        DependencyKindV2::Soname => Some(format!("soname({name})")),
        DependencyKindV2::PkgConfig => Some(format!("pkgconfig({name})")),
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
}
