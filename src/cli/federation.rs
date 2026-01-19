// src/cli/federation.rs
//! CLI definitions for federation commands

use crate::cli::DbArgs;
use clap::Subcommand;

/// Federation commands for CAS sharing across machines
#[derive(Subcommand, Debug, Clone)]
pub enum FederationCommands {
    /// Show federation status and peer information
    Status {
        #[command(flatten)]
        db: DbArgs,

        /// Show detailed peer information
        #[arg(short, long)]
        verbose: bool,
    },

    /// List known federation peers
    Peers {
        #[command(flatten)]
        db: DbArgs,

        /// Filter by tier (region_hub, cell_hub, leaf)
        #[arg(long)]
        tier: Option<String>,

        /// Show only enabled peers
        #[arg(long)]
        enabled_only: bool,
    },

    /// Add a static peer endpoint
    AddPeer {
        /// Peer endpoint URL (e.g., https://remi.conary.io:7891)
        url: String,

        #[command(flatten)]
        db: DbArgs,

        /// Peer tier (region_hub, cell_hub, leaf)
        #[arg(long, default_value = "cell_hub")]
        tier: String,

        /// Human-friendly name for the peer
        #[arg(long)]
        name: Option<String>,
    },

    /// Remove a peer
    RemovePeer {
        /// Peer URL or ID to remove
        peer: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show federation statistics (bandwidth savings, etc.)
    Stats {
        #[command(flatten)]
        db: DbArgs,

        /// Number of days to show (default: 7)
        #[arg(long, default_value = "7")]
        days: u32,
    },

    /// Enable or disable a peer
    EnablePeer {
        /// Peer URL or ID
        peer: String,

        #[command(flatten)]
        db: DbArgs,

        /// Enable the peer (default: true)
        #[arg(long, default_value = "true")]
        enable: bool,
    },

    /// Test connectivity to federation peers
    Test {
        #[command(flatten)]
        db: DbArgs,

        /// Specific peer URL to test (tests all if not specified)
        #[arg(long)]
        peer: Option<String>,

        /// Timeout in milliseconds
        #[arg(long, default_value = "5000")]
        timeout: u64,
    },
}
