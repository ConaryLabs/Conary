// conary-core/src/ccs/v2/validation/identity.rs

use super::super::diagnostics::{V2Diagnostic, V2DiagnosticCode};
use super::super::schema::{AuthorityDocumentV2, DependencyKindV2, PackageKindV2};
use crate::repository::dependency_model::{ProvideArchitectureQualifier, ProvideVersionRelation};
use crate::repository::versioning::VersionScheme;

pub(super) fn validate_identity(
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    if authority.identity.name.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity name is required",
            Some("identity.name".to_string()),
            "set identity.name in signed v2 authority",
        ));
    }
    if authority.identity.version.trim().is_empty() {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity version is required",
            Some("identity.version".to_string()),
            "set identity.version in signed v2 authority",
        ));
    } else if let Err(error) = crate::repository::versioning::validate_repo_version(
        authority.identity.version_scheme,
        &authority.identity.version,
    ) {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            format!("invalid v2 package identity version: {error}"),
            Some("identity.version".to_string()),
            "encode a version valid under identity.version_scheme",
        ));
    }
    if let Err(error) =
        crate::repository::versioning::validate_package_release(&authority.identity.release)
    {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            format!("invalid v2 package identity release: {error}"),
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
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "v2 package identity architecture is required",
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
        (VersionScheme::Debian, None) => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::MissingAuthority,
            "Debian v2 package identity requires exact Multi-Arch authority",
            Some("identity.debian_multi_arch".to_string()),
            "encode no, same, allowed, or foreign from the source control metadata",
        )),
        (_, Some(_)) => diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "non-Debian v2 package identity cannot carry Debian Multi-Arch authority",
            Some("identity.debian_multi_arch".to_string()),
            "remove Debian-only identity metadata from this source scheme",
        )),
    }
}

pub(super) fn validate_provides(
    authority: &AuthorityDocumentV2,
    diagnostics: &mut Vec<V2Diagnostic>,
) {
    let mut exact_self_providers = 0;
    for (index, entry) in authority.provides.iter().enumerate() {
        let field = format!("provides[{index}]");
        if entry.name.trim().is_empty() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::MissingAuthority,
                "provided capability requires a name",
                Some(format!("{field}.name")),
                "write a typed provider name into signed v2 authority",
            ));
        }
        if entry.target.is_some() || entry.component.is_some() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                "provided capability target/component selectors are not supported",
                Some(field.clone()),
                "remove target and component selectors from signed provider authority",
            ));
        }
        if matches!(
            &entry.architecture_qualifier,
            ProvideArchitectureQualifier::Exact(architecture) if architecture.trim().is_empty()
        ) {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                "provided capability has an empty exact architecture qualifier",
                Some(format!("{field}.architecture_qualifier")),
                "encode an implicit, any, or non-empty exact provider architecture",
            ));
        }
        if entry.provider_version.is_some() != entry.version_relation.is_some() {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                "provided capability must carry both a version relation and boundary, or neither",
                Some(field.clone()),
                "pair provider_version with a typed version_relation",
            ));
        }
        if let Some(version) = entry.provider_version.as_deref()
            && let Err(error) =
                crate::repository::versioning::validate_repo_version(entry.version_scheme, version)
        {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                format!("invalid exact provider version: {error}"),
                Some(format!("{field}.provider_version")),
                "encode a provider version valid under its exact version_scheme",
            ));
        }
        if entry.version_scheme != VersionScheme::Rpm
            && entry
                .version_relation
                .is_some_and(|relation| relation != ProvideVersionRelation::Equal)
        {
            diagnostics.push(V2Diagnostic::error(
                V2DiagnosticCode::KindContractViolation,
                "non-RPM provided capabilities may only carry an exact version relation",
                Some(format!("{field}.version_relation")),
                "use the exact relation or preserve the source package's unversioned provide",
            ));
        }
        if entry.kind == DependencyKindV2::Package && entry.name == authority.identity.name {
            if entry.provider_version.as_deref() == Some(authority.identity.version.as_str())
                && entry.version_relation == Some(ProvideVersionRelation::Equal)
                && entry.version_scheme == authority.identity.version_scheme
            {
                exact_self_providers += 1;
            } else {
                diagnostics.push(V2Diagnostic::error(
                    V2DiagnosticCode::KindContractViolation,
                    "package self-provider identity does not match signed package identity",
                    Some(field),
                    "encode the exact identity version and version scheme for the package self-provider",
                ));
            }
        }
    }
    if matches!(authority.kind, PackageKindV2::Package(_)) && exact_self_providers != 1 {
        diagnostics.push(V2Diagnostic::error(
            V2DiagnosticCode::KindContractViolation,
            "installable v2 authority requires exactly one exact package self-provider",
            Some("provides".to_string()),
            "encode one package provider matching identity.name, identity.version, and identity.version_scheme",
        ));
    }
}
