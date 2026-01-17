// src/cli/model.rs
//! System model management commands

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

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
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

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

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

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show details of differences (verbose output)
        #[arg(short, long)]
        verbose: bool,
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

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Add a comment/description to the model
        #[arg(long)]
        description: Option<String>,
    },

    /// Publish a system model as a versioned collection to a repository
    ///
    /// Converts a system.toml into a CCS collection package and stores it
    /// in a local repository. This allows other systems to include the
    /// model using the [include] directive.
    ///
    /// Note: Currently only supports local repositories (file:// URLs).
    /// Remote publishing requires repository authentication (not yet implemented).
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

        /// Repository to publish to (must be a local repository)
        #[arg(short, long)]
        repo: String,

        /// Description of the collection
        #[arg(long)]
        description: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
