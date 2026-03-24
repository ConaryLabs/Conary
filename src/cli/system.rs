// src/cli/system.rs
//! System-level commands: init, completions, gc, state, triggers, redirects, etc.

use clap::Subcommand;
use clap_complete::Shell;

use super::generation::GenerationCommands;
use super::redirect::RedirectCommands;
use super::state::StateCommands;
use super::trigger::TriggerCommands;
use super::{CommonArgs, DbArgs};

/// How far the takeover pipeline should go
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum TakeoverLevel {
    /// Adopt + CAS-back all packages (PM untouched)
    Cas,
    /// CAS + remove from system PM
    Owned,
    /// CAS + PM removal + build generation + boot + live switch
    #[default]
    Generation,
}

#[derive(Subcommand)]
pub enum SystemCommands {
    /// Initialize a new Conary database
    Init {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show changeset history
    History {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Verify installed files
    Verify {
        /// Optional package name to verify (verifies all if not specified)
        package: Option<String>,

        #[command(flatten)]
        common: CommonArgs,

        /// Verify adopted packages against RPM database instead of CAS
        #[arg(long)]
        rpm: bool,
    },

    /// Restore files from CAS to filesystem
    Restore {
        /// Package name to restore (or "all" to check all packages)
        package: String,

        #[command(flatten)]
        common: CommonArgs,

        /// Force restore even if files exist (overwrite)
        #[arg(short, long)]
        force: bool,

        /// Show what would be restored without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Adopt system packages into Conary tracking
    ///
    /// Use --system to adopt all packages, or specify package names.
    /// Use --status to show adoption progress.
    /// Use --refresh to detect version drift and update adopted packages.
    /// Use --convert to batch convert adopted packages to CCS format.
    Adopt {
        /// Package name(s) to adopt (ignored if --system, --status, --refresh, etc.)
        #[arg(required_unless_present_any = ["system", "status", "refresh", "convert", "takeover", "sync_hook"])]
        packages: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Copy files to CAS for full management (enables rollback)
        #[arg(long)]
        full: bool,

        /// Adopt all installed system packages
        #[arg(long)]
        system: bool,

        /// Show adoption status
        #[arg(long)]
        status: bool,

        /// Show what would be adopted without making changes
        #[arg(long)]
        dry_run: bool,

        /// Only adopt packages matching this glob pattern (e.g., "lib*")
        #[arg(long)]
        pattern: Option<String>,

        /// Skip packages matching this glob pattern (e.g., "kernel*")
        #[arg(long)]
        exclude: Option<String>,

        /// Only adopt explicitly installed packages (skip auto-installed deps)
        #[arg(long)]
        explicit_only: bool,

        /// Check adopted packages for version drift and update changed ones
        #[arg(long)]
        refresh: bool,

        /// Convert adopted packages to CCS format
        #[arg(long)]
        convert: bool,

        /// Number of parallel conversion threads (default: CPU count), requires --convert
        #[arg(long, requires = "convert")]
        jobs: Option<usize>,

        /// Disable CDC chunking during conversion, requires --convert
        #[arg(long, requires = "convert")]
        no_chunking: bool,

        /// Take over adopted packages from the system PM (Conary fully owns files)
        #[arg(long)]
        takeover: bool,

        /// Skip interactive confirmation (requires --takeover)
        #[arg(long, requires = "takeover")]
        yes: bool,

        /// Install/remove system PM sync hooks
        #[arg(long)]
        sync_hook: bool,

        /// Remove sync hooks instead of installing (requires --sync-hook)
        #[arg(long, requires = "sync_hook")]
        remove_hook: bool,

        /// Suppress output (for use by PM hooks)
        #[arg(long)]
        quiet: bool,
    },

    /// Garbage collect unreferenced files from CAS storage
    ///
    /// Removes files from the content-addressable store that are no longer
    /// referenced by any installed package. Preserves files needed for rollback
    /// by keeping references from file_history within the retention period.
    Gc {
        #[command(flatten)]
        db: DbArgs,

        /// Path to CAS objects directory
        #[arg(long, default_value = "/var/lib/conary/objects")]
        objects_dir: String,

        /// Days of history to preserve for rollback (default: 30)
        #[arg(long, default_value = "30")]
        keep_days: u32,

        /// Show what would be removed without actually deleting
        #[arg(long)]
        dry_run: bool,

        /// Also garbage collect orphaned chunks from local disk
        #[arg(long)]
        chunks: bool,
    },

    /// Generate SBOM (Software Bill of Materials) for a package
    ///
    /// Outputs a CycloneDX 1.5 format SBOM in JSON. This is useful for
    /// security auditing, compliance, and vulnerability scanning.
    Sbom {
        /// Package name (or "all" for entire system)
        package_name: String,

        #[command(flatten)]
        db: DbArgs,

        /// Output format
        #[arg(short, long, default_value = "cyclonedx")]
        format: String,

        /// Output to file instead of stdout
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Generate repository index for the Remi server
    ///
    /// Scans the database and chunk store to generate a repository index
    /// listing all converted packages and their metadata.
    #[cfg(feature = "server")]
    IndexGen {
        #[command(flatten)]
        db: DbArgs,

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
        #[command(flatten)]
        db: DbArgs,

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

    /// Start the Remi server (CCS conversion proxy)
    ///
    /// Remi converts upstream packages to CCS format on-demand,
    /// serving them with chunk deduplication. Requires --features server.
    #[cfg(feature = "server")]
    Server {
        /// Address to bind to (host:port)
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        bind: String,

        #[command(flatten)]
        db: DbArgs,

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

    // =========================================================================
    // Nested Subcommands
    // =========================================================================
    /// System state snapshots and rollback
    #[command(subcommand)]
    State(StateCommands),

    /// Generation management (build, switch, rollback, gc)
    #[command(subcommand)]
    Generation(GenerationCommands),

    /// Convert entire system to Conary-managed generations
    Takeover {
        /// How far to go: cas, owned, or generation (default: generation)
        #[arg(long, default_value = "generation")]
        up_to: TakeoverLevel,

        /// Auto-confirm
        #[arg(long, short)]
        yes: bool,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Trigger management
    #[command(subcommand)]
    Trigger(TriggerCommands),

    /// Package redirect management (renames, obsoletes)
    #[command(subcommand)]
    Redirect(RedirectCommands),

    /// Manage self-update channel
    #[command(name = "update-channel")]
    UpdateChannel {
        #[command(subcommand)]
        action: UpdateChannelAction,
    },
}

#[derive(Subcommand)]
pub enum UpdateChannelAction {
    /// Show current update channel URL
    Get {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Set a custom update channel URL
    Set {
        /// Update channel URL
        url: String,
        #[command(flatten)]
        db: DbArgs,
    },
    /// Reset to default update channel
    Reset {
        #[command(flatten)]
        db: DbArgs,
    },
}
