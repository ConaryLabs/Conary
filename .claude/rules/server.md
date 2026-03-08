---
paths:
  - "conary-server/**"
---

# Server Crate (conary-server)

Requires `--features server` to build. Contains three major subsystems:
Remi (CCS package server), federation (cross-machine CAS sharing), and
conaryd (local daemon with REST API).

## Remi Server Key Types
- `ServerConfig` -- bind address, db/chunk/cache paths, LRU eviction thresholds
- `RemiConfig` -- full configuration from file
- `ConversionService` -- on-demand RPM/DEB/Arch to CCS conversion
- `ChunkCache` -- LRU cache with `cache_max_bytes` eviction (default 700GB)
- `ChunkBloomFilter` -- fast negative lookups for DoS protection
- `NegativeCache` -- caches "not found" responses
- `JobManager` / `ConversionJob` -- async conversion job queue
- `ServerMetrics` / `MetricsSnapshot` -- observability
- `SearchEngine` -- full-text package search
- `R2Store` -- Cloudflare R2 object storage backend

## Federation Key Types
- `RendezvousRouter` -- deterministic K-peer selection (not Bloom filters)
- `CircuitBreaker` / `CircuitBreakerRegistry` -- per-peer failure tracking
- `RequestCoalescer` -- singleflight pattern for deduplication
- `FederationConfig` / `PeerTier` -- leaf, cell hub, or region hub
- `MdnsDiscovery` -- local peer discovery via mDNS

## Daemon (conaryd) Key Types
- `DaemonClient` -- CLI-to-daemon forwarding
- `AuthChecker` / `PeerCredentials` -- Unix socket auth
- `OperationQueue` / `DaemonJob` -- job queue with priority
- `AuditLogger` / `AuditEntry` -- security audit trail

## Invariants
- Remi proxies through Cloudflare for metadata, serves chunks directly
- Use `spawn_blocking` for SQLite operations in async context
- Federation hierarchy: leaf -> cell hub (LAN) -> region hub (WAN, mTLS)
- Daemon holds exclusive write lock for package operations (`SystemLock`)
- CLI checks for running daemon via `should_forward_to_daemon()`

## Gotchas
- `AsyncRemiClient` in conary-core is feature-gated, not in conary-server
- `chunk_fetcher` module in conary-core is also feature-gated
- Server uses axum framework with tokio async runtime
- `lite.rs` provides a lightweight proxy mode (`ProxyConfig`, `run_proxy`)
- `prewarm.rs` pre-populates cache on startup

## Files
- `server/` -- Remi server (routes, handlers, bloom, cache, conversion, jobs, self-update)
- `federation/` -- CAS federation (router, circuit breaker, coalescer, mDNS, peer)
- `daemon/` -- conaryd (routes, handlers, auth, jobs, lock, socket, systemd)
