# Phase 3: Remaining Stubs + README Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the remaining public CLI dead-ends real and honest by implementing `derivation build`, `profile generate`, `cache populate --sources-only/--full`, `self-update --version`, and the `recipe-audit --trace` downgrade, then align the README and exit-code behavior with what Conary actually supports.

**Architecture:** Reuse Conary's existing derivation, bootstrap, recipe, and self-update paths instead of inventing parallel systems. Phase 3 extracts the recipe discovery logic already living in bootstrap into `conary-core`, keeps derivation execution on `DerivationExecutor`, keeps profile planning on the existing build-order + derivation-ID model, and extends the existing self-update metadata shape so "latest" and "specific version" use the same flow.

**Tech Stack:** Rust, clap command handlers, `rusqlite`, `toml`, `reqwest`, `axum`, existing derivation/bootstrap/self-update modules.

---

## Scope Guard

- Phase 3 covers only the public stubs called out in the spec plus README/exit-code cleanup.
- No Phase 4 substituter work.
- No new daemon, scheduler, or automation work.
- No new package database or derivation metadata store.
- Keep recipe discovery consistent with the existing `recipes/` layout already used by bootstrap and verify.

## File Map

| File | Responsibility in Phase 3 |
|------|----------------------------|
| `crates/conary-core/src/derivation/recipe_loader.rs` | New shared recipe discovery/loading helper extracted from bootstrap |
| `crates/conary-core/src/derivation/mod.rs` | Export recipe loader APIs |
| `apps/conary/src/commands/bootstrap/mod.rs` | Switch bootstrap to the shared recipe loader |
| `apps/conary/src/commands/profile.rs` | Replace the stub with real manifest + recipe + build-order + derivation-ID profile generation |
| `crates/conary-core/src/derivation/pipeline.rs` | Stop generating `"pending"` derivation IDs for profile generation, or expose a helper that computes real IDs from ordered recipes |
| `apps/conary/src/commands/cache.rs` | Implement source prefetch for `--sources-only` and `--full` |
| `crates/conary-core/src/bootstrap/build_runner.rs` | Reuse existing source download/checksum logic from here for cache population |
| `apps/conary/src/commands/derivation.rs` | Replace the derivation build stub with real executor wiring and readonly env-image mount lifecycle |
| `crates/conary-core/src/generation/mount.rs` | Reuse existing read-only EROFS/composefs mount helper for env-image mounting |
| `crates/conary-core/src/self_update/versioning.rs` | Add a version-specific metadata fetch helper that mirrors `/latest` |
| `apps/conary/src/commands/self_update.rs` | Implement `--version X` on top of the shared metadata flow |
| `apps/remi/src/server/handlers/self_update.rs` | Add `GET /v1/ccs/conary/{version}` metadata handler |
| `apps/remi/src/server/routes/public.rs` | Register the new self-update metadata route |
| `apps/conary/src/commands/recipe_audit.rs` | Replace the trace stub print with a warning and keep static analysis behavior |
| `README.md` | Update Phase 3-facing claims once the commands are real |

## Chunk 1: Recipe Loading + Build Profiles + Source Cache

### Task 1: Extract Shared Recipe Loading From Bootstrap

**Files:**
- Create: `crates/conary-core/src/derivation/recipe_loader.rs`
- Modify: `crates/conary-core/src/derivation/mod.rs`
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Test: `crates/conary-core/src/derivation/recipe_loader.rs`

- [ ] **Step 1: Write failing recipe-loader tests**

Add core tests that prove the helper matches current Conary layout:

```rust
#[test]
fn load_recipes_reads_conventional_subdirs() {}

#[test]
fn load_recipes_reads_plain_recipe_root_fallback() {}

#[test]
fn load_recipes_skips_invalid_recipe_with_warning() {}

#[test]
fn find_recipe_path_locates_package_in_standard_roots() {}
```

- [ ] **Step 2: Add the shared loader in `conary-core`**

Use bootstrap's existing discovery logic as the base instead of designing a
new recipe model:

```rust
pub fn load_recipes(recipe_root: &Path) -> Result<HashMap<String, Recipe>, RecipeLoaderError>;
pub fn find_recipe_path(recipe_root: &Path, package: &str) -> Option<PathBuf>;
```

Search order must stay aligned with existing Conary conventions:
- `recipes/cross-tools`
- `recipes/temp-tools`
- `recipes/system`
- `recipes/tier2`
- plain `recipes/` root fallback

- [ ] **Step 3: Export the helper from `derivation/mod.rs`**

Re-export the loader so command code can use:

```rust
pub use recipe_loader::{find_recipe_path, load_recipes, RecipeLoaderError};
```

