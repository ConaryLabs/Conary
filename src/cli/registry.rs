// src/cli/registry.rs
//! Registry management commands

use super::DbArgs;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum RegistryCommands {
    /// Sync canonical registry from rules files
    Update {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show mapping coverage statistics
    Stats {
        #[command(flatten)]
        db: DbArgs,
    },
}
