// src/cli/canonical.rs
//! Canonical package identity commands

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum CanonicalCommands {
    /// Show canonical identity and all implementations for a package
    Show {
        /// Package name (canonical, distro, or AppStream ID)
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },
    /// Search canonical registry
    Search {
        /// Search query
        query: String,

        #[command(flatten)]
        db: DbArgs,
    },
    /// List installed packages without canonical mapping
    Unmapped {
        #[command(flatten)]
        db: DbArgs,
    },
}