- [ ] **Step 4: Switch bootstrap to the shared loader**

Delete the local `load_recipes()` in `apps/conary/src/commands/bootstrap/mod.rs` and replace its call sites with the new core helper.

- [ ] **Step 5: Verify the extraction**

Run: `cargo test -p conary-core derivation::recipe_loader`

Expected: new loader tests pass and bootstrap still compiles cleanly.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/derivation/recipe_loader.rs crates/conary-core/src/derivation/mod.rs apps/conary/src/commands/bootstrap/mod.rs
git commit -m "feat(derivation): extract shared recipe loader"
```

### Task 2: Implement `profile generate` With Real Derivation IDs

**Files:**
- Modify: `apps/conary/src/commands/profile.rs`
- Modify: `crates/conary-core/src/derivation/pipeline.rs`
- Modify if needed: `crates/conary-core/src/derivation/profile.rs`
- Test: `apps/conary/src/commands/profile.rs`
- Test: `crates/conary-core/src/derivation/pipeline.rs`

- [ ] **Step 1: Write failing profile-generation tests**

Add command-level tests that prove Phase 3 behavior instead of the old stub:

```rust
#[test]
fn test_profile_generate_writes_real_derivation_ids() {}

#[test]
fn test_profile_generate_stores_canonical_manifest_path() {}

#[test]
fn test_profile_generate_uses_seed_hash_for_stage_build_env() {}

#[test]
fn test_profile_generate_errors_when_recipe_is_missing() {}
```

Also update/add a core test in `pipeline.rs` so the profile helper no longer emits `"pending"`.

- [ ] **Step 2: Resolve the seed without inventing a new seed system**

In `cmd_profile_generate`, support only Conary-shaped inputs:
- local seed directory: `Seed::load_local(...)`
- already-addressed hash forms: normalize `cas:sha256:<hash>` or raw 64-hex hash and use it directly
- anything else: bail with a clear message instead of guessing or adding remote seed fetch

Use that resolved seed ID as both:
- `profile.seed.id`
- `ProfileStage.build_env` for generated stages, because the current derivation pipeline uses a single seed build environment rather than per-stage environment hashes

- [ ] **Step 3: Replace the stub with real profile planning**

Implement:
1. load `SystemManifest`
2. canonicalize the manifest path before storing it in the profile metadata
3. resolve the recipe root from the canonical manifest location
4. load recipes via the shared loader
5. compute the transitive build dependency closure from `requires + makedepends`
6. run `compute_build_order(...)`
7. compute real derivation IDs in topological order
8. group ordered packages by `Stage`
9. serialize via `BuildProfile::to_toml()`
10. write to `--output` when provided, otherwise print TOML to stdout

Do not keep `"pending"` IDs in generated profiles after this task.

- [ ] **Step 4: Push derivation-ID computation into the existing profile path**

Either:
- update `Pipeline::generate_profile(...)` to compute real IDs, or
- add a sibling helper in `pipeline.rs` that computes a real-ID `BuildProfile`

Choose the smaller change, but keep the logic in `conary-core` so commands and future pipeline code share one profile-generation model.

- [ ] **Step 5: Verify the command path**

Run: `cargo test -p conary commands::profile::tests`

Run: `cargo test -p conary-core derivation::pipeline`

Expected: generated profiles carry real derivation IDs and no longer rely on the old `"pending"` placeholder path.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/profile.rs crates/conary-core/src/derivation/pipeline.rs crates/conary-core/src/derivation/profile.rs
git commit -m "feat(profile): generate real derivation profiles"
```

### Task 3: Implement `cache populate --sources-only` and `--full`

**Files:**
- Modify: `apps/conary/src/commands/cache.rs`
- Modify if needed: `crates/conary-core/src/bootstrap/build_runner.rs`
- Test: `apps/conary/src/commands/cache.rs`

- [ ] **Step 1: Write failing cache-populate tests**

Add tests that prove source prefetch is real:

```rust
#[test]
fn test_cache_populate_sources_only_downloads_recipe_archives() {}

#[test]
fn test_cache_populate_full_downloads_sources_after_outputs() {}

#[test]
fn test_cache_populate_sources_only_uses_profile_manifest_to_find_recipes() {}
```

Use `file://` source URLs in test recipes so the fetch path stays hermetic and does not require network access.

- [ ] **Step 2: Add a small source-prefetch helper in `cache.rs`**

Implement one shared helper for both `--sources-only` and `--full`:

```rust
async fn prefetch_profile_sources(profile: &BuildProfile, db_path: &str) -> Result<SourcePrefetchStats>
```

