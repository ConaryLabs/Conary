// src/cli/groups.rs
//! Package group management commands

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum GroupsCommands {
    /// List all available package groups
    List {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show members of a group
    Show {
        /// Group name
        name: String,

        /// Show distro-specific view
        #[arg(long)]
        distro: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },
}
