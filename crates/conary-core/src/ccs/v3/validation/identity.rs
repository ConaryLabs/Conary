// conary-core/src/ccs/v3/validation/identity.rs

use super::super::diagnostics::{V3Diagnostic, V3DiagnosticCode};
use super::super::schema::{AuthorityDocumentV3, DependencyKindV3};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvidedCapability, RepositoryCapabilityKind,
};
use crate::repository::versioning::VersionScheme;

pub(super) fn validate_identity(
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    if authority.identity.name.trim().is_empty() {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::MissingAuthority,
            "v3 package identity name is required",
            Some("identity.name".to_string()),
            "set identity.name in signed v3 authority",
        ));
    }
    if authority.identity.version.trim().is_empty() {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::MissingAuthority,
            "v3 package identity version is required",
            Some("identity.version".to_string()),
            "set identity.version in signed v3 authority",
        ));
    } else if let Err(error) = crate::repository::versioning::validate_repo_version(
        authority.identity.version_scheme,
        &authority.identity.version,
    ) {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            format!("invalid v3 package identity version: {error}"),
            Some("identity.version".to_string()),
            "encode a version valid under identity.version_scheme",
        ));
    }
    if let Err(error) =
        crate::repository::versioning::validate_package_release(&authority.identity.release)
    {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            format!("invalid v3 package identity release: {error}"),
            Some("identity.release".to_string()),
            "set identity.release to a positive decimal CCS build release",
        ));
    }
    if authority
        .identity
        .architecture
        .as_deref()
        .is_none_or(|architecture| architecture.trim().is_empty())
    {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::MissingAuthority,
            "v3 package identity architecture is required",
            Some("identity.architecture".to_string()),
            "encode the exact source-native architecture token; architecture-independent Conary packages use noarch",
        ));
    }
    match (
        authority.identity.version_scheme,
        authority.identity.debian_multi_arch,
    ) {
        (VersionScheme::Debian, Some(_))
        | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, None) => {}
        (VersionScheme::Debian, None) => diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::MissingAuthority,
            "Debian v3 package identity requires exact Multi-Arch authority",
            Some("identity.debian_multi_arch".to_string()),
            "encode no, same, allowed, or foreign from the source control metadata",
        )),
        (_, Some(_)) => diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            "non-Debian v3 package identity cannot carry Debian Multi-Arch authority",
            Some("identity.debian_multi_arch".to_string()),
            "remove Debian-only identity metadata from this source scheme",
        )),
    }
}

pub(super) fn validate_capabilities(
    authority: &AuthorityDocumentV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    let signed_paths = match &authority.kind {
        crate::ccs::v3::schema::PackageKindV3::Package(data) => data
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        _ => std::collections::BTreeSet::new(),
    };
    for (index, entry) in authority.provided_capabilities.iter().enumerate() {
        let field = format!("capabilities[{index}]");
        if entry.target.is_some() || entry.component.is_some() {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                "provided capability target/component selectors are not supported",
                Some(field.clone()),
                "remove target and component selectors from signed provider authority",
            ));
        }
        if entry.provenance == CapabilityProvenance::ExactIdentity {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                "exact package identity cannot appear in the capability list",
                Some(field.clone()),
                "keep exact identity solely in the signed identity field",
            ));
        }
        let projected = ProvidedCapability {
            kind: match entry.kind {
                DependencyKindV3::Package => RepositoryCapabilityKind::PackageName,
                DependencyKindV3::Capability => RepositoryCapabilityKind::Generic,
                DependencyKindV3::File => RepositoryCapabilityKind::File,
                DependencyKindV3::Path => RepositoryCapabilityKind::Path,
                DependencyKindV3::Binary => RepositoryCapabilityKind::Binary,
                DependencyKindV3::Soname => RepositoryCapabilityKind::Soname,
                DependencyKindV3::PkgConfig => RepositoryCapabilityKind::PkgConfig,
            },
            name: entry.name.clone(),
            version: entry.provider_version.clone(),
            version_relation: entry.version_relation,
            version_scheme: entry.version_scheme,
            architecture_qualifier: entry.architecture_qualifier.clone(),
            provenance: entry.provenance.clone(),
        };
        if let Err(error) = projected.validate() {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!("invalid provided capability authority: {error}"),
                Some(field.clone()),
                "encode a complete provider record accepted by the shared capability grammar",
            ));
        }
        // A promise says a path will be created; signed payload says it already
        // has content. One artifact cannot claim both about one path.
        if !projected.witnesses_installed_content() && signed_paths.contains(entry.name.as_str()) {
            diagnostics.push(V3Diagnostic::error(
                V3DiagnosticCode::KindContractViolation,
                format!(
                    "promised path '{}' is also signed payload content",
                    entry.name
                ),
                Some(field),
                "publish a path either as signed payload or as a promised path, never as both",
            ));
        }
    }
}
