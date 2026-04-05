# Codebase Simplification Pass

**Date:** 2026-04-03
**Scope:** Full codebase (~226k lines Rust, 7 crates)
**Goal:** Dead code removal, large-file decomposition, code clarity, deduplication

## Phase 1: Per-Chunk Simplification

Break the codebase into 12 logical chunks. Each chunk gets a dedicated
`simplify/<chunk-name>` branch created from `main`. Chunks are independent at
the file level and can be reviewed/merged individually.

### Chunks

| # | Branch | Crate(s) | Modules | ~Lines |
|---|--------|----------|---------|--------|
| 1 | `simplify/core-db` | conary-core | db | 19k |
| 2 | `simplify/core-model-ccs` | conary-core | model, ccs | 24k |
| 3 | `simplify/core-repository` | conary-core | repository, canonical, + `tests/canonical.rs` | 19k |
| 4 | `simplify/core-build` | conary-core | derivation, recipe, derived, scriptlet, + `tests/derivation_e2e.rs`, `examples/sign_hash.rs` | 17k |
| 5 | `simplify/core-resolver` | conary-core | resolver, dependencies, flavor, version | 9k |
| 6 | `simplify/core-system` | conary-core | capability, bootstrap, generation, container, filesystem, + `benches/erofs_build.rs` | 25k |
| 7 | `simplify/core-supporting` | conary-core | packages, components, trust, provenance, automation, transaction, delta, trigger, compression, + loose files (hash, json, label, util, self_update, error, lib, progress, federation_discovery) | 19k |
| 8 | `simplify/cli-dispatch` | conary | cli/, dispatch.rs, app.rs, main.rs, live_host_safety.rs | 6k |
| 9 | `simplify/cli-commands` | conary | commands/, tests/, build.rs | 40k |
| 10 | `simplify/remi` | remi | server/, federation/, trust.rs, lib.rs, bin/remi.rs | 34k |
| 11 | `simplify/conaryd` | conaryd | daemon/, lib.rs, bin/conaryd.rs | 8k |
| 12 | `simplify/conary-test` | conary-test | all, plus conary-mcp | 14k |

### Effort Notes

Line count does not equal effort. Key outliers:

- **Chunk 8 (cli-dispatch, 6k):** `dispatch.rs` is 1831 lines with literally
  1 function — the single worst decomposition target in the codebase. Small
  chunk, high structural complexity.
- **Chunk 9 (cli-commands, 40k):** Largest chunk once `tests/` is included
  (~7k lines of integration tests). Also has 26 `#[allow(dead_code)]`
  markers — the most of any chunk. Primary dead-code removal target.
- **Chunk 11 (conaryd, 8k):** `daemon/routes.rs` at 2160 lines is the 2nd
  largest file. 75% of the chunk is in files over 500 lines.
- **Chunk 5 (core-resolver, 9k):** Two files exceed 1500 lines
  (`resolver/provider/mod.rs` at 1728, `resolver/sat.rs` at 1518). Smallest
  core chunk but both top files need serious decomposition.
- **Chunk 6 (core-system, 25k):** Has 17 `#[allow(dead_code)]` markers
  (mostly in `bootstrap/`), plus `container/mod.rs` at 2135 lines. Heaviest
  combined decomp + dead-code work.
- **Chunks 1, 2, 5, 7:** Zero `#[allow(dead_code)]` markers. Purely
  structural/decomposition and clarity work.
- **Chunk 7 (core-supporting, 19k):** 18 modules but `packages/` alone is 5k
  lines (27% of the chunk). Wide breadth, but each module is self-contained.

### What Each Pass Does

#### Removes
- Unused functions, types, imports, constants
- `#[allow(dead_code)]` markers on code that can just be deleted
- Unreachable match arms and dead conditional branches
- Commented-out code

#### Decomposes
- Files over ~500 lines: examine for split opportunities
- Files over 1,000 lines: strong candidates for extraction into submodules
- Functions over ~80 lines: look for extractable helpers

#### Clarifies
- Reduce nesting depth (early returns, guard clauses)
- Simplify complex conditionals
- Replace verbose patterns with idiomatic Rust
- Deduplicate repeated logic within the chunk
- Improve unclear variable/function names

#### Preserves
- All public APIs and module boundaries between crates
- All `pub` and `pub(crate)` item signatures within conary-core (Phase 1 is
  conservative; Phase 2 cleans up after merge)
- All behavior and semantics
- File header path comments (per AGENTS.md convention)
- Test coverage (tests updated only if internal signatures change)
- No new dependencies added

### Cross-Chunk Safety Rules

Chunks 1-7 (all within conary-core) share heavy pub API coupling. In
particular:

- **db::models** (chunk 1) is imported by virtually every other conary-core
  chunk
- **repository::versioning** (chunk 3) is imported throughout the resolver
  (chunk 5)
- **model** (chunk 2) and **db** (chunk 1) have bidirectional imports
- **bootstrap** (chunk 6) imports from derivation/recipe (chunk 4), generation
  (chunk 6), and db (chunk 1)

**Rule 1 — Phase 1 never removes `pub` or `pub(crate)` items.** This is the
bright-line rule that makes parallel work safe. Phase 1 agents only remove
private (module-internal) dead code. Cross-visible items are deferred to
Phase 2, which runs on merged `main` with full codebase visibility.

