// src/cli/label.rs
//! Label and provenance management commands

use clap::Subcommand;

#[derive(Subcommand)]
pub enum LabelCommands {
    /// List all labels
    List {
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
    Add {
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
    Remove {
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
    Path {
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
    Show {
        /// Package name
        package: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Set the label for a package
    Set {
        /// Package name
        package: String,

        /// Label to set
        label: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Find packages by label
    Query {
        /// Label to search for
        label: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },
}
