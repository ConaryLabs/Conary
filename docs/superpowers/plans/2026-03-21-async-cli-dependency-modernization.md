# Async CLI + Dependency Modernization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the CLI to fully async and bump every dependency to latest stable (as of 2026-03-21).

**Architecture:** `main()` becomes `#[tokio::main]`, all 201 `cmd_*` functions become `async fn`, reqwest::blocking replaced with async reqwest. Each phase builds on the previous: async foundation first, then reqwest migration, then remaining dep bumps.

**Tech Stack:** Rust 1.94, tokio 1.x, reqwest 0.13, axum 0.8, rusqlite 0.39, sequoia-openpgp 2

**Spec:** `docs/superpowers/specs/2026-03-21-async-cli-dependency-modernization.md`

---

## File Structure

### Modified files

| File | Change |
|------|--------|
| `src/main.rs` | `fn main()` -> `#[tokio::main] async fn main()`, `fn run()` -> `async fn run()`, all dispatch arms add `.await` |
| `src/commands/*.rs` (44 files) | All `pub fn cmd_*` -> `pub async fn cmd_*` |
| `src/commands/mod.rs` | No signature changes needed (async fns export identically) |
| `conary-core/src/repository/client.rs` | `reqwest::blocking::Client` -> `reqwest::Client`, methods become async |
| `conary-core/src/repository/remi.rs` | Same blocking->async migration |
| `conary-core/src/canonical/client.rs` | Same |
| `conary-core/src/self_update.rs` | Same |
| `conary-core/src/trust/client.rs` | Same |
| `conary-core/src/model/remote.rs` | Same |
| `conary-core/src/derivation/substituter.rs` | Same |
| `Cargo.toml` | reqwest 0.12->0.13 (drop `blocking` feature), axum 0.7->0.8, rand 0.8->0.10, rusqlite 0.34->0.39, toml 0.8->1.0, const-oid 0.9->0.10 |
| `conary-core/Cargo.toml` | sequoia-openpgp 1->2, quick-xml 0.37->0.39 |
| `conary-server/Cargo.toml` | axum-extra 0.10->0.12 |
| `conary-server/src/server/routes.rs` | axum 0.8 router changes |
| `conary-server/src/server/handlers/**` | axum 0.8 State extraction |
| `conary-test/src/server/**` | axum 0.8 State extraction |
| `conary-core/src/repository/gpg.rs` | sequoia-openpgp 2 API rewrite |

---

## Task 1: Async CLI Foundation — main.rs + dispatcher

**Files:**
- Modify: `src/main.rs`

This is the foundation. Make `main()` and `run()` async, add `.await` to every dispatch arm.

- [ ] **Step 1: Make main() async**

Change:
```rust
fn main() {
    // ... tracing setup ...
    if let Err(err) = run() {
```
To:
```rust
#[tokio::main]
async fn main() {
    // ... tracing setup ...
    if let Err(err) = run().await {
```

And change:
```rust
fn run() -> Result<()> {
```
To:
```rust
async fn run() -> Result<()> {
```

- [ ] **Step 2: Add .await to every dispatch arm**

Every `commands::cmd_*()` call in the match block needs `.await`. There are ~80 dispatch arms in main.rs (1876 lines). Add `.await` to every function call that dispatches to a command handler.

Pattern: `commands::cmd_install(...)` -> `commands::cmd_install(...).await`

Also handle the `commands::verify::cmd_*` and `commands::cmd_derivation_sbom` paths.

