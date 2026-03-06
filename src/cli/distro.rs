// src/cli/distro.rs
//! Distro pinning management commands

use super::DbArgs;
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

        #[command(flatten)]
        db: DbArgs,
    },
    /// Remove the current distro pin
    Remove {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show available distros
    List,
    /// Show current pin and affinity stats
    Info {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Change mixing policy on current pin
    Mixing {
        /// New policy: strict, guarded, permissive
        policy: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
