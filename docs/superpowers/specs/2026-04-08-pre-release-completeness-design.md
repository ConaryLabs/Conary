# Pre-Release Completeness: Design Spec

**Date:** 2026-04-08  
**Status:** Approved (rev 5 -- aligned with implementation plan)
**Goal:** Close every stub, dead-end, and misleading exit code in the CLI before
public announcement. Implementation order is bottom-up: user-facing mutators
first, then automation, then stubs, then infrastructure.

---

## Phase 1: Model Apply + State Revert

These are the two flagship features shown in the README that don't work.
They share the same prerequisite work (extracted inner install/remove helpers,
live-host safety gate, architecture selector) so they belong in the same phase.
This phase also tightens the install model Conary should expose going forward:
RPM/DEB/Arch are ingress formats, CCS is the native install model, and shared
mutation code should operate on format-neutral install semantics instead of a
legacy-only enum.

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

### Prerequisite: Extract Inner Install/Remove Helpers

`cmd_install` and `cmd_remove` each own their entire lifecycle: open a DB
transaction, create a changeset, record file history, call
`rebuild_and_mount()`, and mark the changeset applied. A `batch_mode: bool`
flag is not sufficient because the transaction boundaries and
`rebuild_and_mount()` calls are deeply embedded in the function bodies
(`install/mod.rs:1518`, `remove.rs:239`, `install/mod.rs:1689`,
`remove.rs:313`).

**Extract inner helpers:**

- `install_inner(tx: &Transaction<'_>, changeset_id: i64, package, opts)` --
  performs the DB transaction body (trove insert, dependency recording, file
  history, CAS operations) inside the caller-owned DB transaction. Skips
  `rebuild_and_mount()` and explicit state snapshot creation. Returns the trove
  ID and any data needed for post-commit finalization.

- `remove_inner(tx: &Transaction<'_>, changeset_id: i64, package, opts)` --
  performs the DB transaction body (dependency check, trove delete, file
  history recording) inside the caller-owned DB transaction. Skips
  `rebuild_and_mount()` and explicit state snapshot creation. Returns the
  removed `TroveSnapshot` so the caller can persist rollback metadata in a
  backward-compatible format.

The existing `cmd_install` and `cmd_remove` become thin wrappers: create
changeset, call the inner helper, call `rebuild_and_mount()`, and mark the
changeset applied. Snapshot creation remains owned by the generation build that
`rebuild_and_mount()` already performs. No behavioral change for interactive
`conary install` / `conary remove`.

**Install semantics redesign:** This extraction is also where Phase 1 stops
letting `PackageFormatType` drive the mutation path. Legacy packages may still
be parsed or converted during preparation, but shared execution should consume
format-neutral install semantics (version-scheme + scriptlet-format + source
kind) so CCS and legacy-prepared packages use the same install/remove/revert
machinery.

**Atomicity scope:** DB and generation state are atomic (one changeset, one
rebuild, one snapshot). Pre-remove scriptlets that partially execute cannot be
rolled back -- this is consistent with RPM, dpkg, and pacman which also have
no pre-remove rollback mechanism.

### Prerequisite: Architecture Selector

`InstallOptions` does not currently have an `architecture` field. `cmd_remove`
does not accept an architecture parameter. `StateMember` carries
`architecture`, and multi-arch systems have distinct entries like
`glibc.x86_64` vs `glibc.i686`.

Add `architecture: Option<String>` to `InstallOptions` and as a parameter to
`remove_inner`. When set, the resolver and removal logic filter to the
specified architecture. When `None`, existing behavior is preserved.

**Files:** `apps/conary/src/commands/install/mod.rs`,
`apps/conary/src/commands/remove.rs`

### Design: Model Apply

#### `apply_package_changes()` (apply.rs:370)

Make async, add `db_path` and `root` parameters. For each `DiffAction::Install`,
call `cmd_install` with `InstallOptions` (same pattern as
`apply_replatform_changes()` at apply.rs:174). For each `DiffAction::Remove`,
call `cmd_remove`. Collect errors per-package, continue on failure, return
`(applied_count, error_list)`.

Model apply does NOT use the inner helpers -- each operation gets its own changeset,
matching the behavior of `apply_replatform_changes()` which already calls
`cmd_install` individually. Model apply is not an atomic revert; it's a
"make the system match the model" operation where partial progress is acceptable.

#### `DiffAction::Update` handling (apply.rs:545)

Replace the stub print with `cmd_install(package, InstallOptions { version:
Some(target_version), allow_downgrade: true, ... })`. Install with a target
version is how updates work in this codebase.

#### Autoremove step (model.rs:574)

