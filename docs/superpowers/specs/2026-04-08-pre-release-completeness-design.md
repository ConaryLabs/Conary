# Pre-Release Completeness: Design Spec

**Date:** 2026-04-08  
**Status:** Approved  
**Goal:** Close every stub, dead-end, and misleading exit code in the CLI before public announcement. Implementation order is bottom-up: infrastructure first, then user-facing commands, then cleanup.

---

## Phase 1: Substituter Remote Sources

### Problem

`SubstituterChain::fetch_from_source()` in
`crates/conary-core/src/repository/substituter.rs:202` dispatches on
`SubstituterSource`. `LocalCache` works. `Federation` and `Remi` return
`NotFound` with TODO stubs. `Binary` claims "does not serve individual chunks."

Federation has 8 CLI subcommands for peer management, but the underlying chunk
resolution cannot actually fetch packages through peers. Remi's chunk-level
diffing -- a headline feature -- is not wired into the substituter.

### Design

#### Make the substituter async

`fetch_from_source()`, `resolve_chunk()`, and `resolve_chunks()` become
`async fn`. All HTTP clients (`RemiClient`, `HttpChunkFetcher`,
`RepositoryClient`) are already async. The `LocalCache` path uses
`tokio::task::spawn_blocking` or inline `std::fs::read` (acceptable since it is
fast).

Update all callers (~4 call sites in the codebase).

#### Remi source

`AsyncRemiClient` already has a `CompositeChunkFetcher` with
`HttpChunkFetcher` under the hood. `RemiClientCore::chunk_url()` at
`repository/remi.rs:149` constructs `/v1/chunks/{hash}` URLs.

Implementation:
1. Construct `HttpChunkFetcher` with the Remi endpoint's chunk URL pattern.
2. Call `fetcher.fetch(hash).await`.
3. On success, write the chunk to local cache for future hits using
   `LocalCacheFetcher::store()`.
4. ~30 lines of new code in `fetch_from_source()`.

#### Federation source

Needs peer lookup then HTTP fetch.

1. Query `federation_peers` table filtered by `tier` and `is_enabled = true`.
2. Sort by `latency_ms` ascending, break ties by `success_count`.
3. Skip peers with `consecutive_failures > 5` (circuit breaker).
4. For each candidate peer, construct chunk URL from
   `peer.endpoint + "/v1/cas/chunks/{hash}"`.
5. Try peers in order until one succeeds.
6. Update `success_count`/`failure_count`/`latency_ms` on the peer record.
7. Write fetched chunk to local cache.

Requires plumbing `conn: Option<&Connection>` into `SubstituterChain` (not
currently present). Federation source calls are skipped when `conn` is `None`.

#### Remove Binary from chunk-level substituter

Binary repos serve whole packages (RPMs, DEBs), not individual CAS chunks.
Binary resolution already works at the package level in `resolution.rs:530`.
Remove `SubstituterSource::Binary` from the enum. Any remaining references
become compile errors that point to the places needing cleanup.

### Files to modify

| File | Change |
|------|--------|
| `crates/conary-core/src/repository/substituter.rs` | Async conversion, Remi + Federation implementation, Binary removal |
| Callers of `resolve_chunk`/`resolve_chunks` (~4 sites) | Add `.await` |
| `crates/conary-core/src/repository/mod.rs` | Re-export changes if needed |

### Testing

- Unit test: mock `HttpChunkFetcher` returning known chunk data, verify
  `fetch_from_source` for Remi source writes to local cache.
- Unit test: mock federation peer DB with 3 peers (1 disabled, 1 failing, 1
  healthy), verify selection order and circuit-breaker skip.
- Integration test (conary-test phase): verify real Remi chunk fetch against
  remi.conary.io with a known package hash.

---

## Phase 2: Automation Executor

### Problem

`ActionExecutor::execute()` at `crates/conary-core/src/automation/action.rs:253`
returns `Failed` with "Action execution not yet implemented" for every
`AutomationCategory`. The CLI in `automation.rs:255-274` counts `Ok(_)` as
success, so fixing the executor makes the CLI work.

