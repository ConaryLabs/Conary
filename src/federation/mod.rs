// src/federation/mod.rs
//! Cross-Machine CAS Federation
//!
//! Enables multiple machines to share content-addressable storage chunks over a network,
//! reducing bandwidth and storage by deduplicating content across a fleet.
//!
//! # Architecture
//!
//! The federation system uses a hierarchical model:
//! - **Leaf nodes**: Individual machines that fetch chunks
//! - **Cell hubs**: Site-local caches (rack-level, fast LAN access)
//! - **Region hubs**: WAN-accessible caches with mTLS
//!
//! # Key Design Decisions
//!
//! Based on expert review (GPT 5.2 + Gemini 3 Pro):
//! - **Rendezvous hashing** instead of Bloom filters: Deterministic K-peer selection
//! - **Hierarchical cells** instead of full mesh: Prevents O(N²) complexity
//! - **Request coalescing**: Singleflight pattern prevents duplicate fetches
//! - **Circuit breakers**: Per-peer failure tracking with jitter-based cooldowns
//!
//! # Usage
//!
//! ```toml
//! [federation]
//! enabled = true
//! tier = "leaf"
//! region_hubs = ["https://remi.conary.io:7891"]
//! cell_hubs = ["http://rack-cache.local:7891"]
//! ```

mod circuit;
mod coalesce;
mod config;
mod peer;
mod router;

pub use circuit::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
pub use coalesce::RequestCoalescer;
pub use config::{FederationConfig, PeerTier};
pub use peer::{Peer, PeerId, PeerRegistry, PeerScore};
pub use router::RendezvousRouter;

use crate::error::{Error, Result};
use crate::hash::verify_sha256;
use crate::repository::chunk_fetcher::{ChunkFetcher, LocalCacheFetcher};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Main Federation coordinator
///
/// Coordinates chunk fetching across federated peers using:
/// - Rendezvous hashing for peer selection
/// - Request coalescing for deduplication
/// - Circuit breakers for resilience
pub struct Federation {
    /// Federation configuration
    config: FederationConfig,
    /// Registry of known peers
    peers: Arc<RwLock<PeerRegistry>>,
    /// Rendezvous router for peer selection
    router: RendezvousRouter,
    /// Request coalescer (singleflight pattern)
    coalescer: RequestCoalescer,
    /// Per-peer circuit breakers
    circuits: CircuitBreakerRegistry,
    /// HTTP client for chunk fetching
    client: reqwest::Client,
}

impl Federation {
    /// Create a new Federation coordinator
    pub fn new(config: FederationConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .pool_max_idle_per_host(config.rendezvous_k)
            .build()
            .map_err(|e| Error::InitError(format!("Failed to create HTTP client: {e}")))?;

        let mut peer_registry = PeerRegistry::new();

        // Add configured cell hubs
        for endpoint in &config.cell_hubs {
            if let Ok(peer) = Peer::from_endpoint(endpoint, PeerTier::CellHub) {
                peer_registry.add(peer);
            }
        }

        // Add configured region hubs
        for endpoint in &config.region_hubs {
            if let Ok(peer) = Peer::from_endpoint(endpoint, PeerTier::RegionHub) {
                peer_registry.add(peer);
            }
        }

        let circuits = CircuitBreakerRegistry::new(
            config.circuit_threshold,
            Duration::from_secs(config.circuit_cooldown_secs),
            config.jitter_factor,
        );

        Ok(Self {
            config: config.clone(),
            peers: Arc::new(RwLock::new(peer_registry)),
            router: RendezvousRouter::new(config.rendezvous_k),
            coalescer: RequestCoalescer::new(),
            circuits,
            client,
        })
    }

    /// Check if federation is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the current configuration
    pub fn config(&self) -> &FederationConfig {
        &self.config
    }

    /// Get a snapshot of current peers
    pub async fn peers(&self) -> Vec<Peer> {
        let registry = self.peers.read().await;
        registry.all()
    }

    /// Add a peer dynamically
    pub async fn add_peer(&self, peer: Peer) {
        let mut registry = self.peers.write().await;
        registry.add(peer);
    }

    /// Remove a peer
    pub async fn remove_peer(&self, peer_id: &PeerId) {
        let mut registry = self.peers.write().await;
        registry.remove(peer_id);
    }

    /// Fetch a chunk from federated peers
    ///
    /// Uses rendezvous hashing to select K candidate peers, then tries
    /// them in order based on tier (cell first, then region) and circuit
    /// breaker state.
    pub async fn fetch_chunk(&self, hash: &str) -> Result<Vec<u8>> {
        if !self.config.enabled {
            return Err(Error::NotFound("Federation disabled".to_string()));
        }

        // Use coalescer to deduplicate concurrent requests for same chunk
        self.coalescer
            .coalesce(hash, || self.fetch_chunk_inner(hash))
            .await
    }

