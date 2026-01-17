// src/cli/query.rs
//! Query commands: list packages, dependencies, components

use clap::Subcommand;

#[derive(Subcommand)]
pub enum QueryCommands {
    /// Query installed packages
    List {
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
    Reason {
        /// Reason filter pattern (or show all grouped if not specified)
        pattern: Option<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
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
    #[command(name = "list-components")]
    ListComponents {
        /// Package name
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Query files in a specific component (e.g., nginx:lib)
    Component {
        /// Component spec in format "package:component" (e.g., nginx:lib)
        component_spec: String,

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

    /// Show changeset history
    History {
        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Generate SBOM (Software Bill of Materials) for a package
    ///
    /// Outputs a CycloneDX 1.5 format SBOM in JSON. This is useful for
    /// security auditing, compliance, and vulnerability scanning.
    Sbom {
        /// Package name (or "all" for entire system)
        package_name: String,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Output format
        #[arg(short, long, default_value = "cyclonedx")]
        format: String,

        /// Output to file instead of stdout
        #[arg(short, long)]
        output: Option<String>,
    },
}
