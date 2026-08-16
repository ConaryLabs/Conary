// conary-core/src/resolver/mod.rs

//! SAT-based dependency resolution and conflict detection
//!
//! This module provides SAT-based dependency resolution using resolvo,
//! conflict detection, and component-level resolution for package
//! installation and removal safety checking.

pub mod canonical;
pub mod component_resolver;
pub mod conflict;
pub mod identity;
pub mod plan;
pub mod provider;
pub mod provides_index;
pub mod requirements;
pub mod sat;

pub use component_resolver::{
    ComponentResolutionPlan, ComponentResolver, ComponentSpec, MissingComponent,
};
pub use conflict::Conflict;
pub use identity::PackageIdentity;
pub use plan::{MissingDependency, ResolutionPlan};
pub use provides_index::ProvidesIndex;
pub use requirements::{load_installed_package_identities, requirement_expression_satisfied};
pub use sat::{
    SatPackage, SatResolution, SatSource, positive_requirement_group_satisfied_by_package,
    solve_install_with_policy, solve_package_requirements_with_policy, solve_removal,
    solve_removal_troves, solve_requirement_groups_with_policy,
};
