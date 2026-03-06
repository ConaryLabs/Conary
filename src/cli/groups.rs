// src/cli/groups.rs
//! Package group management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum GroupsCommands {
    /// List all available package groups
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show members of a group
    Show {
        /// Group name
        name: String,

        /// Show distro-specific view
        #[arg(long)]
        distro: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
