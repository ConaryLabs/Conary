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

    /// Export a generation artifact as a disk image.
    Export {
        /// Installed generation number to export (defaults to current generation).
        #[arg(conflicts_with = "path")]
        generation: Option<i64>,

        /// Explicit generation directory, e.g. output/generations/1.
        #[arg(long)]
        path: Option<String>,

        /// Output format: raw or qcow2. ISO is reserved and returns a preview NotImplemented error.
        #[arg(long, default_value = "qcow2")]
        format: String,

        /// Output image path.
        #[arg(short, long)]
        output: String,

        /// Optional image size larger than the computed minimum, e.g. 8G.
        #[arg(long)]
        size: Option<String>,
    },

    /// Select a specific generation for next boot; live switching is debug-only
    Switch {
        /// Generation number to select for next boot
        number: i64,

        /// Reboot after selecting the generation
        #[arg(long)]
        reboot: bool,
    },

    /// Select the previous generation for next boot
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
    /// Implements an ordered 4-step recovery:
    ///   1. Mount the generation pointed to by /conary/current if its artifact is valid.
    ///   2. Rebuild the EROFS image from DB state if the current image is missing/truncated.
    ///   3. Scan /conary/generations/ descending and try each valid generation artifact.
    ///   4. Return an error if nothing works.
    ///
    /// Intended for use from the initramfs (Dracut hook) and as a manual repair tool.
    Recover {
        #[command(flatten)]
        db: DbArgs,
    },
}