    /// Inner fetch logic (called via coalescer)
    async fn fetch_chunk_inner(&self, hash: &str) -> Result<Vec<u8>> {
        let peers = self.peers.read().await;
        let all_peers = peers.all();

        if all_peers.is_empty() {
            return Err(Error::NotFound("No federation peers available".to_string()));
        }

        // Select K candidate peers using rendezvous hashing
        let candidates = self.router.select_peers(hash, &all_peers);

        // Try cell-local peers first (if prefer_cell is set)
        if self.config.prefer_cell {
            for peer in candidates.iter().filter(|p| p.tier == PeerTier::CellHub) {
                if self.circuits.is_open(&peer.id) {
                    debug!("Skipping peer {} (circuit open)", peer.id);
                    continue;
                }

                match self.try_fetch(peer, hash).await {
                    Ok(data) => {
                        self.circuits.record_success(&peer.id);
                        return Ok(data);
                    }
                    Err(e) => {
                        debug!("Cell peer {} failed: {}", peer.id, e);
                        self.circuits.record_failure(&peer.id);
                    }
                }
            }
        }

        // Try all candidates (including region hubs)
        for peer in &candidates {
            if self.circuits.is_open(&peer.id) {
                continue;
            }

            // Skip cell hubs if we already tried them
            if self.config.prefer_cell && peer.tier == PeerTier::CellHub {
                continue;
            }

            match self.try_fetch(peer, hash).await {
                Ok(data) => {
                    self.circuits.record_success(&peer.id);
                    return Ok(data);
                }
                Err(e) => {
                    debug!("Peer {} failed: {}", peer.id, e);
                    self.circuits.record_failure(&peer.id);
                }
            }
        }

        Err(Error::NotFound(format!(
            "Chunk {} not available from any federation peer",
            hash
        )))
    }

    /// Try to fetch a chunk from a specific peer
    async fn try_fetch(&self, peer: &Peer, hash: &str) -> Result<Vec<u8>> {
        let url = format!("{}/v1/chunks/{}", peer.endpoint, hash);
        debug!("Fetching chunk {} from {}", hash, peer.endpoint);

        let response = self
            .client
            .get(&url)
            .timeout(Duration::from_millis(self.config.request_timeout_ms))
            .send()
            .await
            .map_err(|e| Error::DownloadError(format!("Request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(Error::NotFound(format!(
                "Chunk {} not found at {} (HTTP {})",
                hash,
                peer.endpoint,
                response.status()
            )));
        }

        let data = response
            .bytes()
            .await
            .map_err(|e| Error::DownloadError(format!("Failed to read response: {e}")))?;

        // Enforce max chunk size
        if data.len() > self.config.max_chunk_size {
            return Err(Error::DownloadError(format!(
                "Chunk {} exceeds max size ({} > {})",
                hash,
                data.len(),
                self.config.max_chunk_size
            )));
        }

        // Verify hash
        verify_sha256(&data, hash).map_err(|e| Error::ChecksumMismatch {
            expected: e.expected,
            actual: e.actual,
        })?;

        info!("[federation] chunk {} from {}", hash, peer.endpoint);
        Ok(data.to_vec())
    }

    /// Check if a chunk exists at any peer (HEAD request)
    pub async fn chunk_exists(&self, hash: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        let peers = self.peers.read().await;
        let all_peers = peers.all();
        let candidates = self.router.select_peers(hash, &all_peers);

        for peer in &candidates {
            if self.circuits.is_open(&peer.id) {
                continue;
            }

            let url = format!("{}/v1/chunks/{}", peer.endpoint, hash);
            if let Ok(response) = self.client.head(&url).send().await
                && response.status().is_success()
            {
                return true;
            }
        }

        false
    }

    /// Get federation statistics
    pub async fn stats(&self) -> FederationStats {
        let peers = self.peers.read().await;
        let all_peers = peers.all();

        let mut cell_hubs = 0;
        let mut region_hubs = 0;
        let mut open_circuits = 0;

        for peer in &all_peers {
            match peer.tier {
                PeerTier::CellHub => cell_hubs += 1,
                PeerTier::RegionHub => region_hubs += 1,
                PeerTier::Leaf => {}
            }

            if self.circuits.is_open(&peer.id) {
                open_circuits += 1;
            }
        }

        FederationStats {
            enabled: self.config.enabled,
            tier: self.config.tier,
            total_peers: all_peers.len(),
            cell_hubs,
            region_hubs,
            open_circuits,
            coalesced_requests: self.coalescer.coalesced_count(),
        }
    }
}

