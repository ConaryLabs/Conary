// src/cli/trust.rs

//! CLI definitions for TUF trust management commands

use clap::Subcommand;
use super::DbArgs;

/// TUF trust management commands
#[derive(Subcommand)]
pub enum TrustCommands {
    /// Generate a new Ed25519 key pair for a TUF role
    KeyGen {
        /// Role to generate key for (root, targets, snapshot, timestamp)
        role: String,

        /// Output directory for key files
        #[arg(short, long, default_value = ".")]
        output: String,
    },

    /// Bootstrap TUF for a repository with initial root metadata
    Init {
        /// Repository name
        repo: String,

        /// Path to root.json (signed root metadata)
        #[arg(long)]
        root: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Enable TUF verification for a repository
    Enable {
        /// Repository name
        repo: String,

        /// Optional TUF metadata URL (defaults to <repo_url>/tuf)
        #[arg(long)]
        tuf_url: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Disable TUF verification for a repository (unsafe)
    Disable {
        /// Repository name
        repo: String,

        /// Confirm the unsafe operation
        #[arg(long)]
        force: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show TUF metadata versions and expiry for a repository
    Status {
        /// Repository name
        repo: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Verify all TUF metadata for a repository is valid
    Verify {
        /// Repository name
        repo: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Sign targets metadata (server-side)
    #[cfg(feature = "server")]
    SignTargets {
        /// Repository name
        repo: String,

        /// Path to signing key
        #[arg(long)]
        key: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Rotate a TUF role key
    #[cfg(feature = "server")]
    RotateKey {
        /// Role to rotate (root, targets, snapshot, timestamp)
        role: String,

        /// Path to old key file
        #[arg(long)]
        old_key: String,

        /// Path to new key file
        #[arg(long)]
        new_key: String,

        /// Path to root key file (for signing the new root)
        #[arg(long)]
        root_key: String,

        /// Repository name
        repo: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
