# Bootstrap v2 Phase 5: Substituters & Community Sharing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add remote derivation caching so pre-built outputs can be shared between builders, turning hours-long from-source builds into minutes-long cache fetches.

**Architecture:** A `DerivationSubstituter` client in conary-core queries Remi server endpoints for pre-built derivation outputs by derivation ID. On hit, only the OutputManifest is transferred — the client diffs against its local CAS and fetches only missing objects via the existing chunk protocol. The pipeline integrates substituter queries before building, and optionally auto-publishes after building. Seed registry, profile publishing, and cache populate provide the surrounding infrastructure for sharing and air-gapped use.

**Tech Stack:** Rust 1.94, rusqlite (migrations), Axum (server handlers), reqwest (client HTTP), serde (serialization), existing CAS/chunk infrastructure

**Spec:** `docs/superpowers/specs/2026-03-20-bootstrap-v2-phase5-substituters-sharing.md` (revision 2)

---

## File Structure

### New files

| File | Purpose |
|------|---------|
| `conary-core/src/derivation/substituter.rs` | `DerivationSubstituter` client — query, fetch, publish |
| `conary-server/src/server/handlers/derivations.rs` | Remi derivation endpoints (GET/PUT/HEAD/probe) |
| `conary-server/src/server/handlers/seeds.rs` | Remi seed registry endpoints |
| `conary-server/src/server/handlers/profiles.rs` | Remi profile CAS endpoints |
| `src/cli/cache.rs` | `CacheCommands` CLI definitions |
| `src/commands/cache.rs` | `cmd_cache_populate`, `cmd_cache_status` handlers |

### Modified files

| File | Change |
|------|--------|
| `conary-core/src/db/migrations.rs` | Add `migrate_v55` (3 new tables) |
| `conary-core/src/db/schema.rs` | Bump `SCHEMA_VERSION` to 55, add dispatch case |
| `conary-core/src/derivation/mod.rs` | Add `pub mod substituter;` |
| `conary-core/src/derivation/pipeline.rs` | Add substituter query before build, auto-publish after |
| `conary-core/src/derivation/seed.rs` | Add `Seed::fetch()`, `Seed::fetch_latest()` |
| `conary-core/src/derivation/profile.rs` | Add `publish()` method |
| `conary-server/src/server/routes.rs` | Register derivation, seed, profile routes |
| `conary-server/src/server/handlers/mod.rs` | Export new handler modules |
| `src/cli/mod.rs` | Add `Cache(CacheCommands)` variant, `Publish` to `ProfileCommands` |
| `src/cli/bootstrap.rs` | Add `--no-substituters`, `--publish` to `Run` variant |
| `src/commands/mod.rs` | Add cache module, export new commands |
| `src/commands/bootstrap/mod.rs` | Update `BootstrapRunOptions` with substituter fields |
| `src/main.rs` | Wire Cache and Profile::Publish dispatch |

---

## Task 1: Database Migration v55

**Files:**
- Modify: `conary-core/src/db/migrations.rs`
- Modify: `conary-core/src/db/schema.rs`

- [ ] **Step 1: Add migrate_v55 function**

In `conary-core/src/db/migrations.rs`, add after `migrate_v54`:

```rust
pub fn migrate_v55(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS substituter_peers (
            endpoint TEXT PRIMARY KEY,
            priority INTEGER NOT NULL DEFAULT 0,
            last_seen TEXT,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS derivation_cache (
            derivation_id TEXT PRIMARY KEY,
            manifest_cas_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_derivation_cache_package
            ON derivation_cache(package_name, package_version);

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
        ",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Update schema.rs**

In `conary-core/src/db/schema.rs`:
- Change `SCHEMA_VERSION` from `54` to `55` (type is `i32`, not `i64`)
- Add dispatch case `55 => migrations::migrate_v55(conn),` in the `apply_migration` match

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core db::schema -- --nocapture`
Run: `cargo test -p conary-core db -- --nocapture`

- [ ] **Step 4: Commit**

