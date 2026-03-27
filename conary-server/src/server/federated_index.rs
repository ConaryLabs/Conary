// conary-server/src/server/federated_index.rs
//! Federated sparse index for merging package metadata from multiple Remi instances
//!
//! When multiple Remi instances exist (e.g., regional mirrors), a leaf Remi can
//! merge sparse index entries from upstream instances to present a unified view
//! of all available packages and versions.
//!
//! Features:
//! - Parallel fetching from upstream peers
//! - TTL-based in-memory cache to avoid repeated upstream queries
//! - Version deduplication with preference for converted packages
//! - Graceful degradation when upstream peers are unavailable

use crate::server::handlers::sparse::{SparseIndexEntry, SparseVersionEntry};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Configuration for federated sparse index
#[derive(Debug, Clone)]
pub struct FederatedIndexConfig {
    /// URLs of upstream Remi instances to query
    pub upstream_urls: Vec<String>,
    /// Timeout for individual upstream requests
    pub timeout: Duration,
    /// How long to cache merged results before re-fetching
    pub cache_ttl: Duration,
}

/// In-memory cache for federated sparse index entries.
///
/// Uses `RwLock` for concurrent access from multiple handler tasks.
/// Each entry is keyed by `(distro, package_name)` and has a timestamp for TTL.
pub struct FederatedIndexCache {
    entries: RwLock<HashMap<(String, String), CacheEntry>>,
}

/// A cached sparse index entry with its insertion time
struct CacheEntry {
    entry: SparseIndexEntry,
    inserted_at: Instant,
}

impl FederatedIndexCache {
    /// Create an empty cache
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Get a cached entry if it exists and has not expired
    pub async fn get(&self, distro: &str, name: &str, ttl: Duration) -> Option<SparseIndexEntry> {
        let entries = self.entries.read().await;
        let key = (distro.to_string(), name.to_string());

        entries.get(&key).and_then(|cached| {
            if cached.inserted_at.elapsed() < ttl {
                Some(cached.entry.clone())
            } else {
                None
            }
        })
    }

