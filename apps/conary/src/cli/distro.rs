// src/cli/distro.rs
//! Source-feed pinning and selection policy commands

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum DistroCommands {
    /// Pin system to a specific distro
    Set {
        /// Distro name (e.g., "ubuntu-26.04", "fedora-44")
        distro: String,

        /// Source mixing policy: strict, guarded, permissive
        #[arg(long, default_value = "guarded")]
        mixing: String,

        #[command(flatten)]
        db: DbArgs,
    },
    /// Remove the current source-feed pin
    Remove {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show available distros
    List {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show the current source-feed pin and affinity stats
    Info {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Change mixing policy on the current source-feed pin
    Mixing {
        /// New policy: strict, guarded, permissive
        policy: String,

        #[command(flatten)]
        db: DbArgs,
    },
    /// Change source-selection ranking mode
    SelectionMode {
        /// New mode: policy, latest
        mode: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