```
feat(db): add v55 migration for substituters, derivation cache, seeds

Three new tables: substituter_peers (client peer registry),
derivation_cache (server derivation output index), seeds (server
seed registry). All created unconditionally via shared migration
system.
```

---

## Task 2: Remi Derivation Endpoints (Server)

**Files:**
- Create: `conary-server/src/server/handlers/derivations.rs`
- Modify: `conary-server/src/server/handlers/mod.rs`
- Modify: `conary-server/src/server/routes.rs`

Requires: `cargo build --features server` for server code.

- [ ] **Step 1: Create derivations handler module**

Create `conary-server/src/server/handlers/derivations.rs` with four handlers following the existing Axum pattern (see `chunks.rs` for reference):

```rust
// conary-server/src/server/handlers/derivations.rs

//! Derivation cache endpoints -- query and publish pre-built derivation outputs.

use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use parking_lot::RwLock;
use crate::server::state::ServerState;
use super::run_blocking;
```

**`get_derivation`** — `GET /v1/derivations/{derivation_id}`:
- Look up `derivation_id` in `derivation_cache` table
- If found, retrieve manifest from CAS via `manifest_cas_hash`
- Return manifest TOML with `Content-Type: application/toml`, or 404

**`head_derivation`** — `HEAD /v1/derivations/{derivation_id}`:
- Look up in DB, return 204 if exists, 404 if not

**`put_derivation`** — `PUT /v1/derivations/{derivation_id}`:
- Requires auth (bearer token — use existing auth middleware)
- Accept TOML body, store as CAS object
- Insert into `derivation_cache` table (derivation_id, manifest_cas_hash, package_name, package_version parsed from manifest)
- Return 201 Created

**`probe_derivations`** — `POST /v1/derivations/probe`:
- Accept JSON array of derivation ID strings
- Query DB for which exist
- Return JSON object mapping each ID to true/false

- [ ] **Step 2: Register in handlers/mod.rs**

Add `pub mod derivations;` to the handler module declarations.

- [ ] **Step 3: Add routes in routes.rs**

In the route builder function, add derivation routes. Public reads (no auth middleware), authenticated writes:

```rust
// Derivation cache (public reads + auth writes on same path)
.route("/v1/derivations/probe", post(derivations::probe_derivations))
.route("/v1/derivations/:derivation_id",
    get(derivations::get_derivation)
    .head(derivations::head_derivation)
    .put(derivations::put_derivation)
)
```

IMPORTANT: Axum panics if the same path is registered twice via `.route()`.
All methods for the same path MUST be combined in a single `.route()` call.
Auth for PUT is handled inside `put_derivation` (extract and validate bearer
token from headers), not via route-level middleware. Check how `chunks::put_chunk`
handles auth for the pattern.

- [ ] **Step 4: Add tests**

In `derivations.rs`, add tests for the probe logic:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn probe_returns_correct_availability() {
        // Create in-memory DB, run migrations
        // Insert one derivation_cache row for "aaa..."
        // Call probe logic with ["aaa...", "bbb..."]
        // Assert: aaa=true, bbb=false
    }
}
```

- [ ] **Step 5: Build and verify**

Run: `cargo build --features server`
Run: `cargo test --features server derivations -- --nocapture`
Run: `cargo clippy --features server -- -D warnings`

- [ ] **Step 6: Commit**

```
feat(server): add derivation cache endpoints

GET/HEAD/PUT /v1/derivations/{id} for querying and publishing pre-built
derivation outputs. POST /v1/derivations/probe for batch existence
checks. Reads are public, writes require bearer token auth.
```

---

## Task 3: Derivation Substituter Client

**Files:**
- Create: `conary-core/src/derivation/substituter.rs`
- Modify: `conary-core/src/derivation/mod.rs`

- [ ] **Step 1: Create substituter module with types**

Create `conary-core/src/derivation/substituter.rs`:

```rust
// conary-core/src/derivation/substituter.rs

//! Derivation substituter -- queries remote peers for pre-built outputs.
//!
//! On cache hit, returns the OutputManifest. The caller diffs against
//! local CAS and fetches missing objects via the chunk protocol.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tracing::{info, warn};

