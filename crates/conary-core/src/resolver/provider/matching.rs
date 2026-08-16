// conary-core/src/resolver/provider/matching.rs

//! Version constraint matching functions.
//!
//! Determines whether a constraint matches a package version or a provided
//! capability version using the source ecosystem's exact version scheme.
//!
//! All functions now work with `(version: &str, scheme: VersionScheme)` pairs
//! from `PackageIdentity` instead of the former `ConaryPackageVersion` enum.

use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    RequirementArchitectureQualifier,
};
use crate::repository::selector::{PackageSelector, package_architectures_match};
use crate::repository::versioning::{
    RepoVersionConstraint, VersionComparisonError, VersionResult, VersionScheme,
    provided_range_matches_requirement, repo_version_satisfies,
};
use crate::resolver::identity::PackageIdentity;
use crate::version::VersionConstraint;

use super::types::{CapabilityExpression, ConaryConstraint};

/// Check source-native architecture semantics before version/capability
/// matching. Every repository candidate must carry an explicit architecture.
pub(crate) fn constraint_architecture_matches_package(
    constraint: &ConaryConstraint,
    package: &PackageIdentity,
    native_architecture: &str,
) -> bool {
    let Some(package_architecture) = package.architecture.as_deref() else {
        return false;
    };
    let Some(package_multi_arch) = valid_multi_arch_authority(package) else {
        return false;
    };
    match constraint {
        ConaryConstraint::RpmRuntime(_) => false,
        ConaryConstraint::Requested(_) | ConaryConstraint::ProviderExpression { .. } => {
            PackageSelector::is_architecture_compatible(
                package.version_scheme,
                Some(package_architecture),
                native_architecture,
            )
        }
        ConaryConstraint::Repository {
            scheme: VersionScheme::Debian,
            architecture_qualifier,
            depending_architecture,
            ..
        } => debian_architecture_matches(
            architecture_qualifier,
            depending_architecture,
            native_architecture,
            package.version_scheme,
            package_architecture,
            package_multi_arch,
        ),
        ConaryConstraint::Repository {
            scheme,
            depending_architecture,
            ..
        } => package_architectures_match(
            package.version_scheme,
            package_architecture,
            *scheme,
            depending_architecture,
            native_architecture,
        ),
    }
}

/// Match dependency architecture authority against one exact provided
/// capability. Debian provider qualifiers are independent of the owning
/// package architecture and therefore cannot be checked at package scope.
pub(crate) fn constraint_architecture_matches_provide(
    constraint: &ConaryConstraint,
    package: &PackageIdentity,
    qualifier: &ProvideArchitectureQualifier,
    provide_scheme: VersionScheme,
    native_architecture: &str,
) -> bool {
    if matches!(constraint, ConaryConstraint::RpmRuntime(_)) {
        return false;
    }
    let Some(package_architecture) = package.architecture.as_deref() else {
        return false;
    };
    let Some(package_multi_arch) = valid_multi_arch_authority(package) else {
        return false;
    };
    match constraint {
        ConaryConstraint::Repository {
            scheme: VersionScheme::Debian,
            architecture_qualifier,
            depending_architecture,
            ..
        } => debian_provide_architecture_matches(
            architecture_qualifier,
            depending_architecture,
            native_architecture,
            DebianProvideArchitecture {
                package_scheme: package.version_scheme,
                package_architecture,
                package_multi_arch,
                provide_scheme,
                provide_qualifier: qualifier,
            },
        ),
        ConaryConstraint::Repository {
            scheme,
            depending_architecture,
            ..
        } => provide_architecture_matches_exact_target(
            package.version_scheme,
            package_architecture,
            provide_scheme,
            qualifier,
            *scheme,
            depending_architecture,
            native_architecture,
        ),
        ConaryConstraint::Requested(_) | ConaryConstraint::ProviderExpression { .. } => {
            provide_architecture_matches_native_target(
                package.version_scheme,
                package_architecture,
                provide_scheme,
                qualifier,
                native_architecture,
            )
        }
        ConaryConstraint::RpmRuntime(_) => unreachable!("returned above"),
    }
}

