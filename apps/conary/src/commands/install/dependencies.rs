// src/commands/install/dependencies.rs

//! Dependency resolution for package installation
//!
//! Currently this module only exposes helpers for extracting runtime
//! dependencies from package metadata.

use conary_core::packages::PackageFormat;
use conary_core::packages::traits::DependencyType;
use conary_core::version::VersionConstraint;

/// A runtime dependency extracted from a package.
#[derive(Debug, Clone)]
pub struct RuntimeDep {
    /// Dependency name (package or capability).
    pub name: String,
    /// Version constraint (Any if unspecified).
    pub constraint: VersionConstraint,
}

/// Extract runtime dependencies from a package as `(name, constraint)` pairs.
#[must_use]
pub fn extract_runtime_deps(pkg: &dyn PackageFormat) -> Vec<RuntimeDep> {
    pkg.dependencies()
        .iter()
        .filter(|d| d.dep_type == DependencyType::Runtime)
        .map(|d| {
            let constraint = d
                .version
                .as_ref()
                .and_then(|v| VersionConstraint::parse(v).ok())
                .unwrap_or(VersionConstraint::Any);
            RuntimeDep {
                name: d.name.clone(),
                constraint,
            }
        })
        .collect()
}