use crate::derivation::output::OutputManifest;
use crate::filesystem::CasStore;
```

Define types:

- `SubstituterError` enum (thiserror): `Http(String)`, `Parse(String)`, `Io(String)`, `NoPeers`
- `SubstituterPeer { endpoint: String, priority: u32 }`
- `PeerHealth { consecutive_failures: u32, last_failure: Option<Instant> }` (private)
- `CacheQueryResult` enum: `Hit { manifest: OutputManifest, peer: String }`, `Miss`
- `FetchReport { objects_fetched: u64, bytes_transferred: u64 }`
- `DerivationSubstituter { client: reqwest::Client, peer_state: HashMap<String, PeerHealth> }`

- [ ] **Step 2: Implement peer loading from DB**

```rust
impl DerivationSubstituter {
    /// Create a new substituter, loading peers from the DB.
    pub fn from_db(conn: &Connection) -> Result<Self, SubstituterError> {
        // Query substituter_peers table ordered by priority
        // Build peer list + empty PeerHealth entries
    }

    /// Seed peers from config into DB (insert if not exists).
    pub fn seed_peers(conn: &Connection, endpoints: &[String]) -> Result<(), SubstituterError> {
        // For each endpoint not in substituter_peers, INSERT with default priority
    }
}
```

- [ ] **Step 3: Implement query method**

```rust
    /// Query remote peers for a pre-built derivation output.
    pub async fn query(&mut self, derivation_id: &str) -> Result<CacheQueryResult, SubstituterError> {
        // For each peer (skip if backed off):
        //   GET {endpoint}/v1/derivations/{derivation_id}
        //   If 200: parse TOML body as OutputManifest, update peer success, return Hit
        //   If 404: continue to next peer
        //   If error: increment failure counter, continue
        // If all peers exhausted: return Miss
    }
```

- [ ] **Step 4: Implement fetch_missing_objects**

```rust
    /// Diff manifest against local CAS, fetch missing objects.
    pub async fn fetch_missing_objects(
        &self,
        manifest: &OutputManifest,
        cas: &CasStore,
        peer_endpoint: &str,
    ) -> Result<FetchReport, SubstituterError> {
        // For each file in manifest.files:
        //   If cas.exists(&file.hash): skip
        //   Else: GET {peer_endpoint}/v1/chunks/{file.hash}, store in CAS
        // Return FetchReport with counts
    }
```

- [ ] **Step 5: Implement publish and batch_probe**

```rust
    /// Upload a manifest to a Remi endpoint.
    pub async fn publish(
        &self,
        derivation_id: &str,
        manifest: &OutputManifest,
        endpoint: &str,
        token: &str,
    ) -> Result<(), SubstituterError> {
        // PUT {endpoint}/v1/derivations/{derivation_id}
        // Body: TOML serialization of manifest
        // Header: Authorization: Bearer {token}
    }

    /// Batch-probe multiple derivation IDs.
    pub async fn batch_probe(
        &self,
        derivation_ids: &[&str],
        endpoint: &str,
    ) -> Result<HashMap<String, bool>, SubstituterError> {
        // POST {endpoint}/v1/derivations/probe
        // Body: JSON array of IDs
        // Parse response as HashMap<String, bool>
    }
```

- [ ] **Step 6: Add PeerHealth backoff logic**

```rust
impl PeerHealth {
    fn is_backed_off(&self) -> bool {
        // Exponential backoff: 2^failures seconds, max 60s
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_failure = Some(Instant::now());
    }
}
```

- [ ] **Step 7: Register module in mod.rs**

Add `pub mod substituter;` to `conary-core/src/derivation/mod.rs`.

- [ ] **Step 8: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_health_backoff() {
        let mut health = PeerHealth { consecutive_failures: 0, last_failure: None };
        assert!(!health.is_backed_off());
        health.record_failure();
        assert!(health.is_backed_off()); // just failed, in backoff
        health.record_success();
        assert!(!health.is_backed_off()); // reset
    }

    #[test]
    fn cache_query_result_variants() {
        let miss = CacheQueryResult::Miss;
        assert!(matches!(miss, CacheQueryResult::Miss));
    }

    #[test]
    fn seed_peers_inserts_new_only() {
        // Test with in-memory DB
        // seed_peers with ["https://a.com", "https://b.com"]
        // Verify both inserted
        // seed_peers again with ["https://a.com", "https://c.com"]
        // Verify a.com not duplicated, c.com added
    }
}
```

