---
last_updated: 2026-03-20
revision: 2
summary: Phase 5 substituters and community sharing — derivation cache protocol, seed registry, profile publishing, cache populate
---

# Bootstrap v2 Phase 5: Substituters & Community Sharing

## Overview

Phase 5 makes builds fast for everyone. Phases 1-3 built the derivation
engine, Phase 4 added developer experience. Phase 5 adds remote caching so
that pre-built derivation outputs can be shared between builders. A fresh
machine can fetch 110/114 packages from cache and build only 4 locally —
turning hours into minutes.

The design reuses existing CAS/chunk infrastructure for file transfer. The
derivation substituter adds a thin query layer: "do you have output for
derivation X?" -> manifest -> diff against local CAS -> fetch missing objects.

**Design date:** 2026-03-20

## Prerequisites

- Derivation engine core (Phase 1) -- complete
- EROFS composition & layered builds (Phase 2) -- complete
- Stage pipeline & profile generation (Phase 3) -- complete
- Developer experience (Phase 4) -- complete
- Existing infrastructure: SubstituterChain, R2Store, CasStore,
  DerivationIndex

## 1. Derivation Substituter (Client)

### Problem

The derivation pipeline always builds from source on cache miss. If another
builder already built the same derivation (same inputs, same hash), there's
no way to reuse their output. Every builder repeats the same work.

### Design

New module: `conary-core/src/derivation/substituter.rs`

`DerivationSubstituter` queries remote peers for pre-built derivation
outputs. It has its own HTTP query protocol, separate from the runtime chunk
SubstituterChain.

**Query flow:**

1. Pipeline calls `substituter.query(derivation_id)` before building.
2. Substituter tries configured peers in priority order.
3. On hit: peer returns `OutputManifest` (file list with CAS hashes).
4. Client diffs manifest against local CAS to find missing objects.
5. Missing CAS objects are fetched via the existing chunk fetch path
   (SubstituterChain / R2 / federation chunk protocol).
6. Manifest is recorded in the local `DerivationIndex` as a cache hit.
7. Pipeline skips the build entirely.

**Peer configuration:**

