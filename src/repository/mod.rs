// src/repository/mod.rs

//! Repository management and package downloading
//!
//! This module provides functionality for:
//! - Managing remote package repositories
//! - Synchronizing repository metadata
//! - Downloading packages with retry and resume support
//! - Verifying package checksums
//! - GPG signature verification

pub mod gpg;
pub mod selector;

pub use gpg::GpgVerifier;
pub use selector::{PackageSelector, PackageWithRepo, SelectionOptions};

use crate::db::models::{PackageDelta, Repository, RepositoryPackage};
use crate::error::{Error, Result};
use rayon::prelude::*;
use reqwest::blocking::Client;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Default timeout for HTTP requests (30 seconds)
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retry attempts for failed downloads
const MAX_RETRIES: u32 = 3;

/// Retry delay in milliseconds
const RETRY_DELAY_MS: u64 = 1000;

/// Repository metadata format (simple JSON index)
#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryMetadata {
    pub name: String,
    pub version: String,
    pub packages: Vec<PackageMetadata>,
}

/// Delta update information for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaInfo {
    pub from_version: String,
    pub from_hash: String,
    pub delta_url: String,
    pub delta_size: i64,
    pub delta_checksum: String,
    pub compression_ratio: f64,
}

/// Package metadata in repository index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub description: Option<String>,
    pub checksum: String,
    pub size: i64,
    pub download_url: String,
    pub dependencies: Option<Vec<String>>,
    /// Available delta updates from previous versions
    pub delta_from: Option<Vec<DeltaInfo>>,
}

/// HTTP client wrapper with retry support
pub struct RepositoryClient {
    client: Client,
    max_retries: u32,
}

impl RepositoryClient {
    /// Create a new repository client
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            max_retries: MAX_RETRIES,
        })
    }

    /// Fetch repository metadata from URL with retry support
    pub fn fetch_metadata(&self, url: &str) -> Result<RepositoryMetadata> {
        let metadata_url = if url.ends_with('/') {
            format!("{}metadata.json", url)
        } else {
            format!("{}/metadata.json", url)
        };

        info!("Fetching repository metadata from {}", metadata_url);

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.client.get(&metadata_url).send() {
                Ok(response) => {
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            response.status(),
                            metadata_url
                        )));
                    }

                    let metadata: RepositoryMetadata = response.json().map_err(|e| {
                        Error::DownloadError(format!("Failed to parse metadata JSON: {}", e))
                    })?;

                    info!("Successfully fetched metadata for {} packages", metadata.packages.len());
                    return Ok(metadata);
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(Error::DownloadError(format!(
                            "Failed to fetch metadata after {} attempts: {}",
                            attempt, e
                        )));
                    }
                    warn!("Metadata fetch attempt {} failed: {}, retrying...", attempt, e);
                    std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS * attempt as u64));
                }
            }
        }
    }

    /// Download a file to the specified path with retry support
    pub fn download_file(&self, url: &str, dest_path: &Path) -> Result<()> {
        info!("Downloading {} to {}", url, dest_path.display());

        // Create parent directory if it doesn't exist
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::IoError(format!("Failed to create directory {}: {}", parent.display(), e))
            })?;
        }

        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.client.get(url).send() {
                Ok(mut response) => {
                    if !response.status().is_success() {
                        return Err(Error::DownloadError(format!(
                            "HTTP {} from {}",
                            response.status(),
                            url
                        )));
                    }

                    // Write to temporary file first
                    let temp_path = dest_path.with_extension("tmp");
                    let mut file = File::create(&temp_path).map_err(|e| {
                        Error::IoError(format!("Failed to create file {}: {}", temp_path.display(), e))
                    })?;

                    // Copy response body to file
                    io::copy(&mut response, &mut file).map_err(|e| {
                        Error::IoError(format!("Failed to write downloaded data: {}", e))
                    })?;

                    // Atomic rename from temp to final destination
                    fs::rename(&temp_path, dest_path).map_err(|e| {
                        Error::IoError(format!(
                            "Failed to move {} to {}: {}",
                            temp_path.display(),
                            dest_path.display(),
                            e
                        ))
                    })?;

                    info!("Successfully downloaded to {}", dest_path.display());
                    return Ok(());
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(Error::DownloadError(format!(
                            "Failed to download after {} attempts: {}",
                            attempt, e
                        )));
                    }
                    warn!("Download attempt {} failed: {}, retrying...", attempt, e);
                    std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS * attempt as u64));
                }
            }
        }
    }
}

