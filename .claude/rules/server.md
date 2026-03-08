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
- `AdminSection` -- external admin API config (bind addr, Forgejo URL/token, bootstrap token, rate limits, audit retention)
- `Scope` -- typed enum (Admin, CiRead, CiTrigger, ReposRead, ReposWrite, FederationRead, FederationWrite)
- `TokenScopes` -- validated token scopes wrapper with `has_scope(Scope)` check
- `TokenName` -- authenticated token name, stored in request extensions by auth middleware
- `ServiceError` -- shared error type for admin_service layer (BadRequest, NotFound, Conflict, Internal)
- `AdminRateLimiters` -- per-IP token buckets (read 60/min, write 10/min, auth-fail 5/min) via governor crate
- `AdminEvent` -- typed event for SSE broadcast (event_type, data, timestamp)
- `RemiMcpServer` -- MCP server exposing 16 admin tools to LLM agents via rmcp
- `RepoRequest` / `RepoResponse` -- admin API repo management types
- `PeerResponse` / `AddPeerRequest` -- admin API federation peer types

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
- External admin API on :8082 requires bearer token auth (scopes: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write)
- Rate limiting on :8082: governor-based per-IP token buckets (read/write/auth-fail), configurable via [admin] section
- Audit logging on :8082: all requests logged to admin_audit_log table, request/response bodies captured for writes only
- Localhost admin :8081 bypasses auth entirely (backwards compatible)
- MCP endpoint at `/mcp` on :8082 uses Streamable HTTP transport (rmcp)
- OpenAPI spec at `/v1/admin/openapi.json` on :8082 (no auth required)
- Repo management uses existing `Repository` model CRUD (no new DB schema needed)
- Federation peer management uses `federation_peer` DB model in conary-core
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
- `server/auth.rs` -- bearer token auth middleware, token hashing/generation, scope validation
- `server/mcp.rs` -- MCP server (rmcp) exposing admin tools for LLM agents
- `server/rate_limit.rs` -- per-IP rate limiting middleware (governor) for external admin API
- `server/audit.rs` -- audit logging middleware with action derivation for external admin API
- `server/admin_service.rs` -- shared service layer (tokens, repos, federation, audit) used by handlers + MCP
- `server/forgejo.rs` -- shared Forgejo/CI client module (get, post, get_text)
- `server/routes.rs` -- axum router construction (internal :8081 + external :8082)
- `server/handlers/admin/` -- admin API handlers split into: tokens.rs, ci.rs, repos.rs, federation.rs, audit.rs, events.rs
- `server/handlers/openapi.rs` -- hand-written OpenAPI 3.1 spec for admin API
- `federation/` -- CAS federation (router, circuit breaker, coalescer, mDNS, peer)
- `daemon/` -- conaryd (routes, handlers, auth, jobs, lock, socket, systemd)
