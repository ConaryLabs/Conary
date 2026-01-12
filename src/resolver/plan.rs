// src/resolver/plan.rs

//! Resolution plan data structures
//!
//! Contains the result types for dependency resolution.

use crate::version::VersionConstraint;
use super::conflict::Conflict;

/// Result of dependency resolution
#[derive(Debug, Clone)]
pub struct ResolutionPlan {
    /// Packages to install in order (dependencies first)
    pub install_order: Vec<String>,
    /// Packages that are missing and need to be fetched
    pub missing: Vec<MissingDependency>,
    /// Conflicts detected during resolution
    pub conflicts: Vec<Conflict>,
}

/// A missing dependency that needs to be installed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingDependency {
    pub name: String,
    pub constraint: VersionConstraint,
    pub required_by: Vec<String>,
}
