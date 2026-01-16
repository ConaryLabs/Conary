// src/repository/sync.rs

//! Repository synchronization
//!
//! Functions for synchronizing repository metadata from remote sources,
//! including native format support for Arch, Debian, and Fedora repositories.

use crate::db::models::{PackageDelta, Repository, RepositoryPackage};
use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use super::client::RepositoryClient;
use super::gpg::GpgVerifier;
use super::parsers;
use super::parsers::RepositoryParser;

/// Detected repository format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryFormat {
    Arch,
    Debian,
    Fedora,
    Json,
}

/// Detect repository format based on repository name and URL
pub fn detect_repository_format(name: &str, url: &str) -> RepositoryFormat {
    let name_lower = name.to_lowercase();
    let url_lower = url.to_lowercase();

    // Check for Arch Linux indicators
    if name_lower.contains("arch")
        || url_lower.contains("archlinux")
        || url_lower.contains("pkgbuild")
        || url_lower.contains(".db.tar")
    {
        return RepositoryFormat::Arch;
    }

    // Check for Fedora indicators
    if name_lower.contains("fedora")
        || url_lower.contains("fedora")
        || url_lower.contains("/repodata/")
    {
        return RepositoryFormat::Fedora;
    }

    // Check for Debian/Ubuntu indicators
    if name_lower.contains("debian")
        || name_lower.contains("ubuntu")
        || url_lower.contains("debian")
        || url_lower.contains("ubuntu")
        || url_lower.contains("/dists/")
    {
        return RepositoryFormat::Debian;
    }

    // Default to JSON format
    RepositoryFormat::Json
}

/// Get current timestamp as ISO 8601 string
pub fn current_timestamp() -> String {
    use chrono::Utc;
    Utc::now().to_rfc3339()
}

/// Parse ISO 8601 timestamp to Unix seconds
pub fn parse_timestamp(timestamp: &str) -> Result<u64> {
    use chrono::DateTime;

    let dt = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|e| Error::ParseError(format!("Invalid timestamp: {e}")))?;

    Ok(dt.timestamp() as u64)
}

/// Rebase a download URL from metadata source to content source
///
/// If the repository has a content_url configured, this function rebases
/// the download URL from the metadata URL to the content URL.
///
/// For example:
/// - metadata_url: "https://your-server.com/fedora/39/metadata"
/// - content_url: "https://mirror.local/fedora"
/// - download_url: "https://your-server.com/fedora/39/metadata/Packages/foo.rpm"
/// - Result: "https://mirror.local/fedora/Packages/foo.rpm"
///
/// Security note: The checksum from trusted metadata is verified after download,
/// so even if content_url points to an untrusted source, the downloaded content
/// is validated against the hash from the signed metadata.
fn rebase_download_url(download_url: &str, metadata_url: &str, content_url: Option<&str>) -> String {
    match content_url {
        Some(content_base) => {
            // Normalize URLs by removing trailing slashes for consistent matching
            let metadata_base = metadata_url.trim_end_matches('/');
            let content_base = content_base.trim_end_matches('/');

            if download_url.starts_with(metadata_base) {
                // Extract relative path after the metadata base
                let relative = &download_url[metadata_base.len()..];

                // Ensure proper path joining - relative should start with /
                // This handles cases like:
                //   metadata_base: "http://foo.com/repo"
                //   relative: "/Packages/foo.rpm" or "Packages/foo.rpm"
                let relative = relative.trim_start_matches('/');
                format!("{}/{}", content_base, relative)
            } else {
                // URL doesn't match metadata base - return as-is
                // This handles absolute URLs in metadata that point elsewhere
                download_url.to_string()
            }
        }
        None => download_url.to_string(),
    }
}

