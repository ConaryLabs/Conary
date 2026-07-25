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

    /// Inspect the exact enforcement plan for a package
    ///
    /// Shows target kernel support and the filesystem, TCP-port, and syscall
    /// rules Conary would enforce without executing package code.
    Audit {
        /// Package name
        package: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Run a command with capability enforcement
    ///
    /// Applies the declared capabilities as restrictions using
    /// landlock (filesystem) and seccomp (syscalls).
    Run {
        /// Package whose capabilities to enforce
        package: String,

        /// Command and arguments to run
        #[arg(last = true)]
        command: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Log violations instead of blocking them
        #[arg(long)]
        audit: bool,
    },
}
