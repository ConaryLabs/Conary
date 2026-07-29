// apps/conary/src/cli/repo.rs
//! Repository management commands

use super::DbArgs;
use clap::{Subcommand, ValueEnum};
use conary_core::repository::OpenPgpTrustRoot;
use std::path::PathBuf;

pub(super) fn parse_public_profile_id(value: &str) -> Result<String, String> {
    conary_core::repository::supported_profiles::profile_by_public_id(value)
        .map(|profile| profile.id().to_string())
        .ok_or_else(|| {
            let supported = conary_core::repository::supported_profiles::public_profiles()
                .iter()
                .map(|profile| profile.id())
                .collect::<Vec<_>>()
                .join(", ");
            format!("unsupported public profile '{value}'; expected one of: {supported}")
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliSecurityAdvisorySupport {
    Unknown,
    Unsupported,
    Supported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliRepositoryFormat {
    Rpm,
    Deb,
    Arch,
    Json,
}

impl From<CliRepositoryFormat> for conary_core::repository::RepositoryFormat {
    fn from(value: CliRepositoryFormat) -> Self {
        match value {
            CliRepositoryFormat::Rpm => Self::Fedora,
            CliRepositoryFormat::Deb => Self::Debian,
            CliRepositoryFormat::Arch => Self::Arch,
            CliRepositoryFormat::Json => Self::Json,
        }
    }
}

fn parse_openpgp_root(value: &str) -> Result<OpenPgpTrustRoot, String> {
    let (fingerprint, url) = value.split_once('=').ok_or_else(|| {
        "expected an exact OpenPGP trust root in FINGERPRINT=URL form".to_string()
    })?;
    OpenPgpTrustRoot::new(url.to_string(), fingerprint.to_string()).map_err(|error| {
        format!("invalid OpenPGP trust root '{value}': {error}; expected FINGERPRINT=URL")
    })
}

fn parse_openpgp_fingerprint(value: &str) -> Result<String, String> {
    if !matches!(value.len(), 40 | 64)
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_lowercase())
    {
        return Err(
            "expected exactly 40 or 64 uppercase hexadecimal fingerprint digits".to_string(),
        );
    }
    Ok(value.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliArchDatabaseSignature {
    Required,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliArchKeyringFormat {
    OpenPgp,
    AlpmPackageZstd,
}

#[derive(clap::Args)]
pub struct RepoAddArgs {
    /// Repository name
    pub name: String,

    /// Repository URL (for metadata)
    pub url: String,

    /// Exact package metadata grammar served by this repository
    ///
    /// Required for non-static repositories. Static Conary repositories
    /// declare their own signed metadata contract.
    #[arg(long, value_enum)]
    pub package_format: Option<CliRepositoryFormat>,

    /// Exact Debian suite/codename (required for --package-format=deb)
    #[arg(long)]
    pub distribution: Option<String>,

    /// Exact Debian component (required for --package-format=deb)
    #[arg(long)]
    pub component: Option<String>,

    /// Exact package-manager architecture (required for rpm and deb)
    #[arg(long)]
    pub architecture: Option<String>,

    /// Exact Arch repository database name (required for arch)
    #[arg(long)]
    pub database: Option<String>,

    #[command(flatten)]
    pub db: DbArgs,

    /// Content mirror URL for package downloads (reference mirror pattern)
    ///
    /// If set, metadata is fetched from --url but packages are downloaded
    /// from --content-url. This enables scenarios like:
    /// - Trusted metadata server with local content mirrors
    /// - Hosting custom metadata pointing to upstream content
    #[arg(long)]
    pub content_url: Option<String>,

    /// Repository priority (higher = preferred)
    #[arg(short, long, default_value = "50")]
    pub priority: i32,

    /// Add repository in disabled state
    #[arg(long)]
    pub disabled: bool,

    /// Debian Release signer in exact FINGERPRINT=URL form; repeatable
    #[arg(
        long = "debian-release-key",
        value_name = "FINGERPRINT=URL",
        value_parser = parse_openpgp_root
    )]
    pub debian_release_keys: Vec<OpenPgpTrustRoot>,

    /// RPM repository-metadata signer in exact FINGERPRINT=URL form; repeatable
    #[arg(
        long = "rpm-metadata-key",
        value_name = "FINGERPRINT=URL",
        value_parser = parse_openpgp_root
    )]
    pub rpm_metadata_keys: Vec<OpenPgpTrustRoot>,

    /// Exact HTTPS metalink authenticating rpm-md repomd.xml
    #[arg(long, value_name = "URL", conflicts_with = "rpm_metadata_keys")]
    pub rpm_metalink: Option<String>,

    /// RPM package signer in exact FINGERPRINT=URL form; repeatable
    #[arg(
        long = "rpm-package-key",
        value_name = "FINGERPRINT=URL",
        value_parser = parse_openpgp_root
    )]
    pub rpm_package_keys: Vec<OpenPgpTrustRoot>,

    /// Exact Arch keyring URL containing masters, certifications, and packager keys
    #[arg(long, value_name = "URL")]
    pub arch_keyring: Option<String>,

    /// Exact Arch keyring source grammar
    #[arg(long, value_enum)]
    pub arch_keyring_format: Option<CliArchKeyringFormat>,

    /// Trusted Arch master-key fingerprint; repeatable
    #[arg(
        long = "arch-master-key",
        value_name = "FINGERPRINT",
        value_parser = parse_openpgp_fingerprint
    )]
    pub arch_master_keys: Vec<String>,

    /// Distinct trusted Arch master certifications required for a packager key
    #[arg(long, value_name = "COUNT")]
    pub arch_packager_key_threshold: Option<usize>,

    /// Whether this Arch repository requires a detached database signature
    #[arg(long, value_enum)]
    pub arch_database_signature: Option<CliArchDatabaseSignature>,

    /// Static repository root key fingerprint; repeat for multi-key roots
    #[arg(long = "fingerprint", value_name = "64-HEX")]
    pub fingerprints: Vec<String>,

    /// Assume yes to interactive static repository trust prompts
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Replace an existing static repository trust pin and repository row
    #[arg(long)]
    pub replace: bool,

    /// Default resolution strategy for packages from this repository
    ///
    /// When no per-package routing entry exists, use this strategy:
    /// - remi: Convert packages via Remi server (requires --remi-endpoint)
    /// - binary: Download pre-built packages directly (default behavior)
    #[arg(long, value_name = "STRATEGY", value_parser = ["remi", "binary"])]
    pub default_strategy: Option<String>,

    /// Remi server endpoint URL (required when --default-strategy=remi)
    #[arg(long, value_name = "URL")]
    pub remi_endpoint: Option<String>,

    /// Authenticated Ed25519 public key allowed to sign CCS packages; repeatable
    ///
    /// Canonical remi.conary.io keys ship with Conary. Self-hosted Remi
    /// repositories must provide their targets.public file explicitly.
    #[arg(
        long = "ccs-package-key",
        value_name = "PUBLIC_KEY_FILE",
        requires = "default_strategy"
    )]
    pub ccs_package_keys: Vec<PathBuf>,

    /// Exact public distro profile served by this repository
    ///
    /// Examples: fedora-44, ubuntu-26.04, arch
    #[arg(long, value_name = "PROFILE", value_parser = parse_public_profile_id)]
    pub source_profile: Option<String>,

    /// Whether this repository publishes security-advisory metadata
    #[arg(long, value_enum, default_value_t = CliSecurityAdvisorySupport::Unknown)]
    pub security_advisories: CliSecurityAdvisorySupport,
}

#[derive(Subcommand)]
pub enum RepoCommands {
    /// Add a repository
    Add {
        #[command(flatten)]
        args: Box<RepoAddArgs>,
    },

    /// List configured repositories
    List {
        #[command(flatten)]
        db: DbArgs,

        /// Show all repositories (including disabled)
        #[arg(short, long)]
        all: bool,
    },

    /// Remove a repository
    Remove {
        /// Repository name
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Reset static repository trust and synced package visibility
    #[command(name = "reset-trust")]
    ResetTrust {
        /// Repository name
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Enable a repository
    Enable {
        /// Repository name
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Disable a repository
    Disable {
        /// Repository name
        name: String,

        #[command(flatten)]
        db: DbArgs,
    },

    /// Sync repository metadata
    Sync {
        /// Optional repository name (syncs all enabled if not specified)
        name: Option<String>,

        #[command(flatten)]
        db: DbArgs,

        /// Force sync even if recently synced
        #[arg(short, long)]
        force: bool,
    },
}