/// Synchronize repository using native metadata format parsers
fn sync_repository_native(
    conn: &Connection,
    repo: &mut Repository,
    format: RepositoryFormat,
) -> Result<usize> {
    info!("Syncing repository {} using native {:?} format", repo.name, format);

    // Parse metadata using appropriate parser
    // Metadata is always fetched from repo.url
    let packages = match format {
        RepositoryFormat::Arch => {
            // Extract repository name from repo.name (e.g., "arch-core" -> "core")
            let repo_name = if let Some(suffix) = repo.name.strip_prefix("arch-") {
                suffix.to_string()
            } else {
                "core".to_string()
            };

            let parser = parsers::arch::ArchParser::new(repo_name);
            parser.sync_metadata(&repo.url)?
        }
        RepositoryFormat::Debian => {
            // For Ubuntu/Debian, we need distribution, component, and architecture
            // Extract from repository name: "ubuntu-noble" -> noble
            let distribution = if let Some(suffix) = repo.name.strip_prefix("ubuntu-") {
                suffix.to_string()
            } else if let Some(suffix) = repo.name.strip_prefix("debian-") {
                suffix.to_string()
            } else {
                "noble".to_string()
            };

            let parser = parsers::debian::DebianParser::new(
                distribution,
                "main".to_string(),
                "amd64".to_string(),
            );
            parser.sync_metadata(&repo.url)?
        }
        RepositoryFormat::Fedora => {
            let parser = parsers::fedora::FedoraParser::new("x86_64".to_string());
            parser.sync_metadata(&repo.url)?
        }
        RepositoryFormat::Json => {
            return Err(Error::ParseError(
                "JSON format should use sync_repository".to_string(),
            ));
        }
    };

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    // Delete old package entries for this repository
    RepositoryPackage::delete_by_repository(conn, repo_id)?;

    // Check if we need to rebase download URLs (reference mirror pattern)
    let needs_rebase = repo.content_url.is_some();
    if needs_rebase {
        info!(
            "Repository {} uses reference mirror - rebasing download URLs to {}",
            repo.name,
            repo.content_url.as_deref().unwrap_or("")
        );
    }

    // Convert and insert package metadata
    let mut count = 0;
    for pkg_meta in packages {
        // Convert parsers::Dependency to Vec<String>
        let deps_json = if !pkg_meta.dependencies.is_empty() {
            let dep_strings: Vec<String> = pkg_meta
                .dependencies
                .iter()
                .map(|dep| {
                    if let Some(constraint) = &dep.constraint {
                        format!("{} {constraint}", dep.name)
                    } else {
                        dep.name.clone()
                    }
                })
                .collect();
            Some(serde_json::to_string(&dep_strings).unwrap_or_default())
        } else {
            None
        };

        // Rebase download URL if content_url is configured (reference mirror)
        let download_url = rebase_download_url(
            &pkg_meta.download_url,
            &repo.url,
            repo.content_url.as_deref(),
        );

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            pkg_meta.name,
            pkg_meta.version,
            pkg_meta.checksum,
            pkg_meta.size as i64,
            download_url,
        );

        repo_pkg.architecture = pkg_meta.architecture;
        repo_pkg.description = pkg_meta.description;
        repo_pkg.dependencies = deps_json;

        repo_pkg.insert(conn)?;
        count += 1;
    }

    // Update last_sync timestamp
    repo.last_sync = Some(current_timestamp());
    repo.update(conn)?;

    info!(
        "Synchronized {} packages from repository {}",
        count, repo.name
    );
    Ok(count)
}

/// Synchronize repository metadata with the database
pub fn sync_repository(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    info!("Synchronizing repository: {}", repo.name);

    // Detect repository format
    let format = detect_repository_format(&repo.name, &repo.url);

    // Try native format first if detected
    if format != RepositoryFormat::Json {
        match sync_repository_native(conn, repo, format) {
            Ok(count) => return Ok(count),
            Err(e) => {
                warn!("Native format sync failed: {}, falling back to JSON", e);
            }
        }
    }

    // Fall back to JSON metadata format
    let client = RepositoryClient::new()?;
    let metadata = client.fetch_metadata(&repo.url)?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    // Delete old package entries for this repository
    RepositoryPackage::delete_by_repository(conn, repo_id)?;

    // Insert new package metadata
    let mut count = 0;
    let mut delta_count = 0;

    for pkg_meta in metadata.packages {
        let deps_json = pkg_meta
            .dependencies
            .as_ref()
            .map(|deps| serde_json::to_string(deps).unwrap_or_default());

        // Rebase download URL if content_url is configured (reference mirror)
        let download_url = rebase_download_url(
            &pkg_meta.download_url,
            &repo.url,
            repo.content_url.as_deref(),
        );

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            pkg_meta.name.clone(),
            pkg_meta.version.clone(),
            pkg_meta.checksum.clone(),
            pkg_meta.size,
            download_url,
        );

        repo_pkg.architecture = pkg_meta.architecture;
        repo_pkg.description = pkg_meta.description;
        repo_pkg.dependencies = deps_json;

        repo_pkg.insert(conn)?;
        count += 1;

        // Store delta metadata if available
        if let Some(deltas) = pkg_meta.delta_from {
            for delta_info in deltas {
                let mut delta = PackageDelta::new(
                    pkg_meta.name.clone(),
                    delta_info.from_version,
                    pkg_meta.version.clone(),
                    delta_info.from_hash,
                    pkg_meta.checksum.clone(),
                    delta_info.delta_url,
                    delta_info.delta_size,
                    delta_info.delta_checksum,
                    pkg_meta.size,
                );

                delta.insert(conn)?;
                delta_count += 1;
            }
        }
    }

    // Update last_sync timestamp
    repo.last_sync = Some(current_timestamp());
    repo.update(conn)?;

    info!(
        "Synchronized {} packages and {} deltas from repository {}",
        count, delta_count, repo.name
    );
    Ok(count)
}