It should:
1. reload the canonical manifest path stored in `profile.profile.manifest`
2. resolve the conventional recipe root next to that manifest
3. locate each package recipe through the shared recipe loader
4. reuse `PackageBuildRunner::fetch_source(...)`
5. skip files already cached with valid checksums

- [ ] **Step 3: Replace the `--sources-only` stub**

If `sources_only` is true:
- skip substituter probing entirely
- run only the source-prefetch helper
- print a real summary with downloaded/skipped counts

- [ ] **Step 4: Make `--full` reuse the same helper**

Keep the existing remote derivation-output prefetch flow, then call the source-prefetch helper afterward so `--full` means "binary outputs plus source tarballs" for real.

- [ ] **Step 5: Verify the cache command**

Run: `cargo test -p conary commands::cache::tests`

Expected: both `--sources-only` and `--full` exercise real source prefetch behavior and no longer return the old stub message.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/src/commands/cache.rs crates/conary-core/src/bootstrap/build_runner.rs
git commit -m "feat(cache): prefetch derivation sources from profiles"
```

## Chunk 2: Derivation Build + Self-Update + Cleanup

### Task 4: Replace the `derivation build` Stub With Real Executor Wiring

**Files:**
- Modify: `apps/conary/src/commands/derivation.rs`
- Reuse: `crates/conary-core/src/generation/mount.rs`
- Test: `apps/conary/src/commands/derivation.rs`

- [ ] **Step 1: Write failing derivation-build helper tests**

Focus tests on the pure command helpers, not on privileged mount behavior:

```rust
#[test]
fn test_open_derivation_db_uses_in_memory_when_db_path_is_none() {}

#[test]
fn test_standalone_derivation_build_uses_temp_cas_dir() {}

#[test]
fn test_current_target_triple_is_non_empty() {}

#[test]
fn test_derivation_build_no_longer_returns_stub_message_on_executor_path() {}
```

The env-image mount itself can stay thin and be covered by existing mount helper tests plus manual sanity.

- [ ] **Step 2: Add a readonly env-image mount guard**

In `cmd_derivation_build`, add a tiny helper that:
- creates a temp mountpoint
- mounts the env image read-only using the existing generation mount path (`mount_generation` / EROFS fallback)
- hands the mounted sysroot path to a closure
- unmounts on success and failure

Keep this helper local to the command module; do not add a new general "image runtime" subsystem.

- [ ] **Step 3: Replace the stub with executor wiring**

Implement the real command flow:
1. parse the recipe
2. hash the env image
3. open either:
   - the supplied SQLite DB, or
   - an in-memory SQLite DB with migrations applied when `db_path` is `None`
4. create/open the CAS store:
   - normal DB-backed CAS path when `db_path` is `Some`
   - tempdir-backed CAS path when `db_path` is `None`
5. mount the env image read-only to get a sysroot directory
6. call `DerivationExecutor::execute(...)`
7. print whether the result was a cache hit or fresh build, plus the derivation ID/output hash

Standalone `derivation build` should keep `dep_ids = BTreeMap::new()` and document that transitive dependency IDs come from profile/pipeline planning.

- [ ] **Step 4: Verify the command**

Run: `cargo test -p conary commands::derivation::tests`

Run: `cargo test -p conary-core derivation::executor`

Expected: helper tests pass, and the command path is now wired to the real executor instead of printing `[NOT YET IMPLEMENTED]`.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/derivation.rs
git commit -m "feat(derivation): wire derivation build to executor"
```

### Task 5: Implement `self-update --version X` End-to-End

**Files:**
- Modify: `apps/remi/src/server/handlers/self_update.rs`
- Modify: `apps/remi/src/server/routes/public.rs`
- Modify: `crates/conary-core/src/self_update/versioning.rs`
- Modify: `crates/conary-core/src/self_update.rs`
- Modify: `apps/conary/src/commands/self_update.rs`
- Test: `apps/remi/src/server/handlers/self_update.rs`
- Test: `crates/conary-core/src/self_update/versioning.rs`
- Test: `apps/conary/tests/integration/remi/manifests/phase3-group-l.toml`

- [ ] **Step 1: Write failing tests for version-specific metadata**

Add server tests:

```rust
#[tokio::test]
async fn test_get_version_info_returns_requested_version_metadata() {}

#[tokio::test]
async fn test_get_version_info_returns_404_for_missing_version() {}
```

Add client/versioning tests:

```rust
#[tokio::test]
async fn test_fetch_version_info_uses_version_endpoint() {}
```

Add one integration-manifest scenario for `conary self-update --version X`.

- [ ] **Step 2: Add the Remi metadata endpoint**

Add `GET /v1/ccs/conary/{version}` that returns the same JSON shape as `/latest`:

