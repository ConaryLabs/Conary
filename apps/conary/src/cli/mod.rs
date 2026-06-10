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

use clap::{Args, Parser, Subcommand, ValueEnum};
use conary_core::scriptlet::SandboxMode;

mod automation;
mod bootstrap;
mod cache;
mod canonical;
mod capability;
mod ccs;
mod collection;
mod config;
mod derivation;
mod derive;
mod distro;
mod federation;
mod generation;
mod groups;
mod label;
mod model;
mod profile;
mod provenance;
mod query;
mod redirect;
mod registry;
mod repo;
mod state;
mod system;
mod trigger;
mod trust;
mod verify;

pub use automation::AutomationCommands;
pub use bootstrap::BootstrapCommands;
pub use cache::CacheCommands;
pub use canonical::CanonicalCommands;
pub use capability::CapabilityCommands;
pub use ccs::CcsCommands;
pub use collection::CollectionCommands;
pub use config::ConfigCommands;
pub use derivation::DerivationCommands;
pub use derive::DeriveCommands;
pub use distro::DistroCommands;
pub use federation::FederationCommands;
pub use generation::GenerationCommands;
pub use groups::GroupsCommands;
pub use label::LabelCommands;
pub use model::ModelCommands;
pub use profile::ProfileCommands;
pub use provenance::ProvenanceCommands;
pub use query::QueryCommands;
pub use redirect::RedirectCommands;
pub use registry::RegistryCommands;
pub use repo::{CliSecurityAdvisorySupport, RepoCommands};
pub use state::StateCommands;
pub use system::{DbBackupCommands, SystemCommands, TakeoverLevel, UpdateChannelAction};
pub use trigger::TriggerCommands;
pub use trust::TrustCommands;
pub use verify::VerifyCommands;

/// CLI-side sandbox mode that maps to `conary_core::scriptlet::SandboxMode`.
///
/// We cannot derive `ValueEnum` on the core type directly (it lives in another
/// crate), so this thin wrapper gives clap type-safe parsing while keeping the
/// conversion trivial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliSandboxMode {
    /// No sandboxing - direct execution
    Never,
    /// Automatic - sandbox based on script risk analysis
    Auto,
    /// Always sandbox all scripts
    Always,
}

impl From<CliSandboxMode> for SandboxMode {
    fn from(cli: CliSandboxMode) -> Self {
        match cli {
            CliSandboxMode::Never => SandboxMode::None,
            CliSandboxMode::Auto => SandboxMode::Auto,
            CliSandboxMode::Always => SandboxMode::Always,
        }
    }
}

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
#[command(
    after_help = "Daily workflow examples:\n  conary install nginx --dry-run\n  conary install nginx --yes\n  conary update --dry-run\n  conary system adopt --refresh\n  conary system completions bash > /tmp/conary-completion.bash\n  conary system generation export --path /conary/generations/1 --format qcow2 --output gen1.qcow2\n  conaryd handles durable package jobs with the same apply-intent boundary"
)]
pub struct Cli {
    /// Use seccomp warn mode for scriptlets instead of enforcing blocked syscalls
    #[arg(long, global = true)]
    pub seccomp_warn: bool,

    /// Deprecated compatibility alias for old persisted retry commands.
    #[arg(long, global = true, hide = true)]
    pub allow_live_system_mutation: bool,

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

        /// Suppress hooks where safe; does not bypass required legacy replay
        #[arg(long)]
        no_scripts: bool,

        /// Allow same-source raw legacy scriptlet replay when the bundle, target, sandbox, and local policy all pass
        #[arg(long)]
        allow_legacy_replay: bool,

        /// Additionally allow explicitly compatible foreign raw replay only under permissive host policy
        #[arg(long)]
        allow_foreign_legacy_replay: bool,

        /// Scriptlet isolation: auto, always, never (default: always).
        /// Protected modes isolate PID/network/mounts and give live-root
        /// scriptlets private writable /etc and /var layers
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

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

