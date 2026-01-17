// src/cli/ccs.rs
//! CCS (Conary Container System) package format commands

use clap::Subcommand;

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

        /// Allow packages without signatures
        #[arg(long)]
        allow_unsigned: bool,
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

        /// Skip dependency checking
        #[arg(long)]
        no_deps: bool,
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

        /// Path to the database file (for dependency resolution)
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,
    },

    /// Spawn a shell with packages available temporarily
    ///
    /// Creates an ephemeral environment with the specified packages available.
    /// When the shell exits, the temporary environment is cleaned up.
    /// Similar to `nix-shell` or `nix develop`.
    Shell {
        /// Packages to make available in the shell
        #[arg(required = true)]
        packages: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Shell to use (defaults to $SHELL or /bin/sh)
        #[arg(long)]
        shell: Option<String>,

        /// Additional environment variables (KEY=VALUE)
        #[arg(short, long)]
        env: Vec<String>,

        /// Keep the temporary directory after exit (for debugging)
        #[arg(long)]
        keep: bool,
    },

    /// Run a command with a package available temporarily
    ///
    /// Executes a command with the specified package available without
    /// permanently installing it. Similar to `nix run` or `npx`.
    Run {
        /// Package to run from
        package: String,

        /// Command and arguments to run (after --)
        #[arg(last = true)]
        command: Vec<String>,

        /// Path to the database file
        #[arg(short, long, default_value = "/var/lib/conary/conary.db")]
        db_path: String,

        /// Additional environment variables (KEY=VALUE)
        #[arg(short, long)]
        env: Vec<String>,
    },
}
