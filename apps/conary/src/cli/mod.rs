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
pub use ccs::{CcsCommands, CcsOutputFormat};
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
pub use repo::{
    CliArchDatabaseSignature, CliArchKeyringFormat, CliNativeStreamKind,
    CliSecurityAdvisorySupport, RepoAddArgs, RepoCommands,
};
pub use state::StateCommands;
pub use system::{
    DbBackupCommands, NativePackageManager, SystemCommands, TakeoverLevel, UpdateChannelAction,
};
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
    /// Require protected lifecycle execution
    Always,
}

impl From<CliSandboxMode> for SandboxMode {
    fn from(cli: CliSandboxMode) -> Self {
        match cli {
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
#[command(about = "Early-preview Linux package manager with native-package adoption", long_about = None)]
#[command(
    after_help = "Daily workflow examples:\n  sudo conary install nginx --dry-run\n  sudo conary install nginx --yes\n  sudo conary update --dry-run\n  sudo conary system adopt --refresh\n  conary system completions bash > /tmp/conary-completion.bash\n  sudo conary system generation export --path /conary/generations/1 --format qcow2 --output gen1.qcow2\n  conaryd handles durable package jobs with the same apply-intent boundary\n\nAdvanced packaging and platform commands: run 'conary --help-advanced'"
)]
pub struct Cli {
    /// List advanced packaging and platform commands
    #[arg(long = "help-advanced")]
    pub help_advanced: bool,

    /// Increase log verbosity (repeat for more: info, debug, trace)
    #[arg(long = "verbose", action = clap::ArgAction::Count)]
    pub log_verbose: u8,

    /// Silence all logs except errors
    #[arg(short = 'q', long, conflicts_with = "log_verbose")]
    pub quiet: bool,

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

        /// Scriptlet isolation (always protected).
        /// Live-root execution isolates PID/network/mounts and gives
        /// scriptlets private writable /etc and /var layers.
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

        /// Allow downgrading to an older version
        #[arg(long)]
        allow_downgrade: bool,

        /// Convert native-format packages (RPM/DEB/Arch) to CCS during install
        #[arg(long)]
        convert_to_ccs: bool,

        /// Skip optional packages (for collection installs)
        #[arg(long)]
        skip_optional: bool,

        /// How to handle an installed package with recorded external ownership
        ///
        /// preserve: retain its recorded external owner
        /// takeover: explicitly transfer the selected package to Conary
        ///
        /// Dependencies are always satisfied from Conary's installed provider
        /// graph and configured repositories.
        ///
        /// When omitted, the system model's convergence intent supplies the
        /// default; if no model exists, uses the preview cas-backed default.
        #[arg(long, value_enum)]
        ownership: Option<crate::commands::OwnershipMode>,

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

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Scriptlet isolation (always protected).
        /// Live-root execution isolates PID/network/mounts and gives
        /// scriptlets private writable /etc and /var layers.
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

        /// Purge preserved config state and delete adopted package files
        #[arg(long)]
        purge: bool,
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

        /// Scriptlet isolation (always protected).
        /// Live-root execution isolates PID/network/mounts and gives
        /// scriptlets private writable /etc and /var layers.
        #[arg(long, value_enum, default_value_t = CliSandboxMode::Always)]
        sandbox: CliSandboxMode,