- [ ] **Step 9: Verify**

Run: `cargo test -p conary-core derivation::substituter -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 10: Commit**

```
feat(derivation): add derivation substituter client

DerivationSubstituter queries remote peers for pre-built outputs by
derivation ID. On hit, returns OutputManifest for CAS diff and fetch.
Includes peer health tracking with exponential backoff, batch probe,
and publish methods.
```

---

## Task 4: Pipeline Integration

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs`

- [ ] **Step 1: Add substituter fields to PipelineConfig**

After the existing `cascade` field:

```rust
    /// Substituter endpoints to query for pre-built outputs.
    pub substituter_sources: Vec<String>,
    /// Endpoint to auto-publish successful builds to. None disables.
    pub publish_endpoint: Option<String>,
    /// Bearer token for publish endpoint.
    pub publish_token: Option<String>,
```

Fix ALL `PipelineConfig` construction sites to add:
```rust
substituter_sources: vec![],
publish_endpoint: None,
publish_token: None,
```

- [ ] **Step 2: Add SubstituterHit event variant**

Add to `PipelineEvent`:

```rust
    /// A package was fetched from a remote substituter.
    SubstituterHit {
        /// Package name.
        name: String,
        /// Peer that had the cache hit.
        peer: String,
        /// Number of CAS objects fetched.
        objects_fetched: u64,
    },
```

- [ ] **Step 3: Add substituter query to execute loop**

In `Pipeline::execute()`, the substituter check goes in the per-package loop, AFTER the existing `--only` filter block but BEFORE calling `executor.execute()`.

Read the current `Pipeline::execute()` method carefully. The integration point is: if the local cache misses (executor would need to build), check the remote substituter first. This requires computing the derivation ID in the pipeline (which the `--only` filter already does for non-targeted packages).

The approach: compute derivation ID for the package, check local index first (like executor does), then if miss check substituter, then if still miss call executor. This means some logic from executor moves to pipeline for the substituter path, but executor.execute() still handles the "no substituter or substituter miss" case and its internal cache check is a fast no-op if the pipeline already checked.

The implementation detail: the agent implementing this should read both executor.rs and pipeline.rs to understand the full derivation ID computation flow, then add the substituter check at the right point. The substituter is only constructed if `substituter_sources` is non-empty.

- [ ] **Step 4: Add auto-publish after successful build**

After a successful `ExecutionResult::Built`, if `publish_endpoint` and `publish_token` are configured:

```rust
// Auto-publish: upload CAS objects and manifest
if let (Some(ref endpoint), Some(ref token)) = (&self.config.publish_endpoint, &self.config.publish_token) {
    // Upload each CAS object from the manifest via chunk protocol
    // Then publish the manifest via substituter.publish()
    // Log success/failure but don't fail the pipeline on publish errors
}
```

Publishing errors should be logged as warnings, not pipeline failures.

- [ ] **Step 5: Fix all PipelineConfig construction sites**

Search: `grep -rn "PipelineConfig {" conary-core/ src/`

- [ ] **Step 6: Add tests**

```rust
#[test]
fn pipeline_config_default_has_no_substituters() {
    // Verify default PipelineConfig has empty substituter_sources
}
```

- [ ] **Step 7: Verify**

Run: `cargo test -p conary-core derivation::pipeline -- --nocapture`
Run: `cargo build`

- [ ] **Step 8: Commit**

```
feat(derivation): integrate substituter into pipeline execution

Pipeline queries remote substituters before building. On hit, fetches
manifest + missing CAS objects and skips the build. Optionally auto-
publishes successful builds to a configured endpoint.
```

---

## Task 5: Seed Registry (Server + Client)

