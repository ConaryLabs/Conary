// conary-core/src/resolver/provider/types.rs

//! Data types for the resolver provider bridge.
//!
//! Contains the solver-facing constraint and dependency types.
//! Package identity is now represented by `PackageIdentity` from
//! `resolver::identity`.

use std::fmt;

use crate::repository::versioning::{RepoVersionConstraint, VersionScheme};
use crate::version::VersionConstraint;

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
