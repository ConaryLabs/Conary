// src/cli/distro.rs
//! Legacy distro pinning compatibility commands

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum DistroCommands {
    /// Pin system to a specific distro
    Set {
        /// Distro name (e.g., "ubuntu-noble", "fedora-43")
        distro: String,

        /// Compatibility mixing policy: strict, guarded, permissive
        #[arg(long, default_value = "guarded")]
        mixing: String,

        #[command(flatten)]
        db: DbArgs,
    },
    /// Remove the current compatibility distro pin
    Remove {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show available distros
    List,
    /// Show current compatibility pin and affinity stats
    Info {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Change mixing policy on the current compatibility pin
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