- [ ] **Step 3: Verify (will fail — cmd functions aren't async yet)**

Run: `cargo build 2>&1 | head -5`
Expected: errors about calling non-async functions with `.await`

- [ ] **Step 4: Commit (WIP)**

```
feat: make CLI main/run async (WIP - commands not yet async)
```

---

## Task 2: Async CLI — Command Files Batch 1 (core operations)

**Files:**
- Modify: `src/commands/install/mod.rs`
- Modify: `src/commands/remove.rs`
- Modify: `src/commands/update.rs`
- Modify: `src/commands/system.rs`
- Modify: `src/commands/repo.rs`
- Modify: `src/commands/config.rs`
- Modify: `src/commands/state.rs`
- Modify: `src/commands/restore.rs`
- Modify: `src/commands/self_update.rs`
- Modify: `src/commands/update_channel.rs`

For each file: add `async` keyword to every `pub fn cmd_*` function signature. No other changes needed yet — the functions don't use `.await` internally until Task 5 (reqwest migration).

- [ ] **Step 1: Add `async` to all cmd functions in each file**

Mechanical transformation for each file:
```rust
// Before
pub fn cmd_install(...) -> Result<()> {
// After
pub async fn cmd_install(...) -> Result<()> {
```

Do this for ALL `pub fn cmd_*` functions in the 10 files listed above.

- [ ] **Step 2: Verify partial build**

Run: `cargo build 2>&1 | grep "^error" | wc -l`
Error count should decrease. Not yet zero — other command files still sync.

- [ ] **Step 3: Commit**

```
feat: make core CLI commands async (batch 1)
```

---

## Task 3: Async CLI — Command Files Batch 2 (package management)

**Files:**
- Modify: `src/commands/adopt/mod.rs`
- Modify: `src/commands/collection.rs`
- Modify: `src/commands/derived.rs`
- Modify: `src/commands/label.rs`
- Modify: `src/commands/model.rs`
- Modify: `src/commands/redirect.rs`
- Modify: `src/commands/groups.rs`
- Modify: `src/commands/query/mod.rs`

Same mechanical transformation: add `async` to every `pub fn cmd_*`.

- [ ] **Step 1: Add `async` to all cmd functions**
- [ ] **Step 2: Commit**

```
feat: make package management CLI commands async (batch 2)
```

---

## Task 4: Async CLI — Command Files Batch 3 (everything else)

**Files:**
- Modify: `src/commands/automation.rs`
- Modify: `src/commands/bootstrap/mod.rs`
- Modify: `src/commands/cache.rs`
- Modify: `src/commands/canonical.rs`
- Modify: `src/commands/capability.rs`
- Modify: `src/commands/ccs/mod.rs`
- Modify: `src/commands/cook.rs`
- Modify: `src/commands/convert_pkgbuild.rs`
- Modify: `src/commands/recipe_audit.rs`
- Modify: `src/commands/derivation.rs`
- Modify: `src/commands/derivation_sbom.rs`
- Modify: `src/commands/distro.rs`
- Modify: `src/commands/export.rs`
- Modify: `src/commands/federation.rs`
- Modify: `src/commands/generation/mod.rs`
- Modify: `src/commands/profile.rs`
- Modify: `src/commands/provenance.rs`
- Modify: `src/commands/registry.rs`
- Modify: `src/commands/triggers.rs`
- Modify: `src/commands/trust.rs`
- Modify: `src/commands/verify.rs`

Same mechanical transformation: add `async` to every `pub fn cmd_*`.

Also update non-cmd public functions that are called from main.rs dispatch (e.g., `cmd_scripts`, `export_oci`, helper functions called directly from main.rs).

- [ ] **Step 1: Add `async` to all remaining cmd functions**
- [ ] **Step 2: Update mod.rs re-exports if needed** (async fns export the same way, but verify)
- [ ] **Step 3: Verify clean build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build (all commands now async, all dispatch arms have `.await`)

- [ ] **Step 4: Run tests**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass (async fn with no .await inside is equivalent to sync)

- [ ] **Step 5: Commit**

```
feat: make all CLI commands async (batch 3)

All 201 cmd_* functions are now async. main() uses #[tokio::main].
No behavioral changes — functions don't use .await internally yet.
```

---

## Task 5: reqwest Blocking -> Async (conary-core)

**Files:**
- Modify: `conary-core/src/repository/client.rs`
- Modify: `conary-core/src/repository/remi.rs`
- Modify: `conary-core/src/canonical/client.rs`
- Modify: `conary-core/src/self_update.rs`
- Modify: `conary-core/src/trust/client.rs`
- Modify: `conary-core/src/model/remote.rs`
- Modify: `conary-core/src/derivation/substituter.rs`

- [ ] **Step 1: Migrate repository/client.rs**

Replace:
```rust
use reqwest::blocking::Client;
```
With:
```rust
use reqwest::Client;
```

Make all methods that use `Client` async. Replace `.send()` with `.send().await`, `.text()` with `.text().await`, `.bytes()` with `.bytes().await`, etc.

The `RepositoryClient` struct methods become `async fn`. All callers (in `src/commands/repo.rs` etc.) already have the `async` keyword from Tasks 2-4, so add `.await` at call sites.

- [ ] **Step 2: Migrate repository/remi.rs**

Same pattern. `RemiClient` methods become async.

- [ ] **Step 3: Migrate canonical/client.rs**

Same pattern. Functions become async.

- [ ] **Step 4: Migrate self_update.rs**

3 `reqwest::blocking::Client::builder()` calls -> `reqwest::Client::builder()`. Functions become async.

- [ ] **Step 5: Migrate trust/client.rs**

2 `reqwest::blocking::get()` calls -> `reqwest::get().await`.

- [ ] **Step 6: Migrate model/remote.rs**

1 `reqwest::blocking::Client::builder()` call.

- [ ] **Step 7: Migrate derivation/substituter.rs**

1 `reqwest::blocking::Client` import.

- [ ] **Step 8: Fix all callers in src/commands/**

Grep for any call to the now-async conary-core functions. Add `.await` at each call site. Key files:
- `src/commands/repo.rs` (uses RepositoryClient)
- `src/commands/self_update.rs` (uses self_update functions)
- `src/commands/federation.rs` (uses reqwest directly)
- `src/commands/profile.rs` (uses reqwest directly)
- `src/commands/provenance.rs` (uses reqwest directly)

- [ ] **Step 9: Verify build**

Run: `cargo build 2>&1 | tail -5`

- [ ] **Step 10: Run tests**

Run: `cargo test 2>&1 | tail -10`

- [ ] **Step 11: Commit**

```
feat: migrate reqwest blocking to async across conary-core and CLI

All HTTP calls now use async reqwest. reqwest::blocking is no longer
used anywhere in the codebase.
```

---

## Task 6: Bump reqwest to 0.13

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Update version and remove blocking feature**

```toml
# Before
reqwest = { version = "0.12", features = ["blocking", "rustls-tls", "json"] }
# After
reqwest = { version = "0.13", features = ["rustls-tls", "json"] }
```

- [ ] **Step 2: Build and fix any API changes**

Run: `cargo build --features server 2>&1 | grep "^error" | head -20`

reqwest 0.13 may have minor API changes beyond dropping blocking. Fix any that appear.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`

- [ ] **Step 4: Commit**

```
chore: bump reqwest to 0.13

The blocking feature is no longer needed — CLI is fully async.
```

---

## Task 7: Bump rusqlite 0.34 -> 0.39

**Files:**
- Modify: `Cargo.toml` (workspace dep)

- [ ] **Step 1: Update version**

```toml
rusqlite = { version = "0.39", features = ["bundled"] }
```

- [ ] **Step 2: Build and fix API changes**

Run: `cargo build --features server 2>&1 | grep "^error"`

Key areas to check: `prepare_cached()` signature, parameter binding, `Connection` methods. Fix any breaking changes.

- [ ] **Step 3: Run tests**

Run: `cargo test`

- [ ] **Step 4: Commit**

```
chore: bump rusqlite to 0.39
```

---

## Task 8: Bump axum 0.7 -> 0.8 + axum-extra 0.10 -> 0.12

**Files:**
- Modify: `Cargo.toml` (workspace dep: axum)
- Modify: `conary-server/Cargo.toml` (axum-extra)
- Modify: `conary-server/src/server/routes.rs`
- Modify: `conary-server/src/server/handlers/**`
- Modify: `conary-test/src/server/**`

- [ ] **Step 1: Update versions**

In `Cargo.toml`:
```toml
axum = "0.8"
```

In `conary-server/Cargo.toml`:
```toml
axum-extra = { version = "0.12", features = ["typed-header"] }
```

- [ ] **Step 2: Build and fix all errors**

axum 0.8 key changes:
- `State` extraction via `FromRef` trait
- Router construction changes
- `Extension` -> `State` for shared app state
- Handler trait changes

Fix every handler in conary-server and conary-test.

- [ ] **Step 3: Run tests**

Run: `cargo test --features server`

- [ ] **Step 4: Commit**

```
chore: bump axum to 0.8, axum-extra to 0.12

Migrate all handlers to new State extraction pattern.
```

---

## Task 9: Bump rand 0.8 -> 0.10

**Files:**
- Modify: `Cargo.toml` (workspace dep)
- Modify: `conary-core/src/ccs/signing.rs`
- Modify: `conary-core/src/model/signing.rs`
- Modify: `conary-core/src/repository/mirror_selector.rs`
- Modify: `conary-core/src/repository/client.rs`
- Modify: `conary-core/src/repository/retry.rs`

- [ ] **Step 1: Update version**

```toml
rand = "0.10"
```

- [ ] **Step 2: Build and fix API changes**

rand 0.10 changes:
- `OsRng` import path may change
- `thread_rng()` may be renamed
- `Rng` trait methods may change
- `WeightedIndex` may move modules

Fix each of the 5 affected files.

- [ ] **Step 3: Run tests**
- [ ] **Step 4: Commit**

```
chore: bump rand to 0.10
```

---

## Task 10: Bump sequoia-openpgp 1 -> 2

**Files:**
- Modify: `conary-core/Cargo.toml`
- Modify: `conary-core/src/repository/gpg.rs`

- [ ] **Step 1: Update version**

```toml
sequoia-openpgp = { version = "2", default-features = false, features = ["crypto-rust", "allow-experimental-crypto", "allow-variable-time-crypto"] }
```

Note: features may change in v2. Check available features.

- [ ] **Step 2: Build and rewrite gpg.rs**

Read the current `gpg.rs`, understand what it does (GPG signature verification for repository metadata), then rewrite using sequoia-openpgp v2 API.

Key functions to rewrite:
- `verify_gpg_signature()`
- `import_key()` / key parsing
- Certificate and signature verification

- [ ] **Step 3: Run tests**
- [ ] **Step 4: Commit**

```
chore: bump sequoia-openpgp to 2, rewrite GPG verification
```

---

## Task 11: Bump remaining deps (toml, quick-xml, const-oid)

**Files:**
- Modify: `Cargo.toml` (toml)
- Modify: `conary-core/Cargo.toml` (quick-xml, const-oid)
- Fix any affected source files

- [ ] **Step 1: Update versions**

In `Cargo.toml`:
```toml
toml = "1.0"
```

In `conary-core/Cargo.toml`:
```toml
quick-xml = "0.39"
const-oid = "0.10"
```

- [ ] **Step 2: Build and fix API changes**

quick-xml 0.39 may have event loop API changes. Fix in:
- `conary-core/src/canonical/appstream.rs`
- `conary-core/src/repository/metalink.rs`
- `conary-core/src/repository/parsers/fedora.rs`

- [ ] **Step 3: Run tests**
- [ ] **Step 4: Commit**

```
chore: bump toml to 1.0, quick-xml to 0.39, const-oid to 0.10
```

---

## Task 12: Final verification + cleanup

- [ ] **Step 1: cargo fmt**

Run: `cargo fmt`

- [ ] **Step 2: cargo clippy**

Run: `cargo clippy --features server -- -D warnings`
Run: `cargo clippy -p conary-test -- -D warnings`

Fix any warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo test`

- [ ] **Step 4: Verify no deps behind latest**

Run: `cargo update --verbose 2>&1 | grep Unchanged`

Should show zero or only transitive deps.

- [ ] **Step 5: Update CLAUDE.md if needed**

If any build commands or conventions changed.

- [ ] **Step 6: Commit**

```
chore: fmt, clippy, final cleanup after dependency modernization
```

---

## Summary

| Task | What | Files | Complexity |
|------|------|-------|-----------|
| 1 | Async main.rs dispatcher | 1 | Mechanical (large file) |
| 2 | Async commands batch 1 | 10 | Mechanical |
| 3 | Async commands batch 2 | 8 | Mechanical |
| 4 | Async commands batch 3 | 21 | Mechanical |
| 5 | reqwest blocking -> async | 12 | Moderate (API changes) |
| 6 | reqwest 0.13 bump | 1 | Low |
| 7 | rusqlite 0.39 | 1+ | Low-Moderate |
| 8 | axum 0.8 | 10+ | Moderate |
| 9 | rand 0.10 | 5 | Low |
| 10 | sequoia-openpgp 2 | 2 | Moderate (rewrite) |
| 11 | toml + quick-xml + const-oid | 5 | Low |
| 12 | Final cleanup | all | Low |

**Dependencies:** 1 -> 2 -> 3 -> 4 (sequential). 5 depends on 4. 6 depends on 5. Tasks 7-11 are independent of each other but all depend on 4. Task 12 depends on all.
