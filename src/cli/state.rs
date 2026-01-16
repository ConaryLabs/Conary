// src/cli/state.rs
//! System state snapshot and rollback commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum StateCommands {
    /// List system state snapshots
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Limit number of states shown
        #[arg(short, long)]
        limit: Option<i64>,
    },

    /// Show details of a specific state
    Show {
        /// State number to show
        state_number: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Compare two system states
    Diff {
        /// Source state number
        from_state: i64,

        /// Target state number
        to_state: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Restore system to a previous state
    Restore {
        /// State number to restore to
        state_number: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Prune old states, keeping only the most recent N
    Prune {
        /// Number of states to keep
        keep: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show what would be pruned without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Create a manual state snapshot
    Create {
        /// Summary description for the state
        summary: String,

        /// Optional detailed description
        #[arg(long)]
        description: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Rollback a changeset
    Rollback {
        /// Changeset ID to rollback
        changeset_id: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },
}