        /// How to handle an installed package with recorded external ownership
        ///
        /// preserve: retain its recorded external owner
        /// takeover: explicitly transfer the selected package to Conary
        ///
        /// Dependencies are always satisfied from Conary's installed provider
        /// graph and configured repositories.
        ///
        /// When omitted, the system model's convergence intent supplies the
        /// default; if no model exists, uses the preview cas-backed default.
        #[arg(long, value_enum)]
        ownership: Option<crate::commands::OwnershipMode>,

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

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Scriptlet isolation (always protected).
        /// Live-root execution isolates PID/network/mounts and gives
        /// scriptlets private writable /etc and /var layers.
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
    #[command(hide = true)]
    Cook {
        /// Recipe file or directory containing recipe.toml
        target: Option<String>,

        /// Recipe file to cook
        #[arg(long)]
        recipe: Option<String>,

        /// Output directory for the built package
        #[arg(short, long, default_value = "./dist")]
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

        /// Build inside the sandboxed isolation path
        #[arg(long)]
        isolated: bool,

        /// Emit structured M3a JSON output
        #[arg(long)]
        json: bool,

        /// Private CCS authority key used to sign cooked package output
        #[arg(long, value_name = "PATH")]
        key: Option<String>,

        /// Run hidden experimental record-mode recipe drafting
        #[arg(long)]
        #[arg(hide = true)]
        record: bool,

        /// Directory for record-mode public outputs
        #[arg(long)]
        #[arg(hide = true)]
        record_output: Option<String>,

        /// Trace backend for record mode: auto, fanotify, or inotify
        #[arg(long)]
        #[arg(hide = true)]
        record_backend: Option<String>,

        /// Validate the generated draft recipe with a normal cook
        #[arg(long)]
        #[arg(hide = true)]
        record_validate: bool,

        /// Keep private raw trace fragments for developer debugging
        #[arg(long)]
        #[arg(hide = true)]
        keep_raw_trace: bool,

        /// Command to record, passed after `--`
        #[arg(last = true)]
        #[arg(hide = true)]
        record_command: Vec<String>,
    },

    /// Create a named package recipe scaffold
    #[command(hide = true)]
    New {
        /// Package project name
        name: String,

        /// Output directory
        #[arg(short, long)]
        output: Option<String>,

        /// Overwrite an existing recipe.toml
        #[arg(long)]
        force: bool,
    },