impl Default for RepositoryClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default repository client")
    }
}

/// Synchronize repository metadata with the database
pub fn sync_repository(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    info!("Synchronizing repository: {}", repo.name);

    let client = RepositoryClient::new()?;
    let metadata = client.fetch_metadata(&repo.url)?;

    // Delete old package entries for this repository
    RepositoryPackage::delete_by_repository(conn, repo.id.unwrap())?;

    // Insert new package metadata
    let mut count = 0;
    let mut delta_count = 0;

    for pkg_meta in metadata.packages {
        let deps_json = pkg_meta.dependencies.as_ref().map(|deps| {
            serde_json::to_string(deps).unwrap_or_default()
        });

        let mut repo_pkg = RepositoryPackage::new(
            repo.id.unwrap(),
            pkg_meta.name.clone(),
            pkg_meta.version.clone(),
            pkg_meta.checksum.clone(),
            pkg_meta.size,
            pkg_meta.download_url,
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
                        .unwrap()
                        .as_secs();

                    let age_seconds = now.saturating_sub(last_sync_time);
                    age_seconds > repo.metadata_expire as u64
                }
                Err(_) => true, // If we can't parse timestamp, force sync
            }
        }
    }
}

/// Download a package from a repository
pub fn download_package(
    repo_pkg: &RepositoryPackage,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path
    let default_filename = format!("{}-{}.rpm", repo_pkg.name, repo_pkg.version);
    let filename = repo_pkg
        .download_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);

    let dest_path = dest_dir.join(filename);

    // Download the file
    client.download_file(&repo_pkg.download_url, &dest_path)?;

    // Verify checksum
    verify_checksum(&dest_path, &repo_pkg.checksum)?;

    Ok(dest_path)
}

/// Download a delta update file
///
/// # Arguments
/// * `delta_info` - Delta metadata from repository
/// * `package_name` - Name of the package (for filename construction)
/// * `to_version` - Target version (for filename construction)
/// * `dest_dir` - Destination directory for the delta file
///
/// # Returns
/// Path to the downloaded and verified delta file
pub fn download_delta(
    delta_info: &DeltaInfo,
    package_name: &str,
    to_version: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let client = RepositoryClient::new()?;

    // Construct destination path
    let default_filename = format!(
        "{}-{}-to-{}.delta",
        package_name, delta_info.from_version, to_version
    );
    let filename = delta_info
        .delta_url
        .split('/')
        .next_back()
        .unwrap_or(&default_filename);

    let dest_path = dest_dir.join(filename);

    info!(
        "Downloading delta for {} ({} -> {})",
        package_name, delta_info.from_version, to_version
    );

    // Download the delta file
    client.download_file(&delta_info.delta_url, &dest_path)?;

    // Verify checksum
    verify_checksum(&dest_path, &delta_info.delta_checksum)?;

    info!(
        "Delta downloaded successfully: {} bytes (compression ratio: {:.1}%)",
        delta_info.delta_size,
        delta_info.compression_ratio * 100.0
    );

    Ok(dest_path)
}

/// Verify file checksum matches expected value
fn verify_checksum(path: &Path, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};

    debug!("Verifying checksum for {}", path.display());

    let mut file = File::open(path).map_err(|e| {
        Error::IoError(format!("Failed to open file for checksum: {}", e))
    })?;

    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| {
        Error::IoError(format!("Failed to read file for checksum: {}", e))
    })?;

    let actual = format!("{:x}", hasher.finalize());

    if actual != expected {
        return Err(Error::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        });
    }

    debug!("Checksum verified: {}", expected);
    Ok(())
}

/// Get current timestamp as ISO 8601 string
fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Parse ISO 8601 timestamp to Unix seconds
fn parse_timestamp(timestamp: &str) -> Result<u64> {
    use chrono::DateTime;

    let dt = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|e| Error::ParseError(format!("Invalid timestamp: {}", e)))?;

    Ok(dt.timestamp() as u64)
}