Additionally:
- `automation configure` prints hardcoded defaults instead of reading config;
  writes are no-ops.
- `automation history` bails with "not yet implemented."
- `automation daemon` background mode is a stub.

### Design

#### Executor changes

**Crate boundary:** `ActionExecutor` lives in `conary-core`, but `cmd_install`
and `cmd_remove` live in `apps/conary`. The executor cannot call CLI functions
directly.

Solution: `execute()` returns an `ActionPlan` describing what operations to
perform. The CLI-level `cmd_automation_apply` iterates the plan and dispatches
to `cmd_install`/`cmd_remove`/verify.

```rust
// In conary-core/src/automation/action.rs
pub enum PlannedOp {
    Install { package: String, version: Option<String> },
    Remove { package: String },
    VerifyAndRestore { files: Vec<PathBuf> },
}

pub struct ActionPlan {
    pub ops: Vec<PlannedOp>,
    pub category: AutomationCategory,
    pub action_id: String,
}
```

`ActionExecutor::plan()` replaces `execute()`. It maps each category to ops:

| Category | Produces |
|----------|----------|
| Security | `PlannedOp::Install` per package (version from action details) |
| Updates | `PlannedOp::Install` per package |
| MajorUpgrades | `PlannedOp::Install` per package |
| Orphans | `PlannedOp::Remove` per package |
| Repair | `PlannedOp::VerifyAndRestore` per file set |

The CLI layer in `cmd_automation_apply` iterates plans and calls
`cmd_install`/`cmd_remove` (same crate, no boundary issue). This keeps
`conary-core` free of CLI dependencies.

Security/Updates/MajorUpgrades differ only in policy (deadline urgency, approval
requirements), which the checker handles before actions reach the executor. By
the time `plan()` runs, the decision to apply has been made.

Each action executes independently. If one package in a multi-package action
fails, record the failure, continue, report partial success. The executor's
existing `executed`/`failed` tracking handles this.

#### Automation history table

New table `automation_history`:

