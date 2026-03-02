// src/cli/model.rs
//! System model management commands

use super::{CommonArgs, DbArgs};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ModelCommands {
    /// Show what changes are needed to reach the model state
    ///
    /// Compares the system model file (default: /etc/conary/system.toml)
    /// against the current system state and shows what packages would be
    /// installed, removed, or updated to reach the desired state.
    Diff {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Use cached remote collections only (no network)
        #[arg(long)]
        offline: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Apply the system model to reach the desired state
    ///
    /// Installs, removes, and updates packages to match the system model.
    /// This is essentially "sync to model" - the system will be modified
    /// to match what's declared in the model file.
    Apply {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        #[command(flatten)]
        common: CommonArgs,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip optional packages
        #[arg(long)]
        skip_optional: bool,

        /// Force remove packages not in model (strict mode)
        #[arg(long)]
        strict: bool,

        /// Skip autoremove after applying
        #[arg(long)]
        no_autoremove: bool,

        /// Use cached remote collections only (no network)
        #[arg(long)]
        offline: bool,
    },

    /// Check if system state matches the model
    ///
    /// Returns success (exit 0) if the system matches the model,
    /// or failure (exit 1) if there are differences.
    /// Useful for drift detection in CI/CD or monitoring.
    Check {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        #[command(flatten)]
        db: DbArgs,

        /// Show details of differences (verbose output)
        #[arg(short, long)]
        verbose: bool,

        /// Use cached remote collections only (no network)
        #[arg(long)]
        offline: bool,
    },

    /// Create a model file from current system state
    ///
    /// Captures the current system state (explicit packages, pins)
    /// and writes it as a system model file. Useful for creating
    /// a baseline or for reproducibility.
    Snapshot {
        /// Output path for the model file
        #[arg(short, long, default_value = "system.toml")]
        output: String,

        #[command(flatten)]
        db: DbArgs,

        /// Add a comment/description to the model
        #[arg(long)]
        description: Option<String>,
    },

    /// Lock remote include hashes for reproducibility
    ///
    /// Resolves all remote includes and records their content hashes
    /// in a model.lock file. This prevents silent upstream changes.
    Lock {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Output lock file path (default: alongside model file)
        #[arg(short, long)]
        output: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Update locked remote includes
    ///
    /// Force-refreshes all remote includes, compares against the lock
    /// file, and updates the lock with new hashes. Shows what changed.
    Update {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Compare local state against remote model collections
    ///
    /// Fetches each remote include from the model, optionally forcing
    /// a refresh, and reports differences between the remote model's
    /// expected state and what's actually installed locally.
    RemoteDiff {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Force refresh remote collections (bypass cache)
        #[arg(long)]
        refresh: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Publish a system model as a versioned collection to a repository
    ///
    /// Converts a system.toml into a CCS collection package and stores it
    /// in a repository. Supports both local (file://) and remote (http/https)
    /// repositories. For remote repos, the collection is sent via HTTP PUT
    /// to the Remi server's admin API.
    Publish {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Name for the published collection (will add group- prefix if missing)
        #[arg(short, long)]
        name: String,

        /// Version string for the published collection
        #[arg(short, long)]
        version: String,

        /// Repository to publish to
        #[arg(short, long)]
        repo: String,

        /// Description of the collection
        #[arg(long)]
        description: Option<String>,

        /// Force overwrite existing collection on remote
        #[arg(long)]
        force: bool,

        /// Path to Ed25519 signing key for collection signature
        #[arg(long)]
        sign_key: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },
}