Peers are configured via TOML and seeded into the `substituter_peers` DB
table on first load. The DB is authoritative at runtime (following the
project's database-first principle). The TOML provides initial values;
the DB tracks runtime state (success/failure counts, last_seen).

```toml
[substituters]
sources = [
    "https://cache.conary.io",
    "https://builds.mycompany.com",
]
publish = "https://cache.conary.io"
publish_token = "bearer-token-here"
```

On startup, the substituter reads `[substituters] sources` from config.
For each endpoint not already in the `substituter_peers` table, it inserts
a row with default priority. Existing rows are not overwritten -- their
runtime-accumulated state (success/failure counts) is preserved. The DB
is queried for the active peer list at query time.

**Key types:**

```rust
/// Client for querying remote derivation caches.
pub struct DerivationSubstituter {
    /// HTTP client (shared).
    client: reqwest::Client,
    /// Per-peer failure tracking (simple counter, not federation CircuitBreakerRegistry).
    peer_state: HashMap<String, PeerHealth>,
}

/// Per-peer health tracking (lightweight, lives in conary-core).
struct PeerHealth {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
}

pub struct SubstituterPeer {
    pub endpoint: String,
    pub priority: u32,
}

/// Result of a derivation cache query.
pub enum CacheQueryResult {
    /// Remote cache hit -- manifest available.
    Hit {
        manifest: OutputManifest,
        peer: String,
    },
    /// No remote peer has this derivation.
    Miss,
}
```

Note: the result type is `CacheQueryResult`, not `SubstituterResult`, to
avoid collision with the existing `SubstituterResult` in
`conary-core/src/repository/substituter.rs`.

Note: `DerivationSubstituter` implements its own simple per-peer failure
tracking (`PeerHealth`) rather than reusing the federation module's
`CircuitBreakerRegistry`, which lives in `conary-server` (behind the
`server` feature gate) and cannot be imported by `conary-core`. The
`PeerHealth` struct tracks consecutive failures and backs off
exponentially (1s, 2s, 4s, ... up to 60s) before retrying a failed peer.

**Methods:**

- `query(derivation_id: &str) -> Result<CacheQueryResult>` -- try peers
  in order (skipping backed-off peers), return first hit or Miss.
- `fetch_missing_objects(manifest: &OutputManifest, cas: &CasStore) ->
  Result<FetchReport>` -- diff manifest against local CAS, fetch missing
  objects via chunk protocol. Returns count of objects fetched and bytes
  transferred.
- `publish(derivation_id: &str, manifest: &OutputManifest, endpoint: &str,
  token: &str) -> Result<()>` -- upload manifest to a Remi endpoint with
  bearer token auth. CAS objects must already be uploaded via chunk
  protocol.
- `batch_probe(derivation_ids: &[&str]) -> Result<HashMap<String, bool>>`
  -- check multiple derivation IDs in one request via `POST /v1/derivations/probe`.

### Pipeline Integration

In `Pipeline::execute()`, before calling `executor.execute()` for a
package, insert a substituter check:

```
1. Local DerivationIndex lookup (existing -- inside executor.execute())
2. If local miss AND substituter configured:
   a. substituter.query(derivation_id)
   b. If hit: fetch_missing_objects(), record in local index, skip build
   c. If miss: build locally via executor
3. After successful build (if publish_endpoint configured):
   a. Upload CAS objects via chunk protocol
   b. substituter.publish(derivation_id, manifest, endpoint, token)
```

The substituter check happens in the pipeline, not the executor. The
executor remains focused on single-package build mechanics. The pipeline
orchestrates the "check remote before building" logic.

### New PipelineConfig Fields

```rust
pub struct PipelineConfig {
    // ... existing fields ...

    /// Substituter endpoints to query for pre-built outputs.
    pub substituter_sources: Vec<String>,

    /// Endpoint to auto-publish successful builds to. None disables.
    pub publish_endpoint: Option<String>,

    /// Bearer token for publish endpoint. Required if publish_endpoint is set.
    pub publish_token: Option<String>,
}
```

## 2. Remi Derivation Endpoints (Server)

### Endpoints

Added to the existing Remi route structure in `conary-server`.

**Publish:**

- `PUT /v1/derivations/{derivation_id}` -- accepts `OutputManifest` as
  TOML body. Stores the manifest as a CAS object and records the mapping
  in the `derivation_cache` DB table. The actual file content (CAS objects
  referenced by the manifest) must already be on the server via chunk
  upload. Auth required (bearer token).

**Query:**

- `GET /v1/derivations/{derivation_id}` -- returns `OutputManifest` as
  TOML if cached, 404 if not. No auth required (public read).
- `HEAD /v1/derivations/{derivation_id}` -- existence check (204/404).

**Batch probe:**

- `POST /v1/derivations/probe` -- accepts JSON array of derivation IDs,
  returns JSON object mapping each ID to `true`/`false`. Used by
  `cache populate` to check an entire profile in one request. No auth
  required.

```json
// Request
["a1b2c3...", "d4e5f6...", "f7a8b9..."]

// Response
{"a1b2c3...": true, "d4e5f6...": true, "f7a8b9...": false}
```

### Server Storage

The `derivation_cache` DB table maps derivation IDs to manifest CAS
hashes. The manifest TOML bytes are stored as a regular CAS object via
`CasStore::store()` -- the same mechanism used for chunk storage. CAS is
content-addressed by SHA-256 of the bytes, so profiles, manifests, and
regular chunks coexist in the same keyspace without collision risk (each
has a unique content hash).

### Auth Model

- Reads (GET, HEAD, POST probe) are public -- no auth required. The
  derivation ID is the trust boundary. If the ID matches, the output is
  interchangeable.
- Writes (PUT) require bearer token auth (same admin token system as
  existing chunk uploads).

## 3. Seed Registry

### Server Endpoints

- `PUT /v1/seeds/{seed_id}` -- upload seed metadata (TOML body). The
  EROFS image and CAS objects are uploaded separately via chunk protocol.
  Auth required.
- `GET /v1/seeds/{seed_id}` -- fetch seed metadata by exact hash.
  Returns TOML.
- `GET /v1/seeds/{seed_id}/image` -- redirect or stream the seed EROFS
  image from CAS/R2.
- `GET /v1/seeds?target=x86_64` -- list available seeds for a target
  triple. Returns JSON array:

```json
[
  {
    "seed_id": "abc123...",
    "target_triple": "x86_64-conary-linux-gnu",
    "source": "community",
    "builder": "conary 0.9.0",
    "package_count": 12,
    "verified_by_count": 3,
    "created_at": "2026-03-20T14:30:00Z"
  }
]
```

No pagination for initial implementation (seed count is expected to be
small -- tens, not thousands). Sorted by `created_at DESC`.

- `GET /v1/seeds/latest?target=x86_64` -- returns the single most recent
  community-verified seed metadata (same JSON format as a single element
  of the list endpoint).

### Client Changes

In `conary-core/src/derivation/seed.rs`:

- `Seed::fetch(endpoint: &str, seed_id: &str, target_dir: &Path) ->
  Result<Seed>` -- download seed metadata + EROFS image from a Remi
  endpoint, verify hash matches `seed_id`, store locally.
- `Seed::fetch_latest(endpoint: &str, target_triple: &str, target_dir:
  &Path) -> Result<Seed>` -- query `/v1/seeds/latest`, then fetch. For
  first-time users.

No seed publishing CLI in Phase 5. Seeds are published via the admin API
or MCP tools. The CLI focus is consuming seeds, not producing them.

## 4. Profile Publishing

### Design

Minimal CAS-only approach -- no indexing, no named profiles.

**Client:**

- `conary profile publish pinned.toml [--endpoint URL]` -- uploads the
  profile TOML to a Remi endpoint via `PUT /v1/profiles/{profile_hash}`.
  Prints the hash URL for sharing.

**Server endpoints:**

- `PUT /v1/profiles/{profile_hash}` -- store profile TOML bytes as a CAS
  object via `CasStore::store()`. The `profile_hash` in the URL is
  verified against the SHA-256 of the uploaded content (reject if
  mismatch). Auth required.
- `GET /v1/profiles/{profile_hash}` -- retrieve profile TOML from CAS.
  No auth. Returns 404 if not found.

No DB table needed -- profiles are pure CAS objects, stored and retrieved
via the same `CasStore` as chunks. The profile_hash (already computed by
`BuildProfile::compute_hash()`) serves as the CAS key. Multiple users
who generate identical profiles get the same hash.

**CLI addition** to `ProfileCommands`:

```rust
/// Publish a profile to a remote endpoint
Publish {
    /// Path to profile TOML file
    profile: String,

    /// Remi endpoint URL
    #[arg(long)]
    endpoint: Option<String>,
},
```

## 5. Cache Populate

### Design

Pre-fetches derivation outputs for air-gapped environments.

```
conary cache populate --profile pinned.toml
conary cache populate --profile pinned.toml --sources-only
conary cache populate --profile pinned.toml --full
```

**Flow:**

1. Parse the profile TOML to extract all derivation IDs across all stages.
2. Batch-probe the configured substituter (`POST /v1/derivations/probe`)
   to find which are available remotely.
3. For each available derivation: fetch the manifest, diff against local
   CAS, download missing CAS objects via chunk protocol.
4. Progress is tracked per-derivation. If interrupted (network failure,
   Ctrl-C), re-running `cache populate` with the same profile skips
   derivations whose manifests are already in the local DerivationIndex
   and whose CAS objects are already present. This provides resume
   semantics without explicit checkpoint files.
5. Report results:
   ```
   Downloaded 110/114 derivation outputs (3.2 GB)
   4 derivations will be built from source.
   ```

On partial failure (e.g., network drops after 50 of 110 derivations),
the command exits with a non-zero status and reports exactly what was
fetched and what remains:

```
Fetched 50/110 available derivation outputs (1.5 GB)
60 derivation outputs still missing. Re-run to resume.
4 derivations have no remote cache and will be built from source.
```

**`--sources-only`:** Downloads source tarballs for each recipe in the
profile. This requires that recipe files are available locally (in the
`recipes/` directory). The profile contains package names and versions;
the command loads the corresponding recipe files to get `[source] archive`
URLs and checksums. If a recipe file is not found locally, a warning is
printed and that source is skipped.

Note: `--sources-only` does NOT work with remotely-fetched profiles
alone -- it requires local recipe files. This is acceptable because the
air-gapped workflow assumes the user has the full source tree (recipes
are committed to the repository).

**`--full`:** Downloads both pre-built outputs and source tarballs.

### cache status

`conary cache status` reports:
- Local CAS directory size (bytes, human-readable)
- Number of derivations in the local DerivationIndex
- Configured substituter peers and their connectivity (reachable/unreachable,
  tested via `HEAD /v1/derivations/probe` or similar lightweight endpoint)

```
$ conary cache status
CAS directory: /var/lib/conary/cas (2.1 GB, 1,847 objects)
Cached derivations: 114
Substituter peers:
  https://cache.conary.io     [reachable] (success: 342, failures: 2)
  https://builds.mycompany.com [unreachable] (last seen: 2h ago)
```

### CLI Structure

New `Cache` subcommand group:

```rust
#[derive(Subcommand)]
pub enum CacheCommands {
    /// Pre-fetch derivation outputs for offline building
    Populate {
        /// Path to profile TOML
        #[arg(long)]
        profile: String,

        /// Download source tarballs only (not pre-built outputs)
        #[arg(long)]
        sources_only: bool,

        /// Download both pre-built outputs and source tarballs
        #[arg(long)]
        full: bool,
    },

    /// Show cache statistics and substituter peer health
    Status,
}
```

### CLI Flags on `bootstrap run`

```
--no-substituters     # skip remote cache, build everything locally
--publish             # auto-publish successful builds to configured endpoint
```

These go on the existing `Run` variant added in Phase 4.

## 6. Database Migrations

Schema v55 adds three tables. All three are created in the single
`migrate_v55` function in `conary-core/src/db/schema.rs`, following the
existing function-dispatch migration pattern. Since client and server
share the same migration system (both call `migrate()`), all tables
exist in every database. The `derivation_cache` and `seeds` tables are
only populated on the Remi server; they exist as empty tables on client
databases (harmless, consistent with the database-first principle).

```sql
-- Substituter peer registry (used by client)
CREATE TABLE IF NOT EXISTS substituter_peers (
    endpoint TEXT PRIMARY KEY,
    priority INTEGER NOT NULL DEFAULT 0,
    last_seen TEXT,
    success_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0
);

-- Derivation output cache (populated by Remi server)
CREATE TABLE IF NOT EXISTS derivation_cache (
    derivation_id TEXT PRIMARY KEY,
    manifest_cas_hash TEXT NOT NULL,
    package_name TEXT NOT NULL,
    package_version TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_derivation_cache_package
    ON derivation_cache(package_name, package_version);

-- Seed registry (populated by Remi server)
CREATE TABLE IF NOT EXISTS seeds (
    seed_id TEXT PRIMARY KEY,
    target_triple TEXT NOT NULL,
    source TEXT NOT NULL,
    builder TEXT,
    packages_json TEXT NOT NULL DEFAULT '[]',
    verified_by_json TEXT NOT NULL DEFAULT '[]',
    image_cas_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_seeds_target
    ON seeds(target_triple, created_at DESC);
```

## 7. End-to-End Flow

### First build (populating cache)

```
$ conary bootstrap run my-system.toml --publish
  Resolving manifest... 114 packages, 4 stages
  Checking substituters... 0/114 cached (fresh cache)
  Building toolchain stage... 12 packages [====] 45m
  Publishing 12 derivation outputs to packages.conary.io...
  Building foundation stage... 12 packages [====] 30m
  Publishing 12 derivation outputs...
  Building system stage... 85 packages [====] 3h
  Publishing 85 derivation outputs...
  Building customization stage... 5 packages [====] 10m
  Publishing 5 derivation outputs...
  Complete: 114 built, 0 cached. Published 114 outputs.
```

### Second build (from cache)

```
$ conary bootstrap run my-system.toml
  Resolving manifest... 114 packages, 4 stages
  Checking substituters... 114/114 cached
  Fetching 114 derivation outputs... [====] 3.2 GB, 2m
  Complete: 0 built, 114 cached. Total: 2m 15s.
```

### Air-gapped workflow

```
# On connected machine:
$ conary profile generate my-system.toml -o pinned.toml
$ conary cache populate --profile pinned.toml --full
  Downloaded 110/114 derivation outputs (3.2 GB)
  Downloaded 114 source tarballs (1.8 GB)
  4 derivations will be built from source.

# Transfer pinned.toml + CAS directory + source cache to air-gapped machine

# On air-gapped machine:
$ conary bootstrap run my-system.toml --no-substituters
  110 cached locally, 4 built from source. Total: 15m.
```

## Summary

| Component | Client (conary-core) | Server (conary-server) |
|-----------|---------------------|----------------------|
| Derivation substituter | `substituter.rs` -- query, fetch, publish | Derivation endpoints (GET/PUT/HEAD/probe) |
| Seed registry | `seed.rs` -- fetch, fetch_latest | Seed endpoints (GET/PUT/list/latest) |
| Profile publishing | `profile.rs` -- publish | Profile endpoints (GET/PUT, CAS-only) |
| Cache populate | `cache` CLI commands | (uses derivation + chunk endpoints) |
| DB migrations | All tables in v55 (shared migration) | Same migration, tables populated on server |

### Recommended Build Order

1. **DB migrations** -- foundation for everything else
2. **Remi derivation endpoints** -- server must exist before client can query
3. **Derivation substituter client** -- core fetch logic
4. **Pipeline integration** -- wire substituter into build loop
5. **Seed registry** (server + client) -- independent of substituter
6. **Profile publishing** -- small, independent
7. **Cache populate CLI** -- ties everything together
8. **Bootstrap run flags** -- `--no-substituters`, `--publish`
