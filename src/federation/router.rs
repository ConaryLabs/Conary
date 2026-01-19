// src/federation/router.rs
//! Rendezvous (HRW) hashing for peer selection
//!
//! Rendezvous hashing provides deterministic peer selection without requiring
//! global state synchronization. Given a chunk hash and a set of peers, any
//! node will independently compute the same K candidate peers.
//!
//! This approach was recommended by both GPT 5.2 and Gemini 3 Pro experts
//! over Bloom filters, which have O(N²) dissemination complexity.

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
}