    /// Store an entry in the cache
    pub async fn put(&self, distro: &str, name: &str, entry: SparseIndexEntry) {
        let mut entries = self.entries.write().await;
        let key = (distro.to_string(), name.to_string());
        entries.insert(
            key,
            CacheEntry {
                entry,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Remove expired entries from the cache
    pub async fn cleanup(&self, ttl: Duration) -> usize {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|_, v| v.inserted_at.elapsed() < ttl);
        before - entries.len()
    }

    /// Number of entries currently in cache
    #[allow(clippy::len_without_is_empty)] // is_empty is async, clippy can't detect it
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Check if the cache is empty
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }
}

impl Default for FederatedIndexCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch a sparse index entry from a remote Remi instance.
///
/// Makes an HTTP GET to `{url}/v1/index/{distro}/{name}` and deserializes
/// the JSON response into a `SparseIndexEntry`.
pub async fn fetch_remote_sparse_entry(
    client: &reqwest::Client,
    url: &str,
    distro: &str,
    name: &str,
) -> Result<Option<SparseIndexEntry>> {
    let fetch_url = format!("{}/v1/index/{}/{}", url.trim_end_matches('/'), distro, name);

    debug!("Fetching remote sparse entry: {}", fetch_url);

    let response = client
        .get(&fetch_url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch sparse entry from {}", fetch_url))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !response.status().is_success() {
        warn!(
            "Upstream {} returned status {} for {}/{}",
            url,
            response.status(),
            distro,
            name
        );
        return Ok(None);
    }

    let entry: SparseIndexEntry = response
        .json()
        .await
        .with_context(|| format!("Failed to parse sparse entry from {}", fetch_url))?;

    Ok(Some(entry))
}

/// Merge multiple sparse index entries into a single unified entry.
///
/// Deduplicates versions by version string. When the same version appears
/// in multiple sources, prefers `converted=true` over `converted=false`.
pub fn merge_sparse_entries(entries: Vec<SparseIndexEntry>) -> SparseIndexEntry {
    if entries.is_empty() {
        return SparseIndexEntry {
            name: String::new(),
            distro: String::new(),
            versions: Vec::new(),
        };
    }

    let name = entries[0].name.clone();
    let distro = entries[0].distro.clone();

    // Merge versions, keyed by version string
    let mut version_map: HashMap<String, SparseVersionEntry> = HashMap::new();

    for entry in entries {
        for version in entry.versions {
            let key = version.version.clone();
            match version_map.entry(key) {
                Entry::Occupied(mut existing) => {
                    if version.converted && !existing.get().converted {
                        existing.insert(version);
                    }
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(version);
                }
            }
        }
    }

    // Sort versions by version string
    let mut versions: Vec<SparseVersionEntry> = version_map.into_values().collect();
    versions.sort_by(|a, b| a.version.cmp(&b.version));

    SparseIndexEntry {
        name,
        distro,
        versions,
    }
}

/// Build a federated sparse index entry by combining local data with upstream sources.
///
/// 1. Builds the local entry from the database
/// 2. Fetches entries from all upstream peers in parallel
/// 3. Merges everything together
/// 4. Caches the result for the configured TTL
pub async fn build_federated_sparse_entry(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    fed_config: &FederatedIndexConfig,
    cache: &Arc<FederatedIndexCache>,
    client: &reqwest::Client,
) -> Result<Option<SparseIndexEntry>> {
    // Check cache first
    if let Some(cached) = cache.get(distro, name, fed_config.cache_ttl).await {
        debug!("Federated cache hit for {}/{}", distro, name);
        return Ok(Some(cached));
    }

    // Build local entry
    let db_path_owned = db_path.to_path_buf();
    let distro_owned = distro.to_string();
    let name_owned = name.to_string();

    let local_entry = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open(&db_path_owned)?;
        build_local_sparse_entry(&conn, &distro_owned, &name_owned)
    })
    .await??;

    // Fetch from all upstream peers in parallel
    let mut fetch_futures = Vec::new();
    for url in &fed_config.upstream_urls {
        let client = client.clone();
        let url = url.clone();
        let distro = distro.to_string();
        let name = name.to_string();
        let timeout = fed_config.timeout;

        fetch_futures.push(tokio::spawn(async move {
            match tokio::time::timeout(
                timeout,
                fetch_remote_sparse_entry(&client, &url, &distro, &name),
            )
            .await
            {
                Ok(Ok(entry)) => entry,
                Ok(Err(e)) => {
                    warn!("Failed to fetch from upstream {}: {}", url, e);
                    None
                }
                Err(_) => {
                    warn!("Timeout fetching from upstream {}", url);
                    None
                }
            }
        }));
    }

    // Collect all results
    let mut all_entries = Vec::new();
    if let Some(local) = local_entry {
        all_entries.push(local);
    }

    for future in fetch_futures {
        match future.await {
            Ok(Some(entry)) => all_entries.push(entry),
            Ok(None) => {} // Upstream had no data for this package
            Err(e) => {
                warn!("Upstream fetch task panicked: {}", e);
            }
        }
    }

    if all_entries.is_empty() {
        return Ok(None);
    }

    // Merge all entries
    let merged = merge_sparse_entries(all_entries);

    // Cache the result
    cache.put(distro, name, merged.clone()).await;

    Ok(Some(merged))
}