        /// How to handle dependencies: satisfy, adopt, takeover
        ///
        /// satisfy:  dependencies on disk satisfy requirements without changes
        /// adopt:    auto-adopt system dependencies into Conary tracking
        /// takeover: download CCS versions from Remi and fully own dependencies
        ///
        /// When omitted, the system model's convergence intent supplies the
        /// default; if no model exists, uses the preview cas-backed default.
        #[arg(long, value_enum)]
        dep_mode: Option<crate::commands::DepMode>,

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

        /// Specific architecture to remove when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,

        /// Suppress hooks where safe; does not bypass required legacy replay
        #[arg(long)]
        no_scripts: bool,

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Allow same-source raw legacy scriptlet replay when the bundle, target, sandbox, and local policy all pass
        #[arg(long)]
        allow_legacy_replay: bool,

        /// Additionally allow explicitly compatible foreign raw replay only under permissive host policy
        #[arg(long)]
        allow_foreign_legacy_replay: bool,

        /// Scriptlet isolation: auto, always, never (default: always).
        /// Protected modes isolate PID/network/mounts and give live-root
        /// scriptlets private writable /etc and /var layers
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

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

        /// Installed package version to select when multiple variants are installed
        #[arg(short, long)]
        version: Option<String>,

        /// Installed package architecture to select when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,

        /// Only apply updates with trusted security-advisory metadata
        #[arg(long)]
        security: bool,

        /// Show what would be updated without making changes
        #[arg(long)]
        dry_run: bool,

        /// Suppress hooks where safe; does not bypass required legacy replay
        #[arg(long)]
        no_scripts: bool,

        /// Allow same-source raw legacy scriptlet replay when the bundle, target, sandbox, and local policy all pass
        #[arg(long)]
        allow_legacy_replay: bool,

        /// Additionally allow explicitly compatible foreign raw replay only under permissive host policy
        #[arg(long)]
        allow_foreign_legacy_replay: bool,

        /// Scriptlet isolation: auto, always, never (default: always).
        /// Protected modes isolate PID/network/mounts and give live-root
        /// scriptlets private writable /etc and /var layers
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

        /// How to handle dependencies: satisfy, adopt, takeover
        ///
        /// satisfy:  dependencies on disk satisfy requirements without changes
        /// adopt:    auto-adopt system dependencies into Conary tracking
        /// takeover: download CCS versions from Remi and fully own dependencies
        ///
        /// When omitted, the system model's convergence intent supplies the
        /// default; if no model exists, uses the preview cas-backed default.
        #[arg(long, value_enum)]
        dep_mode: Option<crate::commands::DepMode>,

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

        /// Installed package version to select when multiple variants are installed
        #[arg(short, long)]
        version: Option<String>,

        /// Installed package architecture to select when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,

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

        /// Suppress hooks where safe; does not bypass required legacy replay
        #[arg(long)]
        no_scripts: bool,

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Allow same-source raw legacy scriptlet replay when the bundle, target, sandbox, and local policy all pass
        #[arg(long)]
        allow_legacy_replay: bool,

        /// Additionally allow explicitly compatible foreign raw replay only under permissive host policy
        #[arg(long)]
        allow_foreign_legacy_replay: bool,

