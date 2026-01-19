// src/federation/peer.rs
//! Peer types and registry for federation

use super::config::PeerTier;
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique peer identifier (SHA-256 of endpoint URL)
pub type PeerId = String;

/// A federation peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Unique identifier (hash of endpoint)
    pub id: PeerId,
    /// HTTP(S) endpoint URL
    pub endpoint: String,
    /// Human-friendly name
    pub name: Option<String>,
    /// Peer's role in the hierarchy
    pub tier: PeerTier,
    /// When this peer was first discovered
    pub first_seen: DateTime<Utc>,
    /// When this peer was last seen/contacted
    pub last_seen: DateTime<Utc>,
    /// Performance and reliability score
    pub score: PeerScore,
}

impl Peer {
    /// Create a peer from an endpoint URL
    pub fn from_endpoint(endpoint: &str, tier: PeerTier) -> Result<Self> {
        // Validate URL
        let _url = url::Url::parse(endpoint)
            .map_err(|e| Error::ParseError(format!("Invalid peer URL '{}': {}", endpoint, e)))?;

        // Generate ID from endpoint hash
        let id = crate::hash::sha256(endpoint.as_bytes());
        let now = Utc::now();

        Ok(Self {
            id,
            endpoint: endpoint.to_string(),
            name: None,
            tier,
            first_seen: now,
            last_seen: now,
            score: PeerScore::default(),
        })
    }

    /// Create a peer with a custom name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Update the last_seen timestamp
    pub fn touch(&mut self) {
        self.last_seen = Utc::now();
    }
}

/// Performance and reliability score for a peer
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerScore {
    /// Exponentially weighted moving average of latency (milliseconds)
    pub latency_ewma_ms: f64,
    /// Recent success rate (0.0 - 1.0)
    pub success_rate: f32,
    /// Estimated bandwidth (bytes per second)
    pub bandwidth_bps: u64,
    /// Consecutive failure count (for circuit breaker)
    pub consecutive_failures: u32,
    /// Total successful requests
    pub total_successes: u64,
    /// Total failed requests
    pub total_failures: u64,
}

impl PeerScore {
    /// EWMA smoothing factor (higher = more weight on recent observations)
    const EWMA_ALPHA: f64 = 0.3;

    /// Record a successful request
    pub fn record_success(&mut self, latency_ms: u64) {
        // Update latency EWMA
        if self.latency_ewma_ms == 0.0 {
            self.latency_ewma_ms = latency_ms as f64;
        } else {
            self.latency_ewma_ms = Self::EWMA_ALPHA * latency_ms as f64
                + (1.0 - Self::EWMA_ALPHA) * self.latency_ewma_ms;
        }

        self.consecutive_failures = 0;
        self.total_successes += 1;
        self.update_success_rate();
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.total_failures += 1;
        self.update_success_rate();
    }

    /// Update the success rate based on total requests
    fn update_success_rate(&mut self) {
        let total = self.total_successes + self.total_failures;
        if total > 0 {
            self.success_rate = self.total_successes as f32 / total as f32;
        }
    }

    /// Get a composite quality score (higher is better)
    pub fn quality(&self) -> f64 {
        // Normalize latency (lower is better, cap at 1000ms)
        let latency_score = 1.0 - (self.latency_ewma_ms / 1000.0).min(1.0);

        // Weight: 60% success rate, 40% latency
        (self.success_rate as f64 * 0.6) + (latency_score * 0.4)
    }
}

/// Registry of known peers
#[derive(Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<PeerId, Peer>,
}

impl PeerRegistry {
    /// Create an empty registry
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Add or update a peer
    pub fn add(&mut self, peer: Peer) {
        self.peers.insert(peer.id.clone(), peer);
    }

    /// Remove a peer by ID
    pub fn remove(&mut self, id: &PeerId) -> Option<Peer> {
        self.peers.remove(id)
    }

    /// Get a peer by ID
    pub fn get(&self, id: &PeerId) -> Option<&Peer> {
        self.peers.get(id)
    }

    /// Get a mutable reference to a peer
    pub fn get_mut(&mut self, id: &PeerId) -> Option<&mut Peer> {
        self.peers.get_mut(id)
    }

    /// Get all peers as a vector
    pub fn all(&self) -> Vec<Peer> {
        self.peers.values().cloned().collect()
    }

    /// Get peers filtered by tier
    pub fn by_tier(&self, tier: PeerTier) -> Vec<Peer> {
        self.peers
            .values()
            .filter(|p| p.tier == tier)
            .cloned()
            .collect()
    }

    /// Get the number of peers
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_from_endpoint() {
        let peer = Peer::from_endpoint("https://remi.conary.io:7891", PeerTier::RegionHub).unwrap();

        assert_eq!(peer.endpoint, "https://remi.conary.io:7891");
        assert_eq!(peer.tier, PeerTier::RegionHub);
        assert!(peer.name.is_none());
        assert_eq!(peer.id.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_peer_invalid_url() {
        let result = Peer::from_endpoint("not-a-url", PeerTier::Leaf);
        assert!(result.is_err());
    }

    #[test]
    fn test_peer_score_success() {
        let mut score = PeerScore::default();

        score.record_success(100);
        assert_eq!(score.latency_ewma_ms, 100.0);
        assert_eq!(score.consecutive_failures, 0);
        assert_eq!(score.total_successes, 1);
        assert_eq!(score.success_rate, 1.0);

        // Second observation with EWMA
        score.record_success(200);
        // 0.3 * 200 + 0.7 * 100 = 60 + 70 = 130
        assert!((score.latency_ewma_ms - 130.0).abs() < 0.1);
    }

    #[test]
    fn test_peer_score_failure() {
        let mut score = PeerScore::default();

        score.record_success(100);
        score.record_failure();
        score.record_failure();

        assert_eq!(score.consecutive_failures, 2);
        assert_eq!(score.total_failures, 2);
        assert_eq!(score.total_successes, 1);
        // 1 / 3 = 0.333...
        assert!((score.success_rate - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_peer_registry() {
        let mut registry = PeerRegistry::new();

        let peer1 = Peer::from_endpoint("http://peer1:7891", PeerTier::CellHub).unwrap();
        let peer2 = Peer::from_endpoint("http://peer2:7891", PeerTier::CellHub).unwrap();
        let peer3 = Peer::from_endpoint("https://region:7891", PeerTier::RegionHub).unwrap();

        registry.add(peer1.clone());
        registry.add(peer2.clone());
        registry.add(peer3.clone());

        assert_eq!(registry.len(), 3);
        assert!(registry.get(&peer1.id).is_some());

        let cell_peers = registry.by_tier(PeerTier::CellHub);
        assert_eq!(cell_peers.len(), 2);

        registry.remove(&peer1.id);
        assert_eq!(registry.len(), 2);
    }
}