/// Build a local sparse index entry from the database.
///
/// Mirrors the logic of `handlers::sparse::build_sparse_entry` but takes a
/// `Connection` reference directly for use within `spawn_blocking`.
fn build_local_sparse_entry(
    conn: &rusqlite::Connection,
    distro: &str,
    name: &str,
) -> Result<Option<SparseIndexEntry>> {
    use crate::server::handlers::find_repositories_for_distro;

    // Use plural lookup so multi-repo distros (e.g. arch-core + arch-extra)
    // are all queried, matching the non-federated sparse path. (fix 10.7)
    let repositories = find_repositories_for_distro(conn, distro)?;
    let repo_ids: Vec<i64> = repositories.into_iter().filter_map(|r| r.id).collect();
    if repo_ids.is_empty() {
        return Ok(None);
    }

    let placeholders: String = (1..=repo_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let name_idx = repo_ids.len() + 1;
    let sql = format!(
        "SELECT id, repository_id, name, version, architecture, description,
                checksum, size, download_url, dependencies, metadata, synced_at,
                is_security_update, severity, cve_ids, advisory_id, advisory_url
         FROM repository_packages
         WHERE repository_id IN ({placeholders}) AND name = ?{name_idx}
         ORDER BY version"
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = repo_ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    params.push(Box::new(name.to_string()));

    let mut stmt = conn.prepare(&sql)?;

    let packages: Vec<conary_core::db::models::RepositoryPackage> = stmt
        .query_map(rusqlite::params_from_iter(&params), |row| {
            Ok(conary_core::db::models::RepositoryPackage {
                id: Some(row.get(0)?),
                repository_id: row.get(1)?,
                name: row.get(2)?,
                version: row.get(3)?,
                architecture: row.get(4)?,
                description: row.get(5)?,
                checksum: row.get(6)?,
                size: row.get(7)?,
                download_url: row.get(8)?,
                dependencies: row.get(9)?,
                metadata: row.get(10)?,
                synced_at: row.get(11)?,
                is_security_update: row.get::<_, i32>(12)? != 0,
                severity: row.get(13)?,
                cve_ids: row.get(14)?,
                advisory_id: row.get(15)?,
                advisory_url: row.get(16)?,
                distro: None,
                version_scheme: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if packages.is_empty() {
        return Ok(None);
    }

    // Build converted lookup
    let mut converted_stmt = conn.prepare(
        "SELECT package_version, content_hash FROM converted_packages
         WHERE distro = ?1 AND package_name = ?2
         AND package_version IS NOT NULL",
    )?;

    let mut converted_map = HashMap::new();
    let mut rows = converted_stmt.query(rusqlite::params![distro, name])?;
    while let Some(row) = rows.next()? {
        let version: String = row.get(0)?;
        let content_hash: Option<String> = row.get(1)?;
        converted_map.insert(version, content_hash);
    }

    let versions = packages
        .into_iter()
        .map(|pkg| {
            let converted_info = converted_map.get(&pkg.version);
            SparseVersionEntry {
                version: pkg.version,
                dependencies: pkg.dependencies,
                provides: pkg.metadata,
                architecture: pkg.architecture,
                size: pkg.size,
                converted: converted_info.is_some(),
                content_hash: converted_info.and_then(Option::clone),
            }
        })
        .collect();

    Ok(Some(SparseIndexEntry {
        name: name.to_string(),
        distro: distro.to_string(),
        versions,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(ver: &str, converted: bool) -> SparseVersionEntry {
        SparseVersionEntry {
            version: ver.to_string(),
            dependencies: None,
            provides: None,
            architecture: Some("x86_64".to_string()),
            size: 1024,
            converted,
            content_hash: if converted {
                Some(format!("sha256:{ver}"))
            } else {
                None
            },
        }
    }

    fn make_entry(name: &str, distro: &str, versions: Vec<SparseVersionEntry>) -> SparseIndexEntry {
        SparseIndexEntry {
            name: name.to_string(),
            distro: distro.to_string(),
            versions,
        }
    }

    #[test]
    fn test_merge_empty() {
        let merged = merge_sparse_entries(vec![]);
        assert!(merged.name.is_empty());
        assert!(merged.versions.is_empty());
    }

    #[test]
    fn test_merge_single_entry() {
        let entry = make_entry(
            "nginx",
            "fedora",
            vec![make_version("1.0", true), make_version("2.0", false)],
        );

        let merged = merge_sparse_entries(vec![entry]);
        assert_eq!(merged.name, "nginx");
        assert_eq!(merged.distro, "fedora");
        assert_eq!(merged.versions.len(), 2);
    }

    #[test]
    fn test_merge_no_overlap() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

        let merged = merge_sparse_entries(vec![entry1, entry2]);
        assert_eq!(merged.versions.len(), 2);
        assert_eq!(merged.versions[0].version, "1.0");
        assert_eq!(merged.versions[1].version, "2.0");
    }

    #[test]
    fn test_merge_overlapping_prefer_converted() {
        // Source 1: v1.0 converted, v2.0 not converted
        let entry1 = make_entry(
            "nginx",
            "fedora",
            vec![make_version("1.0", true), make_version("2.0", false)],
        );

        // Source 2: v2.0 converted, v3.0 not converted
        let entry2 = make_entry(
            "nginx",
            "fedora",
            vec![make_version("2.0", true), make_version("3.0", false)],
        );

        let merged = merge_sparse_entries(vec![entry1, entry2]);
        assert_eq!(merged.versions.len(), 3);

        let v2 = merged.versions.iter().find(|v| v.version == "2.0").unwrap();
        assert!(v2.converted, "Should prefer converted=true for v2.0");
        assert!(v2.content_hash.is_some());
    }

    #[test]
    fn test_merge_keeps_first_when_both_converted() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        let merged = merge_sparse_entries(vec![entry1, entry2]);
        assert_eq!(merged.versions.len(), 1);
        assert!(merged.versions[0].converted);
    }

    #[test]
    fn test_merge_keeps_first_when_both_unconverted() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);

        let merged = merge_sparse_entries(vec![entry1, entry2]);
        assert_eq!(merged.versions.len(), 1);
        assert!(!merged.versions[0].converted);
    }

    #[test]
    fn test_merge_sorted_output() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("3.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry3 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

        let merged = merge_sparse_entries(vec![entry1, entry2, entry3]);
        assert_eq!(merged.versions.len(), 3);
        assert_eq!(merged.versions[0].version, "1.0");
        assert_eq!(merged.versions[1].version, "2.0");
        assert_eq!(merged.versions[2].version, "3.0");
    }

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let cache = FederatedIndexCache::new();
        let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        cache.put("fedora", "nginx", entry.clone()).await;

        let cached = cache.get("fedora", "nginx", Duration::from_secs(60)).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().versions.len(), 1);
    }

    #[tokio::test]
    async fn test_cache_miss_no_entry() {
        let cache = FederatedIndexCache::new();

        let cached = cache.get("fedora", "nginx", Duration::from_secs(60)).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_ttl_expiry() {
        let cache = FederatedIndexCache::new();
        let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        cache.put("fedora", "nginx", entry).await;

        // With a zero TTL, the entry should be considered expired immediately
        let cached = cache.get("fedora", "nginx", Duration::from_secs(0)).await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_cleanup() {
        let cache = FederatedIndexCache::new();

        cache
            .put(
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
            )
            .await;
        cache
            .put(
                "fedora",
                "curl",
                make_entry("curl", "fedora", vec![make_version("8.0", true)]),
            )
            .await;

        assert_eq!(cache.len().await, 2);

        // Cleanup with zero TTL should remove everything
        let removed = cache.cleanup(Duration::from_secs(0)).await;
        assert_eq!(removed, 2);
        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn test_cache_cleanup_preserves_fresh() {
        let cache = FederatedIndexCache::new();

        cache
            .put(
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
            )
            .await;

        // Cleanup with long TTL should preserve entry
        let removed = cache.cleanup(Duration::from_secs(3600)).await;
        assert_eq!(removed, 0);
        assert_eq!(cache.len().await, 1);
    }

    #[tokio::test]
    async fn test_cache_different_keys() {
        let cache = FederatedIndexCache::new();

        cache
            .put(
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
            )
            .await;
        cache
            .put(
                "arch",
                "nginx",
                make_entry("nginx", "arch", vec![make_version("2.0", true)]),
            )
            .await;

        let fedora = cache
            .get("fedora", "nginx", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(fedora.distro, "fedora");

        let arch = cache
            .get("arch", "nginx", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(arch.distro, "arch");
    }
}
