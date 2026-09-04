// apps/remi/src/federation/router.rs
//! Rendezvous (HRW) hashing for peer selection
//!
//! Rendezvous hashing provides deterministic peer selection without requiring
//! global state synchronization. Given a chunk hash and a set of peers, any
//! node will independently compute the same K candidate peers.

use super::config::{PeerTier, TierAllowlists};
use super::peer::Peer;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Rendezvous (Highest Random Weight) router
///
/// Selects K peers for each chunk using deterministic hashing.
/// No global state needed - the same inputs always produce the same outputs.
#[derive(Debug, Clone)]
pub struct RendezvousRouter {
    /// Number of candidate peers to select
    k: usize,
}

impl RendezvousRouter {
    /// Create a new router with the specified K value
    pub fn new(k: usize) -> Self {
        Self { k: k.max(1) }
    }

    /// Select K peers for a chunk using rendezvous hashing
    ///
    /// The algorithm:
    /// 1. For each peer, compute weight = hash(chunk_hash || peer_id)
    /// 2. Sort peers by weight (descending)
    /// 3. Return top K peers
    ///
    /// This is deterministic: any node with the same chunk hash and peer list
    /// will select the same K peers.
    pub fn select_peers<'a>(&self, chunk_hash: &str, peers: &'a [Peer]) -> Vec<&'a Peer> {
        if peers.is_empty() {
            return Vec::new();
        }

        // Use a min-heap to efficiently keep top K
        let mut heap: BinaryHeap<(Reverse<u64>, usize)> = BinaryHeap::new();

        for (idx, peer) in peers.iter().enumerate() {
            let weight = self.compute_weight(chunk_hash, &peer.id);

            if heap.len() < self.k {
                heap.push((Reverse(weight), idx));
            } else if let Some(&(Reverse(min_weight), _)) = heap.peek()
                && weight > min_weight
            {
                heap.pop();
                heap.push((Reverse(weight), idx));
            }
        }

        // Extract peers sorted by weight (highest first)
        let mut result: Vec<_> = heap.into_iter().map(|(Reverse(w), idx)| (w, idx)).collect();
        result.sort_by_key(|entry| Reverse(entry.0)); // Sort descending by weight

        result.into_iter().map(|(_, idx)| &peers[idx]).collect()
    }

    /// Compute the weight for a (chunk, peer) pair
    ///
    /// Uses FNV-1a for speed.
    fn compute_weight(&self, chunk_hash: &str, peer_id: &str) -> u64 {
        // Combine chunk hash and peer ID
        let combined = format!("{}:{}", chunk_hash, peer_id);

        // FNV-1a hash (fast, good distribution)
        fnv1a_hash(combined.as_bytes())
    }

    /// Select peers hierarchically by tier
    ///
    /// This implements the cell → region → leaf routing strategy:
    /// 1. First, return up to K cell-local peers (fast LAN access)
    /// 2. Then, return up to K region hub peers (WAN with mTLS)
    /// 3. Finally, return up to K leaf peers (other nodes)
    ///
    /// Within each tier, peers are selected using rendezvous hashing
    /// for deterministic, consistent selection.
    ///
    /// Returns a `HierarchicalSelection` containing peers grouped by tier.
    pub fn select_peers_hierarchical<'a>(
        &self,
        chunk_hash: &str,
        peers: &'a [Peer],
    ) -> HierarchicalSelection<'a> {
        self.partition_and_select(chunk_hash, peers, |_| true)
    }

    /// Select peers in flattened hierarchical order
    ///
    /// Returns a single vector with peers ordered by tier priority:
    /// cell hubs first, then region hubs, then leaves.
    ///
    /// This is a convenience method for simple iteration.
    pub fn select_peers_ordered<'a>(&self, chunk_hash: &str, peers: &'a [Peer]) -> Vec<&'a Peer> {
        let selection = self.select_peers_hierarchical(chunk_hash, peers);
        selection.into_ordered_vec()
    }

    /// Select peers hierarchically with allowlist filtering
    ///
    /// Like `select_peers_hierarchical`, but first filters peers against
    /// per-tier allowlists. Only peers whose endpoints match the allowlist
    /// patterns for their tier are considered for selection.
    ///
    /// If no allowlist is configured for a tier, all peers of that tier pass.
    pub fn select_peers_hierarchical_filtered<'a>(
        &self,
        chunk_hash: &str,
        peers: &'a [Peer],
        allowlists: &TierAllowlists,
    ) -> HierarchicalSelection<'a> {
        self.partition_and_select(chunk_hash, peers, |peer| {
            allowlists.is_allowed(&peer.endpoint, peer.tier)
        })
    }

    /// Select peers in flattened hierarchical order with allowlist filtering
    ///
    /// Combines `select_peers_hierarchical_filtered` with flattening to
    /// a single vector ordered by tier priority.
    pub fn select_peers_ordered_filtered<'a>(
        &self,
        chunk_hash: &str,
        peers: &'a [Peer],
        allowlists: &TierAllowlists,
    ) -> Vec<&'a Peer> {
        let selection = self.select_peers_hierarchical_filtered(chunk_hash, peers, allowlists);
        selection.into_ordered_vec()
    }

    /// Partition peers by tier, apply a filter, and select up to K from each tier
    fn partition_and_select<'a>(
        &self,
        chunk_hash: &str,
        peers: &'a [Peer],
        filter: impl Fn(&Peer) -> bool,
    ) -> HierarchicalSelection<'a> {
        let mut cell_peers: Vec<&'a Peer> = Vec::new();
        let mut region_peers: Vec<&'a Peer> = Vec::new();
        let mut leaf_peers: Vec<&'a Peer> = Vec::new();

        for peer in peers {
            if !filter(peer) {
                continue;
            }
            match peer.tier {
                PeerTier::CellHub => cell_peers.push(peer),
                PeerTier::RegionHub => region_peers.push(peer),
                PeerTier::Leaf => leaf_peers.push(peer),
            }
        }

        HierarchicalSelection {
            cell_hubs: self.select_k_from_tier(chunk_hash, cell_peers),
            region_hubs: self.select_k_from_tier(chunk_hash, region_peers),
            leaves: self.select_k_from_tier(chunk_hash, leaf_peers),
        }
    }

    /// Rendezvous-hash rank and take up to K peers from a single tier
    fn select_k_from_tier<'a>(&self, chunk_hash: &str, tier_peers: Vec<&'a Peer>) -> Vec<&'a Peer> {
        if tier_peers.is_empty() {
            return Vec::new();
        }

        let mut weighted: Vec<(u64, &'a Peer)> = tier_peers
            .into_iter()
            .map(|p| (self.compute_weight(chunk_hash, &p.id), p))
            .collect();

        weighted.sort_by_key(|entry| Reverse(entry.0));
        weighted.into_iter().take(self.k).map(|(_, p)| p).collect()
    }
}

