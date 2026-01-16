// src/cli/trigger.rs
//! Trigger management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum TriggerCommands {
    /// List all triggers
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show disabled triggers too
        #[arg(long)]
        all: bool,

        /// Show only built-in triggers
        #[arg(long)]
        builtin: bool,
    },

    /// Show details of a trigger
    Show {
        /// Trigger name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Enable a trigger
    Enable {
        /// Trigger name to enable
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Disable a trigger
    Disable {
        /// Trigger name to disable
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add a custom trigger
    Add {
        /// Trigger name
        name: String,

        /// File path pattern (glob, comma-separated for multiple)
        #[arg(long)]
        pattern: String,

        /// Handler command to execute
        #[arg(long)]
        handler: String,

        /// Optional description
        #[arg(long)]
        description: Option<String>,

        /// Priority (lower runs first, default 50)
        #[arg(long)]
        priority: Option<i32>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove a custom trigger (built-in triggers cannot be removed)
    Remove {
        /// Trigger name to remove
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Run pending triggers for a changeset
    Run {
        /// Changeset ID (defaults to most recent)
        changeset_id: Option<i64>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },
}
