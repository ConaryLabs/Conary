// src/commands/query/mod.rs

//! Query and dependency inspection commands
//!
//! This module provides commands for querying installed packages,
//! dependencies, components, repository packages, and more.

mod components;
mod dependency;
mod deptree;
mod history;
mod package;
mod reason;
mod repo;
mod sbom;

// Re-export all public commands
pub use components::{cmd_list_components, cmd_query_component};
pub use dependency::{cmd_depends, cmd_rdepends, cmd_whatbreaks, cmd_whatprovides};
pub use deptree::cmd_deptree;
pub use history::cmd_history;
pub use package::cmd_query;
pub use reason::cmd_query_reason;
pub use repo::cmd_repquery;
pub use sbom::cmd_sbom;

/// Options for the query command
#[derive(Default)]
pub struct QueryOptions {
    /// Show detailed package information
    pub info: bool,
    /// Show files in ls -l style
    pub lsl: bool,
    /// Find package by file path
    pub path: Option<String>,
    /// List files in package
    pub files: bool,
}
