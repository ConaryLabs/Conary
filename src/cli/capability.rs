// src/cli/capability.rs
//! CLI commands for package capability declarations
//!
//! These commands allow users to inspect and validate capability declarations
//! that define what system resources a package needs (network, filesystem, syscalls).

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum CapabilityCommands {
    /// Show declared capabilities for a package
    ///
    /// Displays the capability declaration from the installed package,
    /// showing what network, filesystem, and syscall access it requires.
    Show {
        /// Package name (optionally with @version)
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// Output format: text, json, toml
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Validate capability syntax in a ccs.toml manifest
    ///
    /// Parses the manifest and checks that the capability declarations
    /// are syntactically correct and internally consistent.
    Validate {
        /// Path to ccs.toml manifest file
        path: String,

        /// Show detailed validation information
        #[arg(short, long)]
        verbose: bool,
    },

    /// List packages by capability status
    ///
    /// Shows all installed packages and whether they have capability
    /// declarations. Use --missing to show only packages without declarations.
    List {
        #[command(flatten)]
        db: DbArgs,

        /// Show only packages missing capability declarations
        #[arg(long)]
        missing: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Generate capability declarations by observing a binary
    ///
    /// Runs a binary under strace to observe its actual system calls,
    /// then generates a capability declaration matching the observed behavior.
    /// NOTE: This is a Phase 2 feature, currently unimplemented.
    #[command(hide = true)]
    Generate {
        /// Binary to observe
        binary: String,

        /// Arguments to pass to the binary
        #[arg(last = true)]
        args: Vec<String>,

        /// Output file for generated capabilities (default: stdout)
        #[arg(short, long)]
        output: Option<String>,

        /// Duration to observe in seconds
        #[arg(long, default_value = "30")]
        timeout: u32,
    },

    /// Audit a package against its declared capabilities
    ///
    /// Runs the package under observation and compares actual behavior
    /// against declared capabilities. Reports any discrepancies.
    /// NOTE: This is a Phase 2 feature, currently unimplemented.
    #[command(hide = true)]
    Audit {
        /// Package name
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// Command to run for auditing
        #[arg(long)]
        command: Option<String>,

        /// Duration to observe in seconds
        #[arg(long, default_value = "30")]
        timeout: u32,
    },

    /// Run a command with capability enforcement
    ///
    /// Applies the declared capabilities as restrictions using
    /// landlock (filesystem) and seccomp (syscalls).
    /// NOTE: This is a Phase 3 feature, currently unimplemented.
    #[command(hide = true)]
    Run {
        /// Package whose capabilities to enforce
        package: String,

        /// Command and arguments to run
        #[arg(last = true)]
        command: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Allow violations instead of blocking (audit mode)
        #[arg(long)]
        permissive: bool,
    },
}
