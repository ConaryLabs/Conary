# Pre-Release Completeness: Design Spec

**Date:** 2026-04-08  
**Status:** Approved (rev 2 -- incorporates Codex review fixes)  
**Goal:** Close every stub, dead-end, and misleading exit code in the CLI before
public announcement. Implementation order is bottom-up: user-facing mutators
first, then automation, then stubs, then infrastructure.

---

## Phase 1: Model Apply + State Revert

These are the two flagship features shown in the README that don't work.
They share the same prerequisite work (batch-mode install/remove, live-host
safety gate) so they belong in the same phase.

### Problem

`model apply` computes a correct diff but stubs out the three core operations:
package install, package remove, and package update. It prints notes telling
users to run commands manually and returns success. `state revert` computes a
complete `RestorePlan` with exact operations, displays it, then bails.

Both are shown prominently in the README. Neither is gated by
`require_live_mutation()`, unlike `install`, `remove`, and `state rollback`.

### Prerequisite: Live-Host Safety Gate

Both `model apply` and `state revert` must call `require_live_mutation()` in
their dispatch arms, matching the pattern used by `install` (`dispatch.rs:61`),
`remove` (`dispatch.rs:78`), and `state rollback` (`dispatch.rs:579`).

Add `require_live_mutation(allow_live_system_mutation, ...)` before dispatching
to `cmd_model_apply` and `cmd_state_restore` in `dispatch.rs`.

**Files:** `apps/conary/src/dispatch.rs`

### Prerequisite: Batch-Mode Install/Remove

`cmd_install` creates its own changeset and state snapshot
(`install/mod.rs:1789`). `cmd_remove` does the same (`remove.rs:148`). Calling
them in a loop from `state revert` produces N changesets + N snapshots instead
of one atomic revert.

Add a `batch_mode: bool` field to `InstallOptions` and a corresponding
parameter to `cmd_remove`. When `batch_mode` is true:

- Skip per-operation changeset creation (caller provides the wrapping changeset)
- Skip per-operation state snapshot (caller creates one final snapshot)
- All other behavior (dependency resolution, scriptlets, CAS operations)
  unchanged

Default is `false`, preserving existing behavior for interactive
`conary install` / `conary remove`.

### Prerequisite: Architecture Selector

`InstallOptions` does not currently have an `architecture` field. `cmd_remove`
does not accept an architecture parameter. `StateMember` carries
`architecture`, and multi-arch systems have distinct entries like
`glibc.x86_64` vs `glibc.i686`.

Add `architecture: Option<String>` to `InstallOptions`. Add an `architecture:
Option<String>` parameter to `cmd_remove`. When set, the resolver and removal
logic filter to the specified architecture. When `None`, existing behavior is
preserved.

**Files:** `apps/conary/src/commands/install/mod.rs`,
`apps/conary/src/commands/remove.rs`

### Design: Model Apply

#### `apply_package_changes()` (apply.rs:370)

Make async, add `db_path` and `root` parameters. For each `DiffAction::Install`,
call `cmd_install` with `InstallOptions` (same pattern as
`apply_replatform_changes()` at apply.rs:174). For each `DiffAction::Remove`,
call `cmd_remove`. Collect errors per-package, continue on failure, return
`(applied_count, error_list)`.

Model apply does NOT use `batch_mode` -- each operation gets its own changeset,
matching the behavior of `apply_replatform_changes()` which already calls
`cmd_install` individually. Model apply is not an atomic revert; it's a
"make the system match the model" operation where partial progress is acceptable.

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

Use repository resolution (call `cmd_install`/`cmd_remove` with
`batch_mode: true`) rather than direct database manipulation. This preserves
dependency checking, scriptlet execution, and CAS operations. If a historical
package version is no longer in any enabled repository, the user gets an
explicit error listing what couldn't be restored and why.

#### Implementation (state.rs, replacing bail at line 215)

1. Create a wrapping changeset: `"Revert to state {N}"`.
2. Execute removals first: `cmd_remove` for each entry in `plan.to_remove`,
   with `batch_mode: true` and `architecture` from `StateMember`.
