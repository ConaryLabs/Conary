// apps/remi/src/federation/router/tests.rs

use super::*;
use crate::federation::config::PeerTier;

fn https_peer(host: &str, fingerprint: &str) -> Peer {
    Peer::from_endpoint_with_fingerprint(host, PeerTier::RegionHub, Some(fingerprint)).unwrap()
}

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
        if let Some(peer) = selected.first()
            && let Some(idx) = peers.iter().position(|p| p.id == peer.id)
        {
            counts[idx] += 1;
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
        https_peer(
            "https://region1:7891",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
        https_peer(
            "https://region2:7891",
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
        ),
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
    let cell_count = tiers
        .iter()
        .take_while(|&&t| t == PeerTier::CellHub)
        .count();
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
        PeerTier::CellHub,
        PeerTier::CellHub,
        PeerTier::CellHub,
        PeerTier::RegionHub,
        PeerTier::RegionHub,
        PeerTier::Leaf,
        PeerTier::Leaf,
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
        https_peer(
            "https://west.conary.io:7891",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        https_peer(
            "https://east.conary.io:7891",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ),
        https_peer(
            "https://other.domain.io:7891",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        ),
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
