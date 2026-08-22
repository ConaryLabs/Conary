// src/commands/repo/options.rs

//! Typed input for repository enrollment.

use conary_core::db::models::SecurityAdvisorySupport;
use conary_core::repository::{
    ArchKeyringFormat, ArchSignatureRequirement, OpenPgpTrustRoot, RepositoryFormat,
};
use std::path::PathBuf;

/// Options for adding a new repository.
pub struct RepoAddOptions {
    pub name: String,
    pub url: String,
    pub package_format: Option<RepositoryFormat>,
    pub distribution: Option<String>,
    pub component: Option<String>,
    pub architecture: Option<String>,
    pub database: Option<String>,
    pub db_path: String,
    pub content_url: Option<String>,
    pub priority: i32,
    pub disabled: bool,
    pub debian_release_keys: Vec<OpenPgpTrustRoot>,
    pub rpm_metadata_keys: Vec<OpenPgpTrustRoot>,
    pub rpm_metalink: Option<String>,
    pub rpm_package_keys: Vec<OpenPgpTrustRoot>,
    pub arch_keyring: Option<String>,
    pub arch_keyring_format: Option<ArchKeyringFormat>,
    pub arch_master_keys: Vec<String>,
    pub arch_packager_key_threshold: Option<usize>,
    pub arch_database_signature: Option<ArchSignatureRequirement>,
    pub fingerprints: Vec<String>,
    pub yes: bool,
    pub replace: bool,
    pub default_strategy: Option<String>,
    pub remi_endpoint: Option<String>,
    pub remi_metadata_root: Option<PathBuf>,
    pub ccs_package_keys: Vec<PathBuf>,
    pub source_profile: Option<String>,
    pub source_id: Option<String>,
    pub repository_id: Option<String>,
    pub stream_kind: Option<String>,
    pub stream_id: Option<String>,
    pub policy_group: Option<String>,
    pub follow: bool,
    pub pin_snapshot_sha256: Option<String>,
    pub security_advisory_support: SecurityAdvisorySupport,
}
