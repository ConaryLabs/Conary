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

        /// Use EROFS generation output instead of sysroot (from bootstrap run)
        #[arg(long)]
        from_generation: Option<String>,
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

    /// Package cross-tools output as a derivation seed
    Seed {
        /// Cross-tools directory to package (e.g., /conary/bootstrap/lfs/tools)
        #[arg(long, required_unless_present = "from_adopted")]
        from: Option<String>,

        /// Create seed from current adopted system filesystem
        #[arg(long)]
        from_adopted: bool,

        /// Distro name (required with --from-adopted)
        #[arg(long, requires = "from_adopted")]
        distro: Option<String>,

        /// Distro version (required with --from-adopted)
        #[arg(long, requires = "from_adopted")]
        distro_version: Option<String>,

        /// Output seed directory
        #[arg(short, long)]
        output: String,

        /// Target triple
        #[arg(long, default_value = "x86_64-conary-linux-gnu")]
        target: String,
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

    /// Run the derivation pipeline from a system manifest
    Run {
        /// Path to system manifest TOML
        manifest: String,

        /// Working directory for build artifacts
        #[arg(short, long, default_value = ".conary/bootstrap")]
        work_dir: String,

        /// Path to seed directory (from bootstrap seed)
        #[arg(long)]
        seed: String,

        /// Recipe directory
        #[arg(long, default_value = "recipes")]
        recipe_dir: String,

        /// Stop after completing this stage (toolchain, foundation, system, customization)
        #[arg(long)]
        up_to: Option<String>,

        /// Only build these packages (comma-separated)
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,

        /// Also rebuild reverse dependents of --only targets
        #[arg(long, requires = "only")]
        cascade: bool,

        /// Preserve build logs for successful builds
        #[arg(long)]
        keep_logs: bool,

        /// Spawn interactive shell on build failure
        #[arg(long)]
        shell_on_failure: bool,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,

        /// Skip remote substituters, build everything locally
        #[arg(long)]
        no_substituters: bool,

        /// Auto-publish successful builds to configured endpoint
        #[arg(long)]
        publish: bool,
    },

    /// Verify convergence between builds from two different seeds
    #[command(name = "verify-convergence")]
    VerifyConvergence {
        /// Path to first seed directory
        #[arg(long)]
        seed_a: String,
        /// Path to second seed directory
        #[arg(long)]
        seed_b: String,
        /// Show per-file diff for mismatches
        #[arg(long)]
        diff: bool,
    },

    /// Diff two seed EROFS images
    #[command(name = "diff-seeds")]
    DiffSeeds {
        /// Path to first seed directory
        path_a: String,
        /// Path to second seed directory
        path_b: String,
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

#[cfg(test)]
mod tests {
    use super::BootstrapCommands;
    use clap::Parser;

    #[derive(Parser)]
    struct BootstrapCli {
        #[command(subcommand)]
        command: BootstrapCommands,
    }

    #[test]
    fn cli_accepts_bootstrap_cross_tools_name() {
        let parsed =
            BootstrapCli::try_parse_from(["bootstrap", "cross-tools"]).expect("parse cross-tools");
        assert!(matches!(parsed.command, BootstrapCommands::CrossTools { .. }));
    }

    #[test]
    fn cli_rejects_legacy_stage0_name() {
        assert!(BootstrapCli::try_parse_from(["bootstrap", "stage0"]).is_err());
    }
}
