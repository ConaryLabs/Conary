// src/federation/router.rs
//! Rendezvous (HRW) hashing for peer selection
//!
//! Rendezvous hashing provides deterministic peer selection without requiring
//! global state synchronization. Given a chunk hash and a set of peers, any
//! node will independently compute the same K candidate peers.
//!
//! This approach was recommended by both GPT 5.2 and Gemini 3 Pro experts
//! over Bloom filters, which have O(N²) dissemination complexity.

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
        result.sort_by(|a, b| b.0.cmp(&a.0)); // Sort descending by weight

        result.into_iter().map(|(_, idx)| &peers[idx]).collect()
    }

    /// Compute the weight for a (chunk, peer) pair
    ///
    /// Uses FNV-1a for speed. For even better performance at scale,
    /// consider BLAKE3 (as recommended by Gemini 3 Pro).
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
        // Partition peers by tier
        let mut cell_peers: Vec<&'a Peer> = Vec::new();
        let mut region_peers: Vec<&'a Peer> = Vec::new();
        let mut leaf_peers: Vec<&'a Peer> = Vec::new();

        for peer in peers {
            match peer.tier {
                PeerTier::CellHub => cell_peers.push(peer),
                PeerTier::RegionHub => region_peers.push(peer),
                PeerTier::Leaf => leaf_peers.push(peer),
            }
        }

        // Select up to K from each tier using rendezvous hashing
        let select_k = |tier_peers: Vec<&'a Peer>| -> Vec<&'a Peer> {
            if tier_peers.is_empty() {
                return Vec::new();
            }

            // Compute weights for all peers in this tier
            let mut weighted: Vec<(u64, &Peer)> = tier_peers
                .into_iter()
                .map(|p| (self.compute_weight(chunk_hash, &p.id), p))
                .collect();

            // Sort by weight descending
            weighted.sort_by(|a, b| b.0.cmp(&a.0));

            // Take up to K
            weighted
                .into_iter()
                .take(self.k)
                .map(|(_, p)| p)
                .collect()
        };

        HierarchicalSelection {
            cell_hubs: select_k(cell_peers),
            region_hubs: select_k(region_peers),
            leaves: select_k(leaf_peers),
        }
    }

    /// Select peers in flattened hierarchical order
    ///
    /// Returns a single vector with peers ordered by tier priority:
    /// cell hubs first, then region hubs, then leaves.
    ///
    /// This is a convenience method for simple iteration.
    pub fn select_peers_ordered<'a>(
        &self,
        chunk_hash: &str,
        peers: &'a [Peer],
    ) -> Vec<&'a Peer> {
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
        // Filter peers by allowlist before selection
        let filtered: Vec<&'a Peer> = peers
            .iter()
            .filter(|peer| allowlists.is_allowed(&peer.endpoint, peer.tier))
            .collect();

        // Convert back to owned references for selection
        let filtered_owned: Vec<Peer> = filtered.iter().map(|p| (*p).clone()).collect();

        // Select from filtered peers
        let selection = self.select_peers_hierarchical(chunk_hash, &filtered_owned);

        // Map back to original peer references
        let map_back = |selected: Vec<&Peer>| -> Vec<&'a Peer> {
            selected
                .into_iter()
                .filter_map(|sel| peers.iter().find(|p| p.id == sel.id))
                .collect()
        };

        HierarchicalSelection {
            cell_hubs: map_back(selection.cell_hubs),
            region_hubs: map_back(selection.region_hubs),
            leaves: map_back(selection.leaves),
        }
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
mod tests {
    use super::*;
    use crate::federation::config::PeerTier;

    fn make_peers(n: usize) -> Vec<Peer> {
        (0..n)
            .map(|i| Peer::from_endpoint(&format!("http://peer{}:7891", i), PeerTier::CellHub).unwrap())
            .collect()
    }