Replace stub with `cmd_autoremove(db_path, root, dry_run, no_scripts,
sandbox_mode).await` (matching the real signature at `remove.rs:366`). Only
runs when `opts.autoremove` is true (already gated). `dry_run` comes from
`opts.dry_run`, `no_scripts` defaults to `false`.

#### Exit code

If `opts.strict`, bail on first error. Otherwise collect all errors and report
in the summary. Return `Err` only if every operation failed. Partial success
with some failures returns `Ok` but prints a warning with the failure list.

### Design: State Revert

#### Execution strategy

Use the extracted inner helpers (`install_inner`/`remove_inner`) with a shared
connection and changeset, plus repository resolution for version lookup. This
preserves dependency checking, scriptlet execution, and CAS operations while
giving the caller control over the transaction boundary. If a historical
package version is no longer in any enabled repository, the user gets an
explicit error listing what couldn't be restored and why.

Restore preflight must validate against a capability-aware destination-state
view, not just package names. Conary already resolves tracked dependencies via
provides/capabilities (`ProvideEntry::find_satisfying_provider_fuzzy`,
`check_provides_dependencies`), so restore must reuse that model for target
state validation.

Rollback compatibility is part of Phase 1. The wrapping revert changeset will
contain multiple removals and possibly installs/upgrades, so the existing
single-`TroveSnapshot` metadata assumption in `cmd_rollback` must be extended
in a backward-compatible way.

#### Implementation (state.rs, replacing bail at line 215)

State revert must own the `TransactionEngine` lifecycle that `cmd_install`
and `cmd_remove` normally own individually. This matches the existing mutation
model (`install/mod.rs:1460-1479`, `remove.rs:113-117`).

1. Create `TransactionEngine` from `TransactionConfig::from_paths(root, db_path)`.
2. Call `engine.recover(conn)` to clean up any incomplete transactions from
   prior crashes.
3. Call `engine.begin()` to acquire the mutation lock.
4. Open a DB transaction and create one wrapping changeset:
   `"Revert to state {N}"`.
5. Build a `TargetStateView` for the destination state that includes both:
   destination members keyed by `(trove_name, architecture)`, and a
   capability/provider view compatible with Conary's existing tracked-provider
   semantics.
6. Pre-resolve installs/upgrades without mutating the live system. Legacy
   inputs may be parsed or converted, CCS inputs may be parsed directly, but
   all prepared packages must produce shared `InstallSemantics`.
7. Execute removals: `remove_inner(tx, changeset_id, ...)` for each entry
   in `plan.to_remove`, with `architecture` from `StateMember`. Collect the
   returned `TroveSnapshot`s in memory.
8. Execute installs: resolve the package from repositories (for version and
   CAS content), validate dependencies against the capability-aware target
   state view, then `install_inner(tx, changeset_id, ...)` for each
   `plan.to_install`, with target version and architecture.
9. Execute upgrades: same pattern as installs, but with target version and
   downgrade-aware semantics for each `plan.to_upgrade`.
10. Serialize removed troves into a rollback-compatible revert metadata wrapper,
    e.g. `{ removed_troves: Vec<TroveSnapshot> }`, and persist it on the
    wrapping changeset before commit.
11. If all operations succeeded: commit the DB transaction, call
    `rebuild_and_mount()` once, and rely on the generation build to create the
    single new active state snapshot with summary `"Reverted to state {N}"`.
12. Mark the wrapping changeset `Applied` only after rebuild succeeds.
13. Call `engine.release_lock()`.
14. On any failure: roll back the DB transaction, release the lock, and report
    what failed. No `rebuild_and_mount()`, no snapshot.

Rollback handling in `system.rs` must then accept either:
- the legacy single `TroveSnapshot` JSON used by current remove/upgrade
  rollback, or
- the new revert metadata wrapper containing `removed_troves: Vec<TroveSnapshot>`

For rollback of a revert changeset, Conary should:
1. remove any troves installed by that revert changeset via
   `installed_by_changeset_id`
2. restore every removed trove from the revert metadata
3. mark the original revert changeset `rolled_back`

**Atomicity scope:** DB state and generation image are atomic. Scriptlet
side-effects (pre-remove scripts that partially execute) are not rollback-safe.
This is consistent with RPM, dpkg, and pacman.

### Files to modify

