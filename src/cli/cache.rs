// src/cli/cache.rs

//! CLI definitions for cache commands.

use clap::Subcommand;

use super::DbArgs;

/// Cache management commands for derivation outputs.
#[derive(Subcommand)]
pub enum CacheCommands {
    /// Pre-fetch derivation outputs for offline building
    Populate {
        /// Path to profile TOML
        #[arg(long)]
        profile: String,

        /// Download source tarballs only (not pre-built outputs)
        #[arg(long)]
        sources_only: bool,

        /// Download both pre-built outputs and source tarballs
        #[arg(long, conflicts_with = "sources_only")]
        full: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show cache statistics and substituter peer health
    Status {
        #[command(flatten)]
        db: DbArgs,
    },
}