fn debian_architecture_matches(
    qualifier: &RequirementArchitectureQualifier,
    depending_architecture: &str,
    native_architecture: &str,
    provider_scheme: VersionScheme,
    provider_architecture: &str,
    provider_multi_arch: Option<DebianMultiArch>,
) -> bool {
    match qualifier {
        RequirementArchitectureQualifier::Unqualified => {
            provider_multi_arch == Some(DebianMultiArch::Foreign)
                || package_architectures_match(
                    provider_scheme,
                    provider_architecture,
                    VersionScheme::Debian,
                    depending_architecture,
                    native_architecture,
                )
        }
        RequirementArchitectureQualifier::Any => {
            provider_multi_arch == Some(DebianMultiArch::Allowed)
        }
        RequirementArchitectureQualifier::Native => PackageSelector::is_architecture_compatible(
            provider_scheme,
            Some(provider_architecture),
            native_architecture,
        ),
        RequirementArchitectureQualifier::Exact(architecture) => package_architectures_match(
            provider_scheme,
            provider_architecture,
            VersionScheme::Debian,
            architecture,
            native_architecture,
        ),
    }
}

struct DebianProvideArchitecture<'a> {
    package_scheme: VersionScheme,
    package_architecture: &'a str,
    package_multi_arch: Option<DebianMultiArch>,
    provide_scheme: VersionScheme,
    provide_qualifier: &'a ProvideArchitectureQualifier,
}

fn debian_provide_architecture_matches(
    dependency_qualifier: &RequirementArchitectureQualifier,
    depending_architecture: &str,
    native_architecture: &str,
    provide: DebianProvideArchitecture<'_>,
) -> bool {
    let DebianProvideArchitecture {
        package_scheme,
        package_architecture,
        package_multi_arch,
        provide_scheme,
        provide_qualifier,
    } = provide;
    if matches!(
        dependency_qualifier,
        RequirementArchitectureQualifier::Unqualified
    ) && package_multi_arch == Some(DebianMultiArch::Foreign)
    {
        return true;
    }
    if matches!(dependency_qualifier, RequirementArchitectureQualifier::Any)
        && package_multi_arch == Some(DebianMultiArch::Allowed)
    {
        return true;
    }

    match dependency_qualifier {
        RequirementArchitectureQualifier::Unqualified => {
            !matches!(provide_qualifier, ProvideArchitectureQualifier::Any)
                && provide_architecture_matches_exact_target(
                    package_scheme,
                    package_architecture,
                    provide_scheme,
                    provide_qualifier,
                    VersionScheme::Debian,
                    depending_architecture,
                    native_architecture,
                )
        }
        RequirementArchitectureQualifier::Any => {
            matches!(provide_qualifier, ProvideArchitectureQualifier::Any)
        }
        RequirementArchitectureQualifier::Native => provide_architecture_matches_native_target(
            package_scheme,
            package_architecture,
            provide_scheme,
            provide_qualifier,
            native_architecture,
        ),
        RequirementArchitectureQualifier::Exact(architecture) => {
            provide_architecture_matches_exact_target(
                package_scheme,
                package_architecture,
                provide_scheme,
                provide_qualifier,
                VersionScheme::Debian,
                architecture,
                native_architecture,
            )
        }
    }
}

fn provide_architecture_matches_native_target(
    package_scheme: VersionScheme,
    package_architecture: &str,
    provide_scheme: VersionScheme,
    provide_qualifier: &ProvideArchitectureQualifier,
    native_architecture: &str,
) -> bool {
    match provide_qualifier {
        ProvideArchitectureQualifier::Implicit => PackageSelector::is_architecture_compatible(
            package_scheme,
            Some(package_architecture),
            native_architecture,
        ),
        ProvideArchitectureQualifier::Any => true,
        ProvideArchitectureQualifier::Exact(architecture) => {
            PackageSelector::is_architecture_compatible(
                provide_scheme,
                Some(architecture),
                native_architecture,
            )
        }
    }
}