3. Execute installs: `cmd_install` with target version and architecture for
   each `plan.to_install`, with `batch_mode: true`.
4. Execute upgrades: `cmd_install` with target version for each
   `plan.to_upgrade`, with `batch_mode: true`.
5. Commit the wrapping changeset.
6. Create one final state snapshot via `StateEngine::create_snapshot()` with
   summary `"Reverted to state {N}"`.
7. On partial failure: report what succeeded and what failed. If any removals
   or installs failed, do not commit the changeset -- roll it back and report
   the failures. Atomic semantics: all or nothing.

### Files to modify

| File | Change |
|------|--------|
| `apps/conary/src/dispatch.rs` | Add `require_live_mutation()` gates for model apply and state revert |
| `apps/conary/src/commands/install/mod.rs` | Add `batch_mode` and `architecture` to `InstallOptions`; skip changeset/snapshot when batch |
| `apps/conary/src/commands/remove.rs` | Add `batch_mode` and `architecture` parameters; skip changeset/snapshot when batch |
| `apps/conary/src/commands/model/apply.rs` | Implement `apply_package_changes()`, fix Update handling |
| `apps/conary/src/commands/model.rs` | Update call site, wire autoremove |
| `apps/conary/src/commands/state.rs` | Replace bail with batch-mode execution loop |

### Testing

- Integration test: create a model with 2 installs and 1 remove, run
  `cmd_model_apply`, verify packages are installed/removed in the DB.
- Integration test: `--dry-run` mode still shows the plan without executing.
- Integration test: `--strict` mode bails on first failure.
- Integration test: create state snapshot, install a package, run
  `cmd_state_restore` to the old state, verify the package is removed and
  exactly ONE changeset + ONE snapshot was created for the revert.
- Integration test: state revert with a package version not in any repo
  reports a clear error and rolls back the changeset.
- Integration test: verify `require_live_mutation` blocks model apply and state
  revert when the flag is not set and root is `/`.

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

#### Typed action payloads

`PendingAction.details` is currently `Vec<String>` (free-form). Version info
for updates is embedded as display text, not structured data. Before the
executor can reliably extract target versions, we need typed payloads.

Add to `PendingAction`:

```rust
/// Typed payload for executor dispatch. Details remain for display.
pub enum ActionPayload {
    /// Install/update a package to a specific version.
    UpdatePackage { target_version: String },
    /// Remove packages (names already in PendingAction.packages).
    RemovePackages,
    /// Verify and restore files from CAS.
    RepairFiles { paths: Vec<PathBuf> },
}
```

Add `pub payload: ActionPayload` to `PendingAction`. Update the action builders
in `check.rs` to populate the payload alongside the existing `details` strings.
`details` remains for human-readable display; `payload` is for machine
dispatch.

#### Executor becomes a planner (crate boundary fix)

`ActionExecutor` lives in `conary-core`. `cmd_install`/`cmd_remove` live in
`apps/conary`. The executor cannot call CLI functions directly.

Rename `execute()` to `plan()`. It returns an `ActionPlan`:

