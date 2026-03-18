// src/cli/bootstrap.rs
//! CLI definitions for bootstrap commands

use clap::Subcommand;

/// Bootstrap commands for building Conary from scratch
#[derive(Subcommand)]
pub enum BootstrapCommands {
    /// Initialize bootstrap environment
    Init {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Target architecture (x86_64, aarch64, riscv64)
        #[arg(short, long, default_value = "x86_64")]
        target: String,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,
    },

    /// Check prerequisites for bootstrap
    Check {
        /// Show verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Generate bootable image
    Image {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Output image file
        #[arg(short, long, default_value = "conaryos-base.qcow2")]
        output: String,

        /// Image format (raw, qcow2, iso, erofs)
        #[arg(short, long, default_value = "qcow2")]
        format: String,

        /// Image size (e.g., "4G", "8G")
        #[arg(short, long, default_value = "4G")]
        size: String,
    },

    /// Show bootstrap status and progress
    Status {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Show detailed status for each stage
        #[arg(short, long)]
        verbose: bool,
    },

    /// Resume bootstrap from last checkpoint
    Resume {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Validate the full pipeline without building
    DryRun {
        /// Working directory
        #[arg(long, default_value = ".")]
        work_dir: String,

        /// Recipe directory
        #[arg(long, default_value = "recipes")]
        recipe_dir: String,

        /// Verbose output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Clean bootstrap work directory
    Clean {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Only clean specific stage (cross-tools, temp-tools, system, image)
        #[arg(short, long)]
        stage: Option<String>,

        /// Also remove downloaded source tarballs
        #[arg(long)]
        sources: bool,
    },

    /// Build Phase 1: Cross-toolchain (LFS Chapter 5)
    #[command(name = "cross-tools")]
    CrossTools {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// LFS root directory ($LFS)
        #[arg(long)]
        lfs_root: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Skip checksum verification (development only)
        #[arg(long)]
        skip_verify: bool,
    },

    /// Build Phase 2: Temporary tools (LFS Chapters 6-7)
    #[command(name = "temp-tools")]
    TempTools {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// LFS root directory ($LFS)
        #[arg(long)]
        lfs_root: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Skip checksum verification (development only)
        #[arg(long)]
        skip_verify: bool,
    },

    /// Build Phase 3: Final system (LFS Chapter 8)
    #[command(name = "system")]
    System {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// LFS root directory ($LFS)
        #[arg(long)]
        lfs_root: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Skip checksum verification (development only)
        #[arg(long)]
        skip_verify: bool,
    },

    /// Run Phase 4: System configuration (LFS Chapter 9)
    #[command(name = "config")]
    Config {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// LFS root directory ($LFS)
        #[arg(long)]
        lfs_root: Option<String>,

        /// Show verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Build Phase 6: Tier-2 packages (BLFS + Conary self-hosting)
    #[command(name = "tier2")]
    Tier2 {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// LFS root directory ($LFS)
        #[arg(long)]
        lfs_root: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Skip checksum verification (development only)
        #[arg(long)]
        skip_verify: bool,
    },
}
