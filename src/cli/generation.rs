// src/cli/generation.rs
//! CLI definitions for generation management

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum GenerationCommands {
    /// List all generations
    List,

    /// Build a new generation from current system state
    Build {
        /// Summary description for this generation
        #[arg(long, default_value = "Manual generation build")]
        summary: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Switch to a specific generation
    Switch {
        /// Generation number to switch to
        number: i64,

        /// Reboot after switching
        #[arg(long)]
        reboot: bool,
    },

    /// Roll back to the previous generation
    Rollback,

    /// Remove old generations
    Gc {
        /// Number of generations to keep (default: 3)
        #[arg(long, default_value = "3")]
        keep: usize,
    },

    /// Show detailed info about a generation
    Info {
        /// Generation number
        number: i64,
    },
}
