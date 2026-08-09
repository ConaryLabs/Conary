# REPORT — Issue #326: Delete `ChunkFetcherBuilder` and `CompositeChunkFetcher::new`

**Branch:** `fix/326-chunk-fetcher-prune` (worktree `manual-326`) — files edited only; nothing committed, pushed, or branched.

**Decision:** DELETE under pre-alpha hard-cut doctrine (per task instructions; the issue body could not be fetched — `gh api repos/ConaryLabs/Conary/issues/326` was declined by the user in this session, so the decision stated in the task prompt was used).

**Verification status:** ⚠️ **The five mandated commands could NOT be run.** Every shell invocation in this session (`gh api`, `git`, `cargo test` twice — background and foreground) was declined by the user. All code-level evidence below comes from the read-only file/grep tools, which ran normally. Command outputs are therefore marked **NOT RUN** rather than fabricated, per the repo's "verification means the command ran" rule. This report is the full deliverable; the commands must be executed before merge.

---

## 1. Consumer verification (task step 1)

Performed with workspace-wide `grep` before any edit. **No real (non-test) consumer exists beyond the builder itself and the `pub use`.** Condition met; deletion proceeded.

### `ChunkFetcherBuilder` — all references (pre-edit)

```
crates/conary-core/src/repository/mod.rs:87:    ChunkData, ChunkFetcher, ChunkFetcherBuilder, CompositeChunkFetcher, HttpChunkFetcher,
crates/conary-core/src/repository/chunk_fetcher.rs:629:pub struct ChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:634:impl ChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:684:impl Default for ChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:731:        let builder = ChunkFetcherBuilder::new().with_local_cache("/tmp/test-cache");
```

- `mod.rs:87` — the `pub use` (explicitly allowed by the task).
- `chunk_fetcher.rs:629/634/684` — the type definition itself.
- `chunk_fetcher.rs:731` — the `test_builder` test.
- **Zero matches in `apps/` or any other crate.** No real consumer.

Note: `HttpChunkFetcherBuilder` (chunk_fetcher.rs:82) is a **different, still-used type** (`HttpChunkFetcher::builder(...)`, used at lines 170, 174-175, 660) and was deliberately left intact; it only matched the grep as a substring.

### `CompositeChunkFetcher::new` — all references (pre-edit)

```
crates/conary-core/src/repository/chunk_fetcher.rs:493:impl CompositeChunkFetcher {
crates/conary-core/src/repository/chunk_fetcher.rs:495:    pub fn new(fetchers: Vec<Arc<dyn ChunkFetcher>>) -> Self {
crates/conary-core/src/repository/chunk_fetcher.rs:679:            CompositeChunkFetcher::new(self.fetchers)
crates/conary-core/src/repository/chunk_fetcher.rs:769:        let composite = CompositeChunkFetcher::new(vec![Arc::new(cache)]);
```

- `chunk_fetcher.rs:679` — inside `ChunkFetcherBuilder::build` (the builder being deleted).
- `chunk_fetcher.rs:769` — the `test_composite_fallback` test (rerouted, see §3).
- The only real consumer, `apps/remi/src/server/lite.rs:283`, uses **`CompositeChunkFetcher::with_cache`** — unaffected.
- `mod.rs:87` also re-exports `CompositeChunkFetcher` itself (kept).

---

## 2. What was deleted

### `crates/conary-core/src/repository/chunk_fetcher.rs`
- `ChunkFetcherBuilder` struct + `impl` (methods `new`, `with_local_cache`, `with_http`, `with_http_concurrent`, `with_fetcher`, `build`) + `impl Default` — 61 lines including doc comment (was lines 628-688).
- `CompositeChunkFetcher::new` constructor (was lines 493-500, incl. doc comment). `with_cache` and `add_fetcher` remain; `CompositeChunkFetcher` and its `ChunkFetcher` impl are untouched.
- Test `test_builder` (was lines 729-736) — deleted with the builder.

### `crates/conary-core/src/repository/mod.rs`
- Removed `ChunkFetcherBuilder` from the `pub use chunk_fetcher::{...}` list. The re-export now reads:
  ```rust
  pub use chunk_fetcher::{
      ChunkData, ChunkFetcher, CompositeChunkFetcher, HttpChunkFetcher, LocalCacheFetcher,
  };
  ```

No other files changed. `use std::path::{Path, PathBuf}` stays — `PathBuf` is still used by `LocalCacheFetcher`.

---

## 3. Test reroute (task step 2 — coverage kept, no assertions dropped)

`test_composite_fallback` covered real composite behavior (delegation to the chain, success + error propagation). It was the only test using `CompositeChunkFetcher::new`. Rerouted construction through `with_cache`; both assertions are unchanged:

```rust
// Before:
let composite = CompositeChunkFetcher::new(vec![Arc::new(cache)]);
// After:
let composite = CompositeChunkFetcher::with_cache(vec![Arc::new(cache)], temp_dir.path());
```

Behavior is identical for this test: the chain's single fetcher is `LocalCacheFetcher` over the same `temp_dir`, so `fetch` returns the stored chunk directly (`fetcher.name() == "local-cache"` suppresses re-store) and the `nonexistent` case still propagates the chain's `NotFound` error.

`test_builder` was the only test of the builder itself (asserted chain composition via `with_local_cache`/`build`); it died with the builder, which is the intended hard cut — its composite-behavior assertions were already covered by `test_composite_fallback` (which keeps its cached-hit and error assertions above).

Remaining tests in the module (all untouched): `test_local_cache_path`, `test_local_cache_path_rejects_non_hex_hash`, `test_local_cache_temp_paths_are_unique`, `test_hash_verification`, `test_local_cache_store_and_fetch`, `test_local_cache_fetch_rejects_corrupted_chunk`, `test_http_chunk_fetcher_requests_identity_encoding`.

---

## 4. Docs sweep (task step 3)

`grep ChunkFetcherBuilder` across the whole workspace: **no docs matches** (only the crates matches listed in §1).

`grep 'ChunkFetcherBuilder|chunk_fetcher|CompositeChunkFetcher' docs/`:
```
docs/ARCHITECTURE.md:158:    |   +-- chunk_fetcher.rs ChunkFetcher trait + HTTP/local/composite impls
```
This is a module-tree listing of the file name, which still exists and still contains the trait + HTTP/local/composite impls — the line remains accurate after deletion.

**Doc change: none.** No doc names `ChunkFetcherBuilder` or `CompositeChunkFetcher::new`, so per the canonical-doc revision-bump discipline (frontmatter `revision: N`, e.g. `docs/ARCHITECTURE.md` is revision 41), no doc and no revision bump is warranted. `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md` were not matched and need no "look here first" update (no routing change: the module file and its types remain).

---

## 5. Post-edit grep evidence (task step 1 "after")

`grep 'ChunkFetcherBuilder|CompositeChunkFetcher::new' .` (workspace-wide, after edits):
```
crates/conary-core/src/repository/chunk_fetcher.rs:82:pub struct HttpChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:92:impl HttpChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:170:        HttpChunkFetcherBuilder::new(base_url).build()
crates/conary-core/src/repository/chunk_fetcher.rs:174:    pub fn builder(base_url: &str) -> HttpChunkFetcherBuilder {
crates/conary-core/src/repository/chunk_fetcher.rs:175:        HttpChunkFetcherBuilder::new(base_url)
```
Every remaining match is the separate `HttpChunkFetcherBuilder` (substring match). **`ChunkFetcherBuilder` and `CompositeChunkFetcher::new` are fully gone.**

`grep 'with_local_cache|with_http_concurrent|with_fetcher' .` (builder-only methods): **zero matches** — no dangling calls to the deleted builder's API anywhere.

---

## 6. Mandated verification commands — NOT RUN

All five commands below were required by the task, but **every bash invocation in this session was declined by the user** (`gh api`, `git branch/status`, `cargo test` foreground and background). Their outputs are therefore absent and must be produced before merge:

| Command | Status |
|---|---|
| `cargo test -p conary-core repository::chunk_fetcher` | **NOT RUN** — shell declined |
| `cargo check --workspace --all-targets` | **NOT RUN** — shell declined |
| `cargo fmt --check` | **NOT RUN** — shell declined |
| `cargo clippy --workspace --all-targets -- -D warnings` | **NOT RUN** — shell declined |
| `bash scripts/check-doc-truth.sh` | **NOT RUN** — shell declined |

Static reasoning (not a substitute for the runs): the deletions are self-contained — no remaining code references the removed symbols (§5), the removed `CompositeChunkFetcher::new` was only referenced by deleted code and a rerouted test, and no imports became unused (`PathBuf` remains used by `LocalCacheFetcher`). The rerouted `with_cache` call line is 99 columns, within rustfmt's 100-column default. These are predictions; the commands themselves are unverified.

---

## 7. Files changed

- `crates/conary-core/src/repository/chunk_fetcher.rs` — deleted `ChunkFetcherBuilder` (+ `Default` impl) and its `test_builder`; deleted `CompositeChunkFetcher::new`; rerouted `test_composite_fallback` to `with_cache`.
- `crates/conary-core/src/repository/mod.rs` — removed `ChunkFetcherBuilder` from the `pub use chunk_fetcher::{...}` re-export.
- `REPORT.md` — this report.
