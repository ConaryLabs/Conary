// conary-core/src/repository/declarations/takeover/enrollment.rs

//! Complete native-product enrollment validation.

use super::*;
use crate::repository::declarations::apt::AptSourceKind;
use crate::repository::{
    ArchSigLevel, ArchSignatureRequirement, ArchTrustLevel, RepositoryTrustPolicy,
};
use std::collections::BTreeSet;

pub(super) fn validate_enrollments(
    conn: &Connection,
    declarations: &DiscoveredRepositoryDeclarations,
    trust: &NativeTrustImportPlan,
    repositories: &[TakeoverRepositoryInput],
) -> Result<Vec<TakeoverBlocker>> {
    let mut blockers = Vec::new();
    let mut names = BTreeSet::new();
    let mut identities = BTreeSet::new();

    for input in repositories {
        validate_unique_identity(input, &mut names, &mut identities, &mut blockers);
        let Some(plan) = trust
            .repositories
            .iter()
            .find(|plan| plan.repository == input.declaration)
        else {
            blockers.push(TakeoverBlocker::UnknownEnrollment {
                repository: input.declaration.clone(),
                name: input.name.clone(),
            });
            continue;
        };
        validate_input_contract(conn, declarations, plan, input, &mut blockers)?;
    }

    for plan in &trust.repositories {
        validate_product_coverage(declarations, plan, repositories, &mut blockers)?;
        if plan.enabled.unwrap_or(true)
            && !matches!(plan.disposition, TrustImportDisposition::Importable)
            && !repositories.iter().any(|input| {
                input.declaration == plan.repository
                    && (explicit_alpm_trust_resolves(plan, &input.trust)
                        || explicit_apt_global_trust_paths(&declarations.root, plan, &input.trust)
                            .is_some())
            })
        {
            blockers.push(TakeoverBlocker::EnabledTrust {
                repository: plan.repository.clone(),
                disposition: plan.disposition.clone(),
            });
        }
    }
    Ok(blockers)
}

fn validate_unique_identity(
    input: &TakeoverRepositoryInput,
    names: &mut BTreeSet<String>,
    identities: &mut BTreeSet<(String, String, String)>,
    blockers: &mut Vec<TakeoverBlocker>,
) {
    if !names.insert(input.name.clone()) {
        blockers.push(TakeoverBlocker::DuplicateRepositoryName {
            name: input.name.clone(),
        });
    }
    let scope = match &input.scope {
        TakeoverPolicyScope::Repository { identity } => format!("repository:{identity}"),
        TakeoverPolicyScope::Group { identity } => format!("group:{identity}"),
    };
    if !identities.insert((
        input.source_identity.clone(),
        scope,
        input.repository_identity.clone(),
    )) {
        blockers.push(TakeoverBlocker::DuplicateRepositoryIdentity {
            identity: input.repository_identity.clone(),
        });
    }
}

fn validate_input_contract(
    conn: &Connection,
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
    input: &TakeoverRepositoryInput,
    blockers: &mut Vec<TakeoverBlocker>,
) -> Result<()> {
    let expected_format = reference_format(&input.declaration);
    let requested_format = input.parser.format();
    if requested_format != expected_format {
        blockers.push(TakeoverBlocker::ParserFormatMismatch {
            repository: input.declaration.clone(),
            name: input.name.clone(),
            expected: expected_format,
            requested: requested_format,
        });
    }
    if requested_format == expected_format {
        let trust_matches = if matches!(plan.disposition, TrustImportDisposition::Importable) {
            expected_trust_policy(
                &declarations.root,
                declarations,
                plan,
                input.parser.format(),
            )?
            .as_ref()
                == Some(&input.trust)
        } else {
            explicit_alpm_trust_resolves(plan, &input.trust)
                || explicit_apt_global_trust_paths(&declarations.root, plan, &input.trust).is_some()
        };
        if !trust_matches {
            blockers.push(TakeoverBlocker::TrustPolicyMismatch {
                repository: input.declaration.clone(),
                name: input.name.clone(),
            });
        }
    }
    let declared = plan.enabled.unwrap_or(true);
    if declared != input.enabled {
        blockers.push(TakeoverBlocker::EnabledStateMismatch {
            repository: input.declaration.clone(),
            name: input.name.clone(),
            declared,
            requested: input.enabled,
        });
    }
    if let Some(existing) = Repository::find_by_name(conn, &input.name)? {
        blockers.push(TakeoverBlocker::ExistingRepository {
            name: input.name.clone(),
            ownership: existing.managed_by.as_str().to_string(),
        });
    }
    Ok(())
}

/// Pacman declarations cannot bind their mutable keyring to exact master
/// fingerprints. The takeover manifest closes that one native ambiguity with
/// Conary's typed ALPM keyring contract. Unsupported native SigLevel settings
/// remain non-overridable.
fn explicit_alpm_trust_resolves(
    plan: &NativeRepositoryTrustPlan,
    trust: &RepositoryTrustPolicy,
) -> bool {
    let TrustImportDisposition::Ambiguous { findings } = &plan.disposition else {
        return false;
    };
    if findings.is_empty()
        || findings
            .iter()
            .any(|finding| finding.kind != TrustImportFindingKind::AlpmKeyringBindingMissing)
    {
        return false;
    }
    let RepositoryTrustPolicy::Arch { sig_level, .. } = trust else {
        return false;
    };
    declared_alpm_sig_level(plan).is_some_and(|declared| declared == *sig_level)
}