/// Federation statistics
#[derive(Debug, Clone)]
pub struct FederationStats {
    /// Whether federation is enabled
    pub enabled: bool,
    /// This node's tier
    pub tier: PeerTier,
    /// Total number of known peers
    pub total_peers: usize,
    /// Number of cell hub peers
    pub cell_hubs: usize,
    /// Number of region hub peers
    pub region_hubs: usize,
    /// Number of peers with open circuit breakers
    pub open_circuits: usize,
    /// Number of coalesced (deduplicated) requests
    pub coalesced_requests: u64,
}

/// Federated chunk fetcher that integrates with the existing ChunkFetcher trait
pub struct FederatedChunkFetcher {
    federation: Arc<Federation>,
    local_cache: LocalCacheFetcher,
    fallback: Option<Arc<dyn ChunkFetcher>>,
}

impl FederatedChunkFetcher {
    /// Create a new federated chunk fetcher
    pub fn new(
        federation: Arc<Federation>,
        cache_dir: impl AsRef<std::path::Path>,
        fallback: Option<Arc<dyn ChunkFetcher>>,
    ) -> Self {
        Self {
            federation,
            local_cache: LocalCacheFetcher::new(cache_dir),
            fallback,
        }
    }
}

#[async_trait]
impl ChunkFetcher for FederatedChunkFetcher {
    async fn fetch(&self, hash: &str) -> Result<Vec<u8>> {
        // 1. Check local cache first
        if let Ok(data) = self.local_cache.fetch(hash).await {
            debug!("Cache hit for chunk {}", hash);
            return Ok(data);
        }

        // 2. Try federation
        if self.federation.is_enabled() {
            match self.federation.fetch_chunk(hash).await {
                Ok(data) => {
                    // Cache locally
                    if let Err(e) = self.local_cache.store(hash, &data).await {
                        warn!("Failed to cache chunk {}: {}", hash, e);
                    }
                    return Ok(data);
                }
                Err(e) => {
                    debug!("Federation fetch failed for {}: {}", hash, e);
                }
            }
        }

        // 3. Fall back to upstream (origin)
        if let Some(fallback) = &self.fallback {
            let data = fallback.fetch(hash).await?;
            // Cache locally
            if let Err(e) = self.local_cache.store(hash, &data).await {
                warn!("Failed to cache chunk {}: {}", hash, e);
            }
            return Ok(data);
        }

        Err(Error::NotFound(format!("Chunk {} not found", hash)))
    }

    async fn exists(&self, hash: &str) -> bool {
        // Check local cache
        if self.local_cache.exists(hash).await {
            return true;
        }

        // Check federation
        if self.federation.is_enabled() && self.federation.chunk_exists(hash).await {
            return true;
        }

        // Check fallback
        if let Some(fallback) = &self.fallback {
            return fallback.exists(hash).await;
        }

        false
    }

    async fn fetch_many(&self, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        let mut results = HashMap::new();
        let mut remaining = Vec::new();

        // Check local cache first
        for hash in hashes {
            if let Ok(data) = self.local_cache.fetch(hash).await {
                results.insert(hash.clone(), data);
            } else {
                remaining.push(hash.clone());
            }
        }

        if remaining.is_empty() {
            return Ok(results);
        }

        // Fetch remaining from federation (parallel)
        if self.federation.is_enabled() {
            let federation = &self.federation;
            let cache = &self.local_cache;

            let fetches: Vec<_> = remaining
                .iter()
                .map(|hash| async move {
                    match federation.fetch_chunk(hash).await {
                        Ok(data) => {
                            let _ = cache.store(hash, &data).await;
                            Some((hash.clone(), data))
                        }
                        Err(_) => None,
                    }
                })
                .collect();

            let fetch_results = futures::future::join_all(fetches).await;
            for result in fetch_results.into_iter().flatten() {
                remaining.retain(|h| h != &result.0);
                results.insert(result.0, result.1);
            }
        }

        // Fetch any remaining from fallback
        if !remaining.is_empty()
            && let Some(fallback) = &self.fallback
        {
            let fallback_results = fallback.fetch_many(&remaining).await?;
            for (hash, data) in fallback_results {
                let _ = self.local_cache.store(&hash, &data).await;
                results.insert(hash, data);
            }
        }

        Ok(results)
    }

    fn name(&self) -> &str {
        "federated"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_federation_config_default() {
        let config = FederationConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.tier, PeerTier::Leaf);
        assert_eq!(config.rendezvous_k, 3);
    }

    #[test]
    fn test_federation_new() {
        let config = FederationConfig::default();
        let federation = Federation::new(config).unwrap();
        assert!(!federation.is_enabled());
    }

    #[tokio::test]
    async fn test_federation_stats() {
        let config = FederationConfig::default();
        let federation = Federation::new(config).unwrap();
        let stats = federation.stats().await;

        assert!(!stats.enabled);
        assert_eq!(stats.total_peers, 0);
    }
}
