---
last_updated: 2026-03-21
revision: 1
summary: Full async CLI migration, reqwest 0.13, and all remaining dependency bumps to latest
---

# Async CLI + Dependency Modernization

## Overview

Modernize the entire Conary workspace to use the latest stable version of every
dependency. The blocking change is reqwest 0.13 dropping the `blocking` feature,
which requires migrating the CLI to fully async. All other dependency bumps are
tackled in the same pass.

**Design date:** 2026-03-21

## Current State

- CLI is fully synchronous: `fn main()` calls `fn run() -> Result<()>` which
  dispatches to 201 synchronous `cmd_*` functions
- 12 files use `reqwest::blocking::Client` for HTTP calls
- Server and test crates are already fully async (axum + tokio)
- 12 dependencies are behind latest major versions

## Phase 1: Async CLI Foundation

### Problem

reqwest 0.13 drops the `blocking` feature. The CLI must become async.

### Design

Make the CLI fully async using `#[tokio::main]`:

```rust
// src/main.rs
#[tokio::main]
async fn main() {
    // ... tracing setup ...
    if let Err(err) = run().await { ... }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Install { .. }) => commands::cmd_install(...).await,
        // ... all 201 dispatch arms add .await
    }
}
```

All 201 `cmd_*` functions gain the `async` keyword. This is a mechanical
transformation — add `async` to the signature, add `.await` at each call site
in main.rs.

**SQLite in async context:** rusqlite is sync-only. In the CLI, SQLite calls
run directly on the tokio thread. This is fine for a CLI (single-threaded,
no concurrent requests). The server already uses `spawn_blocking` for SQLite
and continues to do so.

**conary-core library functions** that use reqwest also become async. Their
callers (CLI commands) already await them from the Phase 1 changes. Functions
in conary-core that are called from both sync contexts (tests) and async
contexts (CLI) use the pattern:

```rust
// In conary-core: async function
pub async fn fetch_metadata(client: &reqwest::Client, url: &str) -> Result<...>

// In tests: block_on wrapper if needed
tokio::runtime::Runtime::new().unwrap().block_on(fetch_metadata(&client, url))
```

### Files Modified

- `src/main.rs` — `main()` and `run()` become async, all dispatch arms add `.await`
- `src/commands/*.rs` — all 201 `cmd_*` functions become `async fn`
- `src/commands/mod.rs` — re-exports unchanged (async fns export the same way)

### Scope

- ~60 command files in `src/commands/`
- 1 main.rs dispatcher
- Purely mechanical: add `async` keyword + `.await` at call sites

## Phase 2: reqwest Blocking -> Async

### Problem

12 files use `reqwest::blocking::Client`. These must switch to async reqwest.

### Design

Replace `reqwest::blocking::Client` with `reqwest::Client` in all 12 files.
Replace synchronous `.send()`, `.get()`, `.text()`, `.bytes()` etc. with
their async equivalents (same method names, just add `.await`).

### Files Modified

| File | Changes |
|------|---------|
| `conary-core/src/canonical/client.rs` | Client instantiation |
| `conary-core/src/derivation/substituter.rs` | Client import |
| `conary-core/src/model/remote.rs` | Client instantiation |
| `conary-core/src/repository/client.rs` | Client + Response types |
| `conary-core/src/repository/remi.rs` | Client + Response types |
| `conary-core/src/self_update.rs` | 3 Client instantiations |
| `conary-core/src/trust/client.rs` | 2 blocking::get calls |
| `src/commands/federation.rs` | Client instantiation |
| `src/commands/profile.rs` | Client instantiation |
| `src/commands/provenance.rs` | 2 Client instantiations |
| `src/commands/repo.rs` | 1 blocking::get call |
| `src/commands/self_update.rs` | 1 blocking::get call |

The conary-core functions that become async:
- `RepositoryClient` methods (fetch, download, sync)
- `RemiClient` methods (chunk fetch, index queries)
- `CanonicalClient` methods
- `self_update` functions
- `TrustClient` methods
- `Substituter` query/fetch methods

### Cargo.toml Change

```toml
# Before
reqwest = { version = "0.12", features = ["blocking", "rustls-tls", "json"] }
# After
reqwest = { version = "0.13", features = ["rustls-tls", "json"] }
```

## Phase 3: Remaining Dependency Bumps

### axum 0.7 -> 0.8 + axum-extra 0.10 -> 0.12

axum 0.8 changes:
- `State` extraction uses `FromRef` trait instead of `Extension`
- Router construction syntax changes
- `TypedHeader` moved in axum-extra

Files affected: `conary-server/src/server/routes.rs`,
`conary-server/src/server/handlers/`, `conary-test/src/server/`.

Both server and test crates have ~217 async handler functions that are already
async — the changes are to handler signatures, state extraction, and router
construction.

### rusqlite 0.34 -> 0.39

The `prepare_cached()` API is stable across these versions. 9 uses of
`prepare_cached` and ~159 `prepare()` calls. Primary risk is parameter
binding changes. Test with the full suite.

### rand 0.8 -> 0.10

7 usages across 5 files. rand 0.10 changes:
- `OsRng` moves from `rand::rngs::OsRng` (may need `rand::rngs::OsRng` or
  `rand_core::OsRng`)
- `Rng` trait and `thread_rng()` may change
- `WeightedIndex` may move

### sequoia-openpgp 1 -> 2

Single file: `conary-core/src/repository/gpg.rs`. The v2 API redesigns
certificate and signature verification. Rewrite the GPG verification
functions to use the v2 API.

### toml 0.8 -> 1.0

toml 1.0 stabilizes the TOML spec 1.1 support. API should be mostly
compatible. Verify `Spanned` types if used.

### quick-xml 0.37 -> 0.39

XML parser used in 3 files (appstream, metalink, fedora parser). Check for
event loop API changes.

### const-oid 0.9 -> 0.10

ASN.1 OID types used by sigstore integration. Verify compatibility with
sigstore 0.13.

## Recommended Build Order

1. **Phase 1: Async CLI** — mechanical transformation, high confidence
2. **Phase 2: reqwest 0.13** — depends on Phase 1, moderate effort
3. **Phase 3a: rusqlite 0.39** — independent, test-driven
4. **Phase 3b: axum 0.8 + axum-extra 0.12** — independent, server-only
5. **Phase 3c: rand 0.10** — independent, small scope
6. **Phase 3d: sequoia-openpgp 2** — independent, single file rewrite
7. **Phase 3e: toml 1.0 + quick-xml 0.39 + const-oid 0.10** — low-risk batch

## Testing Strategy

Each phase must pass:
- `cargo build --features server`
- `cargo build -p conary-test`
- `cargo test` (all 269 unit tests)
- `cargo clippy -- -D warnings`

## Summary

| Phase | What | Files | Effort |
|-------|------|-------|--------|
| 1 | Async CLI | ~61 files | Mechanical, high volume |
| 2 | reqwest 0.13 | 12 files + Cargo.toml | Moderate |
| 3a | rusqlite 0.39 | Cargo.toml, test | Low |
| 3b | axum 0.8 | ~10 server files | Moderate |
| 3c | rand 0.10 | 5 files | Low |
| 3d | sequoia-openpgp 2 | 1 file (gpg.rs) | Moderate |
| 3e | toml + quick-xml + const-oid | 3-5 files | Low |
