// src/cli/distro.rs
//! Distro pinning management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum DistroCommands {
    /// Pin system to a specific distro
    Set {
        /// Distro name (e.g., "ubuntu-noble", "fedora-43")
        distro: String,

        /// Mixing policy: strict, guarded, permissive
        #[arg(long, default_value = "guarded")]
        mixing: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Remove the current distro pin
    Remove {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Show available distros
    List,
    /// Show current pin and affinity stats
    Info {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
    /// Change mixing policy on current pin
    Mixing {
        /// New policy: strict, guarded, permissive
        policy: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
