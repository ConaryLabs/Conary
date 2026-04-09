# Phase 4: Substituter Remote Sources Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the repository-level substituter chain actually resolve chunks from Remi and federation peers, cache successful fetches locally, and keep the legacy `Binary` source as a compatibility stub instead of pretending it can serve CAS chunks.

**Architecture:** Keep this phase inside the existing `repository::substituter` and `chunk_fetcher` shape. The chain becomes async, Remi and federation reuse `HttpChunkFetcher` plus `LocalCacheFetcher`, and federation DB work is split into small synchronous pre/post helpers around the async fetch loop so `rusqlite` never crosses an `.await`. Peer selection and health accounting stay in the existing `federation_peer` model, and we avoid inventing a new scheduler, cache index, or repository client abstraction for a subsystem that still has zero non-test `apps/` callers.

**Tech Stack:** Rust async/await, `tokio`, `reqwest`, existing `ChunkFetcher` implementations, `rusqlite`, existing repository/federation DB tables.

---

## Scope Guard

- Phase 4 only covers `crates/conary-core/src/repository/substituter.rs` and the minimum supporting code needed to make Remi/federation chunk resolution real.
- No new CLI surface and no README marketing changes.
- No new repository service, peer-discovery daemon, or automation integration.
- No `Binary`-repo chunk fetching path; binary repos continue to work only at package-resolution/download level, and the substituter keeps `Binary` only so legacy config still deserializes cleanly.
- Keep federation peer selection simple and DB-backed for now. Do not pull `apps/remi` routing/circuit-breaker runtime into `conary-core`.
- Do not add external-network CI requirements. Unit/integration coverage should use local async test servers; any real Remi smoke is manual and non-gating.

## File Map

| File | Responsibility in Phase 4 |
|------|----------------------------|
| `crates/conary-core/src/repository/substituter.rs` | Asyncify the chain, keep `Binary` as compat stub, implement Remi + federation chunk fetch, update tests |
| `crates/conary-core/src/db/models/federation_peer.rs` | Add ordered enabled-peer lookup and success/failure stat update helpers for chunk fetches |
| `crates/conary-core/src/repository/mod.rs` | Re-export changes if async signatures require small doc/test updates |
| `crates/conary-core/src/repository/remi.rs` | Modify only if a tiny shared endpoint helper is extracted; do not duplicate `HttpChunkFetcher` URL composition |

## Chunk 1: Async Chain + Federation Peer Helpers

### Task 1: Convert `SubstituterChain` to the real async shape and keep `Binary` as a compatibility stub

**Files:**
- Modify: `crates/conary-core/src/repository/substituter.rs`
- Modify if needed: `crates/conary-core/src/repository/mod.rs`
- Test: `crates/conary-core/src/repository/substituter.rs`

- [ ] **Step 1: Write failing async substituter tests**

Convert the current sync-only tests into async coverage that proves the public API shape we actually want:

```rust
#[tokio::test]
async fn test_local_cache_resolve_async() {}

#[tokio::test]
async fn test_resolve_chunks_batch_async() {}

#[tokio::test]
async fn test_empty_chain_async() {}

#[test]
fn test_legacy_binary_source_still_deserializes() {}

#[tokio::test]
async fn test_binary_source_is_ignored_for_chunk_resolution() {}
```

Keep the existing local-cache behavior assertions, but make them exercise the async methods.

- [ ] **Step 2: Keep `Binary` in `SubstituterSource`, but make it explicit compatibility-only behavior**

Do **not** delete:

```rust
Binary { base_url: String }
```

from the enum. Existing user config can already deserialize `type = "binary"`, and Phase 4 should not turn that into a startup-time parse failure.

Instead:
- keep `name()` returning `"binary"`
- keep serde compatibility coverage
- make the runtime branch immediately return `NotFound` with a clear “binary sources do not serve individual chunks” message
- update tests so legacy binary entries deserialize but never count as a successful chunk source

