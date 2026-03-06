// src/cli/registry.rs
//! Registry management commands

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum RegistryCommands {
    /// Sync canonical registry from rules files
    Update {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show mapping coverage statistics
    Stats {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
