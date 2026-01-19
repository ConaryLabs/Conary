// src/cli/query.rs
//! Query commands: dependencies, components, labels, and advanced analysis

use clap::Subcommand;

use super::label::LabelCommands;
use super::DbArgs;

#[derive(Subcommand)]
pub enum QueryCommands {
    /// Show dependencies for a package
    Depends {
        /// Package name
        package_name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show reverse dependencies (what depends on this package)
    Rdepends {
        /// Package name
        package_name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show full dependency tree for a package
    Deptree {
        /// Package name
        package_name: String,

        #[command(flatten)]
        db: DbArgs,

        /// Show reverse dependency tree (what depends on this, transitively)
        #[arg(short, long)]
        reverse: bool,

        /// Maximum depth to traverse (default: unlimited)
        #[arg(long)]
        depth: Option<usize>,
    },

    /// Find which package provides a capability
    Whatprovides {
        /// Capability to search for (package name, file path, library, virtual provide)
        capability: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Show what packages would break if a package is removed
    Whatbreaks {
        /// Package name
        package_name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Query packages by installation reason
    ///
    /// Shows why packages were installed. Supports filters:
    /// - "explicit" - directly installed by user
    /// - "dependency" - installed as a dependency
    /// - "collection" - installed via a collection
    /// - "@name" - installed via specific collection
    /// - Custom pattern with * wildcard
    Reason {
        /// Reason filter pattern (or show all grouped if not specified)
        pattern: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Query packages available in repositories (not installed)
    ///
    /// Similar to dnf repoquery or apt-cache search.
    /// Searches package names and descriptions in synced repository metadata.
    Repquery {
        /// Optional pattern to filter packages
        pattern: Option<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Show detailed package information
        #[arg(short, long)]
        info: bool,
    },

    /// Query files in a specific component (e.g., nginx:lib)
    Component {
        /// Component spec in format "package:component" (e.g., nginx:lib)
        component_spec: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// List components of an installed package
    Components {
        /// Package name
        package_name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Display scriptlets (install/remove hooks) from a package file
    Scripts {
        /// Path to package file (RPM, DEB, or Arch)
        package_path: String,
    },

    /// Show delta update statistics
    #[command(name = "delta-stats")]
    DeltaStats {
        #[command(flatten)]
        db: DbArgs,
    },

    /// Check for file conflicts and ownership issues
    Conflicts {
        #[command(flatten)]
        db: DbArgs,

        /// Show detailed output
        #[arg(short, long)]
        verbose: bool,
    },

    // =========================================================================
    // Nested Subcommands
    // =========================================================================
    /// Label and provenance management
    #[command(subcommand)]
    Label(LabelCommands),
}
