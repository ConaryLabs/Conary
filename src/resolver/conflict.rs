// src/resolver/conflict.rs

//! Conflict types for dependency resolution
//!
//! Defines the various types of conflicts that can occur during
//! package dependency resolution.

/// A conflict between package requirements
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Version constraint cannot be satisfied
    UnsatisfiableConstraint {
        package: String,
        installed_version: String,
        required_constraint: String,
        required_by: String,
    },
    /// Multiple packages require incompatible versions
    ConflictingConstraints {
        package: String,
        constraints: Vec<(String, String)>, // (requirer, constraint)
    },
    /// Circular dependency detected
    CircularDependency { cycle: Vec<String> },
    /// Package is missing and cannot be found
    MissingPackage {
        package: String,
        required_by: Vec<String>,
    },
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conflict::UnsatisfiableConstraint {
                package,
                installed_version,
                required_constraint,
                required_by,
            } => write!(
                f,
                "Package {} version {} does not satisfy constraint {} required by {}",
                package, installed_version, required_constraint, required_by
            ),
            Conflict::ConflictingConstraints {
                package,
                constraints,
            } => {
                writeln!(f, "Conflicting version requirements for package {}:", package)?;
                for (requirer, constraint) in constraints {
                    writeln!(f, "  - {} requires {}", requirer, constraint)?;
                }
                Ok(())
            }
            Conflict::CircularDependency { cycle } => {
                write!(f, "Circular dependency: {}", cycle.join(" -> "))
            }
            Conflict::MissingPackage {
                package,
                required_by,
            } => {
                write!(
                    f,
                    "Missing package {} required by {}",
                    package,
                    required_by.join(", ")
                )
            }
        }
    }
}
