// src/repository/mod.rs

//! Repository management and package downloading
//!
//! This module provides functionality for:
//! - Managing remote package repositories
//! - Synchronizing repository metadata
//! - Downloading packages with retry and resume support
//! - Verifying package checksums
//! - GPG signature verification
//! - Native metadata format parsing (Arch, Debian, Fedora)

mod client;
mod dependencies;
mod download;
mod management;
mod metadata;
pub mod remi;
pub mod registry;
pub mod resolution;
mod sync;

pub mod gpg;
pub mod parsers;
pub mod selector;

#[cfg(feature = "server")]
pub mod chunk_fetcher;

// Re-export main types and functions
pub use client::RepositoryClient;
pub use dependencies::{download_dependencies, resolve_dependencies, resolve_dependencies_transitive};
pub use download::{
    download_delta, download_package, download_package_verified,
    download_package_verified_with_progress, download_package_with_progress, verify_checksum,
    DownloadOptions, DownloadProgress,
};
pub use gpg::GpgVerifier;
pub use management::{add_repository, remove_repository, search_packages, set_repository_enabled};
pub use metadata::{DeltaInfo, PackageMetadata, RepositoryMetadata};
pub use parsers::{ChecksumType, Dependency, DependencyType, RepositoryParser};
pub use registry::{create_parser, detect_repository_format, RepositoryFormat};
pub use selector::{PackageSelector, PackageWithRepo, SelectionOptions};
pub use sync::{
    current_timestamp, maybe_fetch_gpg_key, needs_sync, parse_timestamp, sync_repository,
};
pub use remi::{RemiClient, PackageManifest};
pub use resolution::{
    build_gpg_options, resolve_package, PackageResolver, PackageSource, ResolutionOptions,
};
#[cfg(feature = "server")]
pub use remi::AsyncRemiClient;

#[cfg(feature = "server")]
pub use chunk_fetcher::{
    ChunkData, ChunkFetcher, ChunkFetcherBuilder, CompositeChunkFetcher, HttpChunkFetcher,
    LocalCacheFetcher,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Repository;
    use crate::db::schema;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_add_repository() {
        let (_temp, conn) = create_test_db();

        let repo = add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/repo".to_string(),
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
            true,
            10,
        )
        .unwrap();

        // Try to add duplicate
        let result = add_repository(
            &conn,
            "test-repo".to_string(),
            "https://example.com/other".to_string(),
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
            true,
            10,
        )
        .unwrap();

        // Disable
        set_repository_enabled(&conn, "test-repo", false).unwrap();
        let repo = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
        assert!(!repo.enabled);

        // Enable
        set_repository_enabled(&conn, "test-repo", true).unwrap();
        let repo = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
        assert!(repo.enabled);
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
    fn test_repository_gpg_strict() {
        let (_temp, conn) = create_test_db();

        // Create repository with default settings (gpg_strict = false)
        let mut repo = Repository::new("test-repo".to_string(), "https://example.com/repo".to_string());
        assert!(!repo.gpg_strict); // Default should be false
        repo.insert(&conn).unwrap();

        // Verify it was stored correctly
        let fetched = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
        assert!(!fetched.gpg_strict);

        // Update to enable strict mode
        let mut repo = fetched;
        repo.gpg_strict = true;
        repo.update(&conn).unwrap();

        // Verify the update
        let fetched = Repository::find_by_name(&conn, "test-repo").unwrap().unwrap();
        assert!(fetched.gpg_strict);
    }
}
