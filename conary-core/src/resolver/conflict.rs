// conary-core/src/resolver/conflict.rs

//! Conflict types for dependency resolution
//!
//! Defines the various types of conflicts that can occur during
//! package dependency resolution.

/// A conflict between package requirements
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Conflict {
    /// Version constraint cannot be satisfied
    #[error(
        "Package {package} version {installed_version} does not satisfy \
         constraint {required_constraint} required by {required_by}"
    )]
    UnsatisfiableConstraint {
        package: String,
        installed_version: String,
        required_constraint: String,
        required_by: String,
    },
    /// Multiple packages require incompatible versions
    #[error("{}", format_conflicting_constraints(package, constraints))]
    ConflictingConstraints {
        package: String,
        constraints: Vec<(String, String)>, // (requirer, constraint)
    },
    /// Circular dependency detected
    #[error("Circular dependency: {}", cycle.join(" -> "))]
    CircularDependency { cycle: Vec<String> },
    /// Package is missing and cannot be found
    #[error("Missing package {package} required by {}", required_by.join(", "))]
    MissingPackage {
        package: String,
        required_by: Vec<String>,
    },
}

/// Format conflicting constraints without a trailing newline.
fn format_conflicting_constraints(
    package: &str,
    constraints: &[(String, String)],
) -> String {
    use std::fmt::Write;
    let mut out = format!("Conflicting version requirements for package {}:", package);
    for (requirer, constraint) in constraints {
        write!(out, "\n  - {} requires {}", requirer, constraint)
            .expect("string write infallible");
    }
    out
}
