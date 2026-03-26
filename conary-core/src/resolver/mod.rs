// conary-core/src/resolver/mod.rs

//! SAT-based dependency resolution and conflict detection
//!
//! This module provides SAT-based dependency resolution using resolvo,
//! conflict detection, and component-level resolution for package
//! installation and removal safety checking.

pub mod canonical;
pub mod conflict;
pub mod component_resolver;
pub mod identity;
pub mod plan;
pub mod provider;
pub mod provides_index;
pub mod sat;

pub use component_resolver::{
    ComponentResolutionPlan, ComponentResolver, ComponentSpec, MissingComponent,
};
pub use conflict::Conflict;
pub use identity::PackageIdentity;
pub use plan::{MissingDependency, ResolutionPlan};
pub use provides_index::ProvidesIndex;
pub use sat::{SatPackage, SatResolution, SatSource, solve_install, solve_removal};
