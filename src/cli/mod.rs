// src/cli/mod.rs
//! CLI definitions for the Conary package manager
//!
//! This module contains all command-line interface definitions using clap.
//! The actual command implementations are in the `commands` module.
//!
//! Primary commands are hoisted to root level for convenience:
//! - `install` - Install package(s) or collection (@name)
//! - `remove` - Remove a package
//! - `update` - Update package(s) or collection (@name)
//! - `search` - Search for packages
//! - `list` - List installed packages
//! - `autoremove` - Remove orphaned packages
//! - `pin` / `unpin` - Pin/unpin packages from updates
//!
//! Management contexts:
//! - `system` - System administration (state, triggers, redirects, gc, etc.)
//! - `repo` - Repository management
//! - `config` - Configuration file management
//!
//! Advanced/Developer:
//! - `query` - Dependency analysis and advanced queries
//! - `ccs` - Native CCS package format
//! - `derive` - Derived package management
//! - `model` - System model commands
//! - `collection` - Collection management (create, delete, etc.)

use clap::{Parser, Subcommand};

mod ccs;
mod collection;
mod config;
mod derive;
mod label;
mod model;
mod package;
mod query;
mod redirect;
mod repo;
mod state;
mod system;
mod trigger;

pub use ccs::CcsCommands;
pub use collection::CollectionCommands;
pub use config::ConfigCommands;
pub use derive::DeriveCommands;
pub use label::LabelCommands;
pub use model::ModelCommands;
pub use query::QueryCommands;
pub use redirect::RedirectCommands;
pub use repo::RepoCommands;
pub use state::StateCommands;
pub use system::SystemCommands;
pub use trigger::TriggerCommands;

#[derive(Parser)]
#[command(name = "conary")]
#[command(author = "Conary Project")]
#[command(version)]
#[command(about = "A next-generation package manager with atomic transactions", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    // =========================================================================
    // Primary Commands
    // =========================================================================
    /// Install package(s) or collection (@name)
    Install {
        /// Package name, path to package file, or @collection
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific version to install
        #[arg(short, long)]
        version: Option<String>,

        /// Specific repository to use
        #[arg(long)]
        repo: Option<String>,

        /// Show what would be installed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip dependency checking
        #[arg(long)]
        no_deps: bool,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,

        /// Allow downgrading to an older version
        #[arg(long)]
        allow_downgrade: bool,

        /// Convert legacy packages (RPM/DEB/Arch) to CCS format during install
        #[arg(long)]
        convert_to_ccs: bool,

        /// Fetch pre-converted CCS package from a Refinery server
        #[arg(long)]
        refinery: Option<String>,

        /// Distribution for Refinery (arch, fedora, ubuntu, debian)
        #[arg(long)]
        distro: Option<String>,

        /// Skip optional packages (for collection installs)
        #[arg(long)]
        skip_optional: bool,
    },

    /// Remove an installed package
    Remove {
        /// Package name to remove
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific version to remove (required if multiple versions installed)
        #[arg(short, long)]
        version: Option<String>,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    /// Check for and apply package updates
    Update {
        /// Optional package name or @collection (updates all if not specified)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Only apply security updates (critical/important severity)
        #[arg(long)]
        security: bool,
    },

    /// Search for packages in repositories
    Search {
        /// Search pattern
        pattern: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List installed packages
    List {
        /// Optional pattern to filter packages
        pattern: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Find package owning a file path
        #[arg(long)]
        path: Option<String>,

        /// Show detailed package information
        #[arg(short, long)]
        info: bool,

        /// List files in package
        #[arg(short, long)]
        files: bool,

        /// List files in ls -l style format
        #[arg(long)]
        lsl: bool,

        /// Show only pinned packages
        #[arg(long)]
        pinned: bool,
    },

    /// Remove orphaned packages (installed as dependencies but no longer needed)
    Autoremove {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Show what would be removed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    /// Pin a package to prevent updates and removal
    Pin {
        /// Package name to pin
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Unpin a package to allow updates and removal
    Unpin {
        /// Package name to unpin
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    // =========================================================================
    // Management Contexts
    // =========================================================================
    /// System administration (state, triggers, redirects, gc, etc.)
    #[command(subcommand)]
    System(SystemCommands),

    /// Repository management
    #[command(subcommand)]
    Repo(RepoCommands),

    /// Configuration file management
    #[command(subcommand)]
    Config(ConfigCommands),

    // =========================================================================
    // Advanced/Developer
    // =========================================================================
    /// Dependency analysis and advanced queries
    #[command(subcommand)]
    Query(QueryCommands),

    /// Native CCS package format
    #[command(subcommand)]
    Ccs(CcsCommands),

    /// Derived package management
    #[command(subcommand)]
    Derive(DeriveCommands),

    /// System model management
    #[command(subcommand)]
    Model(ModelCommands),

    /// Collection management (create, delete, membership)
    #[command(subcommand)]
    Collection(CollectionCommands),
}
