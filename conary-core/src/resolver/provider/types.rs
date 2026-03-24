// conary-core/src/resolver/provider/types.rs

//! Data types for the resolver provider bridge.
//!
//! Contains the core types that represent packages, versions, constraints,
//! and dependencies in the solver's domain.

use std::fmt;

use crate::repository::versioning::{RepoVersionConstraint, VersionScheme};
use crate::version::{RpmVersion, VersionConstraint};

/// A solvable package — either an installed trove or a repository candidate.
#[derive(Debug, Clone)]
pub struct ConaryPackage {
    pub name: String,
    pub version: ConaryPackageVersion,
    /// `Some` when this package is currently installed.
    pub trove_id: Option<i64>,
    /// `Some` when this package is from a repository.
    pub repo_package_id: Option<i64>,
    /// Capabilities this package provides, along with optional capability versions.
    pub provided_capabilities: Vec<(String, Option<ConaryProvidedVersion>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConaryPackageVersion {
    /// Installed package with RPM version (legacy or actual RPM).
    Installed(RpmVersion),
    /// Installed package with a native (non-RPM) version scheme.
    InstalledNative { raw: String, scheme: VersionScheme },
    Repository {
        raw: String,
        scheme: Option<VersionScheme>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConaryProvidedVersion {
    Installed(RpmVersion),
    Repository { raw: String, scheme: VersionScheme },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConaryConstraint {
    Legacy(VersionConstraint),
    Repository {
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
        raw: Option<String>,
    },
}

/// A single dependency entry for the solver, which may be a simple requirement
/// or an OR-group of alternatives.
#[derive(Debug, Clone)]
pub enum SolverDep {
    /// A single dependency: (name, constraint).
    Single(String, ConaryConstraint),
    /// An OR-group: any one of the alternatives satisfies the dependency.
    /// Each alternative is (name, constraint).
    OrGroup(Vec<(String, ConaryConstraint)>),
}

impl SolverDep {
    /// Returns the name if this is a `Single` dep; `None` for `OrGroup`.
    pub fn single_name(&self) -> Option<&str> {
        match self {
            Self::Single(name, _) => Some(name),
            Self::OrGroup(_) => None,
        }
    }

    /// Returns (name, constraint) if this is a `Single` dep.
    pub fn as_single(&self) -> Option<(&str, &ConaryConstraint)> {
        match self {
            Self::Single(name, constraint) => Some((name, constraint)),
            Self::OrGroup(_) => None,
        }
    }
}

impl fmt::Display for ConaryPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed(version) => write!(f, "{}", version),
            Self::InstalledNative { raw, .. } => write!(f, "{}", raw),
            Self::Repository { raw, .. } => write!(f, "{}", raw),
        }
    }
}

impl fmt::Display for ConaryConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(constraint) => write!(f, "{}", constraint),
            Self::Repository {
                raw, constraint, ..
            } => {
                if let Some(raw) = raw {
                    write!(f, "{}", raw)
                } else {
                    write!(f, "{:?}", constraint)
                }
            }
        }
    }
}