```json
{
  "version": "1.2.3",
  "download_url": "/v1/ccs/conary/1.2.3/download",
  "sha256": "...",
  "size": 12345,
  "signature": "..."
}
```

Reuse the existing scan/hash code; do not build a second metadata cache or new response type.

- [ ] **Step 3: Add a shared client helper in `conary-core`**

Do not hand-roll the version-specific fetch in the CLI command. Extend the existing self-update versioning layer with a helper such as:

```rust
pub async fn fetch_version_info(
    channel_url: &str,
    version: &str,
    user_agent: &str,
) -> Result<LatestVersionInfo>
```

This should share:
- response size limits
- JSON parsing
- download-origin validation

- [ ] **Step 4: Replace the CLI bail**

In `cmd_self_update(...)`:
1. validate `--version` as SemVer
2. fetch version metadata through the new shared helper
3. skip if already on that version unless `--force`
4. reuse the existing signature-check, download, extract, and replace flow

No second update code path beyond choosing which metadata endpoint to query.

- [ ] **Step 5: Verify the end-to-end path**

Run: `cargo test -p remi self_update`

Run: `cargo test -p conary self_update`

If the manifest harness is already available locally, also run the Phase 3 self-update group that covers the new `--version` case.

- [ ] **Step 6: Commit**

```bash
git add apps/remi/src/server/handlers/self_update.rs apps/remi/src/server/routes/public.rs crates/conary-core/src/self_update/versioning.rs crates/conary-core/src/self_update.rs apps/conary/src/commands/self_update.rs apps/conary/tests/integration/remi/manifests/phase3-group-l.toml
git commit -m "feat(self-update): support version-specific updates"
```

### Task 6: Downgrade `recipe-audit --trace`, Audit Exit Codes, and Update README

**Files:**
- Modify: `apps/conary/src/commands/recipe_audit.rs`
- Modify: `README.md`
- Modify if needed: other stub-bearing command files only when they are public and in-scope for Phase 3

- [ ] **Step 1: Replace the trace stub print with a warning**

Change:

```rust
println!("--trace mode is not yet implemented. Running static analysis only.");
```

to a `tracing::warn!` call that keeps the command useful and honest without pretending trace mode exists.

- [ ] **Step 2: Audit public Phase 3-facing exit codes**

Search the command tree for `[NOT YET IMPLEMENTED]` / `not yet implemented` and make sure, after this phase:
- the Phase 3 commands now work
- any remaining public dead-end in scope returns a real `bail!()` instead of `println!(...)` + `Ok(())`
- experimental or intentionally out-of-scope surfaces stay untouched

- [ ] **Step 3: Update the README after code lands**

Make the Phase 3 README edits from the spec:
- comparison table: "Hermetic builds" -> `Partial (experimental)`
- bootstrap section: note that `derivation build` is functional while the broader pipeline is still evolving
- keep the current honest maturity framing; do not oversell ecosystem completeness

- [ ] **Step 4: Verify the cleanup**

Run:

```bash
rg -n "\[NOT YET IMPLEMENTED\]|not yet implemented" apps/conary/src/commands README.md
```

Expected: the targeted Phase 3 command stubs are gone, and any remaining hits are clearly out-of-scope or intentionally experimental.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/recipe_audit.rs README.md
git commit -m "docs(readme): align phase 3 command behavior"
```

### Task 7: Final Verification

**Files:**
- Verify only

- [ ] **Step 1: Run focused package tests**

Run:

```bash
cargo test -p conary commands::profile::tests
cargo test -p conary commands::cache::tests
cargo test -p conary commands::derivation::tests
cargo test -p conary self_update
cargo test -p conary-core derivation::
cargo test -p conary-core self_update::
cargo test -p remi self_update
```

- [ ] **Step 2: Run the package suites**

Run:

```bash
cargo test -p conary
cargo test -p conary-core
cargo test -p remi
```

- [ ] **Step 3: Run lint/format verification**

Run:

```bash
cargo fmt --check
cargo clippy -p conary -p conary-core -p remi -- -D warnings
```

- [ ] **Step 4: Manual CLI sanity**

Run:

```bash
target/debug/conary derivation build --help
target/debug/conary profile generate --help
target/debug/conary cache populate --help
target/debug/conary self-update --help
target/debug/conary recipe-audit --help
```

Then smoke-test the formerly stubbed paths with local fixtures or temp directories as available.

- [ ] **Step 5: Final commit**

```bash
git status --short
```

If clean and verified, create the final Phase 3 implementation commit with a summary matching the actual landed diff.

---

Plan complete and saved to `docs/superpowers/plans/2026-04-08-phase3-stubs-readme.md`. Ready to execute after review.
