// src/cli/canonical.rs
//! Canonical package identity commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum CanonicalCommands {
    /// Show canonical identity and all implementations for a package
    Show {
        /// Package name (canonical, distro, or AppStream ID)
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Search canonical registry
    Search {
        /// Search query
        query: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// List installed packages without canonical mapping
    Unmapped {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
