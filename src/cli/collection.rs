// src/cli/collection.rs
//! Package collection/group management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum CollectionCommands {
    /// Create a new collection (package group)
    Create {
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
    List {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Show details of a collection
    Show {
        /// Name of the collection
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Add packages to a collection
    Add {
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
    Remove {
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
    Delete {
        /// Name of the collection
        name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Install all packages in a collection
    Install {
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
}
