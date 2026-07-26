// src/cli/ccs.rs
//! CCS (Conary Container System) package format commands

use super::{CommonArgs, DbArgs};
use clap::Subcommand;

#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum CcsOutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub enum CcsCommands {
    /// Initialize a new CCS package manifest (ccs.toml)
    Init {
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

        /// Authoring template to generate
        #[arg(long, value_enum)]
        template: Option<crate::commands::CcsInitTemplate>,
    },

    /// Build a CCS package from the current project
    Build {
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

        /// Disable CDC chunking (chunking is enabled by default)
        /// When disabled, files are stored as whole blobs instead of
        /// content-defined chunks.
        #[arg(long)]
        no_chunked: bool,

        /// Show what would be built without creating packages
        #[arg(long)]
        dry_run: bool,

        /// Sign CCS output with the user-local development key
        #[arg(long, conflicts_with = "key")]
        local_dev: bool,

        /// Private signing key for CCS output
        #[arg(long)]
        key: Option<String>,
    },

    /// Lint a CCS authoring manifest
    Lint {
        /// Path to ccs.toml or directory containing it
        #[arg(default_value = ".")]
        path: String,

        /// Output format
        #[arg(long, value_enum, default_value_t = CcsOutputFormat::Text)]
        format: CcsOutputFormat,
    },

    /// Inspect a CCS package file
    Inspect {
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
    Verify {
        /// Path to .ccs package file
        package: String,

        /// Trust policy file (optional)
        #[arg(long)]
        policy: Option<String>,
    },

    /// Verify and dry-run-test a CCS package in an isolated workspace
    Test {
        /// Path to .ccs package file
        package: String,

        /// Run the package-local install proof without creating live state
        #[arg(long)]
        dry_run: bool,

        /// Trust policy file
        #[arg(long)]
        policy: Option<String>,

        /// Keep the isolated test workspace after completion
        #[arg(long)]
        keep_workspace: bool,
    },

    /// Sign a CCS package with an Ed25519 key
    Sign {
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
    Keygen {
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
    Install {
        /// Path to .ccs package file
        package: String,

        #[command(flatten)]
        common: CommonArgs,

        /// Show what would be installed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Confirm applying this command's active-system changes
        #[arg(short = 'y', long)]
        yes: bool,

        /// Trust policy file for signature verification
        #[arg(long)]
        policy: Option<String>,

        /// Specific components to install (comma-separated)
        #[arg(long, value_delimiter = ',')]
        components: Option<Vec<String>>,

        /// Sandbox mode for hooks: always or never (default: always)
        #[arg(long, value_enum, default_value_t = crate::cli::CliSandboxMode::Always)]
        sandbox: crate::cli::CliSandboxMode,

        /// Skip dependency checking
        #[arg(long)]
        no_deps: bool,

        /// Allow reinstalling an already-installed package at the same version
        #[arg(long)]
        reinstall: bool,
    },

    /// Export CCS packages to container image format
    Export {
        /// CCS package file(s) to export
        #[arg(required = true)]
        packages: Vec<String>,

        /// Output file path
        #[arg(short, long)]
        output: String,

        /// Export format: oci (default)
        #[arg(short, long, default_value = "oci")]
        format: String,

        /// Trust policy file for CCS authority verification
        #[arg(long, value_name = "PATH")]
        policy: String,
    },

    /// Enhance converted packages with CCS features
    ///
    /// Records exact conversion provenance for packages converted from native formats.
    Enhance {
        #[command(flatten)]
        db: DbArgs,

        /// Specific trove ID to enhance
        #[arg(long)]
        trove_id: Option<i64>,

        /// Enhance all pending packages
        #[arg(long)]
        all_pending: bool,

        /// Re-enhance packages with outdated enhancement version
        #[arg(long)]
        update_outdated: bool,

        /// Enhancement type to run (provenance)
        #[arg(long, value_delimiter = ',')]
        types: Option<Vec<String>>,

        /// Force re-enhancement even if already done
        #[arg(long)]
        force: bool,

        /// Show enhancement statistics
        #[arg(long)]
        stats: bool,

        /// Show what would be enhanced without making changes
        #[arg(long)]
        dry_run: bool,
    },
}