        /// Scriptlet isolation: auto, always, never (default: always).
        /// Protected modes isolate PID/network/mounts and give live-root
        /// scriptlets private writable /etc and /var layers
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,
    },

    /// Pin a package to prevent updates and removal
    Pin {
        /// Package name to pin
        package_name: String,

        /// Installed package version to select when multiple variants are installed
        #[arg(short, long)]
        version: Option<String>,

        /// Installed package architecture to select when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Unpin a package to allow updates and removal
    Unpin {
        /// Package name to unpin
        package_name: String,

        /// Installed package version to select when multiple variants are installed
        #[arg(short, long)]
        version: Option<String>,

        /// Installed package architecture to select when multiple variants are installed
        #[arg(long = "arch")]
        architecture: Option<String>,

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

    /// Audit a recipe for missing build dependencies
    #[command(name = "recipe-audit")]
    RecipeAudit {
        /// Path to recipe file
        recipe: Option<String>,

        /// Audit all recipes in the recipes/ directory
        #[arg(long)]
        all: bool,

        /// Run build-time tracing (slower, more thorough)
        #[arg(long)]
        trace: bool,
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

    /// Automation maintenance operations
    ///
    /// Manage automated system maintenance including security updates,
    /// orphan cleanup, updates, and integrity repair.
    #[command(subcommand)]
    Automation(AutomationCommands),

    // =========================================================================
    // Bootstrap
    // =========================================================================
    /// Bootstrap a complete Conary system from scratch
    #[command(subcommand)]
    Bootstrap(BootstrapCommands),

    // =========================================================================
    // Cache
    // =========================================================================
    /// Cache management for derivation outputs
    #[command(subcommand)]
    Cache(CacheCommands),

    // =========================================================================
    // Derivation Engine
    // =========================================================================
    /// Derivation engine operations
    #[command(subcommand)]
    Derivation(DerivationCommands),

    /// Build profile operations
    #[command(subcommand)]
    Profile(ProfileCommands),

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

        /// Skip signature verification (NOT RECOMMENDED)
        #[arg(long)]
        no_verify: bool,

        /// Verify a detached signature over a SHA-256 digest without downloading an update
        #[arg(long)]
        verify_sha256: Option<String>,

        /// Path to a detached signature file for offline self-update verification
        #[arg(long)]
        verify_signature_file: Option<String>,

        /// Additional trusted Ed25519 public key (hex) for offline self-update verification
        #[arg(long = "trusted-key")]
        trusted_keys: Vec<String>,

        /// Print the configured self-update trusted keys and exit
        #[arg(long)]
        print_trusted_keys: bool,
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

    /// Derivation verification (chain, rebuild, diverse)
    #[command(subcommand, name = "verify-derivation")]
    VerifyDerivation(VerifyCommands),

    /// Generate SBOM from derivation data
    #[command(name = "sbom")]
    Sbom {
        /// Generate from a profile
        #[arg(long)]
        profile: Option<String>,

        /// Generate for a single derivation
        #[arg(long)]
        derivation: Option<String>,

        /// Output file (default: stdout)
        #[arg(long, short)]
        output: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Federation management
    ///
    /// Manage CAS federation for chunk sharing across machines.
    /// Federation enables bandwidth savings by fetching chunks from
    /// nearby peers instead of the origin server.
    #[command(subcommand)]
    Federation(FederationCommands),

    /// Export a generation as an OCI container image
    ///
    /// Packages a generation's EROFS image and CAS objects into a
    /// standards-compliant OCI Image Layout directory that can be
    /// loaded by podman/docker via skopeo.
    Export {
        /// Generation number to export (default: current active generation)
        #[arg(short, long)]
        generation: Option<i64>,

        /// Output directory for the OCI image layout
        #[arg(short, long)]
        output: String,

        /// Expected CAS objects directory; the generation artifact is authoritative
        #[arg(long, default_value = "/conary/objects")]
        objects_dir: String,
        // NOTE: OCI is the only supported export format. No format flag is needed.
    },
}

#[cfg(test)]
mod tests {
    use super::{CcsCommands, Cli, CliSandboxMode, Commands, GenerationCommands, SystemCommands};
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_accepts_seccomp_warn_flag() {
        Cli::try_parse_from(["conary", "--seccomp-warn", "list"])
            .expect("--seccomp-warn should parse as a global CLI flag");
    }

    #[test]
    fn install_defaults_to_always_sandbox() {
        let cli = Cli::try_parse_from(["conary", "install", "bash"]).unwrap();
        match cli.command {
            Some(Commands::Install { sandbox, .. }) => {
                assert_eq!(sandbox, CliSandboxMode::Always);
            }
            _ => panic!("expected install command"),
        }
    }

    #[test]
    fn install_accepts_legacy_replay_flags_defaulting_false() {
        let cli = Cli::try_parse_from(["conary", "install", "bash"]).unwrap();
        match cli.command {
            Some(Commands::Install {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(!allow_legacy_replay);
                assert!(!allow_foreign_legacy_replay);
            }
            _ => panic!("expected install command"),
        }

        let cli = Cli::try_parse_from([
            "conary",
            "install",
            "bash",
            "--allow-legacy-replay",
            "--allow-foreign-legacy-replay",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Install {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(allow_legacy_replay);
                assert!(allow_foreign_legacy_replay);
            }
            _ => panic!("expected install command"),
        }
    }

    #[test]
    fn update_defaults_to_always_sandbox() {
        let cli = Cli::try_parse_from(["conary", "update"]).unwrap();
        match cli.command {
            Some(Commands::Update { sandbox, .. }) => {
                assert_eq!(sandbox, CliSandboxMode::Always);
            }
            _ => panic!("expected update command"),
        }
    }

    #[test]
    fn update_accepts_legacy_replay_flags_and_no_scripts_defaulting_false() {
        let cli = Cli::try_parse_from(["conary", "update"]).unwrap();
        match cli.command {
            Some(Commands::Update {
                no_scripts,
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(!no_scripts);
                assert!(!allow_legacy_replay);
                assert!(!allow_foreign_legacy_replay);
            }
            _ => panic!("expected update command"),
        }

        let cli = Cli::try_parse_from([
            "conary",
            "update",
            "bash",
            "--no-scripts",
            "--allow-legacy-replay",
            "--allow-foreign-legacy-replay",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Update {
                no_scripts,
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(no_scripts);
                assert!(allow_legacy_replay);
                assert!(allow_foreign_legacy_replay);
            }
            _ => panic!("expected update command"),
        }
    }

    #[test]
    fn remove_accepts_legacy_replay_flags_defaulting_false() {
        let cli = Cli::try_parse_from(["conary", "remove", "bash"]).unwrap();
        match cli.command {
            Some(Commands::Remove {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(!allow_legacy_replay);
                assert!(!allow_foreign_legacy_replay);
            }
            _ => panic!("expected remove command"),
        }

        let cli = Cli::try_parse_from([
            "conary",
            "remove",
            "bash",
            "--allow-legacy-replay",
            "--allow-foreign-legacy-replay",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Remove {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(allow_legacy_replay);
                assert!(allow_foreign_legacy_replay);
            }
            _ => panic!("expected remove command"),
        }
    }

    #[test]
    fn autoremove_accepts_legacy_replay_flags_defaulting_false() {
        let cli = Cli::try_parse_from(["conary", "autoremove"]).unwrap();
        match cli.command {
            Some(Commands::Autoremove {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(!allow_legacy_replay);
                assert!(!allow_foreign_legacy_replay);
            }
            _ => panic!("expected autoremove command"),
        }

        let cli = Cli::try_parse_from([
            "conary",
            "autoremove",
            "--allow-legacy-replay",
            "--allow-foreign-legacy-replay",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Autoremove {
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            }) => {
                assert!(allow_legacy_replay);
                assert!(allow_foreign_legacy_replay);
            }
            _ => panic!("expected autoremove command"),
        }
    }

    #[test]
    fn ccs_install_accepts_legacy_replay_flags_and_no_scripts_defaulting_false() {
        let cli = Cli::try_parse_from(["conary", "ccs", "install", "fixture.ccs"]).unwrap();
        match cli.command {
            Some(Commands::Ccs(CcsCommands::Install {
                no_scripts,
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            })) => {
                assert!(!no_scripts);
                assert!(!allow_legacy_replay);
                assert!(!allow_foreign_legacy_replay);
            }
            _ => panic!("expected ccs install command"),
        }

        let cli = Cli::try_parse_from([
            "conary",
            "ccs",
            "install",
            "fixture.ccs",
            "--no-scripts",
            "--allow-legacy-replay",
            "--allow-foreign-legacy-replay",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Ccs(CcsCommands::Install {
                no_scripts,
                allow_legacy_replay,
                allow_foreign_legacy_replay,
                ..
            })) => {
                assert!(no_scripts);
                assert!(allow_legacy_replay);
                assert!(allow_foreign_legacy_replay);
            }
            _ => panic!("expected ccs install command"),
        }
    }

    #[test]
    fn update_dep_mode_omission_is_model_derived() {
        let cli = Cli::try_parse_from(["conary", "update"]).unwrap();
        match cli.command {
            Some(Commands::Update { dep_mode, .. }) => {
                assert_eq!(dep_mode, None);
            }
            _ => panic!("expected update command"),
        }
    }

    #[test]
    fn update_dep_mode_help_is_model_derived() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("update")
            .expect("update subcommand should exist")
            .render_long_help()
            .to_string();
        let hard_coded_default = ["[default: ", "satisfy]"].concat();

        assert!(
            !help.contains(&hard_coded_default),
            "update dep-mode must not hard-code satisfy as its CLI default:\n{help}"
        );
    }

    fn command_help(command_name: &str) -> String {
        let mut command = Cli::command();
        command
            .find_subcommand_mut(command_name)
            .unwrap_or_else(|| panic!("{command_name} subcommand should exist"))
            .render_long_help()
            .to_string()
    }

    #[test]
    fn installed_package_commands_expose_arch_selector() {
        for command_name in ["remove", "update", "pin", "unpin", "list"] {
            let help = command_help(command_name);
            assert!(
                help.contains("--arch"),
                "{command_name} help should expose --arch:\n{help}"
            );
        }
    }

    #[test]
    fn installed_package_commands_expose_version_selector() {
        for command_name in ["update", "pin", "unpin", "list"] {
            let help = command_help(command_name);
            assert!(
                help.contains("--version"),
                "{command_name} help should expose --version:\n{help}"
            );
        }
    }

    #[test]
    fn export_rejects_legacy_db_argument() {
        let err = match Cli::try_parse_from([
            "conary", "export", "--output", "oci-out", "--db", "old.db",
        ]) {
            Ok(_) => panic!("legacy export --db argument should be rejected"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("unexpected argument '--db'"),
            "legacy export --db argument should be rejected, got {err}"
        );
    }

    #[test]
    fn cli_accepts_allow_live_system_mutation_as_global_flag() {
        let cli = Cli::try_parse_from([
            "conary",
            "--allow-live-system-mutation",
            "system",
            "generation",
            "switch",
            "7",
        ])
        .expect("global live-mutation flag should parse before nested commands");

        assert!(cli.allow_live_system_mutation);
    }

    #[test]
    fn cli_accepts_yes_for_remove_autoremove_and_ccs_install() {
        let remove = Cli::try_parse_from(["conary", "remove", "nginx", "--yes"]).unwrap();
        assert!(matches!(
            remove.command,
            Some(Commands::Remove { yes: true, .. })
        ));

        let autoremove = Cli::try_parse_from(["conary", "autoremove", "--yes"]).unwrap();
        assert!(matches!(
            autoremove.command,
            Some(Commands::Autoremove { yes: true, .. })
        ));

        let ccs = Cli::try_parse_from(["conary", "ccs", "install", "pkg.ccs", "--yes"]).unwrap();
        assert!(matches!(
            ccs.command,
            Some(Commands::Ccs(crate::cli::CcsCommands::Install {
                yes: true,
                ..
            }))
        ));
    }

    #[test]
    fn cli_accepts_yes_for_state_and_generation_apply_commands() {
        let revert =
            Cli::try_parse_from(["conary", "system", "state", "revert", "1", "--yes"]).unwrap();
        assert!(matches!(
            revert.command,
            Some(Commands::System(crate::cli::SystemCommands::State(
                crate::cli::StateCommands::Revert { yes: true, .. }
            )))
        ));

        let build = Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "build",
            "--summary",
            "after install",
            "--yes",
        ])
        .unwrap();
        assert!(matches!(
            build.command,
            Some(Commands::System(crate::cli::SystemCommands::Generation(
                crate::cli::GenerationCommands::Build { yes: true, .. }
            )))
        ));
    }

    #[test]
    fn parses_system_unadopt_all() {
        let cli = Cli::try_parse_from(["conary", "system", "unadopt", "--all"])
            .expect("system unadopt --all should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Unadopt {
                packages,
                all,
                dry_run,
                keep_hooks,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(all);
                assert!(!dry_run);
                assert!(!keep_hooks);
            }
            _ => panic!("expected system unadopt command"),
        }
    }

    #[test]
    fn parses_system_unadopt_package_dry_run() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "unadopt",
            "curl",
            "--dry-run",
            "--keep-hooks",
        ])
        .expect("system unadopt curl --dry-run should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Unadopt {
                packages,
                all,
                dry_run,
                keep_hooks,
                ..
            })) => {
                assert_eq!(packages, vec!["curl"]);
                assert!(!all);
                assert!(dry_run);
                assert!(keep_hooks);
            }
            _ => panic!("expected system unadopt command"),
        }
    }

    #[test]
    fn rejects_system_unadopt_without_scope() {
        let err = match Cli::try_parse_from(["conary", "system", "unadopt"]) {
            Ok(_) => panic!("system unadopt must require --all or package names"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn rejects_system_unadopt_all_with_packages() {
        let err = match Cli::try_parse_from(["conary", "system", "unadopt", "--all", "curl"]) {
            Ok(_) => panic!("system unadopt --all must reject package names"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_system_adopt_system_dry_run_filters() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--system",
            "--dry-run",
            "--pattern",
            "lib*",
            "--exclude",
            "kernel*",
            "--explicit-only",
        ])
        .expect("system adopt --system dry-run filters should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                pattern,
                exclude,
                explicit_only,
                refresh,
                convert,
                sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(!full);
                assert!(system);
                assert!(!status);
                assert!(dry_run);
                assert_eq!(pattern.as_deref(), Some("lib*"));
                assert_eq!(exclude.as_deref(), Some("kernel*"));
                assert!(explicit_only);
                assert!(!refresh);
                assert!(!convert);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_refresh_quiet_from_sync_hook() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
        ])
        .expect("installed sync hook refresh path should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                refresh,
                convert,
                sync_hook,
                quiet,
                from_sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(!full);
                assert!(!system);
                assert!(!status);
                assert!(!dry_run);
                assert!(refresh);
                assert!(!convert);
                assert!(!sync_hook);
                assert!(quiet);
                assert!(from_sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn rejects_system_adopt_from_sync_hook_with_full() {
        let err = match Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
            "--full",
        ]) {
            Ok(_) => panic!("--from-sync-hook must conflict with --full"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_system_adopt_convert_dry_run_jobs() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--convert",
            "--dry-run",
            "--jobs",
            "4",
            "--no-chunking",
        ])
        .expect("system adopt --convert dry-run jobs should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                convert,
                dry_run,
                jobs,
                no_chunking,
                system,
                status,
                refresh,
                sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(convert);
                assert!(dry_run);
                assert_eq!(jobs, Some(4));
                assert!(no_chunking);
                assert!(!system);
                assert!(!status);
                assert!(!refresh);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_sync_hook_remove_hook() {
        let cli =
            Cli::try_parse_from(["conary", "system", "adopt", "--sync-hook", "--remove-hook"])
                .expect("system adopt --sync-hook --remove-hook should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                sync_hook,
                remove_hook,
                system,
                status,
                refresh,
                convert,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(sync_hook);
                assert!(remove_hook);
                assert!(!system);
                assert!(!status);
                assert!(!refresh);
                assert!(!convert);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_package_dry_run_refusal_surface() {
        let cli = Cli::try_parse_from(["conary", "system", "adopt", "curl", "--dry-run"])
            .expect("single-package dry-run should parse before runtime refuses it");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                refresh,
                convert,
                sync_hook,
                ..
            })) => {
                assert_eq!(packages, vec!["curl".to_string()]);
                assert!(!full);
                assert!(!system);
                assert!(!status);
                assert!(dry_run);
                assert!(!refresh);
                assert!(!convert);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn rejects_system_adopt_package_with_refresh_mode() {
        let err = match Cli::try_parse_from(["conary", "system", "adopt", "curl", "--refresh"]) {
            Ok(_) => panic!("package adopt must conflict with --refresh mode"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_system_adopt_quiet_without_refresh() {
        let err = match Cli::try_parse_from(["conary", "system", "adopt", "--quiet"]) {
            Ok(_) => panic!("--quiet must remain scoped to --refresh"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(
            rendered.contains("--refresh"),
            "quiet error should point users back to --refresh: {rendered}"
        );
    }

    #[test]
    fn cli_rejects_bootstrap_image_from_generation() {
        let err = match Cli::try_parse_from([
            "conary",
            "bootstrap",
            "image",
            "--from-generation",
            "output/generations/1",
        ]) {
            Ok(_) => panic!("--from-generation must be removed from bootstrap image"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn cli_accepts_generation_export_from_explicit_path() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "export",
            "--path",
            "output/generations/1",
            "--format",
            "raw",
            "--output",
            "gen1.raw",
        ])
        .expect("generation export from path should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Generation(GenerationCommands::Export {
                generation,
                path,
                format,
                output,
                size,
            }))) => {
                assert_eq!(generation, None);
                assert_eq!(path.as_deref(), Some("output/generations/1"));
                assert_eq!(format, "raw");
                assert_eq!(output, "gen1.raw");
                assert_eq!(size, None);
            }
            _ => panic!("expected system generation export command"),
        }
    }

    #[test]
    fn cli_accepts_generation_db_backup_verification_and_recovery() {
        let verify = Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "verify-db-backup",
            "--current",
        ])
        .expect("generation DB backup verification should parse");
        match verify.command {
            Some(Commands::System(SystemCommands::Generation(
                GenerationCommands::VerifyDbBackup {
                    current,
                    generation,
                    ..
                },
            ))) => {
                assert!(current);
                assert_eq!(generation, None);
            }
            _ => panic!("expected generation verify-db-backup command"),
        }

        let recover = Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "recover-db",
            "--generation",
            "7",
            "--dry-run",
        ])
        .expect("generation DB recovery dry-run should parse");
        match recover.command {
            Some(Commands::System(SystemCommands::Generation(GenerationCommands::RecoverDb {
                generation,
                dry_run,
                yes,
                ..
            }))) => {
                assert_eq!(generation, 7);
                assert!(dry_run);
                assert!(!yes);
            }
            _ => panic!("expected generation recover-db command"),
        }
    }

    #[test]
    fn cli_rejects_generation_export_path_and_number_together() {
        let err = match Cli::try_parse_from([
            "conary",
            "system",
            "generation",
            "export",
            "7",
            "--path",
            "output/generations/1",
            "--format",
            "raw",
            "--output",
            "gen.raw",
        ]) {
            Ok(_) => panic!("--path must conflict with positional generation number"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn self_update_accepts_offline_signature_verification_flags() {
        let cli = Cli::try_parse_from([
            "conary",
            "self-update",
            "--verify-sha256",
            "abc123def456",
            "--verify-signature-file",
            "/tmp/conary.sig",
            "--trusted-key",
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        ])
        .expect("offline self-update verification flags should parse");

        match cli.command {
            Some(Commands::SelfUpdate { .. }) => {}
            _ => panic!("expected self-update command"),
        }
    }
}
