// conary-core/src/resolver/provider/matching.rs

//! Version constraint matching functions.
//!
//! Determines whether a constraint matches a package version or a provided
//! capability version, handling cross-format comparisons between legacy RPM
//! constraints and native (Debian, Arch) version schemes.

use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, compare_repo_versions, repo_version_satisfies,
};
use crate::version::{RpmVersion, VersionConstraint};

use super::types::{ConaryConstraint, ConaryPackageVersion, ConaryProvidedVersion};

pub fn constraint_matches_package(
    constraint: &ConaryConstraint,
    version: &ConaryPackageVersion,
) -> bool {
    match (constraint, version) {
        // Legacy constraint vs RPM installed
        (ConaryConstraint::Legacy(constraint), ConaryPackageVersion::Installed(version)) => {
            constraint.satisfies(version)
        }
        // Legacy `Any` matches anything
        (
            ConaryConstraint::Legacy(VersionConstraint::Any),
            ConaryPackageVersion::Repository { .. } | ConaryPackageVersion::InstalledNative { .. },
        ) => true,
        // Legacy constraint vs RPM repo package
        (
            ConaryConstraint::Legacy(constraint),
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(VersionScheme::Rpm),
            },
        ) => RpmVersion::parse(raw)
            .map(|version| constraint.satisfies(&version))
            .unwrap_or(false),
        // Native RPM constraint vs legacy RPM installed
        (
            ConaryConstraint::Repository {
                scheme: VersionScheme::Rpm,
                constraint,
                ..
            },
            ConaryPackageVersion::Installed(version),
        ) => {
            let legacy = repo_constraint_to_legacy(constraint);
            legacy.satisfies(version)
        }
        // Native constraint vs installed native (same scheme)
        (
            ConaryConstraint::Repository {
                scheme, constraint, ..
            },
            ConaryPackageVersion::InstalledNative {
                raw,
                scheme: installed_scheme,
            },
        ) if scheme == installed_scheme => repo_version_satisfies(*scheme, raw, constraint),
        // Native constraint vs repo package (same scheme)
        (
            ConaryConstraint::Repository {
                scheme, constraint, ..
            },
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(version_scheme),
            },
        ) if scheme == version_scheme => repo_version_satisfies(*scheme, raw, constraint),
        // Native `Any` constraint matches any repo version
        (
            ConaryConstraint::Repository {
                constraint: RepoVersionConstraint::Any,
                ..
            },
            ConaryPackageVersion::Repository { .. } | ConaryPackageVersion::InstalledNative { .. },
        ) => true,
        _ => false,
    }
}

pub(super) fn constraint_matches_provide(
    constraint: &ConaryConstraint,
    provided_version: Option<&ConaryProvidedVersion>,
    package_version: &ConaryPackageVersion,
) -> bool {
    if let Some(version) = provided_version {
        match (constraint, version) {
            (ConaryConstraint::Legacy(constraint), ConaryProvidedVersion::Installed(version)) => {
                return constraint.satisfies(version);
            }
            (
                ConaryConstraint::Legacy(VersionConstraint::Any),
                ConaryProvidedVersion::Repository { .. },
            ) => return true,
            (
                ConaryConstraint::Legacy(constraint),
                ConaryProvidedVersion::Repository {
                    raw,
                    scheme: VersionScheme::Rpm,
                },
            ) => {
                return RpmVersion::parse(raw)
                    .map(|version| constraint.satisfies(&version))
                    .unwrap_or(false);
            }
            (
                ConaryConstraint::Repository {
                    scheme, constraint, ..
                },
                ConaryProvidedVersion::Repository {
                    raw,
                    scheme: provided_scheme,
                },
            ) if scheme == provided_scheme => {
                return repo_version_satisfies(*scheme, raw, constraint);
            }
            (
                ConaryConstraint::Repository {
                    constraint: RepoVersionConstraint::Any,
                    ..
                },
                ConaryProvidedVersion::Repository { .. },
            ) => return true,
            _ => {}
        }
    }

    constraint_matches_package(constraint, package_version)
}

pub(super) fn compare_package_versions_desc(
    a: &ConaryPackageVersion,
    b: &ConaryPackageVersion,
) -> Option<std::cmp::Ordering> {
    let ord = match (a, b) {
        (ConaryPackageVersion::Installed(a), ConaryPackageVersion::Installed(b)) => Some(b.cmp(a)),
        (
            ConaryPackageVersion::InstalledNative {
                raw: a_raw,
                scheme: a_scheme,
            },
            ConaryPackageVersion::InstalledNative {
                raw: b_raw,
                scheme: b_scheme,
            },
        ) if a_scheme == b_scheme => compare_repo_versions(*a_scheme, b_raw, a_raw),
        (
            ConaryPackageVersion::Repository {
                raw: a_raw,
                scheme: Some(a_scheme),
            },
            ConaryPackageVersion::Repository {
                raw: b_raw,
                scheme: Some(b_scheme),
            },
        ) if a_scheme == b_scheme => compare_repo_versions(*a_scheme, b_raw, a_raw),
        _ => None,
    }?;
    Some(ord)
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
