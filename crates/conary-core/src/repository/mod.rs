// crates/conary-core/src/repository/mod.rs

//! Repository management and package downloading
//!
//! This module provides functionality for:
//! - Managing remote package repositories
//! - Synchronizing repository metadata
//! - Downloading packages with retry and resume support
//! - Verifying package checksums
//! - Ecosystem-native repository and package signature verification
//! - Native metadata format parsing (Arch, Debian, Fedora)

pub mod catalog;
mod client;
mod dependencies;
pub mod distro;
mod download;
pub(crate) mod error_helpers;
mod management;
mod metadata;
pub mod mirror_health;
pub mod mirror_selector;
pub mod registry;
pub mod remi;
pub mod remi_authority;
pub mod remi_metadata;
pub mod resolution;
pub mod retry;
mod rpm_verifier;
pub mod substituter;
pub mod supported_profiles;
mod sync;
pub mod trust;

pub mod declarations;
pub mod dependency_model;
mod dependency_source;
pub mod effective_policy;
pub mod enrollment;
mod eopkg_version;
pub mod package_relation;
pub mod parsers;
pub mod requirement;
pub mod resolution_policy;
pub mod rpm_dependency;
pub mod rpm_runtime;
pub mod selector;
pub mod static_repo;
pub mod versioning;

pub mod chunk_fetcher;

// Re-export main types and functions
pub use client::RepositoryClient;
pub(crate) use client::{MAX_BYTES_RESPONSE_SIZE, read_response_bytes_with_limit};
pub use dependencies::download_dependencies;
pub use download::{
    DownloadOptions, DownloadProgress, download_binary_package_verified,
    download_binary_package_verified_with_progress, download_delta, download_package_verified,
    download_package_verified_with_progress, download_package_with_authority_verified,
    download_static_package_verified, download_static_package_verified_with_progress,
    verify_cached_package_verified, verify_checksum,
};
pub use effective_policy::{EffectiveSourcePolicy, load_effective_policy};
pub use management::{add_repository, remove_repository, search_packages, set_repository_enabled};
pub use metadata::{DeltaInfo, PackageMetadata, RepositoryMetadata};
pub use mirror_health::{MirrorHealth, MirrorHealthTracker};
pub use mirror_selector::{MirrorSelector, MirrorStrategy};
pub use parsers::{ChecksumType, RepositoryParser};
pub use registry::{RepositoryFormat, RepositoryParserConfig, create_parser};
pub use remi::{PackageManifest, RemiClient};
pub use resolution::{
    PackageResolver, PackageSource, RepositorySourceKind, RepositorySourceMetadata,
    ResolutionOptions, resolve_package,
};
pub use retry::{RetryConfig, with_retry};
pub use selector::{PackageSelector, PackageWithRepo, SelectionOptions};
pub use static_repo::{
    PackageKeyEntry, PackageKeyStatus, PackageKeysFile, RepoIdentity, StaticIndex,
    StaticPackageEntry,
};
pub use substituter::{SubstituterChain, SubstituterResult, SubstituterSource};
pub use sync::{
    RepositoryWriteAuthority, current_timestamp, needs_sync, parse_timestamp, sync_repository,
    sync_repository_from_db_path,
};
pub use trust::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, ArchSignatureRequirement, ArchTrustLevel,
    OpenPgpTrustRoot, RepositoryTrustPolicy, RpmMetadataAuthority, TrustRole,
    eopkg_origin_from_index_url,
};