**Files:**
- Create: `conary-server/src/server/handlers/seeds.rs`
- Modify: `conary-server/src/server/handlers/mod.rs`
- Modify: `conary-server/src/server/routes.rs`
- Modify: `conary-core/src/derivation/seed.rs`

- [ ] **Step 1: Create seeds handler module**

Create `conary-server/src/server/handlers/seeds.rs` with handlers:

**`put_seed`** — `PUT /v1/seeds/{seed_id}`: Accept TOML metadata, insert into `seeds` table. Auth required.

**`get_seed`** — `GET /v1/seeds/{seed_id}`: Return seed metadata TOML.

**`get_seed_image`** — `GET /v1/seeds/{seed_id}/image`: Look up `image_cas_hash`, retrieve from CAS, stream response.

**`list_seeds`** — `GET /v1/seeds?target=x86_64`: Query seeds table filtered by target_triple, return JSON array with fields: `seed_id`, `target_triple`, `source`, `builder`, `package_count`, `verified_by_count`, `created_at`.

**`get_latest_seed`** — `GET /v1/seeds/latest?target=x86_64`: Return most recent seed metadata (single JSON object) for target, ordered by `created_at DESC LIMIT 1`.

- [ ] **Step 2: Register routes**

Add `pub mod seeds;` to handlers/mod.rs. Add routes in routes.rs:

```rust
.route("/v1/seeds", get(seeds::list_seeds))
.route("/v1/seeds/latest", get(seeds::get_latest_seed))
.route("/v1/seeds/:seed_id", get(seeds::get_seed).put(seeds::put_seed))
.route("/v1/seeds/:seed_id/image", get(seeds::get_seed_image))
```

- [ ] **Step 3: Add Seed::fetch to client**

In `conary-core/src/derivation/seed.rs`, add:

```rust
impl Seed {
    /// Fetch a seed by exact ID from a Remi endpoint.
    pub async fn fetch(
        endpoint: &str,
        seed_id: &str,
        target_dir: &Path,
    ) -> Result<Self, SeedError> {
        // GET {endpoint}/v1/seeds/{seed_id} -> parse metadata TOML
        // GET {endpoint}/v1/seeds/{seed_id}/image -> download EROFS image
        // Verify SHA-256 of downloaded image matches seed_id
        // Save metadata + image to target_dir
        // Return Seed
    }

    /// Fetch the latest seed for a target triple.
    pub async fn fetch_latest(
        endpoint: &str,
        target_triple: &str,
        target_dir: &Path,
    ) -> Result<Self, SeedError> {
        // GET {endpoint}/v1/seeds/latest?target={target_triple} -> get seed_id
        // Delegate to Seed::fetch(endpoint, &seed_id, target_dir)
    }
}
```

Add `SeedError` variants: `HttpError(String)`, `HashMismatch { expected, actual }`, `NotFound`.

- [ ] **Step 4: Add tests**

Server tests in `seeds.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn list_seeds_filters_by_target() {
        // In-memory DB, insert two seeds (x86_64 + aarch64)
        // Query with target=x86_64, verify only x86_64 returned
    }
}
```

Client tests in `seed.rs`:

```rust
#[test]
fn seed_error_variants_exist() {
    // Verify HttpError, HashMismatch, NotFound can be constructed
    let e = SeedError::NotFound;
    assert!(e.to_string().contains("not found"));
}
```

- [ ] **Step 5: Verify**

Run: `cargo build --features server`
Run: `cargo test -p conary-core derivation::seed -- --nocapture`
Run: `cargo clippy --features server -- -D warnings`

- [ ] **Step 6: Commit**

```
feat: add seed registry server endpoints and client fetch

Server: PUT/GET /v1/seeds/{id}, GET /v1/seeds?target=, GET /v1/seeds/latest.
Client: Seed::fetch() and Seed::fetch_latest() download seed metadata
and EROFS image with hash verification.
```

---

## Task 6: Profile Publishing

