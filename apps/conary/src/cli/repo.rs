// apps/conary/src/cli/repo.rs
//! Repository management commands

use super::DbArgs;
use clap::{Subcommand, ValueEnum};
use conary_core::repository::OpenPgpTrustRoot;

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

#[derive(Subcommand)]
pub enum RepoCommands {
    /// Add a repository
    Add {
        /// Repository name
        name: String,

        /// Repository URL (for metadata)
        url: String,

        /// Exact package metadata grammar served by this repository
        ///
        /// Required for non-static repositories. Static Conary repositories
        /// declare their own signed metadata contract.
        #[arg(long, value_enum)]
        package_format: Option<CliRepositoryFormat>,

        /// Exact Debian suite/codename (required for --package-format=deb)
        #[arg(long)]
        distribution: Option<String>,

        /// Exact Debian component (required for --package-format=deb)
        #[arg(long)]
        component: Option<String>,

        /// Exact package-manager architecture (required for rpm and deb)
        #[arg(long)]
        architecture: Option<String>,

        /// Exact Arch repository database name (required for arch)
        #[arg(long)]
        database: Option<String>,

        #[command(flatten)]
        db: DbArgs,

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

        /// Debian Release signer in exact FINGERPRINT=URL form; repeatable
        #[arg(
            long = "debian-release-key",
            value_name = "FINGERPRINT=URL",
            value_parser = parse_openpgp_root
        )]
        debian_release_keys: Vec<OpenPgpTrustRoot>,

        /// RPM repository-metadata signer in exact FINGERPRINT=URL form; repeatable
        #[arg(
            long = "rpm-metadata-key",
            value_name = "FINGERPRINT=URL",
            value_parser = parse_openpgp_root
        )]
        rpm_metadata_keys: Vec<OpenPgpTrustRoot>,

        /// Exact HTTPS metalink authenticating rpm-md repomd.xml
        #[arg(long, value_name = "URL", conflicts_with = "rpm_metadata_keys")]
        rpm_metalink: Option<String>,

        /// RPM package signer in exact FINGERPRINT=URL form; repeatable
        #[arg(
            long = "rpm-package-key",
            value_name = "FINGERPRINT=URL",
            value_parser = parse_openpgp_root
        )]
        rpm_package_keys: Vec<OpenPgpTrustRoot>,

        /// Exact Arch keyring URL containing masters, certifications, and packager keys
        #[arg(long, value_name = "URL")]
        arch_keyring: Option<String>,

        /// Exact Arch keyring source grammar
        #[arg(long, value_enum)]
        arch_keyring_format: Option<CliArchKeyringFormat>,

        /// Trusted Arch master-key fingerprint; repeatable
        #[arg(
            long = "arch-master-key",
            value_name = "FINGERPRINT",
            value_parser = parse_openpgp_fingerprint
        )]
        arch_master_keys: Vec<String>,

        /// Distinct trusted Arch master certifications required for a packager key
        #[arg(long, value_name = "COUNT")]
        arch_packager_key_threshold: Option<usize>,

        /// Whether this Arch repository requires a detached database signature
        #[arg(long, value_enum)]
        arch_database_signature: Option<CliArchDatabaseSignature>,

        /// Static repository root key fingerprint; repeat for multi-key roots
        #[arg(long = "fingerprint", value_name = "64-HEX")]
        fingerprints: Vec<String>,

        /// Assume yes to interactive static repository trust prompts
        #[arg(short = 'y', long)]
        yes: bool,

        /// Replace an existing static repository trust pin and repository row
        #[arg(long)]
        replace: bool,

        /// Default resolution strategy for packages from this repository
        ///
        /// When no per-package routing entry exists, use this strategy:
        /// - remi: Convert packages via Remi server (requires --remi-endpoint and --remi-distro)
        /// - binary: Download pre-built packages directly (default behavior)
        #[arg(long, value_name = "STRATEGY", value_parser = ["remi", "binary"])]
        default_strategy: Option<String>,

        /// Remi server endpoint URL (required when --default-strategy=remi)
        #[arg(long, value_name = "URL")]
        remi_endpoint: Option<String>,

        /// Distribution name for Remi conversion (required when --default-strategy=remi)
        ///
        /// Examples: fedora-44, ubuntu-26.04, arch
        #[arg(long, value_name = "DISTRO", value_parser = parse_public_profile_id)]
        remi_distro: Option<String>,

        /// Whether this repository publishes security-advisory metadata
        #[arg(long, value_enum, default_value_t = CliSecurityAdvisorySupport::Unknown)]
        security_advisories: CliSecurityAdvisorySupport,
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