```sql
CREATE TABLE automation_history (
    id INTEGER PRIMARY KEY,
    action_id TEXT NOT NULL,
    category TEXT NOT NULL,
    packages TEXT,          -- JSON array
    status TEXT NOT NULL,   -- 'applied', 'failed', 'partial'
    error_message TEXT,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

`execute()` inserts a row after each action. `cmd_automation_history` queries
this table with the existing filter parameters (limit, category, status, since).

Schema migration: increment to v66.

#### Automation configure

Wire `cmd_automation_configure` to read/write the `[automation]` section of the
system model (`system.toml`). `--show` calls `load_model()` and displays real
values. Write operations (`--mode`, `--enable`, `--disable`, `--interval`,
`--enable-ai`, `--disable-ai`) update the model and call `save_model()`.

#### Automation daemon background mode

Leave as `bail!()` with clear message. Daemonization (double-fork, setsid,
pidfile management) is a separate concern. The feature works interactively via
`automation apply` and `automation check`.

### Files to modify

| File | Change |
|------|--------|
| `crates/conary-core/src/automation/action.rs` | Executor constructor + category dispatch |
| `crates/conary-core/src/automation/mod.rs` | Re-export changes |
| `crates/conary-core/src/db/schema.rs` | Migration v66: `automation_history` table |
| `apps/conary/src/commands/automation.rs` | Update `cmd_automation_apply` caller, implement `cmd_automation_history`, wire `cmd_automation_configure` |

### Testing

- Unit test: create executor with test DB containing a security-update candidate,
  execute a Security action, verify the package was updated.
- Unit test: create executor with orphaned package, execute Orphans action,
  verify removal.
- Unit test: verify `automation_history` row inserted after execution.
- Unit test: `cmd_automation_configure --show` reads model values, not hardcoded
  defaults.

---

## Phase 3: Model Apply + State Revert

### Problem

`model apply` computes a correct diff but stubs out the three core operations:
package install, package remove, and package update. It prints notes telling
users to run commands manually and returns success. `state revert` computes a
complete `RestorePlan` with exact operations, displays it, then bails.

Both are shown prominently in the README.

### Design: Model Apply

#### `apply_package_changes()` (apply.rs:370)

Make async, add `db_path` and `root` parameters. For each `DiffAction::Install`,
call `cmd_install` with `InstallOptions` (same pattern as
`apply_replatform_changes()` at apply.rs:174). For each `DiffAction::Remove`,
call `cmd_remove`. Collect errors per-package, continue on failure, return
`(applied_count, error_list)`.

#### `DiffAction::Update` handling (apply.rs:545)

Replace the stub print with `cmd_install(package, InstallOptions { version:
Some(target_version), allow_downgrade: true, ... })`. Install with a target
version is how updates work in this codebase.

#### Autoremove step (model.rs:574)

Replace stub with `cmd_autoremove(db_path, root, sandbox_mode).await`. Only
runs when `opts.autoremove` is true (already gated).

#### Exit code

If `opts.strict`, bail on first error. Otherwise collect all errors and report
in the summary. Return `Err` only if every operation failed. Partial success
with some failures returns `Ok` but prints a warning with the failure list.

### Design: State Revert

#### Execution strategy

Use repository resolution (call `cmd_install`/`cmd_remove`) rather than
direct database manipulation. This preserves all safety guarantees: dependency
checking, scriptlet execution, changeset recording. If a historical package
version is no longer in any enabled repository, the user gets an explicit error
listing what couldn't be restored and why.

#### Implementation (state.rs, replacing bail at line 215)

1. Create a wrapping changeset: `"Revert to state {N}"`.
2. Execute removals first: `cmd_remove` for each entry in `plan.to_remove`.
3. Execute installs: `cmd_install` with target version for each `plan.to_install`.
4. Execute upgrades: `cmd_install` with target version for each `plan.to_upgrade`.
5. On full success: `StateEngine::create_snapshot()` with summary
   `"Reverted to state {N}"`.
6. On partial failure: report what succeeded and what failed, do not snapshot.

#### Multi-arch handling

`StateMember` carries `architecture`. Pass it through to `cmd_install` /
`cmd_remove` so that `glibc.x86_64` and `glibc.i686` are treated as distinct
operations.

### Files to modify

| File | Change |
|------|--------|
| `apps/conary/src/commands/model/apply.rs` | Implement `apply_package_changes()`, fix Update handling |
| `apps/conary/src/commands/model.rs` | Update call site, wire autoremove |
| `apps/conary/src/commands/state.rs` | Replace bail with execution loop |

### Testing

- Integration test: create a model with 2 installs and 1 remove, run
  `cmd_model_apply`, verify packages are installed/removed in the DB.
- Integration test: `--dry-run` mode still shows the plan without executing.
- Integration test: `--strict` mode bails on first failure.
- Integration test: create state snapshot, install a package, run
  `cmd_state_restore` to the old state, verify the package is removed.
- Integration test: state revert with a package version not in any repo
  reports a clear error.

---

## Phase 4: Implement Remaining Stubs + README

### `derivation build` -- Wire Up Existing Pipeline

**Current:** Computes derivation ID, prints `[NOT IMPLEMENTED]`, returns Ok.

**Implementation:** The entire build pipeline exists in `DerivationExecutor`
(`crates/conary-core/src/derivation/executor.rs:301`): cache lookup, Kitchen
build (prep/unpack/patch/simmer), output capture into CAS, manifest
serialization, provenance tracking, index recording.

Replace the stub (~lines 46-55 in `commands/derivation.rs`) with:
1. Create `CasStore` from `cas_dir`.
2. Create `DerivationExecutor` with the store.
3. Open database from `db_path`.
4. Call `executor.execute(recipe, build_env_hash, &dep_ids, target_triple,
   &sysroot, &conn)`.
5. Print `CacheHit` or `Built` result.

~25 lines replacing the stub.

**File:** `apps/conary/src/commands/derivation.rs`

### `profile generate` -- Recipe Loading Pipeline

**Current:** Checks manifest exists, prints `[NOT IMPLEMENTED]`, returns Ok.

**Implementation:** Types exist (`BuildProfile`, `SystemManifest`,
`ProfileStage`, `ProfileDerivation`). `Pipeline::generate_profile()` exists but
produces "pending" derivation IDs. Recipe parsing works. Stage classification
exists in `build_order.rs`.

Missing piece: recipe loading + dependency resolution glue.

1. Load `SystemManifest` from provided path (parser exists).
2. For each package in manifest, locate recipe file in recipes directory.
3. Parse each recipe, extract `makedepends`/`requires`.
4. Resolve dependency graph to build order (topological sort using stage
   classification from `build_order.rs`).
5. Compute derivation IDs via `DerivationId::compute()`.
6. Populate profile stages with real IDs.
7. Write via `BuildProfile::to_toml()`.

The recipe loading + dependency resolution is ~100-150 lines of new code in a
shared helper in `crates/conary-core/src/derivation/`. Both `profile generate`
and `cache populate` consume it.

**Files:** `apps/conary/src/commands/profile.rs`,
new `crates/conary-core/src/derivation/recipe_loader.rs`

### `cache populate --sources-only` -- Download Source Tarballs

**Current:** Prints `[NOT IMPLEMENTED]`, returns Ok.

**Implementation:** Source download infrastructure exists in
`bootstrap/build_runner.rs` (`fetch_source()` with URL extraction and checksum
verification). Recipe `archive_url()` and `archive_filename()` methods exist.

1. Load profile, iterate derivations.
2. For each derivation, load its recipe file (recipe loading helper from above).
3. Extract source URL via `recipe.archive_url()`.
4. Download to sources cache with checksum verification.
5. Skip already-cached sources.

~40 lines replacing the stub, using recipe loading helper and existing download
infrastructure.

**Depends on:** recipe loading helper from `profile generate`.

**File:** `apps/conary/src/commands/cache.rs`

### `self-update --version X` -- Version-Specific Endpoint

**Current:** Prints "not yet implemented", returns Err.

**Implementation:** 95% done. Existing flow queries `{channel_url}/latest` for
`LatestVersionInfo`, downloads, verifies signature, applies atomically.

Replace the bail at `self_update.rs:128`:
1. Validate version string is valid SemVer.
2. Fetch version-specific metadata: `{channel_url}/conary/{requested_version}`.
3. Check not already at that version (skip unless forced).
4. Remainder of flow (download, verify, replace) is identical.

~30 lines replacing the stub.

**File:** `apps/conary/src/commands/self_update.rs`

### `recipe-audit --trace` -- Downgrade to Warning

Static analysis runs regardless. Change `println!` to
`tracing::warn!("--trace mode is not yet available; running static analysis
only")`. The command still does useful work. Runtime trace instrumentation is a
separate project.

**File:** `apps/conary/src/commands/recipe_audit.rs`

### Exit Code Audit

Grep for `println!("...[NOT IMPLEMENTED]..."); Ok(())` across all commands.
After this push, the only remaining instances should be:
- `automation ai *` (behind `--features experimental`, invisible to users)
- `automation daemon` background mode (bails with clear message)

Convert any remaining instances to `bail!()` with a helpful message.

### README Updates

After Phases 1-4, every example in the README works.

- Change "Hermetic builds" in comparison table from "Yes (experimental)" to
  "Partial (experimental)" -- lockfile module is not yet integrated and
  bootstrap build helpers are unused. Out of scope for this push.
- Add a note in Bootstrap section that `derivation build` is functional but the
  full derivation pipeline is evolving.
- Keep the honest "No (early)" ecosystem maturity framing.
- No examples need to be removed or hidden.

---

## Implementation Order Summary

| Phase | Description | Key risk | Approx scope |
|-------|-------------|----------|--------------|
| 1 | Substituter remote sources | Async conversion touches ~4 call sites | Medium |
| 2 | Automation executor | New schema migration (v66) | Medium |
| 3 | Model apply + state revert | Error handling for partial failures | Medium |
| 4 | Stubs + README | Recipe loading helper is shared dependency | Mixed (small to medium) |

Each phase is independently testable and committable. Phase 4's `profile
generate` and `cache populate --sources-only` share a recipe loading helper that
should be built first within that phase.
