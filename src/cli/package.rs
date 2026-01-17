// src/cli/package.rs
//! Package management commands: install, remove, update, pin

use clap::Subcommand;

#[derive(Subcommand)]
pub enum PackageCommands {
    /// Install a package
    Install {
        /// Package name or path to package file
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific version to install
        #[arg(short, long)]
        version: Option<String>,

        /// Specific repository to use
        #[arg(long)]
        repo: Option<String>,

        /// Show what would be installed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip dependency checking
        #[arg(long)]
        no_deps: bool,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,

        /// Allow downgrading to an older version
        #[arg(long)]
        allow_downgrade: bool,

        /// Convert legacy packages (RPM/DEB/Arch) to CCS format during install
        ///
        /// Enables CAS deduplication, component selection, and atomic transactions.
        /// Extracted hooks (users, groups, directories, systemd units) are run
        /// declaratively before the original scriptlet.
        #[arg(long)]
        convert_to_ccs: bool,
    },

    /// Remove an installed package
    Remove {
        /// Package name to remove
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific version to remove (required if multiple versions installed)
        #[arg(short, long)]
        version: Option<String>,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    /// Remove orphaned packages (installed as dependencies but no longer needed)
    Autoremove {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Show what would be removed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    /// Check for and apply package updates
    Update {
        /// Optional package name (updates all if not specified)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Only apply security updates (critical/important severity)
        #[arg(long)]
        security: bool,
    },

    /// Update all members of a collection/group atomically
    #[command(name = "update-group")]
    UpdateGroup {
        /// Collection name to update
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Only apply security updates
        #[arg(long)]
        security: bool,
    },

    /// Pin a package to prevent updates and removal
    Pin {
        /// Package name to pin
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Unpin a package to allow updates and removal
    Unpin {
        /// Package name to unpin
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List all pinned packages
    #[command(name = "list-pinned")]
    ListPinned {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Adopt all installed system packages into Conary tracking
    #[command(name = "adopt-system")]
    AdoptSystem {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Copy files to CAS for full management (slower but enables rollback)
        #[arg(long)]
        full: bool,

        /// Show what would be adopted without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Adopt specific system package(s) into Conary tracking
    Adopt {
        /// Package name(s) to adopt
        packages: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Copy files to CAS for full management
        #[arg(long)]
        full: bool,
    },

    /// Show adoption status
    #[command(name = "adopt-status")]
    AdoptStatus {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Check for file conflicts and ownership issues
    Conflicts {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show detailed output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Verify installed files
    Verify {
        /// Optional package name to verify (verifies all if not specified)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Verify adopted packages against RPM database instead of CAS
        #[arg(long)]
        rpm: bool,
    },

    /// Restore files from CAS to filesystem
    Restore {
        /// Package name to restore (or "all" to check all packages)
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Force restore even if files exist (overwrite)
        #[arg(short, long)]
        force: bool,

        /// Show what would be restored without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Display scriptlets (install/remove hooks) from a package file
    Scripts {
        /// Path to package file (RPM, DEB, or Arch)
        package_path: String,
    },

    /// Show delta update statistics
    #[command(name = "delta-stats")]
    DeltaStats {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
