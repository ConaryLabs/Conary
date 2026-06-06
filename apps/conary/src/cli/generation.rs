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

        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Publish committed DB state into the selected generation
    Publish {
        /// Assert that this pending changeset is covered by the publication
        #[arg(long)]
        changeset: Option<i64>,

        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show generation publication debt that still needs operator attention
    Pending {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Verify the SQLite DB backup stored with a generation
    VerifyDbBackup {
        /// Installed generation number to verify
        #[arg(long, conflicts_with = "current", required_unless_present = "current")]
        generation: Option<i64>,

        /// Verify the backup for the currently selected generation
        #[arg(long, conflicts_with = "generation")]
        current: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Recover the Conary SQLite DB from a generation-bound backup
    RecoverDb {
        /// Installed generation number whose DB backup should be recovered
        #[arg(long)]
        generation: i64,

        /// Verify the recovery copy without touching the live DB
        #[arg(long)]
        dry_run: bool,

        /// Keep the temporary verified copy created by --dry-run
        #[arg(long)]
        keep_temp: bool,

        /// Confirm live DB replacement
        #[arg(short, long)]
        yes: bool,

        /// Replace a live DB even if it passes integrity checks
        #[arg(long, hide = true)]
        replace_healthy_db: bool,

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

        /// Output format: raw, qcow2, or iso.
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

        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Select the previous generation for next boot
    Rollback {
        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Remove old generations and unreferenced CAS objects
    Gc {
        /// Number of generations to keep (default: 3)
        #[arg(long, default_value = "3")]
        keep: usize,

        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,

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
        /// Confirm applying this generation, boot, publication, or recovery change
        #[arg(short = 'y', long)]
        yes: bool,

        #[command(flatten)]
        db: DbArgs,
    },
}