**Files:**
- Create: `conary-server/src/server/handlers/profiles.rs`
- Modify: `conary-server/src/server/handlers/mod.rs`
- Modify: `conary-server/src/server/routes.rs`
- Modify: `src/cli/mod.rs` (ProfileCommands)
- Modify: `src/commands/profile.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create profiles handler module**

Create `conary-server/src/server/handlers/profiles.rs` with two handlers:

**`put_profile`** — `PUT /v1/profiles/{profile_hash}`: Accept TOML body, verify SHA-256 matches URL hash, store as CAS object. Auth required. Return 201.

**`get_profile`** — `GET /v1/profiles/{profile_hash}`: Retrieve from CAS, return TOML. 404 if not found.

- [ ] **Step 2: Register routes**

Add `pub mod profiles;` to handlers/mod.rs. Add routes in routes.rs.

- [ ] **Step 3: Add Publish variant to ProfileCommands**

In `src/cli/mod.rs`, find `ProfileCommands` enum. Add:

```rust
    /// Publish a profile to a remote endpoint
    Publish {
        /// Path to profile TOML file
        profile: String,

        /// Remi endpoint URL (defaults to configured substituter)
        #[arg(long)]
        endpoint: Option<String>,

        /// Auth token for the endpoint
        #[arg(long)]
        token: Option<String>,
    },
```

- [ ] **Step 4: Add cmd_profile_publish handler**

In `src/commands/profile.rs`, add:

```rust
pub fn cmd_profile_publish(profile_path: &str, endpoint: Option<&str>, token: Option<&str>) -> Result<()> {
    // Read profile TOML from file
    // Compute SHA-256 of content
    // PUT to {endpoint}/v1/profiles/{hash} with auth header
    // Print: "Published profile {hash} to {endpoint}"
}
```

- [ ] **Step 5: Wire CLI dispatch**

In `src/main.rs`, add the `ProfileCommands::Publish` match arm.

- [ ] **Step 6: Add tests**

Server test in `profiles.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn put_profile_rejects_hash_mismatch() {
        // Verify that uploading content whose SHA-256 doesn't match
        // the URL hash returns an error
    }
}
```

- [ ] **Step 7: Verify**

Run: `cargo build --features server`
Run: `cargo build`

- [ ] **Step 8: Commit**

```
feat: add profile publishing to Remi

Server: PUT/GET /v1/profiles/{hash} with CAS storage and hash
verification. Client: conary profile publish uploads profile TOML
and prints the hash URL.
```

---

## Task 7: Cache Populate CLI

**Files:**
- Create: `src/cli/cache.rs`
- Create: `src/commands/cache.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/commands/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create CLI definitions**

Create `src/cli/cache.rs`:

```rust
// src/cli/cache.rs
//! CLI definitions for cache commands.

use clap::Subcommand;

/// Cache management commands for derivation outputs.
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
        #[arg(long, conflicts_with = "sources_only")]
        full: bool,
    },

    /// Show cache statistics and substituter peer health
    Status,
}
```

- [ ] **Step 2: Add Cache to Commands enum**

In `src/cli/mod.rs`:
- Add `mod cache;` to module declarations
- Add `pub use cache::CacheCommands;`
- Add to `Commands` enum:

```rust
    /// Cache management for derivation outputs
    #[command(subcommand)]
    Cache(CacheCommands),
```

- [ ] **Step 3: Create command handlers**

Create `src/commands/cache.rs`:

```rust
// src/commands/cache.rs
//! Implementation of `conary cache` commands.

use std::path::Path;
use anyhow::Result;
use conary_core::derivation::profile::BuildProfile;
use conary_core::derivation::substituter::DerivationSubstituter;

/// Pre-fetch derivation outputs from remote substituters.
///
/// Uses `tokio::runtime::Runtime` to bridge the async substituter client
/// into the sync command handler (matching the pattern used elsewhere in
/// the codebase for sync CLI -> async HTTP calls).
pub fn cmd_cache_populate(profile_path: &str, sources_only: bool, full: bool) -> Result<()> {
    // 1. Read and parse profile TOML
    // 2. Extract all derivation IDs from all stages
    // 3. Create tokio runtime: let rt = tokio::runtime::Runtime::new()?;
    // 4. Load substituter config, create DerivationSubstituter
    // 5. rt.block_on(async { substituter.batch_probe(...) }) to find available
    // 6. For each available: rt.block_on(async { fetch manifest, diff CAS, download })
    // 7. If --sources-only or --full: load local recipes, download source archives
    // 8. Report results
    //
    // Note: all async substituter calls are bridged via rt.block_on().
    // The CLI command handlers are sync (matching existing codebase pattern).
}

/// Show cache statistics.
pub fn cmd_cache_status() -> Result<()> {
    // 1. Report CAS directory size
    // 2. Count derivations in DerivationIndex
    // 3. List substituter peers with health status
}
```

