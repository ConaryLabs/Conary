// src/cli/redirect.rs
//! Redirect management commands for package aliasing and supersession

use clap::Subcommand;

#[derive(Subcommand)]
pub enum RedirectCommands {
    /// List all redirects
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Filter by redirect type: rename, obsolete, merge, split
        #[arg(short, long)]
        r#type: Option<String>,

        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
    },

    /// Create a new redirect
    ///
    /// Creates a redirect from source package to target package.
    /// When someone tries to install the source package, they'll get
    /// the target instead.
    Add {
        /// Source package name (the name to redirect FROM)
        source: String,

        /// Target package name (the name to redirect TO)
        target: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Redirect type: rename, obsolete, merge, split (default: rename)
        #[arg(short, long, default_value = "rename")]
        r#type: String,

        /// Source version constraint (only redirect specific versions)
        #[arg(long)]
        source_version: Option<String>,

        /// Target version constraint (redirect to specific version)
        #[arg(long)]
        target_version: Option<String>,

        /// User-facing message explaining the redirect
        #[arg(short, long)]
        message: Option<String>,
    },

    /// Show details of a redirect
    Show {
        /// Source package name
        source: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Source version (for version-specific redirects)
        #[arg(long)]
        version: Option<String>,
    },

    /// Remove a redirect
    Remove {
        /// Source package name
        source: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Resolve a package name through redirect chain
    ///
    /// Shows what package name a request would resolve to after
    /// following all redirects.
    Resolve {
        /// Package name to resolve
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Package version (for version-specific resolution)
        #[arg(long)]
        version: Option<String>,
    },
}
