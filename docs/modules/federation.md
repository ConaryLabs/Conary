---
last_updated: 2026-03-28
revision: 2
summary: Document current federation trust controls, peer identity, discovery rules, and CLI peer management
---

# Federation Module (apps/remi/src/federation/)

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
        +-- Fetch chunk via LAN HTTP or HTTPS with pinned TLS identity
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
| `Federation` | mod.rs | Main coordinator -- routing, coalescing, circuit breakers, trust checks |
| `FederatedChunkFetcher` | mod.rs | ChunkFetcher trait impl: local cache, federation, fallback |
| `Peer` | peer.rs | Federation node (endpoint, tier, certificate-bound identity, score) |
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

## Trust Model

- **HTTPS peers** use a pinned SHA-256 TLS certificate fingerprint as their `PeerId`
- **HTTP peers** use a hash of the endpoint URL and are intended for trusted LAN scopes
- **mDNS-discovered peers** are only admitted when an allowlist is configured or authenticated transport is available
- **Chunk manifests** are signed and fetched chunks are hash-verified before use

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

Federation is Remi-owned server functionality.
It extends the ChunkFetcher trait used by the Remi app, adding a
peer layer between local CAS and upstream origin. Manifests reuse
the CCS Ed25519 signing infrastructure, while peer admission layers on
allowlists, TLS pinning, and optional mDNS discovery.

## CLI Notes

The local CLI peer-management path now follows the same identity rule as the
server admin API:

```bash
conary federation add-peer https://peer.example:7891 \
  --tier cell_hub \
  --tls-fingerprint 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

HTTPS peers require `--tls-fingerprint`; HTTP peers cannot use one and remain
appropriate only for explicitly trusted LAN scopes.

See also: [docs/ARCHITECTURE.md](/docs/ARCHITECTURE.md).