- [ ] **Step 3: Make the chain async without threading `rusqlite::Connection` through awaited code**

Change the main API to:

```rust
pub type PreparedFederationPeers = HashMap<String, Vec<FederationPeer>>;

pub struct PeerFetchMetric {
    pub peer_id: String,
    pub latency_ms: i64,
    pub succeeded: bool,
}

pub async fn resolve_chunk(
    &self,
    hash: &str,
    federation_peers: Option<&PreparedFederationPeers>,
) -> Result<(Vec<u8>, SubstituterResult)>;

pub async fn resolve_chunks(
    &self,
    hashes: &[String],
    federation_peers: Option<&PreparedFederationPeers>,
) -> Result<SubstituterBatchResult>;
```

where `SubstituterResult` / `SubstituterBatchResult` carry peer fetch metrics back to the synchronous caller, and make `fetch_from_source(...)` async as well.

Also add two tiny sync helpers in `substituter.rs`:

```rust
pub fn prepare_federation_peers(
    &self,
    conn: &rusqlite::Connection,
) -> Result<PreparedFederationPeers>;

pub fn apply_peer_metrics(
    conn: &rusqlite::Connection,
    metrics: &[PeerFetchMetric],
) -> Result<()>;
```

Important constraint: federation peer lookup and metric persistence happen outside the async fetch loop. The async path consumes owned peer data and emits owned telemetry; no `rusqlite::Connection` borrow crosses an `.await`.

- [ ] **Step 4: Keep the existing batch-fallthrough algorithm, just async**

Preserve the current “try each source in order, keep a `remaining` set, stop when resolved” structure in `resolve_chunks()`.

Do **not** add a new parallel scheduler or a second batch-resolution abstraction in this phase. A simple async conversion is enough because the chain still has zero production callers.

- [ ] **Step 5: Update tests and any re-export fallout**

Fix the tests and `repository/mod.rs` only as far as the async signature change requires. There are no non-test call sites to migrate in `apps/`.

- [ ] **Step 6: Verify**

Run: `cargo test -p conary-core repository::substituter::tests`

Expected: local-cache async behavior passes, legacy `Binary` config still deserializes, and binary sources never pretend to resolve individual chunks.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/repository/substituter.rs crates/conary-core/src/repository/mod.rs
git commit -m "feat(substituter): asyncify chain and preserve binary compat"
```

### Task 2: Add federation-peer query and stat-update helpers

**Files:**
- Modify: `crates/conary-core/src/db/models/federation_peer.rs`
- Test: `crates/conary-core/src/db/models/federation_peer.rs`

- [ ] **Step 1: Write failing federation-peer helper tests**

Add tests that pin the exact DB behavior the substituter needs:

```rust
#[test]
fn test_list_enabled_for_tier_orders_by_latency_then_success() {}

#[test]
fn test_list_enabled_for_tier_skips_disabled_peers() {}

#[test]
fn test_record_success_updates_latency_and_resets_consecutive_failures() {}

#[test]
fn test_record_failure_increments_failure_counters() {}
```

- [ ] **Step 2: Add ordered peer lookup**

Add a helper like:

```rust
pub fn list_enabled_for_tier(conn: &Connection, tier: &str) -> Result<Vec<FederationPeer>>;
```

with SQL ordering that matches the current Phase 4 design:
- `is_enabled = 1`
- requested `tier`
- `latency_ms ASC`
- `success_count DESC`
- stable tie-break on `endpoint`

Do **not** hide the `consecutive_failures > 5` rule in SQL yet; keep that threshold in the substituter so the fetch loop owns the circuit-breaker decision.

- [ ] **Step 3: Add success/failure update helpers**

Add:

```rust
pub fn record_success(conn: &Connection, id: &str, latency_ms: i64) -> Result<()>;
pub fn record_failure(conn: &Connection, id: &str) -> Result<()>;
```

Behavior:
- success increments `success_count`
- success resets `consecutive_failures` to `0`
- success stores the latest observed `latency_ms`
- failure increments `failure_count`
- failure increments `consecutive_failures`

Keep this simple and Conary-shaped. No health-score subsystem, EMA, or new peer-health table.

- [ ] **Step 4: Verify**

Run: `cargo test -p conary-core federation_peer`

Expected: lookup ordering and counter updates are covered in-memory via SQLite tests.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/db/models/federation_peer.rs
git commit -m "feat(federation): add substituter peer lookup helpers"
```

