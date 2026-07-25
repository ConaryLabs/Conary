// conary-core/src/resolver/provider/matching.rs

//! Version constraint matching functions.
//!
//! Determines whether a constraint matches a package version or a provided
//! capability version using the source ecosystem's exact version scheme.
//!
//! All functions now work with `(version: &str, scheme: VersionScheme)` pairs
//! from `PackageIdentity` instead of the former `ConaryPackageVersion` enum.

use crate::repository::versioning::{
    RepoVersionConstraint, VersionResult, VersionScheme, compare_mixed_repo_versions,
    repo_version_satisfies,
};
use crate::resolver::identity::PackageIdentity;
use crate::version::VersionConstraint;

use super::types::{CapabilityExpression, ConaryConstraint};

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
        ConaryConstraint::ProviderExpression { .. } => Ok(false),
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
    provide_scheme: VersionScheme,
) -> VersionResult<bool> {
    if let Some(pv) = provide_version {
        return constraint_matches_package(constraint, pv, provide_scheme);
    }
    Ok(constraint_is_unversioned(constraint))
}

pub(crate) fn provider_expression_matches_package(
    expression: &CapabilityExpression,
    package: &PackageIdentity,
) -> VersionResult<bool> {
    match expression {
        CapabilityExpression::Atom {
            name,
            scheme,
            constraint,
        } => {
            if package.name == *name {
                if matches!(constraint, RepoVersionConstraint::Any) {
                    return Ok(true);
                }
                if *scheme != package.version_scheme {
                    return Ok(false);
                }
                return repo_version_satisfies(*scheme, &package.version, constraint);
            }
            for capability in package
                .provided_capabilities
                .iter()
                .filter(|capability| capability.name == *name)
            {
                if matches!(constraint, RepoVersionConstraint::Any) {
                    return Ok(true);
                }
                if let Some(version) = capability.version.as_deref()
                    && *scheme == capability.version_scheme
                    && repo_version_satisfies(*scheme, version, constraint)?
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

fn constraint_is_unversioned(constraint: &ConaryConstraint) -> bool {
    match constraint {
        ConaryConstraint::Requested(VersionConstraint::Any)
        | ConaryConstraint::Repository {
            constraint: RepoVersionConstraint::Any,
            ..
        } => true,
        ConaryConstraint::Requested(_)
        | ConaryConstraint::Repository { .. }
        | ConaryConstraint::ProviderExpression { .. } => false,
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

/// Compare two package versions in descending order (highest first).
///
/// Scheme disagreement is a typed error rather than a fallback ordering.
pub(super) fn compare_package_versions_desc(
    a_version: &str,
    a_scheme: VersionScheme,
    b_version: &str,
    b_scheme: VersionScheme,
) -> VersionResult<std::cmp::Ordering> {
    // compare_repo_versions returns descending when args are (scheme, b, a)
    compare_mixed_repo_versions(b_scheme, b_version, a_scheme, a_version)
}
