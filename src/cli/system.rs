// src/cli/system.rs
//! System-level commands: init, completions, server

use clap::Subcommand;
use clap_complete::Shell;

#[derive(Subcommand)]
pub enum SystemCommands {
    /// Initialize a new Conary database
    Init {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Start the Refinery server (CCS conversion proxy)
    ///
    /// The Refinery converts upstream packages to CCS format on-demand,
    /// serving them with chunk deduplication. Requires --features server.
    #[cfg(feature = "server")]
    Server {
        /// Address to bind to (host:port)
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        bind: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Path to chunk storage directory
        #[arg(long, default_value = "/var/lib/conary/data/chunks")]
        chunk_dir: String,

        /// Path to cache/scratch directory
        #[arg(long, default_value = "/var/lib/conary/data/cache")]
        cache_dir: String,

        /// Maximum concurrent conversions
        #[arg(long, default_value = "4")]
        max_concurrent: usize,

        /// Maximum cache size in GB (triggers LRU eviction)
        #[arg(long, default_value = "700")]
        max_cache_gb: u64,

        /// Chunk TTL in days before LRU eviction
        #[arg(long, default_value = "30")]
        chunk_ttl_days: u32,
    },
}