fn provide_architecture_matches_exact_target(
    package_scheme: VersionScheme,
    package_architecture: &str,
    provide_scheme: VersionScheme,
    provide_qualifier: &ProvideArchitectureQualifier,
    target_scheme: VersionScheme,
    target_architecture: &str,
    native_architecture: &str,
) -> bool {
    match provide_qualifier {
        ProvideArchitectureQualifier::Implicit => package_architectures_match(
            package_scheme,
            package_architecture,
            target_scheme,
            target_architecture,
            native_architecture,
        ),
        ProvideArchitectureQualifier::Any => true,
        ProvideArchitectureQualifier::Exact(architecture) => package_architectures_match(
            provide_scheme,
            architecture,
            target_scheme,
            target_architecture,
            native_architecture,
        ),
    }
}

fn valid_multi_arch_authority(package: &PackageIdentity) -> Option<Option<DebianMultiArch>> {
    match (package.version_scheme, package.debian_multi_arch) {
        (VersionScheme::Debian, Some(value)) => Some(Some(value)),
        (VersionScheme::Debian, None) | (_, Some(_)) => None,
        (_, None) => Some(None),
    }
}

/// Check whether a constraint matches a package's version.
///
/// `version` is the raw version string, `scheme` is how to interpret it.
pub fn constraint_matches_package(
    constraint: &ConaryConstraint,
    version: &str,
    scheme: VersionScheme,
) -> VersionResult<bool> {
    match constraint {
        ConaryConstraint::Requested(constraint) => {
            requested_constraint_matches(constraint, version, scheme)
        }
        ConaryConstraint::Repository {
            scheme: constraint_scheme,
            constraint: repo_constraint,
            ..
        } => {
            // `Any` matches everything regardless of scheme
            if matches!(repo_constraint, RepoVersionConstraint::Any) {
                return Ok(true);
            }
            if constraint_scheme == &scheme {
                return repo_version_satisfies(scheme, version, repo_constraint);
            }
            Ok(false)
        }
        ConaryConstraint::RpmRuntime(_) | ConaryConstraint::ProviderExpression { .. } => Ok(false),
    }
}

/// Check whether a constraint matches a provide's version.
///
/// If `provide_version` is `Some`, it is authoritative -- the package version
/// is NOT used as a fallback. A package at 2.0 providing `foo = 1.0` must not
/// satisfy `foo >= 2.0` just because the package version is high enough.
///
pub(crate) fn constraint_matches_provide(
    constraint: &ConaryConstraint,
    provide_version: Option<&str>,
    provide_relation: Option<ProvideVersionRelation>,
    provide_scheme: VersionScheme,
) -> VersionResult<bool> {
    match constraint {
        ConaryConstraint::Repository {
            scheme, constraint, ..
        } => {
            if !matches!(constraint, RepoVersionConstraint::Any) && *scheme != provide_scheme {
                return Ok(false);
            }
            provided_range_matches_requirement(
                provide_scheme,
                provide_relation,
                provide_version,
                constraint,
            )
        }
        ConaryConstraint::Requested(VersionConstraint::Any) => Ok(true),
        ConaryConstraint::Requested(requested) => match (provide_relation, provide_version) {
            (Some(ProvideVersionRelation::Equal), Some(version)) => {
                requested_constraint_matches(requested, version, provide_scheme)
            }
            (None, None) => Ok(false),
            (Some(relation), Some(version)) => Err(VersionComparisonError::InvalidConstraint {
                scheme: provide_scheme.as_str(),
                constraint: format!("{} {version}", relation.as_str()),
                reason:
                    "non-exact native provider ranges require a source-native repository constraint"
                        .to_string(),
            }),
            _ => Err(VersionComparisonError::InvalidConstraint {
                scheme: provide_scheme.as_str(),
                constraint: "incomplete provider range".to_string(),
                reason: "a versioned provide must carry both relation and boundary".to_string(),
            }),
        },
        ConaryConstraint::RpmRuntime(_) | ConaryConstraint::ProviderExpression { .. } => Ok(false),
    }
}

