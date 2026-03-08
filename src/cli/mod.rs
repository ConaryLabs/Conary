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
mod canonical;
mod capability;
mod ccs;
mod collection;
mod config;
mod derive;
mod distro;
mod federation;
mod generation;
mod groups;
mod label;
mod model;
mod provenance;
mod query;
mod redirect;
mod registry;
mod repo;
mod state;
mod system;
mod trigger;
mod trust;

#[cfg(feature = "experimental")]
pub use automation::AiCommands;
pub use automation::AutomationCommands;
pub use bootstrap::BootstrapCommands;
pub use canonical::CanonicalCommands;
pub use capability::CapabilityCommands;
pub use ccs::CcsCommands;
pub use collection::CollectionCommands;
pub use config::ConfigCommands;
pub use derive::DeriveCommands;
pub use distro::DistroCommands;
pub use federation::FederationCommands;
pub use generation::GenerationCommands;
pub use groups::GroupsCommands;
pub use label::LabelCommands;
pub use model::ModelCommands;
pub use provenance::ProvenanceCommands;
pub use query::QueryCommands;
pub use redirect::RedirectCommands;
pub use registry::RegistryCommands;
pub use repo::RepoCommands;
pub use state::StateCommands;
pub use system::{SystemCommands, UpdateChannelAction};
pub use trigger::TriggerCommands;
pub use trust::TrustCommands;

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

        /// Force install even if the package is adopted from the system package manager
        #[arg(long)]
        force: bool,

        /// How to handle dependencies: satisfy (default), adopt, takeover
        ///
        /// satisfy:  dependencies on disk satisfy requirements without changes
        /// adopt:    auto-adopt system dependencies into Conary tracking
        /// takeover: download CCS versions from Remi and fully own dependencies
        #[arg(long, default_value = "satisfy")]
        dep_mode: String,

        /// Install from a specific distro (cross-distro override)
        #[arg(long)]
        from: Option<String>,

        /// Assume yes to all prompts
        #[arg(short = 'y', long)]
        yes: bool,
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

        /// Delete adopted package files from disk (default: DB-only removal)
        #[arg(long)]
        purge_files: bool,
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

        /// Scriptlet sandbox mode: auto, always, never
        #[arg(long, default_value = "never")]
        sandbox: String,

        /// How to handle dependencies: satisfy (default), adopt, takeover
        #[arg(long, default_value = "satisfy")]
        dep_mode: String,

        /// Assume yes to all prompts
        #[arg(short = 'y', long)]
        yes: bool,
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

    /// Distro pinning management
    #[command(subcommand)]
    Distro(DistroCommands),

    /// Canonical package identity
    #[command(subcommand)]
    Canonical(CanonicalCommands),

    /// Package group management
    #[command(subcommand)]
    Groups(GroupsCommands),

    /// Canonical registry management
    #[command(subcommand)]
    Registry(RegistryCommands),

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

    // =========================================================================
    // Self-Update
    // =========================================================================
    /// Update conary itself to the latest version
    #[command(name = "self-update")]
    SelfUpdate {
        #[command(flatten)]
        db: DbArgs,

        /// Check for updates without installing
        #[arg(long)]
        check: bool,

        /// Reinstall even if already at latest version
        #[arg(long)]
        force: bool,

        /// Install a specific version
        #[arg(long)]
        version: Option<String>,
    },

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

    /// TUF trust management
    ///
    /// Manage TUF (The Update Framework) supply chain trust for repositories.
    /// Protects against rollback, freeze, replay, and mix-and-match attacks.
    #[command(subcommand)]
    Trust(TrustCommands),

    /// Federation management
    ///
    /// Manage CAS federation for chunk sharing across machines.
    /// Federation enables bandwidth savings by fetching chunks from
    /// nearby peers instead of the origin server.
    #[command(subcommand)]
    Federation(FederationCommands),

    /// Run the conaryd daemon
    ///
    /// The daemon provides a REST API for package operations with
    /// SSE for progress streaming and job queue management.
    #[cfg(feature = "server")]
    Daemon {
        /// Database path
        #[command(flatten)]
        db: DbArgs,

        /// Unix socket path
        #[arg(long, default_value = "/run/conary/conaryd.sock")]
        socket: String,

        /// Optional TCP bind address (e.g., 127.0.0.1:7890)
        #[arg(long)]
        tcp: Option<String>,

        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,
    },

    /// Run a zero-config Remi LAN proxy
    ///
    /// Starts a lightweight pull-through caching proxy for CCS chunks and
    /// index data. Automatically discovers upstream Remi instances via mDNS
    /// and advertises itself on the LAN for other Conary clients to find.
    ///
    /// Designed for CI pipelines, air-gapped networks, and fleet deployment.
    #[cfg(feature = "server")]
    RemiProxy {
        /// Port to listen on
        #[arg(long, default_value = "7891")]
        port: u16,
        /// Explicit upstream Remi URL (skips mDNS discovery)
        #[arg(long)]
        upstream: Option<String>,
        /// Disable mDNS auto-discovery
        #[arg(long)]
        no_mdns: bool,
        /// Local cache directory
        #[arg(long, default_value = "/var/cache/conary/proxy")]
        cache_dir: String,
        /// Serve only from cache (no upstream)
        #[arg(long)]
        offline: bool,
        /// Don't advertise via mDNS
        #[arg(long)]
        no_advertise: bool,
    },

    /// Run the Remi server (CCS conversion proxy)
    ///
    /// Remi is a server that proxies upstream package repositories and converts
    /// legacy packages (RPM/DEB/Arch) to CCS format on-demand. It handles:
    /// - Package metadata serving
    /// - On-demand CCS conversion with caching
    /// - Chunk storage and LRU eviction
    /// - Rate limiting and DoS protection
    #[cfg(feature = "server")]
    Remi {
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,

        /// Override bind address (default from config or 0.0.0.0:8080)
        #[arg(long)]
        bind: Option<String>,

        /// Override admin bind address (default from config or 127.0.0.1:8081)
        #[arg(long)]
        admin_bind: Option<String>,

        /// Storage root directory (default from config or /conary)
        #[arg(long)]
        storage: Option<String>,

        /// Initialize storage directories if they don't exist
        #[arg(long)]
        init: bool,

        /// Validate configuration and exit
        #[arg(long)]
        validate: bool,
    },
}