## Chunk 2: Remi + Federation Source Resolution

### Task 3: Implement Remi chunk fetching and local-cache population

**Files:**
- Modify: `crates/conary-core/src/repository/substituter.rs`
- Modify if chosen: `crates/conary-core/src/repository/remi.rs`
- Test: `crates/conary-core/src/repository/substituter.rs`

- [ ] **Step 1: Write failing Remi-source tests with a local async HTTP server**

Add tests that start a tiny local server serving `/v1/chunks/{hash}` and then prove:

```rust
#[tokio::test]
async fn test_remi_source_fetches_chunk_and_populates_cache() {}

#[tokio::test]
async fn test_resolve_chunk_uses_cached_copy_after_remi_hit() {}
```

Do not depend on external network or `remi.conary.io` for automated coverage.

- [ ] **Step 2: Implement the `Remi` source branch**

In `fetch_from_source(...)`:
1. build the chunk endpoint from `endpoint.trim_end_matches('/')`
2. construct `HttpChunkFetcher` with the bare `endpoint`
3. fetch via `HttpChunkFetcher::fetch(hash)`
4. on success, write the chunk into the first configured local cache source using `LocalCacheFetcher::store()`
5. return the bytes

If you prefer not to inline endpoint normalization, extract a tiny shared helper in `repository/remi.rs`; do **not** expose the whole `RemiClientCore` just for this, and do **not** append `/v1/chunks/{hash}` yourself because `HttpChunkFetcher` already does that.

- [ ] **Step 3: Add a small local-cache write helper inside the substituter**

Add a helper in `substituter.rs` that finds the first `LocalCache` source and stores fetched bytes there:

```rust
async fn cache_remote_hit(&self, hash: &str, data: &[u8]) -> Result<()>;
```

Use it for Remi and federation so cache-write behavior stays in one place.

- [ ] **Step 4: Keep error behavior honest**

If the HTTP fetch fails, return a normal `Error::NotFound` / `Error::DownloadError` path and let the chain continue to the next source. Do not treat cache-write failure as success; log context and return an error so the caller sees the broken remote path.

- [ ] **Step 5: Verify**

Run: `cargo test -p conary-core repository::substituter::tests::test_remi_source_fetches_chunk_and_populates_cache -- --nocapture`

Expected: the fetched chunk is returned and also becomes visible through the local-cache source on the next lookup.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/repository/substituter.rs crates/conary-core/src/repository/remi.rs
git commit -m "feat(substituter): fetch chunks from remi sources"
```

### Task 4: Implement federation peer fallback and stat updates

**Files:**
- Modify: `crates/conary-core/src/repository/substituter.rs`
- Modify: `crates/conary-core/src/db/models/federation_peer.rs`
- Test: `crates/conary-core/src/repository/substituter.rs`

- [ ] **Step 1: Write failing federation-source tests**

Add local-server + in-memory-DB tests for the important fallback behavior:

```rust
#[tokio::test]
async fn test_federation_source_skips_disabled_and_open_circuit_peers() {}

#[tokio::test]
async fn test_federation_source_falls_through_after_failed_peer() {}

#[tokio::test]
async fn test_federation_source_records_success_metrics_and_caches_chunk() {}