| File | Change |
|------|--------|
| `apps/conary/src/dispatch.rs` | Add `require_live_mutation()` gates for model apply and state revert |
| `apps/conary/src/commands/install/mod.rs` | Extract `install_inner()`; add `architecture` to `InstallOptions`; add owned prepared-install helpers; `cmd_install` becomes thin wrapper |
| `apps/conary/src/commands/install/prepare.rs` | Move version/upgrade logic behind format-neutral install semantics |
| `apps/conary/src/commands/install/scriptlets.rs` | Move scriptlet-format logic behind format-neutral install semantics |
| `apps/conary/src/commands/install/conversion.rs` | Expose non-executing CCS preparation for shared restore path |
| `apps/conary/src/commands/install/resolve.rs` | Thread `architecture` through `ResolutionOptions` |
| `apps/conary/src/commands/remove.rs` | Extract `remove_inner()`; add `architecture` parameter; `cmd_remove` becomes thin wrapper |
| `apps/conary/src/commands/model/apply.rs` | Implement `apply_package_changes()`, fix Update handling |
| `apps/conary/src/commands/model.rs` | Update call site, wire autoremove |
| `apps/conary/src/commands/state.rs` | Replace bail with inner-helper execution loop + TransactionEngine lifecycle |
| `apps/conary/src/commands/system.rs` | Extend rollback parser/dispatcher to accept revert metadata wrapper |

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
- Integration test: restore preflight accepts a target package that satisfies
  dependencies via a tracked capability/provide, not just exact package name.
- Unit/integration test: rollback metadata parser accepts both the legacy
  single `TroveSnapshot` JSON and the new revert metadata wrapper.
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
pub struct InstalledPackageRef {
    pub name: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
}

