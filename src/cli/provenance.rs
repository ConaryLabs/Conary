// src/cli/provenance.rs

//! CLI commands for Package DNA / Provenance queries
//!
//! These commands allow users to query the complete lineage of packages:
//! - What source produced this binary?
//! - Who signed off on this package?
//! - What dependencies (with their DNA) were used to build it?
//! - Verify transparency log entries

use clap::Subcommand;
use super::DbArgs;

#[derive(Subcommand)]
pub enum ProvenanceCommands {
    /// Show provenance information for a package
    ///
    /// Displays the complete lineage (Package DNA) including:
    /// - Source: upstream URL, hash, git commit, patches
    /// - Build: recipe, dependencies, environment
    /// - Signatures: builder, reviewers, transparency logs
    /// - Content: merkle root, component hashes
    Show {
        /// Package name (optionally with @version)
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// What to show: source, build, signatures, content, all (default: all)
        #[arg(long, default_value = "all")]
        section: String,

        /// Show recursive dependency provenance
        #[arg(long)]
        recursive: bool,

        /// Output format: text, json, tree
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Verify provenance against transparency log
    ///
    /// Checks the package's entry in Sigstore Rekor to verify
    /// the signature and provenance haven't been tampered with.
    Verify {
        /// Package name (optionally with @version)
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// Verify all signatures (builder + reviewers)
        #[arg(long)]
        all_signatures: bool,
    },

    /// Compare provenance between two package versions
    ///
    /// Shows what changed in the provenance chain between versions:
    /// - Source changes (new patches, different upstream)
    /// - Build changes (different dependencies, environment)
    /// - Signature changes (different signers)
    Diff {
        /// First package (name@version)
        package1: String,

        /// Second package (name@version)
        package2: String,

        #[command(flatten)]
        db: DbArgs,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Find packages built with a specific dependency
    ///
    /// Useful for security audits: "What packages used this vulnerable
    /// version of openssl?" Returns packages whose build_deps include
    /// the specified dependency DNA.
    FindByDep {
        /// Dependency name
        dep_name: String,

        /// Optional version constraint
        #[arg(long)]
        version: Option<String>,

        /// Optional DNA hash to match exactly
        #[arg(long)]
        dna: Option<String>,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Export provenance as SBOM (Software Bill of Materials)
    ///
    /// Generates an SBOM in SPDX or CycloneDX format containing
    /// the complete dependency tree with provenance information.
    Export {
        /// Package name (optionally with @version)
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// Output format: spdx, cyclonedx
        #[arg(long, default_value = "spdx")]
        format: String,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,

        /// Include recursive dependencies
        #[arg(long)]
        recursive: bool,
    },

    /// Register provenance in transparency log
    ///
    /// Uploads the package's provenance to Sigstore Rekor for
    /// public auditability. Requires signing key.
    Register {
        /// Package name (optionally with @version)
        package: String,

        #[command(flatten)]
        db: DbArgs,

        /// Signing key ID or path
        #[arg(long)]
        key: String,

        /// Dry run - show what would be registered
        #[arg(long)]
        dry_run: bool,
    },

    /// Show packages without complete provenance
    ///
    /// Lists packages that are missing provenance information,
    /// useful for identifying packages that need provenance backfill.
    Audit {
        #[command(flatten)]
        db: DbArgs,

        /// Show only packages missing specific section: source, build, signatures
        #[arg(long)]
        missing: Option<String>,

        /// Include converted (legacy) packages
        #[arg(long)]
        include_converted: bool,
    },
}
