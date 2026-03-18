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

    /// Remove old generations and unreferenced CAS objects
    Gc {
        /// Number of generations to keep (default: 3)
        #[arg(long, default_value = "3")]
        keep: usize,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show detailed info about a generation
    Info {
        /// Generation number
        number: i64,
    },

    /// Recover the system to a bootable generation
    ///
    /// Implements a 4-step fallback:
    ///   1. Mount the generation pointed to by /conary/current if its EROFS image is valid.
    ///   2. Rebuild the EROFS image from DB state if the current image is missing/truncated.
    ///   3. Scan /conary/generations/ descending and try each intact EROFS image.
    ///   4. Return an error if nothing works.
    ///
    /// Intended for use from the initramfs (Dracut hook) and as a manual repair tool.
    Recover {
        #[command(flatten)]
        db: DbArgs,
    },
}