/// Typed payload for executor dispatch. Details remain for display.
pub enum ActionPayload {
    /// Install/update a package to a specific version.
    UpdatePackage {
        target_version: String,
        architecture: Option<String>,
    },
    /// Remove concrete installed troves. packages remains display-oriented.
    RemovePackages { installed: Vec<InstalledPackageRef> },
    /// Restore a concrete installed trove with corrupted/missing files from CAS.
    RestorePackage { installed: InstalledPackageRef },
}
```

Add `pub payload: ActionPayload` to `PendingAction`. Update the action builders
in `check.rs` to populate the payload alongside the existing `details` strings.
`details` remains for human-readable display; `payload` is for machine
dispatch. Keep `PendingAction.packages` as display-oriented strings; the typed
payload carries the concrete installed trove identity Conary already needs for
multi-version and multi-arch correctness.

Security actions must also thread the repository target version through
`find_security_updates()` into `security_update_action()`. The query already
selects `rp.version`; Phase 2 must preserve that value instead of dropping it
before action construction.

Typed payloads also need a stable action identity. The current builder uses a
timestamp-based ID, which makes the same logical action appear new on every
scan and breaks defer/history correlation. Phase 2 should replace that with a
deterministic action key derived from normalized category + payload + concrete
package selectors. `identified_at` remains the scan timestamp; only the action
ID becomes stable.

#### Executor becomes a planner (crate boundary fix)

`ActionExecutor` lives in `conary-core`. `cmd_install`/`cmd_remove` live in
`apps/conary`. The executor cannot call CLI functions directly.

Rename `execute()` to `plan()`. It returns an `ActionPlan`:

```rust
pub enum PlannedOp {
    Install {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Remove {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Restore {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
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
| Security | `UpdatePackage { target_version, architecture }` | `PlannedOp::Install` per package with version + architecture |
| Updates | `UpdatePackage { target_version, architecture }` | `PlannedOp::Install` per package with version + architecture |
| MajorUpgrades | `UpdatePackage { target_version, architecture }` | `PlannedOp::Install` per package with version + architecture |
| Orphans | `RemovePackages { installed }` | `PlannedOp::Remove` per concrete installed trove |
| Repair | `RestorePackage { installed }` | `PlannedOp::Restore` per concrete installed trove (see below) |

The detection queries in `check.rs` must explicitly select and parse the needed
architecture columns, rather than letting payload architecture default to
`None`. For update/security/major-upgrade actions, select `t.architecture` and
`rp.architecture`, then prefer the repo-package architecture when present and
otherwise fall back to the installed trove architecture. For orphan/repair
actions, select the installed trove architecture directly from `troves`.

#### Repair: group by package in check.rs

Current code in `check.rs:465-477` detects corrupted files as raw paths and
emits a single repair action with `package: None`. This doesn't map to the
package-level `cmd_restore` path.

Change `check_integrity()` to:
1. Query `files` joined with `troves` (`SELECT t.name, f.path, f.sha256_hash
   FROM files f JOIN troves t ON f.trove_id = t.id`) so each corrupted file
   is associated with its owning package.
2. Group corrupted files by concrete installed trove identity
   (`name/version/architecture`), not just display name.
3. Emit one `PendingAction` per affected package, with
   `packages: vec![package_name]`,
   `payload: ActionPayload::RestorePackage { installed }`, and `details`
   listing the corrupted file paths for display.

This gives the planner one `PlannedOp::Restore { package, version, architecture
}` per package, which the CLI dispatches to `cmd_restore(...)` with a concrete
selector.

#### MajorUpgrades: wire end-to-end

The `MajorUpgrades` category exists in the planner table but is not wired
through the CLI surface:
- CLI category parser (`automation.rs:32`) does not accept `"major_upgrades"`
- `automation status` hardcodes `major_upgrades: 0`
- `automation check` never prints MajorUpgrades actions
- Daemon summary zeroes it out

Wire MajorUpgrades through all CLI paths:
- Add `"major_upgrades" | "major-upgrades"` to the category parser
- `automation status` queries the checker for real MajorUpgrades count
- `automation check` prints MajorUpgrades actions alongside other categories
- Daemon summary includes MajorUpgrades in its reporting

#### Repair: delegate to existing cmd_restore

Conary already has `cmd_restore` (`restore.rs:21`) and `cmd_restore_all`
(`restore.rs:129`) which verify CAS presence, rebuild EROFS, and remount.

```rust
pub enum PlannedOp {
    Install {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Remove {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },
    Restore {
        package: String,
        version: Option<String>,
        architecture: Option<String>,
    },  // delegates to cmd_restore
}
```

The CLI layer dispatches `PlannedOp::Restore { package, version, architecture }` to
`cmd_restore(...)`. If the current restore/remove helpers are still name-only,
Phase 2 extends those existing command entry points to accept `version` /
`architecture` selectors and to honor the passed `root`, rather than inventing
a parallel automation-only executor. This keeps repair tied to Conary's
existing restore semantics. For restore specifically, replace the current
`find_one_by_name()` lookup with `find_by_name()` plus explicit
version/architecture filtering so automation does not silently restore the first
matching package on multi-version or multi-arch systems.

#### CLI-side execution and history logging

The CLI-level `cmd_automation_apply` iterates plans and dispatches to
`cmd_install`/`cmd_remove`/`cmd_restore` helper (same crate, no boundary
issue). After each action's plan is executed, the CLI inserts a row into
`automation_history`.

This keeps history logging in the CLI layer where the execution results are
known, not in the core crate's `plan()` method.

Because `automation apply` now performs the same live-system mutations as
install/remove/restore, its dispatcher arm must also call
`require_live_mutation(...)` before invoking `cmd_automation_apply`. Dry-run
remains allowed without the override; real live mutation does not.

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
That migration work includes both:
- the new `migrate_v66()` function, and
- the `66 => migrations::migrate_v66(conn)` arm in the schema dispatcher
  (`db/schema.rs`)

#### Automation configure

Wire `cmd_automation_configure` to read/write the `[automation]` section of the
system model (`system.toml`). No `save_model()` currently exists in
`conary-core`.

**`--show`:** call `load_model()` and display real values from the parsed
`AutomationConfig`.

**Write operations** (`--mode`, `--enable`, `--disable`, `--interval`,
`--enable-ai`, `--disable-ai`): load the raw TOML string, use `toml_edit` to
modify the `[automation]` and `[automation.ai_assist]` sections in place
(preserving comments and formatting), write back to the model path. This avoids
needing a full `save_model()` API in core -- `toml_edit` is already a workspace
dependency. The write logic lives in the CLI crate (`commands/automation.rs`).

To keep this testable without writing `/etc/conary/system.toml`, Phase 2 should
add a small path-aware helper in the CLI layer so tests can point
`cmd_automation_configure` at a temp model file while production still defaults
to `DEFAULT_MODEL_PATH`.

When config writes succeed, `cmd_automation_configure` should print a short note
that an already-running foreground automation daemon must be restarted to pick
up the new settings. Dynamic config reload is not part of Phase 2.

#### Automation daemon: foreground-only, fix help text

Background daemonization (double-fork, setsid) is not worth implementing when
the supported deployment model is systemd service management. The current CLI
help says "Run automation daemon in background" which is misleading.

Concrete change:
- Change the CLI help text (`cli/automation.rs:112`) to:
  `"Run automation daemon (use systemd for background operation)"`
- Remove the `--foreground` flag entirely -- foreground is the only mode.
- Remove the `if !foreground` bail in `cmd_automation_daemon`
  (`automation.rs:426-431`).

### Files to modify

| File | Change |
|------|--------|
| `crates/conary-core/src/automation/mod.rs` | Add `InstalledPackageRef` + `ActionPayload` to `PendingAction` |
| `crates/conary-core/src/automation/action.rs` | Rename `execute()` to `plan()`, return `ActionPlan` with concrete trove selectors |
| `crates/conary-core/src/automation/check.rs` | Populate typed payloads in action builders; thread security target versions; wire MajorUpgrades detection |
| `crates/conary-core/src/db/schema.rs` | Migration v66: bump schema version and add the v66 dispatcher arm |
| `apps/conary/src/dispatch.rs` | Add `require_live_mutation()` gate for `automation apply` |
| `apps/conary/src/commands/automation.rs` | Execute plans via cmd_install/cmd_remove/restore, log history, add path-aware configure helper, wire MajorUpgrades status/check, fix daemon help |
| `apps/conary/src/cli/automation.rs` | Add `major_upgrades` to category parser, fix daemon help text |
| `apps/conary/src/commands/restore.rs` | Accept concrete package selectors and honor the caller-provided root |
| `apps/conary/src/commands/remove.rs` | Accept concrete package selectors when automation removal cannot rely on name-only matching |

### Testing

- Unit test: build a `PendingAction` with `ActionPayload::UpdatePackage`,
  call `plan()`, verify `PlannedOp::Install` with correct version.
- Unit test: `find_security_updates()` / `security_update_action()` preserve the
  repository target version into the typed payload.
- Unit test: repeated scans of the same logical action produce the same
  deterministic `PendingAction.id`.
- Unit test/integration test: multi-version or multi-arch orphan/repair actions
  produce concrete selectors instead of ambiguous package-name-only ops.
- Integration test: `automation apply` is blocked by `require_live_mutation`
  on `/` without the override flag.
- Unit test: verify `automation_history` row inserted after CLI execution.
- Unit test: `cmd_automation_configure --show` reads model values, not hardcoded
  defaults.
- Unit test: `cmd_automation_configure --mode auto` writes to system.toml via
  `toml_edit`, edits `[automation.ai_assist]` correctly, and preserves comments.

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
   database for the derivation index and a temporary on-disk CAS directory for
   output capture. When `Some`, open the database and use the normal CAS path.
   The executor needs the connection for its derivation index
   (`index.rs:lookup/insert`), but `DerivationExecutor` still needs a real
   filesystem-backed `CasStore`.
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

`profile generate` must canonicalize the manifest path before storing it in
`profile.profile.manifest`. Phase 3's `cache populate` reopens that path later,
often from a different working directory, so keeping a raw relative path would
make profiles fragile in CI and when moved across directories.

The recipe loading logic already exists: `load_recipes()` at
`apps/conary/src/commands/bootstrap/mod.rs:730` walks recipe subdirectories,
parses TOML files, and returns a `HashMap<String, Recipe>`. Extract this into
a shared helper in `crates/conary-core/src/derivation/recipe_loader.rs` so
both `profile generate` and `cache populate` can consume it without depending
on the bootstrap command module. The dependency resolution (topological sort
with stage classification) is the only genuinely new logic (~50-80 lines).
The shared helper should preserve the existing bootstrap subdirectory search
(`cross-tools`, `temp-tools`, `system`, `tier2`) and add a plain `recipes/`
root fallback so Phase 3 can also find non-bootstrap recipes.

**Files:** `apps/conary/src/commands/profile.rs`,
`apps/conary/src/commands/bootstrap/mod.rs` (extract `load_recipes`),
new `crates/conary-core/src/derivation/recipe_loader.rs`

### `cache populate --sources-only` -- Download Source Tarballs

**Current:** Prints `[NOT IMPLEMENTED]`, returns Ok.

**Implementation:** Source download infrastructure exists in
`bootstrap/build_runner.rs` (`fetch_source()` with URL extraction and checksum
verification). Recipe `archive_url()` and `archive_filename()` methods exist.

1. Load profile, iterate derivations.
2. Reopen the canonical manifest path stored in the profile, resolve the
   recipe root from that manifest location, then load each derivation's recipe
   via the shared recipe-loading helper.
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
| 1 | Model apply + state revert | Extracting install/remove inner helpers touches core mutation paths | **Yes** |
| 2 | Automation executor | Typed payloads change `PendingAction` struct | **Yes** |
| 3 | Stubs + README | Recipe loading helper, self-update server endpoint | **Yes** |
| 4 | Substituter remote sources | Async conversion, no production callers yet | No |

Each phase is independently testable and committable. Phase 3's `profile
generate` and `cache populate --sources-only` share a recipe loading helper that
should be built first within that phase. Phase 3's `self-update --version`
requires a server-side endpoint addition before the client change.
