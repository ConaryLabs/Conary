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

use clap::{Args, Parser, Subcommand};

mod automation;
mod bootstrap;
mod capability;
mod ccs;
mod collection;
mod config;
mod derive;
mod federation;
mod label;
mod model;
mod package;
mod provenance;
mod query;
mod redirect;
mod repo;
mod state;
mod system;
mod trigger;

pub use automation::{AutomationCommands, AiCommands};
pub use bootstrap::BootstrapCommands;
pub use capability::CapabilityCommands;
pub use ccs::CcsCommands;
pub use collection::CollectionCommands;
pub use config::ConfigCommands;
pub use derive::DeriveCommands;
pub use federation::FederationCommands;
pub use label::LabelCommands;
pub use model::ModelCommands;
pub use provenance::ProvenanceCommands;
pub use query::QueryCommands;
pub use redirect::RedirectCommands;
pub use repo::RepoCommands;
pub use state::StateCommands;
pub use system::SystemCommands;
pub use trigger::TriggerCommands;

/// Database path arguments
#[derive(Args, Clone, Debug)]
pub struct DbArgs {
    /// Path to the database file
    #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
    pub db_path: String,
}

/// Common arguments for filesystem operations
#[derive(Args, Clone, Debug)]
pub struct CommonArgs {
    #[command(flatten)]
    pub db: DbArgs,

    /// Installation root directory
    #[arg(short, long, default_value = "/")]
    pub root: String,
}

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

        #[command(flatten)]
        common: CommonArgs,

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
        ///
        /// Scriptlets are automatically captured and converted to declarative hooks
        /// unless --no-capture is specified.
        #[arg(long)]
        convert_to_ccs: bool,

        /// Disable scriptlet capture during conversion (unsafe - runs scriptlets at install time)
        #[arg(long)]
        no_capture: bool,

        /// Skip optional packages (for collection installs)
        #[arg(long)]
        skip_optional: bool,
    },

    /// Remove an installed package
    Remove {
        /// Package name to remove
        package_name: String,

        #[command(flatten)]
        common: CommonArgs,

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

        #[command(flatten)]
        common: CommonArgs,

        /// Only apply security updates (critical/important severity)
        #[arg(long)]
        security: bool,
    },

    /// Search for packages in repositories
    Search {
        /// Search pattern
        pattern: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// List installed packages
    List {
        /// Optional pattern to filter packages
        pattern: Option<String>,

        #[command(flatten)]
        db: DbArgs,

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
        #[command(flatten)]
        common: CommonArgs,

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

        #[command(flatten)]
        db: DbArgs,
    },

    /// Unpin a package to allow updates and removal
    Unpin {
        /// Package name to unpin
        package_name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Cook a package from a recipe (build from source)
    ///
    /// Recipes are TOML files that describe how to build a package from source.
    /// By default, the cooking process runs in an isolated container for security
    /// and reproducibility. Network access is blocked during the build phase.
    Cook {
        /// Path to recipe file (.recipe or .toml)
        recipe: String,

        /// Output directory for the built package
        #[arg(short, long, default_value = ".")]
        output: String,

        /// Source cache directory
        #[arg(long, default_value = "/var/cache/conary/sources")]
        source_cache: String,

        /// Number of parallel build jobs (default: auto)
        #[arg(short, long)]
        jobs: Option<u32>,

        /// Keep build directory after completion (for debugging)
        #[arg(long)]
        keep_builddir: bool,

        /// Validate recipe without cooking
        #[arg(long)]
        validate_only: bool,

        /// Only fetch sources, don't build
        ///
        /// Downloads and caches all source archives and patches without building.
        /// Useful for pre-fetching sources for offline builds.
        #[arg(long)]
        fetch_only: bool,

        /// Disable container isolation (unsafe - allows network access during build)
        ///
        /// WARNING: This flag disables security protections and may produce
        /// non-reproducible builds. Only use for debugging or in trusted environments.
        #[arg(long)]
        no_isolation: bool,

        /// Enable hermetic mode (maximum isolation, no host mounts)
        ///
        /// Provides BuildStream-grade reproducibility guarantees by isolating
        /// the build from host system libraries and toolchains.
        #[arg(long)]
        hermetic: bool,
    },

    /// Convert an Arch Linux PKGBUILD to a Conary recipe
    ///
    /// Reads a PKGBUILD file and outputs the equivalent recipe in TOML format.
    ConvertPkgbuild {
        /// Path to PKGBUILD file
        pkgbuild: String,

        /// Output file for the recipe (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
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

    /// Automation and AI-assisted operations
    ///
    /// Manage automated system maintenance including security updates,
    /// orphan cleanup, and AI-assisted package management.
    #[command(subcommand)]
    Automation(AutomationCommands),

    // =========================================================================
    // Bootstrap
    // =========================================================================
    /// Bootstrap a complete Conary system from scratch
    #[command(subcommand)]
    Bootstrap(BootstrapCommands),

    /// Package DNA / Provenance queries
    ///
    /// Query complete package lineage: source origin, build environment,
    /// signatures, and content hashes. Enables trust verification and
    /// security audits.
    #[command(subcommand)]
    Provenance(ProvenanceCommands),

    /// Package capability declarations
    ///
    /// View and validate capability declarations that define what system
    /// resources a package needs (network, filesystem, syscalls).
    #[command(subcommand)]
    Capability(CapabilityCommands),

    /// Federation management
    ///
    /// Manage CAS federation for chunk sharing across machines.
    /// Federation enables bandwidth savings by fetching chunks from
    /// nearby peers instead of the origin server.
    #[command(subcommand)]
    Federation(FederationCommands),
}