    /// Try a package artifact with explicit keep or rollback
    Try {
        /// Package artifact, or one of: status, rollback, keep
        target: Option<String>,

        /// Activate globally instead of the default namespace try
        #[arg(long)]
        activate: bool,

        /// CCS trust-policy file for a direct package try
        #[arg(long, value_name = "PATH")]
        policy: Option<String>,

        /// Watch an explicit recipe project and refresh a namespace try session
        #[arg(long)]
        watch: bool,

        /// Use the hermetic isolated cook path for watch mode
        #[arg(long)]
        isolated: bool,

        /// Recipe file to use for watch mode
        #[arg(long)]
        recipe: Option<String>,

        /// Private CCS authority key used for each watch-mode cook
        #[arg(long, value_name = "PATH", requires = "watch")]
        key: Option<String>,

        /// Stream watch events as newline-delimited JSON
        #[arg(long)]
        json: bool,

        /// Command to run inside the try session
        #[arg(last = true)]
        run: Vec<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Publish a recipe project or attested CCS artifact
    #[command(hide = true)]
    Publish {
        /// Project-form destination, or artifact path when TARGET is present
        what: String,

        /// Artifact-form destination target
        target: Option<String>,

        /// Recipe file to publish
        #[arg(long)]
        recipe: Option<String>,

        /// Static repository key directory
        #[arg(long)]
        key_dir: Option<String>,

        /// Static repository publish state file
        #[arg(long)]
        state_file: Option<String>,

        /// Refresh expiring TUF metadata even when packages are unchanged
        #[arg(long)]
        refresh: bool,

        /// Rotate the active publish key
        #[arg(long)]
        rotate_publish_key: bool,

        /// Rotate the root identity key
        #[arg(long)]
        rotate_root_key: bool,

        /// Assume yes to all prompts
        #[arg(short = 'y', long)]
        yes: bool,

        /// Emit structured M3a JSON output
        #[arg(long)]
        json: bool,
    },

    /// Audit a recipe for missing build dependencies
    #[command(name = "recipe-audit", hide = true)]
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
    #[command(subcommand, hide = true)]
    Canonical(CanonicalCommands),

    /// Package group management
    #[command(subcommand, hide = true)]
    Groups(GroupsCommands),

    /// Canonical registry management
    #[command(subcommand, hide = true)]
    Registry(RegistryCommands),

    // =========================================================================
    // Advanced/Developer
    // =========================================================================
    /// Local MCP servers for agent integrations
    #[command(subcommand, hide = true)]
    Mcp(McpCommands),

    /// Dependency analysis and advanced queries
    #[command(subcommand, hide = true)]
    Query(QueryCommands),

    /// Native CCS package format
    #[command(subcommand, hide = true)]
    Ccs(CcsCommands),

    /// Derived package management
    #[command(subcommand, hide = true)]
    Derive(DeriveCommands),

    /// System model management
    #[command(subcommand, hide = true)]
    Model(ModelCommands),

    /// Collection management (create, delete, membership)
    #[command(subcommand, hide = true)]
    Collection(CollectionCommands),

    /// Automation maintenance operations
    ///
    /// Manage automated system maintenance including security updates,
    /// orphan cleanup, updates, and integrity repair.
    #[command(subcommand, hide = true)]
    Automation(AutomationCommands),

    // =========================================================================
    // Bootstrap
    // =========================================================================
    /// Bootstrap a complete Conary system from scratch
    #[command(subcommand, hide = true)]
    Bootstrap(BootstrapCommands),

    // =========================================================================
    // Cache
    // =========================================================================
    /// Cache management for derivation outputs
    #[command(subcommand, hide = true)]
    Cache(CacheCommands),

    // =========================================================================
    // Derivation Engine
    // =========================================================================
    /// Derivation engine operations
    #[command(subcommand, hide = true)]
    Derivation(DerivationCommands),

    /// Build profile operations
    #[command(subcommand, hide = true)]
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
    #[command(subcommand, hide = true)]
    Provenance(ProvenanceCommands),

    /// Package capability declarations
    ///
    /// View and validate capability declarations that define what system
    /// resources a package needs (network, filesystem, syscalls).
    #[command(subcommand, hide = true)]
    Capability(CapabilityCommands),

    /// TUF trust management
    ///
    /// Manage TUF (The Update Framework) supply chain trust for repositories.
    /// Protects against rollback, freeze, replay, and mix-and-match attacks.
    #[command(subcommand, hide = true)]
    Trust(TrustCommands),

    /// Derivation verification (chain, rebuild, diverse)
    #[command(subcommand, name = "verify-derivation", hide = true)]
    VerifyDerivation(VerifyCommands),

    /// Generate SBOM from derivation data
    #[command(name = "sbom", hide = true)]
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
    #[command(subcommand, hide = true)]
    Federation(FederationCommands),

    /// Export a generation as an OCI container image
    ///
    /// Packages a generation's EROFS image and CAS objects into a
    /// standards-compliant OCI Image Layout directory that can be
    /// loaded by podman/docker via skopeo.
    #[command(hide = true)]
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

#[derive(Subcommand)]
pub enum McpCommands {
    /// Start the local packaging MCP server on stdio
    Packaging,
}

/// Render the advanced-command listing from the live clap command tree so it
/// cannot drift from the real surface. Everything marked `hide = true` on the
/// root command is, by definition, the advanced tier (this includes `mcp`,
/// which is intentionally part of the truthful surface).
pub fn render_advanced_help() -> String {
    use clap::CommandFactory;

    let cmd = Cli::command();
    let mut rows: Vec<(String, String)> = cmd
        .get_subcommands()
        .filter(|sub| sub.is_hide_set())
        .map(|sub| {
            (
                sub.get_name().to_string(),
                sub.get_about().map(|a| a.to_string()).unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);

    let mut out =
        String::from("Advanced packaging and platform commands (hidden from default help):\n");
    for (name, about) in rows {
        out.push_str(&format!("  {name:<width$}  {about}\n"));
    }
    out.push_str("\nRun 'conary <command> --help' for details on any command.\n");
    out
}

impl std::fmt::Debug for Commands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Commands")
    }
}

#[cfg(test)]
mod tests;