#[tokio::test]
async fn test_federation_source_without_prepared_peers_is_skipped() {}
```

Use three seeded peers:
- one disabled
- one enabled but with `consecutive_failures > 5`
- one healthy peer serving the chunk

- [ ] **Step 2: Prepare federation peer inputs synchronously before awaiting**

Add/implement `prepare_federation_peers(...)` so the caller can:
1. inspect the chain for `Federation { tier }` sources
2. call `federation_peer::list_enabled_for_tier(...)` for each distinct tier
3. build an owned `PreparedFederationPeers`
4. pass that map into the async `resolve_chunk()` / `resolve_chunks()` call

Inside the async `Federation` branch, missing prepared peers should return `NotFound` with a clear “federation source requires prepared peer data” message. This keeps `rusqlite` fully out of the async portion of the fetch loop.

- [ ] **Step 3: Implement ordered peer fallback**

For each candidate peer:
1. skip if `peer.consecutive_failures > 5`
2. construct `HttpChunkFetcher` with the bare `peer.endpoint`
3. fetch via `HttpChunkFetcher::fetch(hash)`
4. measure latency with `Instant::now()`
5. on success:
   - append a success `PeerFetchMetric`
   - cache the chunk locally
   - return the bytes immediately
6. on failure:
   - append a failure `PeerFetchMetric`
   - continue to the next peer

After the async resolution returns, the synchronous caller applies those metrics with `apply_peer_metrics(...)`, which in turn uses `record_success(...)` / `record_failure(...)`. Do not import `apps/remi` router/circuit types into core. This phase should stay DB-model + HTTP-fetcher based.

- [ ] **Step 4: Keep `resolve_chunks()` behavior conservative**

When `resolve_chunks()` uses a federation source, let it reuse the same per-hash federation fallback path and accumulate peer metrics into the batch result. Do not add a new cross-peer batch fanout layer in this phase.

- [ ] **Step 5: Verify**

Run: `cargo test -p conary-core repository::substituter::tests::test_federation_source_falls_through_after_failed_peer -- --nocapture`

Run: `cargo test -p conary-core repository::substituter::tests::test_federation_source_records_success_metrics_and_caches_chunk -- --nocapture`

Expected: peer ordering, failure fallback, emitted peer metrics, synchronous stat updates, and local cache population all behave as designed.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/repository/substituter.rs crates/conary-core/src/db/models/federation_peer.rs
git commit -m "feat(substituter): resolve chunks from federation peers"
```

## Chunk 3: Final Verification

### Task 5: Verification and cleanup

**Files:**
- Modify if needed: `crates/conary-core/src/repository/substituter.rs`
- Modify if needed: `crates/conary-core/src/db/models/federation_peer.rs`

- [ ] **Step 1: Run focused repository test coverage**

Run:

```bash
cargo test -p conary-core repository::substituter::tests
cargo test -p conary-core federation_peer
```

Expected: new async substituter and peer-model tests pass.

- [ ] **Step 2: Run broader package verification**

Run:

```bash
cargo test -p conary-core
cargo fmt --check
cargo clippy -p conary-core -- -D warnings
```

Expected: the repository package remains green and lint-clean after the async API conversion.

- [ ] **Step 3: Optional manual smoke (non-gating)**

If you want a reality check beyond unit tests, run a manual local smoke with a temporary HTTP server serving one known chunk and confirm:
- `resolve_chunk(..., None)` hits `LocalCache`
- `resolve_chunk(..., None)` hits `Remi`
- `prepare_federation_peers(&conn)` + `resolve_chunk(..., Some(&prepared))` can fall through federation peers, then `apply_peer_metrics(&conn, ...)` persists the counters

Do not make external-network access a required CI or completion gate for Phase 4.

- [ ] **Step 4: Final commit**

```bash
git add crates/conary-core/src/repository/substituter.rs crates/conary-core/src/db/models/federation_peer.rs crates/conary-core/src/repository/mod.rs crates/conary-core/src/repository/remi.rs
git commit -m "test(substituter): verify remote source resolution"
```
