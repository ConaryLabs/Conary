// src/cli/derive.rs
//! Derived package management commands

use clap::Subcommand;
use super::DbArgs;

#[derive(Subcommand)]
pub enum DeriveCommands {
    /// List all derived packages
    List {
        #[command(flatten)]
        db: DbArgs,

        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show details of a derived package
    Show {
        /// Name of the derived package
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Create a new derived package
    ///
    /// Derived packages allow customizing existing packages without rebuilding.
    /// Use patches and file overrides to make modifications.
    Create {
        /// Name for the derived package
        name: String,

        /// Parent package to derive from
        #[arg(long)]
        from: String,

        /// Version suffix (e.g., "+custom")
        #[arg(long)]
        version_suffix: Option<String>,

        /// Description
        #[arg(long)]
        description: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Add a patch to a derived package
    Patch {
        /// Name of the derived package
        name: String,

        /// Path to the patch file
        patch_file: String,

        /// Strip level for patch application (default: 1)
        #[arg(long)]
        strip: Option<i32>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Add a file override to a derived package
    ///
    /// Replace a file in the parent package, or remove it.
    Override {
        /// Name of the derived package
        name: String,

        /// Target path in the package to override
        target: String,

        /// Source file to replace with (omit to remove the file)
        #[arg(long)]
        source: Option<String>,

        /// File permissions (octal, e.g., 644)
        #[arg(long)]
        mode: Option<u32>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Build a derived package
    Build {
        /// Name of the derived package
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Delete a derived package
    Delete {
        /// Name of the derived package
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// List stale derived packages (parent was updated)
    Stale {
        #[command(flatten)]
        db: DbArgs,
    },
}