    #[test]
    fn test_select_peers_deterministic() {
        let router = RendezvousRouter::new(3);
        let peers = make_peers(10);
        let chunk_hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";

        let selected1 = router.select_peers(chunk_hash, &peers);
        let selected2 = router.select_peers(chunk_hash, &peers);

        // Same inputs = same outputs
        assert_eq!(selected1.len(), selected2.len());
        for (p1, p2) in selected1.iter().zip(selected2.iter()) {
            assert_eq!(p1.id, p2.id);
        }
    }

    #[test]
    fn test_select_peers_k_limit() {
        let router = RendezvousRouter::new(3);
        let peers = make_peers(10);
        let chunk_hash = "test_hash";

        let selected = router.select_peers(chunk_hash, &peers);
        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn test_select_peers_fewer_than_k() {
        let router = RendezvousRouter::new(5);
        let peers = make_peers(2);
        let chunk_hash = "test_hash";

        let selected = router.select_peers(chunk_hash, &peers);
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn test_select_peers_empty() {
        let router = RendezvousRouter::new(3);
        let peers: Vec<Peer> = Vec::new();
        let chunk_hash = "test_hash";

        let selected = router.select_peers(chunk_hash, &peers);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_different_chunks_different_peers() {
        let router = RendezvousRouter::new(3);
        let peers = make_peers(10);

        let selected1 = router.select_peers("chunk_a", &peers);
        let selected2 = router.select_peers("chunk_b", &peers);

        // Different chunks should generally select different peers
        // (not guaranteed, but highly likely with good hashing)
        let ids1: Vec<_> = selected1.iter().map(|p| &p.id).collect();
        let ids2: Vec<_> = selected2.iter().map(|p| &p.id).collect();

        // At least one peer should differ (with high probability)
        let all_same = ids1.iter().zip(ids2.iter()).all(|(a, b)| a == b);
        // This could theoretically fail, but is extremely unlikely
        assert!(!all_same || peers.len() <= 3);
    }

    #[test]
    fn test_fnv1a_hash() {
        // Known test vectors
        assert_eq!(fnv1a_hash(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_hash(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a_hash(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn test_distribution() {
        // Verify that rendezvous hashing distributes chunks reasonably evenly
        let router = RendezvousRouter::new(1);
        let peers = make_peers(5);

        let mut counts = vec![0usize; 5];

        // Simulate 1000 chunks
        for i in 0..1000 {
            let chunk_hash = format!("chunk_{}", i);
            let selected = router.select_peers(&chunk_hash, &peers);
            if let Some(peer) = selected.first() {
                if let Some(idx) = peers.iter().position(|p| p.id == peer.id) {
                    counts[idx] += 1;
                }
            }
        }

        // Each peer should get roughly 200 chunks (1000/5)
        // Allow for significant variance (chi-squared would be more rigorous)
        for count in &counts {
            assert!(*count > 100, "Peer got too few chunks: {}", count);
            assert!(*count < 300, "Peer got too many chunks: {}", count);
        }
    }

    // =========================================================================
    // Hierarchical Routing Tests
    // =========================================================================

    fn make_mixed_peers() -> Vec<Peer> {
        vec![
            Peer::from_endpoint("http://cell1:7891", PeerTier::CellHub).unwrap(),
            Peer::from_endpoint("http://cell2:7891", PeerTier::CellHub).unwrap(),
            Peer::from_endpoint("http://cell3:7891", PeerTier::CellHub).unwrap(),
            Peer::from_endpoint("https://region1:7891", PeerTier::RegionHub).unwrap(),
            Peer::from_endpoint("https://region2:7891", PeerTier::RegionHub).unwrap(),
            Peer::from_endpoint("http://leaf1:7891", PeerTier::Leaf).unwrap(),
            Peer::from_endpoint("http://leaf2:7891", PeerTier::Leaf).unwrap(),
        ]
    }

    #[test]
    fn test_hierarchical_groups_by_tier() {
        let router = RendezvousRouter::new(10); // K > peer count
        let peers = make_mixed_peers();

        let selection = router.select_peers_hierarchical("test_chunk", &peers);

        assert_eq!(selection.cell_hubs.len(), 3);
        assert_eq!(selection.region_hubs.len(), 2);
        assert_eq!(selection.leaves.len(), 2);
        assert_eq!(selection.total_count(), 7);
    }

    #[test]
    fn test_hierarchical_respects_k_per_tier() {
        let router = RendezvousRouter::new(2); // K = 2
        let peers = make_mixed_peers();

        let selection = router.select_peers_hierarchical("test_chunk", &peers);

        // Should only select K=2 from each tier
        assert_eq!(selection.cell_hubs.len(), 2);
        assert_eq!(selection.region_hubs.len(), 2);
        assert_eq!(selection.leaves.len(), 2);
        assert_eq!(selection.total_count(), 6);
    }

    #[test]
    fn test_hierarchical_ordered_vec() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();

        let ordered = router.select_peers_ordered("test_chunk", &peers);

        // Verify order: cell hubs first, then region, then leaves
        let tiers: Vec<_> = ordered.iter().map(|p| p.tier).collect();

        // Find transition points
        let cell_count = tiers.iter().take_while(|&&t| t == PeerTier::CellHub).count();
        let region_start = cell_count;
        let region_count = tiers[region_start..]
            .iter()
            .take_while(|&&t| t == PeerTier::RegionHub)
            .count();

        assert_eq!(cell_count, 3, "Cell hubs should come first");
        assert_eq!(region_count, 2, "Region hubs should come after cell hubs");
        assert_eq!(
            ordered.len() - cell_count - region_count,
            2,
            "Leaves should come last"
        );
    }

    #[test]
    fn test_hierarchical_deterministic() {
        let router = RendezvousRouter::new(2);
        let peers = make_mixed_peers();

        let selection1 = router.select_peers_hierarchical("chunk_xyz", &peers);
        let selection2 = router.select_peers_hierarchical("chunk_xyz", &peers);

        // Same inputs = same outputs
        let ids1: Vec<_> = selection1.iter().map(|p| &p.id).collect();
        let ids2: Vec<_> = selection2.iter().map(|p| &p.id).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn test_hierarchical_empty_tiers() {
        let router = RendezvousRouter::new(3);

        // Only cell hubs
        let cell_only: Vec<Peer> = (0..5)
            .map(|i| Peer::from_endpoint(&format!("http://cell{}:7891", i), PeerTier::CellHub).unwrap())
            .collect();

        let selection = router.select_peers_hierarchical("test", &cell_only);

        assert_eq!(selection.cell_hubs.len(), 3);
        assert!(selection.region_hubs.is_empty());
        assert!(selection.leaves.is_empty());
    }

    #[test]
    fn test_hierarchical_iter_with_tier() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();

        let selection = router.select_peers_hierarchical("test", &peers);

        let collected: Vec<_> = selection.iter_with_tier().collect();

        // Verify tier annotations are correct
        for (peer, tier) in &collected {
            assert_eq!(peer.tier, *tier, "Tier annotation should match peer tier");
        }

        // Verify order
        let tiers: Vec<_> = collected.iter().map(|(_, t)| *t).collect();
        let expected = [
            PeerTier::CellHub, PeerTier::CellHub, PeerTier::CellHub,
            PeerTier::RegionHub, PeerTier::RegionHub,
            PeerTier::Leaf, PeerTier::Leaf,
        ];
        assert_eq!(tiers, expected);
    }

    #[test]
    fn test_hierarchical_selection_is_empty() {
        let router = RendezvousRouter::new(3);
        let empty: Vec<Peer> = Vec::new();

        let selection = router.select_peers_hierarchical("test", &empty);

        assert!(selection.is_empty());
        assert_eq!(selection.total_count(), 0);
    }

    // =========================================================================
    // Allowlist Filtering Tests
    // =========================================================================

    #[test]
    fn test_filtered_no_allowlist() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();
        let allowlists = TierAllowlists::default();

        let selection = router.select_peers_hierarchical_filtered("test", &peers, &allowlists);

        // No restrictions = all peers included
        assert_eq!(selection.total_count(), 7);
    }

    #[test]
    fn test_filtered_blocks_cell_hubs() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();

        // Block all cell hubs by allowing only a non-matching pattern
        let allowlists = TierAllowlists {
            cell_hubs: Some(vec!["http://nonexistent:9999".to_string()]),
            region_hubs: None,
            leaves: None,
        };

        let selection = router.select_peers_hierarchical_filtered("test", &peers, &allowlists);

        // Cell hubs blocked
        assert!(selection.cell_hubs.is_empty());
        // Region hubs and leaves unchanged
        assert_eq!(selection.region_hubs.len(), 2);
        assert_eq!(selection.leaves.len(), 2);
    }

    #[test]
    fn test_filtered_allows_specific_region() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();

        // Allow only region1
        let allowlists = TierAllowlists {
            cell_hubs: None,
            region_hubs: Some(vec!["https://region1:7891".to_string()]),
            leaves: None,
        };

        let selection = router.select_peers_hierarchical_filtered("test", &peers, &allowlists);

        // Only region1 allowed
        assert_eq!(selection.region_hubs.len(), 1);
        assert!(selection.region_hubs[0].endpoint.contains("region1"));
        // Other tiers unchanged
        assert_eq!(selection.cell_hubs.len(), 3);
        assert_eq!(selection.leaves.len(), 2);
    }

    #[test]
    fn test_filtered_port_wildcard() {
        let router = RendezvousRouter::new(10);

        // Create peers with different ports
        let peers = vec![
            Peer::from_endpoint("http://cell:7891", PeerTier::CellHub).unwrap(),
            Peer::from_endpoint("http://cell:8080", PeerTier::CellHub).unwrap(),
            Peer::from_endpoint("http://cell:443", PeerTier::CellHub).unwrap(),
        ];

        // Allow any port on 'cell'
        let allowlists = TierAllowlists {
            cell_hubs: Some(vec!["http://cell:*".to_string()]),
            ..Default::default()
        };

        let selection = router.select_peers_hierarchical_filtered("test", &peers, &allowlists);

        // All cell peers should match
        assert_eq!(selection.cell_hubs.len(), 3);
    }

    #[test]
    fn test_filtered_subdomain_wildcard() {
        let router = RendezvousRouter::new(10);

        // Create region hubs with subdomains
        let peers = vec![
            Peer::from_endpoint("https://west.conary.io:7891", PeerTier::RegionHub).unwrap(),
            Peer::from_endpoint("https://east.conary.io:7891", PeerTier::RegionHub).unwrap(),
            Peer::from_endpoint("https://other.domain.io:7891", PeerTier::RegionHub).unwrap(),
        ];

        // Allow *.conary.io
        let allowlists = TierAllowlists {
            region_hubs: Some(vec!["https://*.conary.io:7891".to_string()]),
            ..Default::default()
        };

        let selection = router.select_peers_hierarchical_filtered("test", &peers, &allowlists);

        // Only conary.io subdomains allowed
        assert_eq!(selection.region_hubs.len(), 2);
        for peer in &selection.region_hubs {
            assert!(peer.endpoint.contains("conary.io"));
        }
    }

    #[test]
    fn test_filtered_ordered_convenience() {
        let router = RendezvousRouter::new(10);
        let peers = make_mixed_peers();

        // Block leaves
        let allowlists = TierAllowlists {
            leaves: Some(vec!["http://nonexistent:9999".to_string()]),
            ..Default::default()
        };

        let ordered = router.select_peers_ordered_filtered("test", &peers, &allowlists);

        // Leaves should be absent
        assert_eq!(ordered.len(), 5); // 3 cell + 2 region
        for peer in &ordered {
            assert_ne!(peer.tier, PeerTier::Leaf);
        }
    }
}