**Rule 2 — respect non-textual liveness.** Not all usage is visible via grep.
Items that must be preserved even if grep shows no direct callers:

- Fields on `#[derive(Serialize, Deserialize)]` structs (wire format
  compatibility)
- Fields/methods consumed by macro-generated code (`rmcp`, `axum`, `clap`,
  etc.)
- Items behind `#[allow(dead_code)]` with a comment explaining why (e.g.,
  "Read by rmcp's tool_router macro") — these are intentionally kept alive
- Re-exports in `lib.rs` or `mod.rs` files

If an item looks dead but has a `#[derive(...)]` or sits in a struct used by
framework macros, leave it alone in Phase 1.

**Rule 3 — do not edit files outside the chunk.** Chunk 7 owns
`crates/conary-core/src/lib.rs`. If another chunk discovers an entire module
is dead, note it for chunk 7 — do not edit `lib.rs` from another chunk's
branch.

### Feature-Gated Code

Three feature flags affect dead-code visibility:

- **`composefs-rs`** (conary-core, default ON): gates code in `generation/`
  and `bootstrap/image.rs`. There are explicit `#[cfg(not(feature =
  "composefs-rs"))]` fallback paths in `generation/mount.rs` and
  `generation/builder.rs` that only compile without the feature.
- **`experimental`** (conary CLI, default OFF): gates the entire automation
  CLI surface in `cli/automation.rs`, `commands/automation.rs`, and
  `dispatch.rs`. Chunks 7 (automation module), 8, and 9 must build with
  `--features experimental` to avoid false dead-code positives.
- **`polkit`** (conaryd, default OFF): gates a branch in `daemon/auth.rs`.
  Chunk 11 must verify with `--features polkit`.

### Verification

Each chunk branch must pass the full workspace build before being marked
complete. The canonical commands (matching PR-gate CI; post-merge smoke in
`merge-validation.yml` is a separate backstop):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

Additional per-chunk feature verification:

```
# Chunk 6 — both sides of the composefs feature gate
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings

# Chunks 7, 8, 9 — automation CLI surface
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo test -p conary --features experimental --verbose

# Chunk 11 — polkit gate
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
```

### Merge Protocol

Chunks are merged to `main` one at a time. Before each merge:

1. **Rebase** the chunk branch onto current `main` (which may include
   previously merged chunks).
2. **Re-run full verification** (all commands above) after rebase.
3. **Resolve any conflicts** introduced by prior chunk merges.
4. Only merge when the rebased branch is fully green.

This is necessary because two independently green branches can conflict after
sequential merges — especially chunks 1-7 within conary-core where pub API
coupling is high.

### Execution

Chunks run in parallel (2-3 at a time) using isolated git worktrees. Each
agent works only within its designated modules. The branch receives one or more
commits describing what was simplified.

### Ordering

Start with smaller chunks (core-resolver, cli-dispatch, conaryd) to validate
the approach, then tackle the larger ones (core-db, core-model-ccs,
cli-commands, remi).

### Review

Each branch is independently reviewable. Branches that cause test failures
should be fixed before marking complete. Final merge follows the merge
protocol above.

## Phase 2: Cross-Chunk Simplification

Runs after all Phase 1 branches are merged to `main`. Phase 1 simplifies each
chunk in isolation and conservatively preserves all `pub`/`pub(crate)` items;
Phase 2 runs on the merged result and can see the full picture.

### What Phase 2 Does

#### Dead workspace-internal APIs
After Phase 1 cleans up internal callers independently, some `pub` items in
conary-core may have zero remaining callers in any crate. Neither Phase 1
agent could detect this because each saw the other's callers still present in
their worktree snapshot. Phase 2 runs on merged `main` and can verify true
global liveness.

To be clear: Phase 1 preserves all pub/pub(crate) items as a safety measure.
Phase 2 relaxes that constraint because it has full codebase visibility. Items
removed in Phase 2 are workspace-internal; the crate is not published, so
there are no external semver consumers.

#### Unused re-exports
`lib.rs` pub-uses that no external crate actually imports. Phase 1 agents
can't touch lib.rs (chunk 7 owns it) and can't verify cross-crate callers
from a worktree snapshot.

#### Shared trait/type bloat
Phase 1 may simplify internals while leaving over-specified shared interfaces.
Phase 2 looks at trait bounds, generic parameters, and type aliases that are
now wider than any consumer needs.

#### Cross-crate duplication (identify only)
Similar patterns repeated across app crates (config loading, error mapping,
HTTP response construction in conaryd vs remi, common job patterns). Phase 2
**identifies and documents** these but does not fix them — meaningful dedup
usually requires new shared abstractions, which is a refactor, not a
simplification. Findings are recorded for a future refactor pass.

### Execution

Phase 2 runs as a single pass on `main` (no worktree parallelism needed). It
is read-heavy: mostly grepping for callers and mapping usage before making
targeted removals. A single branch `simplify/cross-chunk` collects the
changes.

### Scope Boundary

Phase 2 is still simplification — removing dead code, trimming unused API
surface, clarifying. It does not introduce new abstractions, refactor module
boundaries, or add shared utilities. Cross-crate duplication that requires new
shared code is documented and deferred.
