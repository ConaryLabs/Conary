// src/cli/state.rs
//! System state snapshot and rollback commands

use clap::Subcommand;
use super::{DbArgs, CommonArgs};

#[derive(Subcommand)]
pub enum StateCommands {
    /// List system state snapshots
    List {
        #[command(flatten)]
        db: DbArgs,

        /// Limit number of states shown
        #[arg(short, long)]
        limit: Option<i64>,
    },

    /// Show details of a specific state
    Show {
        /// State number to show
        state_number: i64,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Compare two system states
    Diff {
        /// Source state number
        from_state: i64,

        /// Target state number
        to_state: i64,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Revert system to a previous state
    Revert {
        /// State number to revert to
        state_number: i64,

        #[command(flatten)]
        db: DbArgs,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Prune old states, keeping only the most recent N
    Prune {
        /// Number of states to keep
        keep: i64,

        #[command(flatten)]
        db: DbArgs,

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

        #[command(flatten)]
        db: DbArgs,
    },

    /// Rollback a changeset
    Rollback {
        /// Changeset ID to rollback
        changeset_id: i64,

        #[command(flatten)]
        common: CommonArgs,
    },
}
