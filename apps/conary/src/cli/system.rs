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
    /// Internal/debug: adopt and CAS-back packages without PM removal
    Cas,
    /// Internal/debug: CAS-back packages and remove them from the system PM
    Owned,
    /// Supported path: build generation + boot entry, then stop ready to activate
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

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Adopt system packages into Conary tracking
    ///
    /// Use --system to adopt all packages, or specify package names.
    /// Use --status to show adoption progress.
    /// Use --refresh to detect version drift and update adopted packages.
    /// Use --convert to batch convert adopted packages to CCS format.
    Adopt {
        /// Package name(s) to adopt (ignored if --system, --status, --refresh, etc.)
        #[arg(required_unless_present_any = ["system", "status", "refresh", "convert", "sync_hook"])]
        #[arg(conflicts_with_all = ["system", "status", "refresh", "convert", "sync_hook", "from_sync_hook"])]
        packages: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Copy files to CAS for full management (enables rollback)
        /// Used by: default (package adopt), --system, --refresh
        #[arg(long, conflicts_with_all = ["status", "convert", "sync_hook", "from_sync_hook"])]
        full: bool,

        /// Adopt all installed system packages
        #[arg(long, conflicts_with_all = ["status", "refresh", "convert", "sync_hook"])]
        system: bool,

        /// Show adoption status
        #[arg(long, conflicts_with_all = ["system", "refresh", "convert", "sync_hook"])]
        status: bool,

        /// Show what would be adopted without making changes
        /// Used by: --system, --convert, --refresh. Single-package dry-run is
        /// rejected until it has a true non-mutating preview path.
        #[arg(long, conflicts_with_all = ["status", "sync_hook"])]
        dry_run: bool,

        /// Only adopt packages matching this glob pattern (e.g., "lib*")
        /// Used by: --system only
        #[arg(long, requires = "system", conflicts_with_all = ["status", "refresh", "convert", "sync_hook"])]
        pattern: Option<String>,

        /// Skip packages matching this glob pattern (e.g., "kernel*")
        /// Used by: --system only
        #[arg(long, requires = "system", conflicts_with_all = ["status", "refresh", "convert", "sync_hook"])]
        exclude: Option<String>,

        /// Only adopt explicitly installed packages (skip auto-installed deps)
        /// Used by: --system only
        #[arg(long, requires = "system", conflicts_with_all = ["status", "refresh", "convert", "sync_hook"])]
        explicit_only: bool,

        /// Check adopted packages for version drift and update changed ones
        #[arg(long, conflicts_with_all = ["system", "status", "convert", "sync_hook"])]
        refresh: bool,

        /// Convert adopted packages to CCS format
        #[arg(long, conflicts_with_all = ["system", "status", "refresh", "sync_hook"])]
        convert: bool,

        /// Number of parallel conversion threads (default: CPU count), requires --convert
        #[arg(long, requires = "convert")]
        jobs: Option<usize>,

        /// Disable CDC chunking during conversion, requires --convert
        #[arg(long, requires = "convert")]
        no_chunking: bool,

        /// Install/remove system PM sync hooks
        #[arg(long, conflicts_with_all = ["system", "status", "refresh", "convert"])]
        sync_hook: bool,

        /// Remove sync hooks instead of installing (requires --sync-hook)
        #[arg(long, requires = "sync_hook")]
        remove_hook: bool,

        /// Suppress output (for use by PM hooks and --refresh)
        /// Used by: --refresh only
        #[arg(long, requires = "refresh", conflicts_with_all = ["system", "status", "convert", "sync_hook"])]
        quiet: bool,

        /// Internal path used by installed native package-manager sync hooks.
        ///
        /// Requires --refresh --quiet and cannot be combined with --full; hook
        /// install/remove remains the explicit consent point.
        #[arg(long, hide = true, requires_all = ["refresh", "quiet"], conflicts_with_all = ["system", "status", "convert", "sync_hook", "full"])]
        from_sync_hook: bool,
    },

    /// Stop Conary tracking adopted native packages without deleting files
    ///
    /// Use --all to unadopt every package that is still owned by the native
    /// package manager, or specify one or more package names. This refuses to
    /// run while a Conary generation is selected unless --dry-run is used.
    Unadopt {
        /// Adopted package name(s) to stop tracking
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        packages: Vec<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Stop tracking every adopted package
        #[arg(long)]
        all: bool,

        /// Show what would be unadopted without changing the database
        #[arg(long)]
        dry_run: bool,

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Leave installed native package manager refresh hooks in place
        #[arg(long)]
        keep_hooks: bool,
    },

    /// Hand selected-generation adopted packages back to native authority
    ///
    /// Clears the selected Conary generation pointer before removing adopted
    /// tracking rows. Native package files and native package-manager databases
    /// are not modified. Use --dry-run first to inspect the staged plan.
    #[command(name = "native-handoff")]
    NativeHandoff {
        #[command(flatten)]
        db: DbArgs,

        /// Show the staged handoff plan without changing state
        #[arg(long)]
        dry_run: bool,

        /// Confirm clearing the selected generation and removing adopted tracking
        #[arg(long, short)]
        yes: bool,

        /// Resume an interrupted native authority handoff
        #[arg(long)]
        recover: bool,

        /// Leave installed native package manager refresh hooks in place
        #[arg(long)]
        keep_hooks: bool,
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

    /// Inspect, verify, or recover Conary database checkpoint backups
    #[command(name = "db-backup")]
    DbBackup {
        #[command(subcommand)]
        command: DbBackupCommands,
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
        /// How far to go: generation by default; cas/owned are internal debug checkpoints
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
pub enum DbBackupCommands {
    /// List available database checkpoint backups
    List {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Verify a database checkpoint backup
    Verify {
        /// Verify the newest backup
        #[arg(long)]
        latest: bool,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Recover the live Conary database from a checkpoint backup
    Recover {
        /// Recover the newest backup
        #[arg(long)]
        latest: bool,

        /// Verify and show the selected backup without changing the live DB
        #[arg(long)]
        dry_run: bool,

        /// Confirm replacing a missing or corrupt live DB
        #[arg(long, short)]
        yes: bool,

        /// Debug escape hatch: replace a live DB that still passes integrity checks
        #[arg(long, hide = true)]
        replace_healthy_db: bool,

        #[command(flatten)]
        db: DbArgs,
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
