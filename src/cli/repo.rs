// src/cli/repo.rs
//! Repository management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum RepoCommands {
    /// Add a repository
    Add {
        /// Repository name
        name: String,

        /// Repository URL (for metadata)
        url: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Content mirror URL for package downloads (reference mirror pattern)
        ///
        /// If set, metadata is fetched from --url but packages are downloaded
        /// from --content-url. This enables scenarios like:
        /// - Trusted metadata server with local content mirrors
        /// - Hosting custom metadata pointing to upstream content
        #[arg(long)]
        content_url: Option<String>,

        /// Repository priority (higher = preferred)
        #[arg(short, long, default_value = "50")]
        priority: i32,

        /// Add repository in disabled state
        #[arg(long)]
        disabled: bool,

        /// URL or path to GPG public key for signature verification
        #[arg(long)]
        gpg_key: Option<String>,

        /// Disable GPG signature checking for this repository
        #[arg(long)]
        no_gpg_check: bool,

        /// Require all packages to have valid GPG signatures (strict mode)
        #[arg(long)]
        gpg_strict: bool,

        /// Default resolution strategy for packages from this repository
        ///
        /// When no per-package routing entry exists, use this strategy:
        /// - remi: Convert packages via Remi server (requires --remi-endpoint and --remi-distro)
        /// - binary: Download pre-built packages directly (default behavior)
        /// - legacy: Use repository_packages table (same as binary)
        #[arg(long, value_name = "STRATEGY")]
        default_strategy: Option<String>,

        /// Remi server endpoint URL (required when --default-strategy=remi)
        #[arg(long, value_name = "URL")]
        remi_endpoint: Option<String>,

        /// Distribution name for Remi conversion (required when --default-strategy=remi)
        ///
        /// Examples: fedora, arch, debian, ubuntu
        #[arg(long, value_name = "DISTRO")]
        remi_distro: Option<String>,
    },

    /// List configured repositories
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show all repositories (including disabled)
        #[arg(short, long)]
        all: bool,
    },

    /// Remove a repository
    Remove {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Enable a repository
    Enable {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Disable a repository
    Disable {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Sync repository metadata
    Sync {
        /// Optional repository name (syncs all enabled if not specified)
        name: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Force sync even if recently synced
        #[arg(short, long)]
        force: bool,
    },

    /// Import a GPG key for a repository
    #[command(name = "key-import")]
    KeyImport {
        /// Repository name to associate the key with
        repository: String,

        /// Path to GPG public key file, or URL to fetch from
        key: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List imported GPG keys
    #[command(name = "key-list")]
    KeyList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove a GPG key for a repository
    #[command(name = "key-remove")]
    KeyRemove {
        /// Repository name whose key to remove
        repository: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
