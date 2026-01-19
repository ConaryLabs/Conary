// src/cli/collection.rs
//! Package collection/group management commands

use clap::Subcommand;
use super::DbArgs;

#[derive(Subcommand)]
pub enum CollectionCommands {
    /// Create a new collection (package group)
    Create {
        /// Name of the collection
        name: String,

        /// Description of the collection
        #[arg(long)]
        description: Option<String>,

        /// Comma-separated list of member packages
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// List all collections
    List {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Show details of a collection
    Show {
        /// Name of the collection
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Add packages to a collection
    Add {
        /// Name of the collection
        name: String,

        /// Packages to add (comma-separated)
        #[arg(value_delimiter = ',')]
        members: Vec<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Remove packages from a collection
    Remove {
        /// Name of the collection
        name: String,

        /// Packages to remove (comma-separated)
        #[arg(value_delimiter = ',')]
        members: Vec<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Delete a collection
    Delete {
        /// Name of the collection
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
