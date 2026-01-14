// src/main.rs
//! Conary Package Manager - CLI Entry Point

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::io;

mod commands;

// =============================================================================
// CLI Definitions
// =============================================================================

#[derive(Parser)]
#[command(name = "conary")]
#[command(author = "Conary Project")]
#[command(version)]
#[command(about = "A next-generation package manager with atomic transactions", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Conary database
    Init {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

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

        /// Skip running package scriptlets (install/remove hooks)
        #[arg(long)]
        no_scripts: bool,
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
    },

    /// Adopt all installed system packages into Conary tracking
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

    /// Query installed packages
    Query {
        /// Optional pattern to filter packages
        pattern: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Find package owning a file path
        #[arg(long)]
        path: Option<String>,

        /// Show detailed package information
        #[arg(short, long)]
        info: bool,

        /// List files in package
        #[arg(short, long)]
        files: bool,

        /// List files in ls -l style format
        #[arg(long)]
        lsl: bool,
    },

    /// Query packages available in repositories (not installed)
    ///
    /// Similar to dnf repoquery or apt-cache search.
    /// Searches package names and descriptions in synced repository metadata.
    Repquery {
        /// Optional pattern to filter packages
        pattern: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show detailed package information
        #[arg(short, long)]
        info: bool,
    },

    /// Query packages by installation reason
    ///
    /// Shows why packages were installed. Supports filters:
    /// - "explicit" - directly installed by user
    /// - "dependency" - installed as a dependency
    /// - "collection" - installed via a collection
    /// - "@name" - installed via specific collection
    /// - Custom pattern with * wildcard
    QueryReason {
        /// Reason filter pattern (or show all grouped if not specified)
        pattern: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show changeset history
    History {
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

    /// Show dependencies for a package
    Depends {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show reverse dependencies (what depends on this package)
    Rdepends {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show full dependency tree for a package
    Deptree {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show reverse dependency tree (what depends on this, transitively)
        #[arg(short, long)]
        reverse: bool,

        /// Maximum depth to traverse (default: unlimited)
        #[arg(long)]
        depth: Option<usize>,
    },

    /// Show what packages would break if a package is removed
    Whatbreaks {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Find which package provides a capability
    Whatprovides {
        /// Capability to search for (package name, file path, library, virtual provide)
        capability: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List components of an installed package
    ListComponents {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Query files in a specific component (e.g., nginx:lib)
    QueryComponent {
        /// Component spec in format "package:component" (e.g., nginx:lib)
        component_spec: String,

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

    /// Add a repository
    RepoAdd {
        /// Repository name
        name: String,

        /// Repository URL
        url: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

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
    },

    /// List configured repositories
    RepoList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show all repositories (including disabled)
        #[arg(short, long)]
        all: bool,
    },

    /// Remove a repository
    RepoRemove {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Enable a repository
    RepoEnable {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Disable a repository
    RepoDisable {
        /// Repository name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Sync repository metadata
    RepoSync {
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
    KeyList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove a GPG key for a repository
    KeyRemove {
        /// Repository name whose key to remove
        repository: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Search for packages in repositories
    Search {
        /// Search pattern
        pattern: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
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
    ListPinned {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show delta update statistics
    DeltaStats {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List all triggers
    TriggerList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show disabled triggers too
        #[arg(long)]
        all: bool,

        /// Show only built-in triggers
        #[arg(long)]
        builtin: bool,
    },

    /// Show details of a trigger
    TriggerShow {
        /// Trigger name
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Enable a trigger
    TriggerEnable {
        /// Trigger name to enable
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Disable a trigger
    TriggerDisable {
        /// Trigger name to disable
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add a custom trigger
    TriggerAdd {
        /// Trigger name
        name: String,

        /// File path pattern (glob, comma-separated for multiple)
        #[arg(long)]
        pattern: String,

        /// Handler command to execute
        #[arg(long)]
        handler: String,

        /// Optional description
        #[arg(long)]
        description: Option<String>,

        /// Priority (lower runs first, default 50)
        #[arg(long)]
        priority: Option<i32>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove a custom trigger (built-in triggers cannot be removed)
    TriggerRemove {
        /// Trigger name to remove
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Run pending triggers for a changeset
    TriggerRun {
        /// Changeset ID (defaults to most recent)
        changeset_id: Option<i64>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// List system state snapshots
    StateList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Limit number of states shown
        #[arg(short, long)]
        limit: Option<i64>,
    },

    /// Show details of a specific state
    StateShow {
        /// State number to show
        state_number: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Compare two system states
    StateDiff {
        /// Source state number
        from_state: i64,

        /// Target state number
        to_state: i64,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Restore system to a previous state
    StateRestore {
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
    StatePrune {
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
    StateCreate {
        /// Summary description for the state
        summary: String,

        /// Optional detailed description
        #[arg(long)]
        description: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
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

    /// Create a new collection (package group)
    CollectionCreate {
        /// Name of the collection
        name: String,

        /// Description of the collection
        #[arg(long)]
        description: Option<String>,

        /// Comma-separated list of member packages
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List all collections
    CollectionList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show details of a collection
    CollectionShow {
        /// Name of the collection
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add packages to a collection
    CollectionAdd {
        /// Name of the collection
        name: String,

        /// Packages to add (comma-separated)
        #[arg(value_delimiter = ',')]
        members: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove packages from a collection
    CollectionRemove {
        /// Name of the collection
        name: String,

        /// Packages to remove (comma-separated)
        #[arg(value_delimiter = ',')]
        members: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Delete a collection
    CollectionDelete {
        /// Name of the collection
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Install all packages in a collection
    CollectionInstall {
        /// Name of the collection
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Show what would be installed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip optional packages in the collection
        #[arg(long)]
        skip_optional: bool,
    },
}

// =============================================================================
// Main Entry Point
// =============================================================================

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { db_path }) => commands::cmd_init(&db_path),

        Some(Commands::Install { package, db_path, root, version, repo, dry_run, no_deps, no_scripts }) => {
            commands::cmd_install(&package, &db_path, &root, version, repo, dry_run, no_deps, no_scripts, None)
        }

        Some(Commands::Remove { package_name, db_path, root, no_scripts }) => {
            commands::cmd_remove(&package_name, &db_path, &root, no_scripts)
        }

        Some(Commands::Autoremove { db_path, root, dry_run, no_scripts }) => {
            commands::cmd_autoremove(&db_path, &root, dry_run, no_scripts)
        }

        Some(Commands::AdoptSystem { db_path, full, dry_run }) => {
            commands::cmd_adopt_system(&db_path, full, dry_run)
        }

        Some(Commands::Adopt { packages, db_path, full }) => {
            commands::cmd_adopt(&packages, &db_path, full)
        }

        Some(Commands::AdoptStatus { db_path }) => {
            commands::cmd_adopt_status(&db_path)
        }

        Some(Commands::Conflicts { db_path, verbose }) => {
            commands::cmd_conflicts(&db_path, verbose)
        }

        Some(Commands::Query { pattern, db_path, path, info, files, lsl }) => {
            let options = commands::QueryOptions {
                info,
                lsl,
                path,
                files,
            };
            commands::cmd_query(pattern.as_deref(), &db_path, options)
        }

        Some(Commands::Repquery { pattern, db_path, info }) => {
            commands::cmd_repquery(pattern.as_deref(), &db_path, info)
        }

        Some(Commands::QueryReason { pattern, db_path }) => {
            commands::cmd_query_reason(pattern.as_deref(), &db_path)
        }

        Some(Commands::History { db_path }) => commands::cmd_history(&db_path),

        Some(Commands::Rollback { changeset_id, db_path, root }) => {
            commands::cmd_rollback(changeset_id, &db_path, &root)
        }

        Some(Commands::Verify { package, db_path, root, rpm }) => {
            commands::cmd_verify(package, &db_path, &root, rpm)
        }

        Some(Commands::Depends { package_name, db_path }) => {
            commands::cmd_depends(&package_name, &db_path)
        }

        Some(Commands::Rdepends { package_name, db_path }) => {
            commands::cmd_rdepends(&package_name, &db_path)
        }

        Some(Commands::Deptree { package_name, db_path, reverse, depth }) => {
            commands::cmd_deptree(&package_name, &db_path, reverse, depth)
        }

        Some(Commands::Whatbreaks { package_name, db_path }) => {
            commands::cmd_whatbreaks(&package_name, &db_path)
        }

        Some(Commands::Whatprovides { capability, db_path }) => {
            commands::cmd_whatprovides(&capability, &db_path)
        }

        Some(Commands::ListComponents { package_name, db_path }) => {
            commands::cmd_list_components(&package_name, &db_path)
        }

        Some(Commands::QueryComponent { component_spec, db_path }) => {
            commands::cmd_query_component(&component_spec, &db_path)
        }

        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "conary", &mut io::stdout());
            Ok(())
        }

        Some(Commands::RepoAdd { name, url, db_path, priority, disabled, gpg_key, no_gpg_check, gpg_strict }) => {
            commands::cmd_repo_add(&name, &url, &db_path, priority, disabled, gpg_key, no_gpg_check, gpg_strict)
        }

        Some(Commands::RepoList { db_path, all }) => commands::cmd_repo_list(&db_path, all),

        Some(Commands::RepoRemove { name, db_path }) => {
            commands::cmd_repo_remove(&name, &db_path)
        }

        Some(Commands::RepoEnable { name, db_path }) => {
            commands::cmd_repo_enable(&name, &db_path)
        }

        Some(Commands::RepoDisable { name, db_path }) => {
            commands::cmd_repo_disable(&name, &db_path)
        }

        Some(Commands::RepoSync { name, db_path, force }) => {
            commands::cmd_repo_sync(name, &db_path, force)
        }

        Some(Commands::KeyImport { repository, key, db_path }) => {
            commands::cmd_key_import(&repository, &key, &db_path)
        }

        Some(Commands::KeyList { db_path }) => commands::cmd_key_list(&db_path),

        Some(Commands::KeyRemove { repository, db_path }) => {
            commands::cmd_key_remove(&repository, &db_path)
        }

        Some(Commands::Search { pattern, db_path }) => {
            commands::cmd_search(&pattern, &db_path)
        }

        Some(Commands::Update { package, db_path, root }) => {
            commands::cmd_update(package, &db_path, &root)
        }

        Some(Commands::Pin { package_name, db_path }) => {
            commands::cmd_pin(&package_name, &db_path)
        }

        Some(Commands::Unpin { package_name, db_path }) => {
            commands::cmd_unpin(&package_name, &db_path)
        }

        Some(Commands::ListPinned { db_path }) => commands::cmd_list_pinned(&db_path),

        Some(Commands::DeltaStats { db_path }) => commands::cmd_delta_stats(&db_path),

        Some(Commands::TriggerList { db_path, all, builtin }) => {
            commands::cmd_trigger_list(&db_path, all, builtin)
        }

        Some(Commands::TriggerShow { name, db_path }) => {
            commands::cmd_trigger_show(&name, &db_path)
        }

        Some(Commands::TriggerEnable { name, db_path }) => {
            commands::cmd_trigger_enable(&name, &db_path)
        }

        Some(Commands::TriggerDisable { name, db_path }) => {
            commands::cmd_trigger_disable(&name, &db_path)
        }

        Some(Commands::TriggerAdd { name, pattern, handler, description, priority, db_path }) => {
            commands::cmd_trigger_add(&name, &pattern, &handler, description.as_deref(), priority, &db_path)
        }

        Some(Commands::TriggerRemove { name, db_path }) => {
            commands::cmd_trigger_remove(&name, &db_path)
        }

        Some(Commands::TriggerRun { changeset_id, db_path, root }) => {
            commands::cmd_trigger_run(changeset_id, &db_path, &root)
        }

        Some(Commands::StateList { db_path, limit }) => {
            commands::cmd_state_list(&db_path, limit)
        }

        Some(Commands::StateShow { state_number, db_path }) => {
            commands::cmd_state_show(&db_path, state_number)
        }

        Some(Commands::StateDiff { from_state, to_state, db_path }) => {
            commands::cmd_state_diff(&db_path, from_state, to_state)
        }

        Some(Commands::StateRestore { state_number, db_path, dry_run }) => {
            commands::cmd_state_restore(&db_path, state_number, dry_run)
        }

        Some(Commands::StatePrune { keep, db_path, dry_run }) => {
            commands::cmd_state_prune(&db_path, keep, dry_run)
        }

        Some(Commands::StateCreate { summary, description, db_path }) => {
            commands::cmd_state_create(&db_path, &summary, description.as_deref())
        }

        Some(Commands::Restore { package, db_path, root, force, dry_run }) => {
            if package == "all" {
                commands::cmd_restore_all(&db_path, &root, dry_run)
            } else {
                commands::cmd_restore(&package, &db_path, &root, force, dry_run)
            }
        }

        Some(Commands::Scripts { package_path }) => {
            commands::cmd_scripts(&package_path)
        }

        Some(Commands::CollectionCreate { name, description, members, db_path }) => {
            commands::cmd_collection_create(&name, description.as_deref(), &members, &db_path)
        }

        Some(Commands::CollectionList { db_path }) => {
            commands::cmd_collection_list(&db_path)
        }

        Some(Commands::CollectionShow { name, db_path }) => {
            commands::cmd_collection_show(&name, &db_path)
        }

        Some(Commands::CollectionAdd { name, members, db_path }) => {
            commands::cmd_collection_add(&name, &members, &db_path)
        }

        Some(Commands::CollectionRemove { name, members, db_path }) => {
            commands::cmd_collection_remove_member(&name, &members, &db_path)
        }

        Some(Commands::CollectionDelete { name, db_path }) => {
            commands::cmd_collection_delete(&name, &db_path)
        }

        Some(Commands::CollectionInstall { name, db_path, root, dry_run, skip_optional }) => {
            commands::cmd_collection_install(&name, &db_path, &root, dry_run, skip_optional)
        }

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
