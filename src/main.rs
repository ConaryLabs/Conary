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
    },

    /// Remove an installed package
    Remove {
        /// Package name to remove
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Query installed packages
    Query {
        /// Optional pattern to filter packages
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

    /// Show what packages would break if a package is removed
    Whatbreaks {
        /// Package name
        package_name: String,

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

    /// Show delta update statistics
    DeltaStats {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
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

        Some(Commands::Install { package, db_path, root, version, repo, dry_run }) => {
            commands::cmd_install(&package, &db_path, &root, version, repo, dry_run)
        }

        Some(Commands::Remove { package_name, db_path }) => {
            commands::cmd_remove(&package_name, &db_path)
        }

        Some(Commands::Query { pattern, db_path }) => {
            commands::cmd_query(pattern.as_deref(), &db_path)
        }

        Some(Commands::History { db_path }) => commands::cmd_history(&db_path),

        Some(Commands::Rollback { changeset_id, db_path, root }) => {
            commands::cmd_rollback(changeset_id, &db_path, &root)
        }

        Some(Commands::Verify { package, db_path, root }) => {
            commands::cmd_verify(package, &db_path, &root)
        }

        Some(Commands::Depends { package_name, db_path }) => {
            commands::cmd_depends(&package_name, &db_path)
        }

        Some(Commands::Rdepends { package_name, db_path }) => {
            commands::cmd_rdepends(&package_name, &db_path)
        }

        Some(Commands::Whatbreaks { package_name, db_path }) => {
            commands::cmd_whatbreaks(&package_name, &db_path)
        }

        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "conary", &mut io::stdout());
            Ok(())
        }

        Some(Commands::RepoAdd { name, url, db_path, priority, disabled }) => {
            commands::cmd_repo_add(&name, &url, &db_path, priority, disabled)
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

        Some(Commands::Search { pattern, db_path }) => {
            commands::cmd_search(&pattern, &db_path)
        }

        Some(Commands::Update { package, db_path, root }) => {
            commands::cmd_update(package, &db_path, &root)
        }

        Some(Commands::DeltaStats { db_path }) => commands::cmd_delta_stats(&db_path),

        None => {
            println!("Conary Package Manager v{}", env!("CARGO_PKG_VERSION"));
            println!("Run 'conary --help' for usage information");
            Ok(())
        }
    }
}
