// src/cli/verify.rs

//! CLI definitions for verification commands.

use clap::Subcommand;

use super::DbArgs;

/// Verification commands for derivation integrity.
#[derive(Subcommand)]
pub enum VerifyCommands {
    /// Trace all packages in a profile back to the seed
    Chain {
        /// Path to profile TOML
        #[arg(long)]
        profile: String,

        /// Show full provenance details
        #[arg(long)]
        verbose: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Compare builds from two different seeds
    Diverse {
        /// Profile from first seed build
        #[arg(long)]
        profile_a: String,

        /// Profile from second seed build
        #[arg(long)]
        profile_b: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