- [ ] **Step 4: Wire into mod.rs and main.rs**

In `src/commands/mod.rs`:
- Add `mod cache;` and `pub use cache::{cmd_cache_populate, cmd_cache_status};`

In `src/main.rs`, add to the Commands match:

```rust
Some(cli::Commands::Cache(cmd)) => match cmd {
    cli::CacheCommands::Populate { profile, sources_only, full } => {
        commands::cmd_cache_populate(&profile, sources_only, full)
    }
    cli::CacheCommands::Status => commands::cmd_cache_status(),
},
```

- [ ] **Step 5: Verify**

Run: `cargo build`
Run: `cargo run -- cache --help`
Run: `cargo run -- cache populate --help`
Run: `cargo run -- cache status --help`

- [ ] **Step 6: Commit**

```
feat: add conary cache populate and status commands

cache populate pre-fetches derivation outputs from remote substituters
for air-gapped building. cache status reports CAS size, cached
derivation count, and substituter peer health.
```

---

## Task 8: Bootstrap Run Substituter Flags

**Files:**
- Modify: `src/cli/bootstrap.rs`
- Modify: `src/commands/bootstrap/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add flags to Run variant**

In `src/cli/bootstrap.rs`, add to the `Run` variant (after `verbose`):

```rust
        /// Skip remote substituters, build everything locally
        #[arg(long)]
        no_substituters: bool,

        /// Auto-publish successful builds to configured endpoint
        #[arg(long)]
        publish: bool,
```

- [ ] **Step 2: Update BootstrapRunOptions**

In `src/commands/bootstrap/mod.rs`, add to `BootstrapRunOptions`:

```rust
    /// Skip remote substituters.
    pub no_substituters: bool,
    /// Auto-publish successful builds.
    pub publish: bool,
```

Update `cmd_bootstrap_run` to log the new options.

- [ ] **Step 3: Update CLI dispatch in main.rs**

Add the new fields to the `BootstrapCommands::Run` destructuring and pass them through to `BootstrapRunOptions`.

- [ ] **Step 4: Verify**

Run: `cargo build`
Run: `cargo run -- bootstrap run --help`
Expected: shows `--no-substituters` and `--publish` flags

- [ ] **Step 5: Commit**

```
feat(cli): add --no-substituters and --publish flags to bootstrap run

--no-substituters skips remote cache queries, forcing local builds.
--publish auto-uploads successful build outputs to the configured
substituter endpoint.
```

---

## Summary

| Task | What | Crate | Key Risk |
|------|------|-------|----------|
| 1 | DB migration v55 | conary-core | Schema version bump affects all DBs |
| 2 | Remi derivation endpoints | conary-server | Server handler patterns, auth middleware |
| 3 | Substituter client | conary-core | HTTP client, peer backoff, async |
| 4 | Pipeline integration | conary-core | Complex execute() loop, derivation ID computation |
| 5 | Seed registry | both | Server endpoints + client fetch with hash verification |
| 6 | Profile publishing | both | CAS storage semantics, hash verification |
| 7 | Cache populate CLI | conary (bin) | Wiring substituter client + profile parsing |
| 8 | Bootstrap run flags | conary (bin) | CLI flag threading |

**Dependencies:** Task 1 must be first. Tasks 2-3 must precede 4. Tasks 5-6 are independent of 2-4. Task 7 depends on 3. Task 8 depends on 4.

**Parallelization:** After Task 1, Tasks 2+3 can run in parallel (different crates). After those, Tasks 5+6 can run in parallel. Tasks 7+8 are small and sequential.