pub use chunk_fetcher::{
    ChunkData, ChunkFetcher, CompositeChunkFetcher, HttpChunkFetcher, LocalCacheFetcher,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        NativeSourceEcosystem, NativeSourceStream, Repository, RepositoryPolicyScope,
        RepositorySourcePolicy, RepositoryUpdateMode,
    };
    use crate::db::testing::create_test_db;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_add_repository() {
        let (_temp, conn) = create_test_db();

        let repo = add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
            RepositoryParserConfig::Json,
            None,
            true,
            10,
        )
        .unwrap();

        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.url, "https://example.com/repo");
        assert!(repo.enabled);
        assert_eq!(repo.priority, 10);
    }

    #[test]
    fn test_add_duplicate_repository() {
        let (_temp, conn) = create_test_db();

        add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
            RepositoryParserConfig::Json,
            None,
            true,
            10,
        )
        .unwrap();

        // Try to add duplicate
        let result = add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/other".to_string(),
            RepositoryParserConfig::Json,
            None,
            true,
            10,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_remove_repository() {
        let (_temp, conn) = create_test_db();

        add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
            RepositoryParserConfig::Json,
            None,
            true,
            10,
        )
        .unwrap();

        remove_repository(&conn, "test-repo").unwrap();

        let found = Repository::find_by_name(&conn, "test-repo").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_enable_disable_repository() {
        let (_temp, conn) = create_test_db();

        add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
            RepositoryParserConfig::Json,
            None,
            true,
            10,
        )
        .unwrap();

        // Disable
        set_repository_enabled(&conn, "test-repo", false).unwrap();
        let repo = Repository::find_by_name(&conn, "test-repo")
            .unwrap()
            .unwrap();
        assert!(!repo.enabled);

        // Enable
        set_repository_enabled(&conn, "test-repo", true).unwrap();
        let repo = Repository::find_by_name(&conn, "test-repo")
            .unwrap()
            .unwrap();
        assert!(repo.enabled);
    }

    #[test]
    fn native_repository_cannot_be_persisted_before_exact_enrollment() {
        let (_temp, conn) = create_test_db();

        let error = add_repository(
            &conn,
            "arch-pending-trust".to_string(),
            "https://mirror.example.test/arch".to_string(),
            RepositoryParserConfig::Arch {
                database: "core".to_string(),
            },
            Some("arch".to_string()),
            false,
            10,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has no ecosystem-native trust policy")
        );
        assert!(
            Repository::find_by_name(&conn, "arch-pending-trust")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_needs_sync() {
        let repo_never_synced = Repository::new("test".to_string(), "url".to_string());
        assert!(needs_sync(&repo_never_synced));

        let mut repo_recently_synced = Repository::new("test".to_string(), "url".to_string());
        repo_recently_synced.last_sync = Some(current_timestamp());
        repo_recently_synced.metadata_expire = 3600; // 1 hour
        assert!(!needs_sync(&repo_recently_synced));
    }

    #[test]
    fn test_timestamp_functions() {
        let ts = current_timestamp();
        let parsed = parse_timestamp(&ts).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Should be within a few seconds
        assert!((now as i64 - parsed as i64).abs() < 5);
    }

    #[test]
    fn test_repository_native_trust_policy_round_trips() {
        let (_temp, conn) = create_test_db();

        let mut repo = Repository::new(
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
        );
        repo.set_parser_config(RepositoryParserConfig::Deb {
            distribution: "stable".to_string(),
            component: "main".to_string(),
            architecture: "amd64".to_string(),
        })
        .unwrap();
        repo.source_profile = Some("ubuntu-26.04".to_string());
        repo.set_trust_policy(RepositoryTrustPolicy::Debian {
            release_keys: vec![OpenPgpTrustRoot {
                url: "https://keys.example.test/debian.gpg".to_string(),
                fingerprint: "A".repeat(40),
            }],
        })
        .unwrap();
        repo.set_native_source_policy(
            RepositorySourcePolicy::new(
                "example-debian",
                RepositoryPolicyScope::repository("stable:main:amd64").unwrap(),
                NativeSourceEcosystem::Deb,
                NativeSourceStream::release("stable").unwrap(),
                RepositoryUpdateMode::Follow,
            )
            .unwrap(),
            "stable:main:amd64",
            None,
        )
        .unwrap();
        repo.insert(&conn).unwrap();

        let fetched = Repository::find_by_name(&conn, "test-repo")
            .unwrap()
            .unwrap();
        assert_eq!(fetched.trust_policy, repo.trust_policy);
    }
}
