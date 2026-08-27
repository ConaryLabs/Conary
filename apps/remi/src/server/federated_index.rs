// apps/remi/src/server/federated_index.rs
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

use crate::server::catalog_authority::{CatalogAuthority, ProfileRevisionSelection};
use crate::server::handlers::sparse::{
    SparseIndexEntry, SparseVersionEntry, build_sparse_entry_with_revision,
};
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
/// Each entry is keyed by the exact active profile revision plus route and
/// package name, so activation cannot leave an old merged result cache-valid.
pub struct FederatedIndexCache {
    entries: RwLock<HashMap<(String, String, String), CacheEntry>>,
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
    pub async fn get(
        &self,
        profile_revision_sha256: &str,
        distro: &str,
        name: &str,
        ttl: Duration,
    ) -> Option<SparseIndexEntry> {
        let entries = self.entries.read().await;
        let key = (
            profile_revision_sha256.to_string(),
            distro.to_string(),
            name.to_string(),
        );

        entries.get(&key).and_then(|cached| {
            if cached.inserted_at.elapsed() < ttl {
                Some(cached.entry.clone())
            } else {
                None
            }
        })
    }

    /// Store an entry in the cache
    pub async fn put(
        &self,
        profile_revision_sha256: &str,
        distro: &str,
        name: &str,
        entry: SparseIndexEntry,
    ) {
        let mut entries = self.entries.write().await;
        let key = (
            profile_revision_sha256.to_string(),
            distro.to_string(),
            name.to_string(),
        );
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
    expected_profile_revision_sha256: &str,
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

    let remote_revision = response
        .headers()
        .get(crate::server::handlers::public_read::PROFILE_REVISION_HEADER)
        .and_then(|value| value.to_str().ok());
    if remote_revision != Some(expected_profile_revision_sha256) {
        warn!(
            "Upstream {url} did not prove profile revision {expected_profile_revision_sha256} for {distro}/{name}"
        );
        return Ok(None);
    }

    let entry: SparseIndexEntry = response
        .json()
        .await
        .with_context(|| format!("Failed to parse sparse entry from {}", fetch_url))?;
    if entry.name != name || entry.distro != distro {
        anyhow::bail!(
            "upstream {url} returned sparse identity {}/{} for requested {distro}/{name}",
            entry.distro,
            entry.name
        );
    }

    Ok(Some(entry))
}

/// Merge multiple sparse index entries into a single unified entry.
///
/// Deduplicates versions by the complete public package identity. Conflicting
/// resolution metadata or serving state for one identity fails closed; a
/// converted result may replace an otherwise identical unconverted result.
pub fn merge_sparse_entries(
    distro: &str,
    name: &str,
    entries: Vec<SparseIndexEntry>,
) -> Result<SparseIndexEntry> {
    let mut version_map: HashMap<(String, Option<String>, Option<String>), SparseVersionEntry> =
        HashMap::new();

    for entry in entries {
        if entry.name != name || entry.distro != distro {
            anyhow::bail!(
                "cannot merge sparse identity {}/{} into requested {distro}/{name}",
                entry.distro,
                entry.name
            );
        }
        for version in entry.versions {
            if version.converted && version.content_hash.is_none() {
                anyhow::bail!(
                    "converted sparse package {name} {} has no content hash",
                    version.version
                );
            }
            let key = (
                version.version.clone(),
                version.release.clone(),
                version.architecture.clone(),
            );
            match version_map.entry(key) {
                Entry::Occupied(mut existing) => {
                    let current = existing.get();
                    if current.provides != version.provides
                        || current.requirement_groups != version.requirement_groups
                        || current.size != version.size
                    {
                        anyhow::bail!(
                            "federated sparse sources disagree on resolution metadata for {name} {}-{}.{}",
                            version.version,
                            version.release.as_deref().unwrap_or(""),
                            version.architecture.as_deref().unwrap_or("noarch")
                        );
                    }
                    if version.converted && !existing.get().converted {
                        existing.insert(version);
                    } else if version.converted == existing.get().converted
                        && version.content_hash != existing.get().content_hash
                    {
                        anyhow::bail!(
                            "federated sparse sources disagree on content identity for {name} {}-{}.{}",
                            version.version,
                            version.release.as_deref().unwrap_or(""),
                            version.architecture.as_deref().unwrap_or("noarch")
                        );
                    }
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(version);
                }
            }
        }
    }

    let mut versions: Vec<SparseVersionEntry> = version_map.into_values().collect();
    versions.sort_by(|left, right| {
        (&left.version, &left.release, &left.architecture).cmp(&(
            &right.version,
            &right.release,
            &right.architecture,
        ))
    });

    Ok(SparseIndexEntry {
        name: name.to_string(),
        distro: distro.to_string(),
        versions,
    })
}

/// Build a federated sparse index entry by combining local data with upstream sources.
///
/// 1. Builds the local entry from one universe-selected profile catalog
/// 2. Fetches entries from peers that prove the same profile revision
/// 3. Merges everything together
/// 4. Caches the result for the configured TTL
pub async fn build_federated_sparse_entry(
    catalog_authority: CatalogAuthority,
    db_path: &std::path::Path,
    name: &str,
    selection: ProfileRevisionSelection,
    fed_config: &FederatedIndexConfig,
    cache: &Arc<FederatedIndexCache>,
    client: &reqwest::Client,
) -> Result<Option<SparseIndexEntry>> {
    let distro = conary_core::repository::supported_profiles::profile_by_public_id(
        &selection.source_profile,
    )
    .with_context(|| {
        format!(
            "federated sparse selection names unsupported profile '{}'",
            selection.source_profile
        )
    })?
    .remi_route_slug()
    .to_string();
    let db_path_owned = db_path.to_path_buf();
    let distro_owned = distro.clone();
    let name_owned = name.to_string();
    let local_selection = selection.clone();

    let (profile_revision_sha256, local_entry) = tokio::task::spawn_blocking(move || {
        build_sparse_entry_with_revision(
            &catalog_authority,
            &db_path_owned,
            &distro_owned,
            &name_owned,
            &local_selection,
        )
    })
    .await??;

    if let Some(cached) = cache
        .get(
            &profile_revision_sha256,
            &distro,
            name,
            fed_config.cache_ttl,
        )
        .await
    {
        debug!("Federated cache hit for {}/{}", distro, name);
        return Ok(Some(cached));
    }

    // Fetch from all upstream peers in parallel
    let mut fetch_futures = Vec::new();
    for url in &fed_config.upstream_urls {
        let client = client.clone();
        let url = url.clone();
        let distro = distro.clone();
        let name = name.to_string();
        let timeout = fed_config.timeout;
        let expected_profile_revision_sha256 = selection.profile_revision_sha256.clone();

        fetch_futures.push(tokio::spawn(async move {
            match tokio::time::timeout(
                timeout,
                fetch_remote_sparse_entry(
                    &client,
                    &url,
                    &distro,
                    &name,
                    &expected_profile_revision_sha256,
                ),
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
    let merged = merge_sparse_entries(&distro, name, all_entries)?;

    // Cache the result
    cache
        .put(&profile_revision_sha256, &distro, name, merged.clone())
        .await;

    Ok(Some(merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVISION_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REVISION_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn make_version(ver: &str, converted: bool) -> SparseVersionEntry {
        SparseVersionEntry {
            version: ver.to_string(),
            release: None,
            provides: Vec::new(),
            requirement_groups: Vec::new(),
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
        let merged = merge_sparse_entries("fedora", "nginx", vec![]).unwrap();
        assert_eq!(merged.name, "nginx");
        assert_eq!(merged.distro, "fedora");
        assert!(merged.versions.is_empty());
    }

    #[test]
    fn test_merge_single_entry() {
        let entry = make_entry(
            "nginx",
            "fedora",
            vec![make_version("1.0", true), make_version("2.0", false)],
        );

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry]).unwrap();
        assert_eq!(merged.name, "nginx");
        assert_eq!(merged.distro, "fedora");
        assert_eq!(merged.versions.len(), 2);
    }

    #[test]
    fn test_merge_no_overlap() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
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

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
        assert_eq!(merged.versions.len(), 3);

        let v2 = merged.versions.iter().find(|v| v.version == "2.0").unwrap();
        assert!(v2.converted, "Should prefer converted=true for v2.0");
        assert!(v2.content_hash.is_some());
    }

    #[test]
    fn test_merge_keeps_first_when_both_converted() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
        assert_eq!(merged.versions.len(), 1);
        assert!(merged.versions[0].converted);
    }

    #[test]
    fn test_merge_keeps_first_when_both_unconverted() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", false)]);

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2]).unwrap();
        assert_eq!(merged.versions.len(), 1);
        assert!(!merged.versions[0].converted);
    }

    #[test]
    fn test_merge_sorted_output() {
        let entry1 = make_entry("nginx", "fedora", vec![make_version("3.0", true)]);
        let entry2 = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);
        let entry3 = make_entry("nginx", "fedora", vec![make_version("2.0", true)]);

        let merged = merge_sparse_entries("fedora", "nginx", vec![entry1, entry2, entry3]).unwrap();
        assert_eq!(merged.versions.len(), 3);
        assert_eq!(merged.versions[0].version, "1.0");
        assert_eq!(merged.versions[1].version, "2.0");
        assert_eq!(merged.versions[2].version, "3.0");
    }

    #[test]
    fn merge_preserves_sibling_releases_and_architectures() {
        let mut first = make_version("1.0", false);
        first.release = Some("1".to_string());
        let mut second = make_version("1.0", false);
        second.release = Some("2".to_string());
        let mut third = make_version("1.0", false);
        third.release = Some("2".to_string());
        third.architecture = Some("aarch64".to_string());

        let merged = merge_sparse_entries(
            "fedora",
            "demo",
            vec![
                make_entry("demo", "fedora", vec![first]),
                make_entry("demo", "fedora", vec![second, third]),
            ],
        )
        .unwrap();

        assert_eq!(merged.versions.len(), 3);
        assert_eq!(merged.versions[0].release.as_deref(), Some("1"));
        assert_eq!(merged.versions[1].architecture.as_deref(), Some("aarch64"));
        assert_eq!(merged.versions[2].architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn merge_rejects_response_identity_mismatch() {
        let error = merge_sparse_entries(
            "fedora",
            "nginx",
            vec![make_entry(
                "curl",
                "fedora",
                vec![make_version("1.0", false)],
            )],
        )
        .expect_err("wrong package identity must fail closed");
        assert!(error.to_string().contains("requested fedora/nginx"));
    }

    #[test]
    fn merge_rejects_conflicting_resolution_or_content_identity() {
        let first = make_version("1.0", true);
        let mut conflicting_metadata = first.clone();
        conflicting_metadata.size += 1;
        let error = merge_sparse_entries(
            "fedora",
            "nginx",
            vec![
                make_entry("nginx", "fedora", vec![first.clone()]),
                make_entry("nginx", "fedora", vec![conflicting_metadata]),
            ],
        )
        .expect_err("resolution metadata conflict must fail closed");
        assert!(error.to_string().contains("resolution metadata"));

        let mut conflicting_content = first.clone();
        conflicting_content.content_hash = Some("sha256:different".to_string());
        let error = merge_sparse_entries(
            "fedora",
            "nginx",
            vec![
                make_entry("nginx", "fedora", vec![first]),
                make_entry("nginx", "fedora", vec![conflicting_content]),
            ],
        )
        .expect_err("content identity conflict must fail closed");
        assert!(error.to_string().contains("content identity"));
    }

    #[tokio::test]
    async fn remote_fetch_rejects_body_identity_that_disagrees_with_url() {
        let app = axum::Router::new().route(
            "/v1/index/fedora/nginx",
            axum::routing::get(|| async {
                (
                    [(
                        crate::server::handlers::public_read::PROFILE_REVISION_HEADER,
                        REVISION_A,
                    )],
                    axum::Json(make_entry(
                        "curl",
                        "fedora",
                        vec![make_version("1.0", false)],
                    )),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let error = fetch_remote_sparse_entry(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "fedora",
            "nginx",
            REVISION_A,
        )
        .await
        .expect_err("upstream body identity must match its requested URL");
        server.abort();

        assert!(error.to_string().contains("requested fedora/nginx"));
    }

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let cache = FederatedIndexCache::new();
        let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        cache
            .put(REVISION_A, "fedora", "nginx", entry.clone())
            .await;

        let cached = cache
            .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
            .await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().versions.len(), 1);
    }

    #[tokio::test]
    async fn test_cache_miss_no_entry() {
        let cache = FederatedIndexCache::new();

        let cached = cache
            .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
            .await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_ttl_expiry() {
        let cache = FederatedIndexCache::new();
        let entry = make_entry("nginx", "fedora", vec![make_version("1.0", true)]);

        cache.put(REVISION_A, "fedora", "nginx", entry).await;

        // With a zero TTL, the entry should be considered expired immediately
        let cached = cache
            .get(REVISION_A, "fedora", "nginx", Duration::from_secs(0))
            .await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_cleanup() {
        let cache = FederatedIndexCache::new();

        cache
            .put(
                REVISION_A,
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
            )
            .await;
        cache
            .put(
                REVISION_A,
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
                REVISION_A,
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
                REVISION_A,
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", true)]),
            )
            .await;
        cache
            .put(
                REVISION_A,
                "arch",
                "nginx",
                make_entry("nginx", "arch", vec![make_version("2.0", true)]),
            )
            .await;

        let fedora = cache
            .get(REVISION_A, "fedora", "nginx", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(fedora.distro, "fedora");

        let arch = cache
            .get(REVISION_A, "arch", "nginx", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(arch.distro, "arch");
    }

    #[tokio::test]
    async fn cache_never_reuses_an_entry_across_profile_revisions() {
        let cache = FederatedIndexCache::new();
        cache
            .put(
                REVISION_A,
                "fedora",
                "nginx",
                make_entry("nginx", "fedora", vec![make_version("1.0", false)]),
            )
            .await;

        assert!(
            cache
                .get(REVISION_B, "fedora", "nginx", Duration::from_secs(60))
                .await
                .is_none()
        );
    }
}
