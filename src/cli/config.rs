// src/cli/config.rs
//! Configuration file management commands

use clap::Subcommand;
use super::{DbArgs, CommonArgs};

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// List configuration files
    List {
        /// Package name (optional - if omitted, shows modified configs)
        package: Option<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Show all config files, not just modified
        #[arg(short, long)]
        all: bool,
    },

    /// Show diff between installed config and package version
    Diff {
        /// Path to the config file
        path: String,

        #[command(flatten)]
        common: CommonArgs,
    },

    /// Backup a configuration file
    Backup {
        /// Path to the config file
        path: String,

        #[command(flatten)]
        common: CommonArgs,
    },

    /// Restore a configuration file from backup
    Restore {
        /// Path to the config file
        path: String,

        #[command(flatten)]
        common: CommonArgs,

        /// Specific backup ID to restore (default: latest)
        #[arg(long)]
        backup_id: Option<i64>,
    },

    /// Check status of configuration files
    Check {
        /// Package name (optional - if omitted, checks all)
        package: Option<String>,

        #[command(flatten)]
        common: CommonArgs,
    },

    /// List backups for a configuration file
    Backups {
        /// Path to the config file
        path: String,

        #[command(flatten)]
        db: DbArgs,
    },
}