/// Result of hierarchical peer selection
///
/// Contains peers grouped by tier, already sorted by rendezvous weight
/// within each tier.
#[derive(Debug, Clone)]
pub struct HierarchicalSelection<'a> {
    /// Cell-local peers (fast LAN access) - highest priority
    pub cell_hubs: Vec<&'a Peer>,
    /// Region hub peers (WAN with mTLS) - medium priority
    pub region_hubs: Vec<&'a Peer>,
    /// Leaf peers (other nodes) - lowest priority
    pub leaves: Vec<&'a Peer>,
}

impl<'a> HierarchicalSelection<'a> {
    /// Total number of selected peers across all tiers
    pub fn total_count(&self) -> usize {
        self.cell_hubs.len() + self.region_hubs.len() + self.leaves.len()
    }

    /// Check if any peers were selected
    pub fn is_empty(&self) -> bool {
        self.cell_hubs.is_empty() && self.region_hubs.is_empty() && self.leaves.is_empty()
    }

    /// Convert to a flat vector in tier priority order
    pub fn into_ordered_vec(self) -> Vec<&'a Peer> {
        let mut result = Vec::with_capacity(self.total_count());
        result.extend(self.cell_hubs);
        result.extend(self.region_hubs);
        result.extend(self.leaves);
        result
    }

    /// Iterate over all peers in tier priority order
    pub fn iter(&self) -> impl Iterator<Item = &'a Peer> + '_ {
        self.cell_hubs
            .iter()
            .chain(self.region_hubs.iter())
            .chain(self.leaves.iter())
            .copied()
    }

    /// Iterate over peers with their tier
    pub fn iter_with_tier(&self) -> impl Iterator<Item = (&'a Peer, PeerTier)> + '_ {
        self.cell_hubs
            .iter()
            .map(|p| (*p, PeerTier::CellHub))
            .chain(self.region_hubs.iter().map(|p| (*p, PeerTier::RegionHub)))
            .chain(self.leaves.iter().map(|p| (*p, PeerTier::Leaf)))
    }
}

/// FNV-1a hash function (64-bit)
///
/// Fast and has good distribution properties for hash-based routing.
fn fnv1a_hash(data: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl Default for RendezvousRouter {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
mod tests;
