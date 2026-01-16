// src/cli.rs
//! CLI definitions for the Conary package manager
//!
//! This module contains all command-line interface definitions using clap.
//! The actual command implementations are in the `commands` module.

use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser)]
#[command(name = "conary")]
#[command(author = "Conary Project")]
#[command(version)]
#[command(about = "A next-generation package manager with atomic transactions", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
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

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,

        /// Allow downgrading to an older version
        #[arg(long)]
        allow_downgrade: bool,
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

    // ===== System Model Commands =====

    /// Show what changes are needed to reach the model state
    ///
    /// Compares the system model file (default: /etc/conary/system.toml)
    /// against the current system state and shows what packages would be
    /// installed, removed, or updated to reach the desired state.
    ModelDiff {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Apply the system model to reach the desired state
    ///
    /// Installs, removes, and updates packages to match the system model.
    /// This is essentially "sync to model" - the system will be modified
    /// to match what's declared in the model file.
    ModelApply {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// Skip optional packages
        #[arg(long)]
        skip_optional: bool,

        /// Force remove packages not in model (strict mode)
        #[arg(long)]
        strict: bool,

        /// Skip autoremove after applying
        #[arg(long)]
        no_autoremove: bool,
    },

    /// Check if system state matches the model
    ///
    /// Returns success (exit 0) if the system matches the model,
    /// or failure (exit 1) if there are differences.
    /// Useful for drift detection in CI/CD or monitoring.
    ModelCheck {
        /// Path to system model file
        #[arg(short, long, default_value = "/etc/conary/system.toml")]
        model: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show details of differences (verbose output)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Create a model file from current system state
    ///
    /// Captures the current system state (explicit packages, pins)
    /// and writes it as a system model file. Useful for creating
    /// a baseline or for reproducibility.
    ModelSnapshot {
        /// Output path for the model file
        #[arg(short, long, default_value = "system.toml")]
        output: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Add a comment/description to the model
        #[arg(long)]
        description: Option<String>,
    },

    /// List all labels
    LabelList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show detailed information (description, package count, parent)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Add a new label
    ///
    /// Labels use format: repository@namespace:tag
    /// Example: conary.example.com@rpl:2
    LabelAdd {
        /// Label in format repository@namespace:tag
        label: String,

        /// Description for the label
        #[arg(long)]
        description: Option<String>,

        /// Parent label (for branch history)
        #[arg(long)]
        parent: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Remove a label
    LabelRemove {
        /// Label to remove
        label: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Force removal even if packages use this label
        #[arg(short, long)]
        force: bool,
    },

    /// Show or modify the label path (search order for packages)
    LabelPath {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Add a label to the path
        #[arg(long)]
        add: Option<String>,

        /// Remove a label from the path
        #[arg(long)]
        remove: Option<String>,

        /// Priority for the label (lower = higher priority)
        #[arg(long)]
        priority: Option<i32>,
    },

    /// Show the label for a package
    LabelShow {
        /// Package name
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Set the label for a package
    LabelSet {
        /// Package name
        package: String,

        /// Label to set
        label: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Find packages by label
    LabelQuery {
        /// Label to search for
        label: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List configuration files
    ConfigList {
        /// Package name (optional - if omitted, shows modified configs)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show all config files, not just modified
        #[arg(short, long)]
        all: bool,
    },

    /// Show diff between installed config and package version
    ConfigDiff {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// Backup a configuration file
    ConfigBackup {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// Restore a configuration file from backup
    ConfigRestore {
        /// Path to the config file
        path: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Specific backup ID to restore (default: latest)
        #[arg(long)]
        backup_id: Option<i64>,
    },

    /// Check status of configuration files
    ConfigCheck {
        /// Package name (optional - if omitted, checks all)
        package: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,
    },

    /// List backups for a configuration file
    ConfigBackups {
        /// Path to the config file
        path: String,

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

        /// Sandbox mode for scriptlets: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    // =========================================================================
    // CCS Package Format Commands
    // =========================================================================

    /// Initialize a new CCS package manifest (ccs.toml)
    #[command(name = "ccs-init")]
    CcsInit {
        /// Directory to initialize (defaults to current directory)
        #[arg(default_value = ".")]
        path: String,

        /// Package name (defaults to directory name)
        #[arg(short, long)]
        name: Option<String>,

        /// Package version
        #[arg(short, long, default_value = "0.1.0")]
        version: String,

        /// Overwrite existing ccs.toml
        #[arg(long)]
        force: bool,
    },

    /// Build a CCS package from the current project
    #[command(name = "ccs-build")]
    CcsBuild {
        /// Path to ccs.toml or directory containing it
        #[arg(default_value = ".")]
        path: String,

        /// Output directory for built packages
        #[arg(short, long, default_value = "./target/ccs")]
        output: String,

        /// Target format(s): ccs, deb, rpm, arch, all
        #[arg(short, long, default_value = "ccs")]
        target: String,

        /// Source directory containing files to package
        #[arg(long)]
        source: Option<String>,

        /// Don't auto-classify files into components
        #[arg(long)]
        no_classify: bool,

        /// Disable CDC chunking (chunking is enabled by default)
        /// When disabled, files are stored as whole blobs instead of
        /// content-defined chunks.
        #[arg(long)]
        no_chunked: bool,

        /// Show what would be built without creating packages
        #[arg(long)]
        dry_run: bool,
    },

    /// Inspect a CCS package file
    #[command(name = "ccs-inspect")]
    CcsInspect {
        /// Path to .ccs package file
        package: String,

        /// Show file listing
        #[arg(short, long)]
        files: bool,

        /// Show hook definitions
        #[arg(long)]
        hooks: bool,

        /// Show dependencies and provides
        #[arg(long)]
        deps: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Verify a CCS package signature and contents
    #[command(name = "ccs-verify")]
    CcsVerify {
        /// Path to .ccs package file
        package: String,

        /// Trust policy file (optional)
        #[arg(long)]
        policy: Option<String>,

        /// Allow packages without signatures
        #[arg(long)]
        allow_unsigned: bool,
    },

    /// Sign a CCS package with an Ed25519 key
    #[command(name = "ccs-sign")]
    CcsSign {
        /// Path to .ccs package file to sign
        package: String,

        /// Path to private signing key file
        #[arg(short, long)]
        key: String,

        /// Output path (default: overwrites input)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Generate an Ed25519 signing key pair
    #[command(name = "ccs-keygen")]
    CcsKeygen {
        /// Output path for key files (without extension)
        #[arg(short, long, default_value = "ccs-signing-key")]
        output: String,

        /// Key identifier (e.g., name or email)
        #[arg(long)]
        key_id: Option<String>,

        /// Overwrite existing key files
        #[arg(long)]
        force: bool,
    },

    /// Install a native CCS package
    #[command(name = "ccs-install")]
    CcsInstall {
        /// Path to .ccs package file
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Installation root directory
        #[arg(short, long, default_value = "/")]
        root: String,

        /// Show what would be installed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Allow installation of unsigned packages
        #[arg(long)]
        allow_unsigned: bool,

        /// Trust policy file for signature verification
        #[arg(long)]
        policy: Option<String>,

        /// Specific components to install (comma-separated)
        #[arg(long, value_delimiter = ',')]
        components: Option<Vec<String>>,

        /// Sandbox mode for hooks: auto, always, never (default: never)
        #[arg(long, default_value = "never")]
        sandbox: String,
    },

    /// Export CCS packages to container image format
    #[command(name = "ccs-export")]
    CcsExport {
        /// CCS package file(s) to export
        #[arg(required = true)]
        packages: Vec<String>,

        /// Output file path
        #[arg(short, long)]
        output: String,

        /// Export format: oci (default)
        #[arg(short, long, default_value = "oci")]
        format: String,

        /// Path to the database file (for dependency resolution)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    // =========================================================================
    // Derived Packages Commands
    // =========================================================================

    /// List all derived packages
    #[command(name = "derive-list")]
    DeriveList {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Show detailed information
        #[arg(short, long)]
        verbose: bool,
    },

    /// Show details of a derived package
    #[command(name = "derive-show")]
    DeriveShow {
        /// Name of the derived package
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Create a new derived package
    ///
    /// Derived packages allow customizing existing packages without rebuilding.
    /// Use patches and file overrides to make modifications.
    #[command(name = "derive")]
    DeriveCreate {
        /// Name for the derived package
        name: String,

        /// Parent package to derive from
        #[arg(long)]
        from: String,

        /// Version suffix (e.g., "+custom")
        #[arg(long)]
        version_suffix: Option<String>,

        /// Description
        #[arg(long)]
        description: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add a patch to a derived package
    #[command(name = "derive-patch")]
    DerivePatch {
        /// Name of the derived package
        name: String,

        /// Path to the patch file
        patch_file: String,

        /// Strip level for patch application (default: 1)
        #[arg(long)]
        strip: Option<i32>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add a file override to a derived package
    ///
    /// Replace a file in the parent package, or remove it.
    #[command(name = "derive-override")]
    DeriveOverride {
        /// Name of the derived package
        name: String,

        /// Target path in the package to override
        target: String,

        /// Source file to replace with (omit to remove the file)
        #[arg(long)]
        source: Option<String>,

        /// File permissions (octal, e.g., 644)
        #[arg(long)]
        mode: Option<u32>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Build a derived package
    #[command(name = "derive-build")]
    DeriveBuild {
        /// Name of the derived package
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Delete a derived package
    #[command(name = "derive-delete")]
    DeriveDelete {
        /// Name of the derived package
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// List stale derived packages (parent was updated)
    #[command(name = "derive-stale")]
    DeriveStale {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