pub(crate) fn provider_expression_matches_package(
    expression: &CapabilityExpression,
    package: &PackageIdentity,
) -> VersionResult<bool> {
    match expression {
        CapabilityExpression::Atom {
            name,
            capability_kind,
            scheme,
            constraint,
        } => {
            if matches!(
                capability_kind,
                None | Some(
                    crate::repository::dependency_model::RepositoryCapabilityKind::PackageName
                )
            ) && package.name == *name
            {
                if matches!(constraint, RepoVersionConstraint::Any) {
                    return Ok(true);
                }
                if *scheme != package.version_scheme {
                    return Ok(false);
                }
                return repo_version_satisfies(*scheme, &package.version, constraint);
            }
            for capability in package.provided_capabilities.iter().filter(|capability| {
                capability.name == *name
                    && capability_kind.is_none_or(|kind| capability.kind == kind)
            }) {
                if matches!(constraint, RepoVersionConstraint::Any) {
                    return Ok(true);
                }
                if *scheme == capability.version_scheme
                    && provided_range_matches_requirement(
                        *scheme,
                        capability.version_relation,
                        capability.version.as_deref(),
                        constraint,
                    )?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        CapabilityExpression::And(operands) => {
            for operand in operands {
                if !provider_expression_matches_package(operand, package)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        CapabilityExpression::Or(operands) => {
            for operand in operands {
                if provider_expression_matches_package(operand, package)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        CapabilityExpression::Not(operand) => {
            Ok(!provider_expression_matches_package(operand, package)?)
        }
    }
}

/// Evaluate one solver constraint against a package using exact identity,
/// capability, architecture, and native-version authority. Callers decide
/// whether the requested name is an admitted identity (exact for incoming
/// packages; exact or canonical for persisted SAT candidates).
pub(crate) fn constraint_matches_candidate(
    requested_name: &str,
    constraint: &ConaryConstraint,
    package: &PackageIdentity,
    native_architecture: &str,
    identity_name_matches: bool,
) -> VersionResult<bool> {
    let requested_kind = match constraint {
        ConaryConstraint::Repository {
            capability_kind, ..
        } => *capability_kind,
        ConaryConstraint::Requested(_)
        | ConaryConstraint::RpmRuntime(_)
        | ConaryConstraint::ProviderExpression { .. } => None,
    };

    match constraint {
        ConaryConstraint::RpmRuntime(_) => Ok(false),
        ConaryConstraint::ProviderExpression { expression } => Ok(
            constraint_architecture_matches_package(constraint, package, native_architecture)
                && provider_expression_matches_package(expression, package)?,
        ),
        constraint
            if matches!(
                requested_kind,
                None | Some(
                    crate::repository::dependency_model::RepositoryCapabilityKind::PackageName
                )
            ) && identity_name_matches =>
        {
            Ok(
                constraint_architecture_matches_package(constraint, package, native_architecture)
                    && constraint_matches_package(
                        constraint,
                        &package.version,
                        package.version_scheme,
                    )?,
            )
        }
        constraint => {
            for capability in package.provided_capabilities.iter().filter(|capability| {
                capability.name == requested_name
                    && requested_kind.is_none_or(|kind| capability.kind == kind)
            }) {
                if constraint_architecture_matches_provide(
                    constraint,
                    package,
                    &capability.architecture_qualifier,
                    capability.version_scheme,
                    native_architecture,
                ) && constraint_matches_provide(
                    constraint,
                    capability.version.as_deref(),
                    capability.version_relation,
                    capability.version_scheme,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn requested_constraint_matches(
    constraint: &VersionConstraint,
    version: &str,
    scheme: VersionScheme,
) -> VersionResult<bool> {
    let native = match constraint {
        VersionConstraint::Any => return Ok(true),
        VersionConstraint::Exact(expected) => RepoVersionConstraint::Exact(expected.to_string()),
        VersionConstraint::GreaterThan(expected) => {
            RepoVersionConstraint::GreaterThan(expected.to_string())
        }
        VersionConstraint::GreaterOrEqual(expected) => {
            RepoVersionConstraint::GreaterOrEqual(expected.to_string())
        }
        VersionConstraint::LessThan(expected) => {
            RepoVersionConstraint::LessThan(expected.to_string())
        }
        VersionConstraint::LessOrEqual(expected) => {
            RepoVersionConstraint::LessOrEqual(expected.to_string())
        }
        VersionConstraint::NotEqual(expected) => {
            RepoVersionConstraint::NotEqual(expected.to_string())
        }
        VersionConstraint::And(left, right) => {
            return Ok(requested_constraint_matches(left, version, scheme)?
                && requested_constraint_matches(right, version, scheme)?);
        }
    };
    repo_version_satisfies(scheme, version, &native)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::versioning::RepoVersionConstraint;

    fn package(architecture: &str, multi_arch: DebianMultiArch) -> PackageIdentity {
        PackageIdentity {
            repo_package_id: Some(1),
            name: "libfoo".to_string(),
            version: "1".to_string(),
            package_release: None,
            architecture: Some(architecture.to_string()),
            debian_multi_arch: Some(multi_arch),
            version_scheme: VersionScheme::Debian,
            repository_id: Some(1),
            repository_name: "debian".to_string(),
            repository_profile: Some("ubuntu-26.04".to_string()),
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: None,
            installed_pinned: false,
            provided_capabilities: Vec::new(),
        }
    }

    fn constraint(qualifier: RequirementArchitectureQualifier) -> ConaryConstraint {
        ConaryConstraint::Repository {
            scheme: VersionScheme::Debian,
            constraint: RepoVersionConstraint::Any,
            capability_kind: None,
            raw: None,
            architecture_qualifier: qualifier,
            depending_architecture: "amd64".to_string(),
        }
    }

    #[test]
    fn debian_multi_arch_qualifiers_follow_dpkg_contract() {
        let native = "x86_64";
        let same = package("amd64", DebianMultiArch::No);
        let foreign = package("arm64", DebianMultiArch::Foreign);
        let allowed = package("arm64", DebianMultiArch::Allowed);

        assert!(constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &same,
            native,
        ));
        assert!(constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &foreign,
            native,
        ));
        assert!(!constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &allowed,
            native,
        ));
        assert!(constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Any),
            &allowed,
            native,
        ));
        assert!(!constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Any),
            &foreign,
            native,
        ));
        assert!(constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Native),
            &same,
            native,
        ));
        assert!(constraint_architecture_matches_package(
            &constraint(RequirementArchitectureQualifier::Exact("arm64".to_string())),
            &allowed,
            native,
        ));
    }

    #[test]
    fn debian_provide_qualifiers_follow_dpkg_contract() {
        let native = "x86_64";
        let arm64 = package("arm64", DebianMultiArch::No);
        let foreign = package("arm64", DebianMultiArch::Foreign);

        assert!(constraint_architecture_matches_provide(
            &constraint(RequirementArchitectureQualifier::Any),
            &arm64,
            &ProvideArchitectureQualifier::Any,
            VersionScheme::Debian,
            native,
        ));
        assert!(!constraint_architecture_matches_provide(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &arm64,
            &ProvideArchitectureQualifier::Any,
            VersionScheme::Debian,
            native,
        ));
        assert!(constraint_architecture_matches_provide(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &arm64,
            &ProvideArchitectureQualifier::Exact("amd64".to_string()),
            VersionScheme::Debian,
            native,
        ));
        assert!(constraint_architecture_matches_provide(
            &constraint(RequirementArchitectureQualifier::Unqualified),
            &foreign,
            &ProvideArchitectureQualifier::Implicit,
            VersionScheme::Debian,
            native,
        ));
    }

    #[test]
    fn unversioned_debian_provide_never_satisfies_versioned_dependency() {
        let constraint = ConaryConstraint::Repository {
            scheme: VersionScheme::Debian,
            constraint: RepoVersionConstraint::GreaterOrEqual("2".to_string()),
            capability_kind: None,
            raw: Some(">= 2".to_string()),
            architecture_qualifier: RequirementArchitectureQualifier::Unqualified,
            depending_architecture: "amd64".to_string(),
        };

        assert!(
            !constraint_matches_provide(&constraint, None, None, VersionScheme::Debian).unwrap()
        );
        assert!(
            constraint_matches_provide(
                &constraint,
                Some("2"),
                Some(ProvideVersionRelation::Equal),
                VersionScheme::Debian,
            )
            .unwrap()
        );
    }
}
