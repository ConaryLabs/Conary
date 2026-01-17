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

    /// Generate repository index for the Refinery server
    ///
    /// Scans the database and chunk store to generate a repository index
    /// listing all converted packages and their metadata.
    #[cfg(feature = "server")]
    IndexGen {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Path to chunk storage directory
        #[arg(long, default_value = "/var/lib/conary/data/chunks")]
        chunk_dir: String,

        /// Output directory for generated index files
        #[arg(short, long, default_value = "/var/lib/conary/data/repo")]
        output_dir: String,

        /// Distribution to generate index for (arch, fedora, ubuntu, debian)
        /// If not specified, generates for all distributions.
        #[arg(long)]
        distro: Option<String>,

        /// Sign the index with the specified key file
        #[arg(long)]
        sign_key: Option<String>,
    },

    /// Pre-warm the chunk cache by converting popular packages
    ///
    /// Downloads and converts packages proactively, reducing latency for
    /// first-time requests. Can use a popularity file to prioritize packages.
    #[cfg(feature = "server")]
    Prewarm {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Path to chunk storage directory
        #[arg(long, default_value = "/var/lib/conary/data/chunks")]
        chunk_dir: String,

        /// Path to cache/scratch directory
        #[arg(long, default_value = "/var/lib/conary/data/cache")]
        cache_dir: String,

        /// Distribution to pre-warm (arch, fedora, ubuntu, debian)
        #[arg(long)]
        distro: String,

        /// Maximum number of packages to convert
        #[arg(long, default_value = "100")]
        max_packages: usize,

        /// Path to popularity data file (JSON with name/score pairs)
        #[arg(long)]
        popularity_file: Option<String>,

        /// Only convert packages matching this regex pattern
        #[arg(long)]
        pattern: Option<String>,

        /// Show what would be converted without actually converting
        #[arg(long)]
        dry_run: bool,
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

    /// Garbage collect unreferenced files from CAS storage
    ///
    /// Removes files from the content-addressable store that are no longer
    /// referenced by any installed package. Preserves files needed for rollback
    /// by keeping references from file_history within the retention period.
    Gc {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Path to CAS objects directory
        #[arg(long, default_value = "/var/lib/conary/objects")]
        objects_dir: String,

        /// Days of history to preserve for rollback (default: 30)
        #[arg(long, default_value = "30")]
        keep_days: u32,

        /// Show what would be removed without actually deleting
        #[arg(long)]
        dry_run: bool,
    },
}
