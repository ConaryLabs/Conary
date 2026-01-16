// src/cli/config.rs
//! Configuration file management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// List configuration files
    List {
        /// Package name (optional - if omitted, shows modified configs)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show all config files, not just modified
        #[arg(short, long)]
        all: bool,
    },

    /// Show diff between installed config and package version
    Diff {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// Backup a configuration file
    Backup {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// Restore a configuration file from backup
    Restore {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific backup ID to restore (default: latest)
        #[arg(long)]
        backup_id: Option<i64>,
    },

    /// Check status of configuration files
    Check {
        /// Package name (optional - if omitted, checks all)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// List backups for a configuration file
    Backups {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