/// Check if repository metadata needs refresh
pub fn needs_sync(repo: &Repository) -> bool {
    match &repo.last_sync {
        None => true, // Never synced
        Some(last_sync) => {
            // Parse timestamp and check if expired
            match parse_timestamp(last_sync) {
                Ok(last_sync_time) => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let age_seconds = now.saturating_sub(last_sync_time);
                    age_seconds > repo.metadata_expire as u64
                }
                Err(_) => true, // If we can't parse timestamp, force sync
            }
        }
    }
}

/// Attempt to fetch and import GPG key if configured for the repository
///
/// This function should be called before sync when gpg_check is enabled.
/// It will:
/// 1. Check if gpg_key_url is configured
/// 2. Check if key already exists (skip if so)
/// 3. Download and import the key
///
/// # Arguments
/// * `repo` - Repository with gpg_key_url to fetch from
/// * `keyring_dir` - Directory to store GPG keys
///
/// # Returns
/// * `Ok(Some(fingerprint))` - Key was fetched and imported
/// * `Ok(None)` - No key URL configured, or key already exists
/// * `Err(_)` - Failed to fetch or import key
pub fn maybe_fetch_gpg_key(repo: &Repository, keyring_dir: &Path) -> Result<Option<String>> {
    // Skip if no key URL configured
    let key_url = match &repo.gpg_key_url {
        Some(url) => url,
        None => {
            debug!("No gpg_key_url configured for repository '{}'", repo.name);
            return Ok(None);
        }
    };

    // Create verifier
    let verifier = GpgVerifier::new(keyring_dir.to_path_buf())?;

    // Skip if key already exists
    if verifier.has_key(&repo.name) {
        debug!(
            "GPG key already exists for repository '{}', skipping fetch",
            repo.name
        );
        return Ok(None);
    }

    info!(
        "Fetching GPG key for repository '{}' from {}",
        repo.name, key_url
    );

    // Download the key
    let client = RepositoryClient::new()?;
    let key_data = client.download_to_bytes(key_url).map_err(|e| {
        Error::DownloadError(format!(
            "Failed to fetch GPG key for '{}': {}",
            repo.name, e
        ))
    })?;

    // Import the key
    let fingerprint = verifier.import_key(&key_data, &repo.name)?;

    info!(
        "Imported GPG key for repository '{}' (fingerprint: {})",
        repo.name, fingerprint
    );

    Ok(Some(fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rebase_download_url_no_content_url() {
        // When no content_url, the download_url should be unchanged
        let download_url = "https://example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://example.com/fedora";

        let result = rebase_download_url(download_url, metadata_url, None);
        assert_eq!(result, download_url);
    }

    #[test]
    fn test_rebase_download_url_with_content_url() {
        // Rebase from metadata URL to content URL
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_trailing_slashes() {
        // Handle trailing slashes correctly - no double slashes in output
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora/";
        let content_url = "https://mirror.local/fedora/";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
        // Verify no double slashes
        assert!(!result.contains("//P"), "Should not have double slashes before path");
    }

    #[test]
    fn test_rebase_download_url_no_leading_slash() {
        // Handle case where relative path has no leading slash
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/content";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/content/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_ubuntu_example() {
        // Real-world example: Ubuntu with local metadata but archive.ubuntu.com content
        let download_url = "https://your-server.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb";
        let metadata_url = "https://your-server.com/ubuntu";
        let content_url = "https://archive.ubuntu.com/ubuntu";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://archive.ubuntu.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb");
    }

    #[test]
    fn test_rebase_download_url_different_base() {
        // When download_url doesn't match metadata_url prefix, return as-is
        let download_url = "https://other-server.com/packages/foo.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        // Can't rebase - different base, return as-is
        assert_eq!(result, "https://other-server.com/packages/foo.rpm");
    }
}