/// Add a new repository to the database
pub fn add_repository(
    conn: &Connection,
    name: String,
    url: String,
    enabled: bool,
    priority: i32,
) -> Result<Repository> {
    // Check if repository with this name already exists
    if Repository::find_by_name(conn, &name)?.is_some() {
        return Err(Error::ConflictError(format!(
            "Repository '{}' already exists",
            name
        )));
    }

    let mut repo = Repository::new(name, url);
    repo.enabled = enabled;
    repo.priority = priority;

    repo.insert(conn)?;

    info!("Added repository: {} ({})", repo.name, repo.url);
    Ok(repo)
}

/// Remove a repository from the database
pub fn remove_repository(conn: &Connection, name: &str) -> Result<()> {
    let repo = Repository::find_by_name(conn, name)?
        .ok_or_else(|| Error::NotFoundError(format!("Repository '{}' not found", name)))?;

    Repository::delete(conn, repo.id.unwrap())?;
    info!("Removed repository: {}", name);
    Ok(())
}

/// Enable or disable a repository
pub fn set_repository_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<()> {
    let mut repo = Repository::find_by_name(conn, name)?
        .ok_or_else(|| Error::NotFoundError(format!("Repository '{}' not found", name)))?;

    repo.enabled = enabled;
    repo.update(conn)?;

    info!(
        "Repository '{}' {}",
        name,
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Search for packages across all enabled repositories
pub fn search_packages(conn: &Connection, pattern: &str) -> Result<Vec<RepositoryPackage>> {
    let packages = RepositoryPackage::search(conn, pattern)?;
    Ok(packages)
}

/// Resolve dependencies and return list of packages to download
///
/// This function takes a list of dependency names and searches repositories
/// for matching packages. It checks which dependencies are already installed
/// and returns only the ones that need to be downloaded.
///
/// Returns: Vec<(dependency_name, PackageWithRepo)>
pub fn resolve_dependencies(
    conn: &Connection,
    dependencies: &[String],
) -> Result<Vec<(String, PackageWithRepo)>> {
    use crate::db::models::Trove;

    let mut to_download = Vec::new();

    for dep_name in dependencies {
        // Skip rpmlib dependencies and file paths
        if dep_name.starts_with("rpmlib(") || dep_name.starts_with('/') {
            continue;
        }

        // Check if already installed
        let installed = Trove::find_by_name(conn, dep_name)?;
        if !installed.is_empty() {
            debug!("Dependency {} already installed, skipping", dep_name);
            continue;
        }

        // Search repositories for this dependency
        let options = SelectionOptions::default();
        match PackageSelector::find_best_package(conn, dep_name, &options) {
            Ok(pkg_with_repo) => {
                info!(
                    "Found dependency {} version {} in repository {}",
                    dep_name, pkg_with_repo.package.version, pkg_with_repo.repository.name
                );
                to_download.push((dep_name.clone(), pkg_with_repo));
            }
            Err(e) => {
                // Dependency not found - this is a critical error
                return Err(Error::NotFoundError(format!(
                    "Required dependency '{}' not found in any repository: {}",
                    dep_name, e
                )));
            }
        }
    }

    Ok(to_download)
}

/// Resolve dependencies transitively (recursively resolve all dependencies)
///
/// This function performs a breadth-first search through the dependency tree,
/// resolving all transitive dependencies. It tracks visited packages to avoid
/// cycles and respects a maximum depth to prevent infinite loops.
///
/// Returns: Vec<(dependency_name, PackageWithRepo)> in topological order (dependencies before dependents)
pub fn resolve_dependencies_transitive(
    conn: &Connection,
    initial_dependencies: &[String],
    max_depth: usize,
) -> Result<Vec<(String, PackageWithRepo)>> {
    use crate::db::models::Trove;
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut to_download: HashMap<String, PackageWithRepo> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    // Seed queue with initial dependencies
    for dep in initial_dependencies {
        // Skip rpmlib dependencies and file paths
        if dep.starts_with("rpmlib(") || dep.starts_with('/') {
            continue;
        }
        queue.push_back((dep.clone(), 0));
    }

    while let Some((dep_name, depth)) = queue.pop_front() {
        // Check depth limit
        if depth > max_depth {
            warn!(
                "Maximum dependency depth {} reached for package {}",
                max_depth, dep_name
            );
            continue;
        }

        // Skip if already visited
        if visited.contains(&dep_name) {
            continue;
        }
        visited.insert(dep_name.clone());

        // Check if already installed
        let installed = Trove::find_by_name(conn, &dep_name)?;
        if !installed.is_empty() {
            debug!("Dependency {} already installed, skipping", dep_name);
            continue;
        }

        // Check if already in to_download list
        if to_download.contains_key(&dep_name) {
            continue;
        }

        // Search repositories for this dependency
        let options = SelectionOptions::default();
        let pkg_with_repo = PackageSelector::find_best_package(conn, &dep_name, &options)
            .map_err(|e| {
                Error::NotFoundError(format!(
                    "Required dependency '{}' not found in any repository: {}",
                    dep_name, e
                ))
            })?;

        info!(
            "Found dependency {} version {} in repository {} (depth: {})",
            dep_name, pkg_with_repo.package.version, pkg_with_repo.repository.name, depth
        );

        // Parse this package's dependencies and add to queue
        if let Ok(sub_deps) = pkg_with_repo.package.parse_dependencies() {
            for sub_dep in sub_deps {
                if !visited.contains(&sub_dep) {
                    queue.push_back((sub_dep, depth + 1));
                }
            }
        }

        to_download.insert(dep_name, pkg_with_repo);
    }

    // Convert HashMap to Vec and perform topological sort for install order
    let mut result: Vec<(String, PackageWithRepo)> =
        to_download.into_iter().collect();

    // Build dependency graph for topological sorting
    let mut dep_graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    // Initialize in_degree for all packages
    for (name, _) in &result {
        in_degree.insert(name.clone(), 0);
        dep_graph.insert(name.clone(), Vec::new());
    }

    // Build edges: package -> dependencies
    for (name, pkg_with_repo) in &result {
        if let Ok(deps) = pkg_with_repo.package.parse_dependencies() {
            for dep in deps {
                // Only count edges to packages we're actually installing
                if in_degree.contains_key(&dep) {
                    dep_graph.entry(name.clone()).or_default().push(dep.clone());
                    *in_degree.entry(dep).or_default() += 1;
                }
            }
        }
    }

    // Topological sort using Kahn's algorithm
    let mut sorted = Vec::new();
    let mut zero_in_degree: VecDeque<String> = in_degree
        .iter()
        .filter(|&(_, &degree)| degree == 0)
        .map(|(name, _)| name.clone())
        .collect();

    while let Some(node) = zero_in_degree.pop_front() {
        sorted.push(node.clone());

        if let Some(dependents) = dep_graph.get(&node) {
            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        zero_in_degree.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    // If sorted doesn't contain all nodes, there's a cycle
    if sorted.len() != result.len() {
        warn!("Circular dependency detected in transitive resolution, using partial order");
        // Fall back to original order if there's a cycle
    } else {
        // Reorder result based on topological sort (dependencies before dependents)
        let pkg_map: HashMap<String, PackageWithRepo> = result.into_iter().collect();
        result = sorted
            .into_iter()
            .filter_map(|name| pkg_map.get(&name).map(|pkg| (name, pkg.clone())))
            .collect();
    }

    Ok(result)
}

/// Download all dependencies to a directory in parallel
///
/// Downloads are performed concurrently using rayon's parallel iterators.
/// This significantly speeds up the download of multiple dependencies.
///
/// Returns: Vec<(dependency_name, downloaded_path)>
pub fn download_dependencies(
    dependencies: &[(String, PackageWithRepo)],
    dest_dir: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    // Use parallel iterator for concurrent downloads
    let results: Result<Vec<_>> = dependencies
        .par_iter()
        .map(|(dep_name, pkg_with_repo)| {
            info!("Downloading dependency: {}", dep_name);
            let path = download_package(&pkg_with_repo.package, dest_dir)?;
            Ok((dep_name.clone(), path))
        })
        .collect();

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
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
}
