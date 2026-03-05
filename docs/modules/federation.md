# Federation Module (conary-server/src/federation/)

Peer-to-peer CAS chunk distribution. Enables LAN caching, multi-tier
routing, and resilient chunk fetching across Conary nodes.

## Data Flow: Federated Chunk Fetch

```
FederatedChunkFetcher.fetch(chunk_hash)
        |
  1. Check local cache (LocalCacheFetcher)
        |  [miss]
  2. RequestCoalescer -- deduplicate concurrent requests for same hash
        |  [new request]
  3. RendezvousRouter -- select K peers via FNV-1a HRW hashing
        |  (hierarchical: cell_hubs -> region_hubs -> leaves)
        |
  For each peer (tier priority order):
        +-- CircuitBreaker check -- skip if open
        +-- Fetch chunk via HTTP (mTLS for WAN, plain for LAN)
        +-- Record success/failure -> update PeerScore (EWMA)
        +-- On failure: increment circuit breaker counter
        |
  4. Cache result locally
        |
  5. Broadcast to coalesced waiters
        |
  Fallback: upstream origin if all peers fail
```

## Key Types

| Type | File | Purpose |
|------|------|---------|
| `Federation` | mod.rs | Main coordinator -- routing, coalescing, circuit breakers, mTLS |
| `FederatedChunkFetcher` | mod.rs | ChunkFetcher trait impl: local cache, federation, fallback |
| `Peer` | peer.rs | Federation node (endpoint, tier, score, timestamps) |
| `PeerScore` | peer.rs | EWMA latency, success rate, bandwidth, failure count |
| `PeerRegistry` | peer.rs | HashMap-based peer collection with tier filtering |
| `PeerTier` | config.rs | RegionHub (WAN/mTLS), CellHub (LAN), Leaf |
| `FederationConfig` | config.rs | Full config with defaults for all parameters |
| `RendezvousRouter` | router.rs | Deterministic K-peer selection via FNV-1a hashing |
| `HierarchicalSelection` | router.rs | Grouped peer lists maintaining tier priority |
| `CircuitBreaker` | circuit.rs | Per-peer state machine: Closed, Open (jitter cooldown), HalfOpen |
| `CircuitBreakerRegistry` | circuit.rs | DashMap-based lock-free registry |
| `RequestCoalescer` | coalesce.rs | Singleflight pattern via broadcast channels |
| `FederationManifest` | manifest.rs | Signed resource descriptor (chunks, Ed25519 signature) |
| `ManifestTrustPolicy` | manifest.rs | Verification rules (trusted keys, allow_unsigned) |
| `MdnsDiscovery` | mdns.rs | LAN peer auto-discovery via `_conary-cas._tcp.local.` |

## Routing

Rendezvous (highest random weight) hashing deterministically maps each
chunk to K peers. No global state or coordination needed -- any node
computes the same peer list for the same chunk hash. Peers are partitioned
by tier before selection, so LAN cell hubs are always tried first.

## Circuit Breakers

Per-peer state machine prevents cascading failures:
- **Closed**: normal operation, count consecutive failures
- **Open**: requests blocked during jitter-based cooldown
- **HalfOpen**: one probe request; success closes, failure reopens

Jitter prevents synchronized retry storms across clients.

## Request Coalescing

Singleflight pattern: when multiple tasks request the same chunk
concurrently, only one network request is made. Others subscribe to a
broadcast channel and receive the cached result. Reduces bandwidth
during fleet-wide simultaneous updates.

## Architecture Context

Federation is server-side (feature-gated behind `--features server`).
It extends the ChunkFetcher trait used by the Remi server, adding a
peer layer between local CAS and upstream origin. Manifests reuse
the CCS Ed25519 signing infrastructure.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
