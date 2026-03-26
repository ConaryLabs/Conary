// conary-core/src/resolver/provider/matching.rs

//! Version constraint matching functions.
//!
//! Determines whether a constraint matches a package version or a provided
//! capability version, handling cross-format comparisons between legacy RPM
//! constraints and native (Debian, Arch) version schemes.
//!
//! All functions now work with `(version: &str, scheme: VersionScheme)` pairs
//! from `PackageIdentity` instead of the former `ConaryPackageVersion` enum.

use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, compare_repo_versions, repo_version_satisfies,
};
use crate::version::{RpmVersion, VersionConstraint};

use super::types::ConaryConstraint;

/// Check whether a constraint matches a package's version.
///
/// `version` is the raw version string, `scheme` is how to interpret it.
pub fn constraint_matches_package(
    constraint: &ConaryConstraint,
    version: &str,
    scheme: VersionScheme,
) -> bool {
    match constraint {
        // Legacy constraint (RPM-style)
        ConaryConstraint::Legacy(vc) => match vc {
            VersionConstraint::Any => true,
            _ => match scheme {
                VersionScheme::Rpm => RpmVersion::parse(version)
                    .map(|v| vc.satisfies(&v))
                    .unwrap_or(false),
                // Legacy constraint against non-RPM version: only `Any` matches
                // (handled above), all others fail.
                _ => false,
            },
        },
        // Native (scheme-aware) constraint
        ConaryConstraint::Repository {
            scheme: constraint_scheme,
            constraint: repo_constraint,
            ..
        } => {
            // `Any` matches everything regardless of scheme
            if matches!(repo_constraint, RepoVersionConstraint::Any) {
                return true;
            }
            if constraint_scheme == &scheme {
                return repo_version_satisfies(scheme, version, repo_constraint);
            }
            // Cross-scheme RPM: native RPM constraint vs RPM version string
            if *constraint_scheme == VersionScheme::Rpm && scheme == VersionScheme::Rpm {
                let legacy = repo_constraint_to_legacy(repo_constraint);
                return RpmVersion::parse(version)
                    .map(|v| legacy.satisfies(&v))
                    .unwrap_or(false);
            }
            false
        }
    }
}

/// Check whether a constraint matches a provide's version.
///
/// If `provide_version` is `Some`, it is authoritative -- the package version
/// is NOT used as a fallback. A package at 2.0 providing `foo = 1.0` must not
/// satisfy `foo >= 2.0` just because the package version is high enough.
///
/// Only when `provide_version` is `None` (unversioned provide) do we fall back
/// to the owning package's version.
pub(super) fn constraint_matches_provide(
    constraint: &ConaryConstraint,
    provide_version: Option<&str>,
    provide_scheme: VersionScheme,
    package_version: &str,
    package_scheme: VersionScheme,
) -> bool {
    if let Some(pv) = provide_version {
        // Explicit provide version is authoritative -- no fallback.
        return constraint_matches_package(constraint, pv, provide_scheme);
    }
    // Unversioned provide: fall back to the owning package's version.
    constraint_matches_package(constraint, package_version, package_scheme)
}

/// Compare two package versions in descending order (highest first).
///
/// Returns `None` when the schemes differ and comparison is not meaningful.
pub(super) fn compare_package_versions_desc(
    a_version: &str,
    a_scheme: VersionScheme,
    b_version: &str,
    b_scheme: VersionScheme,
) -> Option<std::cmp::Ordering> {
    if a_scheme != b_scheme {
        return None;
    }
    // compare_repo_versions returns descending when args are (scheme, b, a)
    compare_repo_versions(a_scheme, b_version, a_version)
}

/// Convert a `RepoVersionConstraint` to a legacy `VersionConstraint` for RPM
/// cross-format matching (repo RPM constraint vs installed RPM `RpmVersion`).
fn repo_constraint_to_legacy(constraint: &RepoVersionConstraint) -> VersionConstraint {
    match constraint {
        RepoVersionConstraint::Any => VersionConstraint::Any,
        RepoVersionConstraint::Exact(v) => {
            VersionConstraint::parse(&format!("= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::GreaterThan(v) => {
            VersionConstraint::parse(&format!("> {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::GreaterOrEqual(v) => {
            VersionConstraint::parse(&format!(">= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::LessThan(v) => {
            VersionConstraint::parse(&format!("< {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::LessOrEqual(v) => {
            VersionConstraint::parse(&format!("<= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::NotEqual(v) => {
            VersionConstraint::parse(&format!("!= {v}")).unwrap_or(VersionConstraint::Any)
        }
    }
}