fn declared_alpm_sig_level(plan: &NativeRepositoryTrustPlan) -> Option<ArchSigLevel> {
    let values = plan
        .evidence
        .iter()
        .find_map(|evidence| match &evidence.source {
            TrustEvidenceSource::AlpmSigLevel { values } => Some(values.as_slice()),
            _ => None,
        })?;
    let mut level = ArchSigLevel::distribution_default();
    for value in values {
        match value.as_str() {
            "Required" => {
                level.package = ArchSignatureRequirement::Required;
                level.database = ArchSignatureRequirement::Required;
            }
            "Optional" => {
                level.package = ArchSignatureRequirement::Optional;
                level.database = ArchSignatureRequirement::Optional;
            }
            "PackageRequired" => level.package = ArchSignatureRequirement::Required,
            "PackageOptional" => level.package = ArchSignatureRequirement::Optional,
            "DatabaseRequired" => level.database = ArchSignatureRequirement::Required,
            "DatabaseOptional" => level.database = ArchSignatureRequirement::Optional,
            "TrustedOnly" | "PackageTrustedOnly" | "DatabaseTrustedOnly" => {
                level.trust = ArchTrustLevel::TrustedOnly;
            }
            "Never" | "PackageNever" | "DatabaseNever" | "TrustAll" | "PackageTrustAll"
            | "DatabaseTrustAll" => return None,
            _ => return None,
        }
    }
    (level.package == ArchSignatureRequirement::Required).then_some(level)
}

fn validate_product_coverage(
    declarations: &DiscoveredRepositoryDeclarations,
    plan: &NativeRepositoryTrustPlan,
    repositories: &[TakeoverRepositoryInput],
    blockers: &mut Vec<TakeoverBlocker>,
) -> Result<()> {
    let expected = expected_products(declarations, &plan.repository)?;
    let matching = repositories
        .iter()
        .filter(|input| input.declaration == plan.repository)
        .collect::<Vec<_>>();
    let mut covered = vec![Vec::<String>::new(); expected.len()];

    for input in matching {
        let product = input_product(input);
        if let Some(index) = expected
            .iter()
            .position(|expected| product_matches(expected, &product))
        {
            covered[index].push(input.name.clone());
        } else {
            blockers.push(TakeoverBlocker::UnknownEnrollmentProduct {
                repository: plan.repository.clone(),
                name: input.name.clone(),
                product,
            });
        }
    }

    let missing = expected
        .iter()
        .zip(&covered)
        .filter(|(_, names)| names.is_empty())
        .map(|(product, _)| product.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        blockers.push(TakeoverBlocker::MissingEnrollment {
            repository: plan.repository.clone(),
            products: missing,
        });
    }
    for (product, names) in expected.into_iter().zip(covered) {
        if names.len() > 1 {
            blockers.push(TakeoverBlocker::DuplicateEnrollmentProduct {
                repository: plan.repository.clone(),
                product,
                names,
            });
        }
    }
    Ok(())
}

fn expected_products(
    declarations: &DiscoveredRepositoryDeclarations,
    reference: &NativeRepositoryReference,
) -> Result<Vec<NativeEnrollmentProduct>> {
    let NativeRepositoryReference::Apt {
        document,
        entry_index,
        ..
    } = reference
    else {
        return Ok(vec![NativeEnrollmentProduct::Repository]);
    };
    let entry = declarations
        .apt
        .iter()
        .find(|candidate| &candidate.path == document)
        .and_then(|candidate| candidate.entries.get(*entry_index))
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "APT enrollment declaration {} entry {} is absent",
                document.display(),
                entry_index
            ))
        })?;
    if !entry.kinds.contains(&AptSourceKind::Deb) {
        return Ok(Vec::new());
    }
    let components: Vec<Option<&str>> = if entry.components.is_empty() {
        vec![None]
    } else {
        entry
            .components
            .iter()
            .map(|value| Some(value.as_str()))
            .collect()
    };
    let mut products = Vec::new();
    for uri in &entry.uris {
        for suite in &entry.suites {
            for component in &components {
                let product = NativeEnrollmentProduct::Apt {
                    uri: uri.clone(),
                    suite: suite.clone(),
                    component: component.map(str::to_string),
                };
                if !products
                    .iter()
                    .any(|existing| product_matches(existing, &product))
                {
                    products.push(product);
                }
            }
        }
    }
    Ok(products)
}

fn input_product(input: &TakeoverRepositoryInput) -> NativeEnrollmentProduct {
    match &input.parser {
        RepositoryParserConfig::Deb {
            distribution,
            component,
            ..
        } => NativeEnrollmentProduct::Apt {
            uri: input.metadata_url.clone(),
            suite: distribution.clone(),
            component: Some(component.clone()),
        },
        _ => NativeEnrollmentProduct::Repository,
    }
}

fn product_matches(expected: &NativeEnrollmentProduct, observed: &NativeEnrollmentProduct) -> bool {
    match (expected, observed) {
        (NativeEnrollmentProduct::Repository, NativeEnrollmentProduct::Repository) => true,
        (
            NativeEnrollmentProduct::Apt {
                uri: expected_uri,
                suite: expected_suite,
                component: expected_component,
            },
            NativeEnrollmentProduct::Apt {
                uri: observed_uri,
                suite: observed_suite,
                component: observed_component,
            },
        ) => {
            expected_uri.trim_end_matches('/') == observed_uri.trim_end_matches('/')
                && expected_suite == observed_suite
                && expected_component == observed_component
        }
        _ => false,
    }
}

fn reference_format(reference: &NativeRepositoryReference) -> RepositoryFormat {
    match reference {
        NativeRepositoryReference::Apt { .. } => RepositoryFormat::Debian,
        NativeRepositoryReference::Dnf { .. } | NativeRepositoryReference::Zypper { .. } => {
            RepositoryFormat::Fedora
        }
        NativeRepositoryReference::Alpm { .. } => RepositoryFormat::Arch,
    }
}