```rust
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

`plan()` reads the typed `ActionPayload` to produce ops:

| Category | Payload | Produces |
|----------|---------|----------|
| Security | `UpdatePackage { target_version }` | `PlannedOp::Install` per package with version |
| Updates | `UpdatePackage { target_version }` | `PlannedOp::Install` per package with version |
| MajorUpgrades | `UpdatePackage { target_version }` | `PlannedOp::Install` per package with version |
| Orphans | `RemovePackages` | `PlannedOp::Remove` per package |
| Repair | `RepairFiles { paths }` | `PlannedOp::VerifyAndRestore` with paths |

#### CLI-side execution and history logging

The CLI-level `cmd_automation_apply` iterates plans and dispatches to
`cmd_install`/`cmd_remove`/verify (same crate, no boundary issue). After each
action's plan is executed, the CLI inserts a row into `automation_history`.

This keeps history logging in the CLI layer where the execution results are
known, not in the core crate's `plan()` method.

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

`cmd_automation_history` queries this table with the existing filter parameters
(limit, category, status, since).

Schema migration: increment to v66.

#### Automation configure

Wire `cmd_automation_configure` to read/write the `[automation]` section of the
system model (`system.toml`). No `save_model()` currently exists in
`conary-core`.

**`--show`:** call `load_model()` and display real values from the parsed
`AutomationConfig`.

**Write operations** (`--mode`, `--enable`, `--disable`, `--interval`,
`--enable-ai`, `--disable-ai`): load the raw TOML string, use `toml_edit` to
modify the `[automation]` section in place (preserving comments and formatting),
write back to the model path. This avoids needing a full `save_model()` API in
core -- `toml_edit` is already a workspace dependency. The write logic lives in
the CLI crate (`commands/automation.rs`).

#### Automation daemon background mode

Leave as `bail!()` with clear message. Daemonization (double-fork, setsid,
pidfile management) is a separate concern. The feature works interactively via
`automation apply` and `automation check`.

### Files to modify

| File | Change |
|------|--------|
| `crates/conary-core/src/automation/mod.rs` | Add `ActionPayload` to `PendingAction` |
| `crates/conary-core/src/automation/action.rs` | Rename `execute()` to `plan()`, return `ActionPlan` |
| `crates/conary-core/src/automation/check.rs` | Populate typed payloads in action builders |
| `crates/conary-core/src/db/schema.rs` | Migration v66: `automation_history` table |
| `apps/conary/src/commands/automation.rs` | Execute plans via cmd_install/cmd_remove, log history, wire configure |

### Testing

- Unit test: build a `PendingAction` with `ActionPayload::UpdatePackage`,
  call `plan()`, verify `PlannedOp::Install` with correct version.
- Unit test: verify `automation_history` row inserted after CLI execution.
- Unit test: `cmd_automation_configure --show` reads model values, not hardcoded
  defaults.
- Unit test: `cmd_automation_configure --mode auto` writes to system.toml via
  `toml_edit` and preserves comments.

---

## Phase 3: Implement Remaining Stubs + README

### `derivation build` -- Wire Up Existing Pipeline

**Current:** Computes derivation ID, prints `[NOT IMPLEMENTED]`, returns Ok.

**Implementation:** The build pipeline exists in `DerivationExecutor`
(`crates/conary-core/src/derivation/executor.rs:301`): cache lookup, Kitchen
build (prep/unpack/patch/simmer), output capture into CAS, manifest
serialization, provenance tracking, index recording.

**Executor signature requires:**
```rust
pub fn execute(
    &self,
    recipe: &Recipe,           // parsed, not a path
    build_env_hash: &str,
    dep_ids: &BTreeMap<String, DerivationId>,
    target_triple: &str,
    sysroot: &Path,            // directory, not an image file
    conn: &Connection,
) -> Result<ExecutionResult, ExecutorError>
```

**The CLI currently has** `recipe: &Path` and `env: &Path` (an image file).
Bridging this requires:

1. **Parse the recipe**: Call `parse_recipe_file(recipe)` (already done in
   the stub).
2. **Extract sysroot from env image**: The `env` parameter is a filesystem
   image (e.g., an ext4 or EROFS image). Mount it to a temp directory using
   `mount -o loop,ro` (or the generation mount infrastructure in
   `crates/conary-core/src/generation/`). The sysroot is the mount point.
   On cleanup, unmount.
3. **Handle `db_path: Option<&Path>`**: When `None`, use a temporary in-memory
   database for the derivation index. When `Some`, open the database. The
   executor needs the connection for its derivation index
   (`index.rs:lookup/insert`).
4. **Dependency IDs**: Currently passed as `BTreeMap::new()`. For standalone
   `derivation build`, this is acceptable -- full dependency resolution is the
   job of `profile generate` + `derivation pipeline`. Document that standalone
   `derivation build` does not resolve transitive dependency IDs.
5. **Target triple**: Call `current_target_triple()` (already exists in the
   stub).

This is ~50 lines, not ~25. The mount/unmount and db_path fallback add real
logic.

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

### `self-update --version X` -- Version-Specific Download

**Current:** Prints "not yet implemented", returns Err.

**Server endpoints available:**
- `GET /v1/ccs/conary/latest` -- returns `LatestVersionInfo` (version,
  download_url, sha256, signature)
- `GET /v1/ccs/conary/versions` -- returns list of available versions
- `GET /v1/ccs/conary/{version}/download` -- streams the CCS package

There is **no per-version metadata endpoint** returning sha256 + signature.
The `/latest` endpoint returns this info only for the latest version.

**Two-part fix:**

1. **Server side** (`apps/remi/src/server/handlers/self_update.rs`): add a new
   handler `get_version_info` for route
   `GET /v1/ccs/conary/{version}` that returns the same `LatestVersionInfo`
   format for a specific version. This mirrors `get_latest` but looks up the
   requested version instead of the most recent. Register the route in
   `routes/public.rs`.

2. **Client side** (`apps/conary/src/commands/self_update.rs`): replace the
   bail at line 128:
   - Validate version string is valid SemVer.
   - Call the new endpoint: `{channel_url}/conary/{requested_version}`.
   - Parse the same `LatestVersionInfo` response.
   - Check not already at that version (skip unless forced).
   - Remainder of flow (download, verify signature, replace) is identical to
     the latest-version path.

**Files:** `apps/remi/src/server/handlers/self_update.rs`,
`apps/remi/src/server/routes/public.rs`,
`apps/conary/src/commands/self_update.rs`

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

After Phases 1-3, every example in the README works.

- Change "Hermetic builds" in comparison table from "Yes (experimental)" to
  "Partial (experimental)" -- lockfile module is not yet integrated and
  bootstrap build helpers are unused. Out of scope for this push.
- Add a note in Bootstrap section that `derivation build` is functional but the
  full derivation pipeline is evolving.
- Keep the honest "No (early)" ecosystem maturity framing.
- No examples need to be removed or hidden.

---

## Phase 4: Substituter Remote Sources (Non-Critical Path)

**Note:** `SubstituterChain` has zero non-test callers in `apps/`. This phase
is real infrastructure work that enables future federation chunk resolution, but
it is **not on the critical path** for the public announcement. All user-facing
CLI commands work without it. Prioritize Phases 1-3 first.

### Problem

`SubstituterChain::fetch_from_source()` in
`crates/conary-core/src/repository/substituter.rs:202` dispatches on
`SubstituterSource`. `LocalCache` works. `Federation` and `Remi` return
`NotFound` with TODO stubs. `Binary` claims "does not serve individual chunks."

### Design

#### Make the substituter async

`fetch_from_source()`, `resolve_chunk()`, and `resolve_chunks()` become
`async fn`. All HTTP clients (`RemiClient`, `HttpChunkFetcher`,
`RepositoryClient`) are already async. The `LocalCache` path uses
`tokio::task::spawn_blocking` or inline `std::fs::read` (acceptable since it is
fast).

Update all callers (currently only test code and the re-export in
`repository/mod.rs`).

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
| Callers of `resolve_chunk`/`resolve_chunks` (test code + re-export) | Add `.await` |
| `crates/conary-core/src/repository/mod.rs` | Re-export changes if needed |

### Testing

- Unit test: mock `HttpChunkFetcher` returning known chunk data, verify
  `fetch_from_source` for Remi source writes to local cache.
- Unit test: mock federation peer DB with 3 peers (1 disabled, 1 failing, 1
  healthy), verify selection order and circuit-breaker skip.
- Integration test (conary-test phase): verify real Remi chunk fetch against
  remi.conary.io with a known package hash.

---

## Implementation Order Summary

| Phase | Description | Key risk | Critical path? |
|-------|-------------|----------|----------------|
| 1 | Model apply + state revert | batch_mode and arch additions touch install/remove internals | **Yes** |
| 2 | Automation executor | Typed payloads change `PendingAction` struct | **Yes** |
| 3 | Stubs + README | Recipe loading helper, self-update server endpoint | **Yes** |
| 4 | Substituter remote sources | Async conversion, no production callers yet | No |

Each phase is independently testable and committable. Phase 3's `profile
generate` and `cache populate --sources-only` share a recipe loading helper that
should be built first within that phase. Phase 3's `self-update --version`
requires a server-side endpoint addition before the client change.
