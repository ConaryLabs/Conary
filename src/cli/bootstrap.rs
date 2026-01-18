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

    /// Build Stage 0 cross-compilation toolchain
    Stage0 {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Path to custom crosstool-ng config
        #[arg(short, long)]
        config: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Only download sources, don't build
        #[arg(long)]
        download_only: bool,

        /// Clean work directory before building
        #[arg(long)]
        clean: bool,
    },

    /// Build Stage 1 self-hosted toolchain
    Stage1 {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Directory containing recipes (default: recipes/core)
        #[arg(short, long)]
        recipe_dir: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Build base system packages
    Base {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Target root directory for installation
        #[arg(long, default_value = "/conary/sysroot")]
        root: String,

        /// Directory containing recipes (default: recipes/core)
        #[arg(short, long)]
        recipe_dir: Option<String>,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Generate bootable image
    Image {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Output image file
        #[arg(short, long, default_value = "conary.img")]
        output: String,

        /// Image format (raw, qcow2, iso)
        #[arg(short, long, default_value = "raw")]
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

    /// Clean bootstrap work directory
    Clean {
        /// Directory for bootstrap work
        #[arg(short, long, default_value = "/var/lib/conary/bootstrap")]
        work_dir: String,

        /// Only clean specific stage (stage0, stage1, base, image)
        #[arg(short, long)]
        stage: Option<String>,

        /// Also remove downloaded source tarballs
        #[arg(long)]
        sources: bool,
    },
}
