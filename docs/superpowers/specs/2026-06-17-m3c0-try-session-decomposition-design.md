# M3c0 Try-Session Decomposition Design

**Date:** 2026-06-17
**Status:** Approved design; implementation plan next
**Parent design:** `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
**Prerequisite milestone:** M3b packaging MCP landed

## Purpose

M3c0 is the refactor gate before `conary try --watch`. The current
`apps/conary/src/commands/try_session.rs` file is 3065 lines and owns package
policy validation, scratch install planning, session lifecycle, SQLite
promotion, namespace mounting, launcher execution, and a large unit-test block.
Adding watch mode directly to that file would make the next feature harder to
review and would increase the chance of accidentally weakening try-session
safety.

The goal is to move existing try-session behavior into a reviewed ownership
boundary while preserving public CLI behavior and persisted state. M3c0 may
make tiny behavior fixes discovered during the split, but only when the fix is
local, covered by a focused test, and does not change schema, storage shape, or
the broad public command contract.

The core invariant remains:

> M3c0 may make try-session code easier to understand, test, and reuse, but it
> must not weaken the one-active-session invariant, keep/rollback safety, or
> host-mutation boundaries.

## Current Repo Facts

- `apps/conary/src/commands/try_session.rs` is a single 3065-line Rust file.
- Existing command entrypoints are re-exported from
  `apps/conary/src/commands/mod.rs`: `cmd_try_package`, `cmd_try_status`,
  `cmd_try_rollback`, `cmd_try_keep`, and `rollback_active_try_session`.
- `apps/conary/src/dispatch/root.rs` parses `conary try` package/status/
  rollback/keep actions and owns command-risk routing and preflight decisions
  around active or orphaned try sessions.
- The active/orphan liveness predicates
  `namespace_try_session_is_decision_pending`, `activated_try_session_is_live`,
  `try_launcher_pid_is_alive`, and the test-aware `current_boot_id` helper
  currently live in `apps/conary/src/dispatch/root.rs` alongside preflight
  routing. M3c0 moves the reusable liveness decisions into the try-session
  ownership boundary while leaving `run_try_session_preflight` orchestration in
  dispatch. `env_forces_non_interactive` also lives in dispatch today, but it is
  interaction policy and stays with dispatch preflight.
- `crates/conary-core/src/db/models/try_session.rs` owns persisted
  `try_sessions` rows, modes, statuses, single-open-session enforcement, and
  status transitions.
- Existing `apps/conary/src/commands/try_session.rs` unit tests cover policy
  validation, namespace start, active-session refusal, generation behavior,
  rollback retryability, keep promotion, DB restore, hook verification,
  namespace launcher behavior, and launcher liveness recording.
- Existing `apps/conary/tests/packaging_m1b.rs` integration tests cover
  user-facing `conary try`, `try status`, `try rollback`, `try keep`, one-active
  refusal, current-generation preservation, and promotion.

## Scope

In scope:

- Replace `apps/conary/src/commands/try_session.rs` with
  `apps/conary/src/commands/try_session/`.
- Move existing behavior into modules with narrow ownership:
  `mod.rs`, `validation.rs`, `install.rs`, `session.rs`, `namespace.rs`,
  `executor.rs`, and a small private `util.rs` for shared file/path helpers
  when avoiding sideways dependencies would otherwise bloat `mod.rs`.
- Preserve existing CLI output and command behavior for package try, status,
  rollback, and keep unless a tiny tested fix is explicitly accepted.
- Preserve the `try_sessions` schema, row semantics, one-active-session
  invariant, and keep/rollback status transitions.
- Move reusable active/orphan liveness predicates and canonical boot-id lookup
  into the try-session ownership boundary while leaving command-risk routing and
  interactive/non-interactive policy in dispatch.
- Keep the watch-mode seam narrow enough that M3c can call try-session logic
  without reaching into mount, DB-promotion, or launcher internals.
- Update assistant-facing docs that tell future contributors where to start.

Out of scope:

- Adding `conary try --watch`.
- Adding debounce, file watching, cook orchestration, record-mode trace capture,
  or streaming watch events.
- Changing persisted state, adding a schema migration, or changing the
  `try_sessions` table contract.
- Changing status output, keep/rollback user semantics, or the one-active
  policy as a feature.
- Moving try-session state models out of `conary-core` or changing the
  database model API beyond what is needed for existing callers.

## Ownership Boundary

The new module directory should keep command routing thin and make each
try-session responsibility inspectable.

`apps/conary/src/commands/try_session/mod.rs` owns the command-facing surface:

- module declarations
- `TryStartRequest`
- `TryStartOutcome`
- `cmd_try_package`
- `cmd_try_status`
- `cmd_try_rollback`
- `cmd_try_keep`
- `rollback_active_try_session`
- `begin_try_session`
- the two request/outcome types above
- `current_boot_id`, `namespace_try_session_is_decision_pending`, and
  `activated_try_session_is_live` for dispatch preflight

This is the expected crate-facing allowlist. Child modules stay private unless
the implementation summary names the caller and reason for widening visibility.
M3c watch may use `begin_try_session`, `TryStartRequest`, and
`TryStartOutcome`; it should not depend on namespace, install, executor, or DB
promotion internals without a later reviewed plan update.

`apps/conary/src/commands/try_session/validation.rs` owns package and manifest
policy:

- `TryExecutionRoot`
- `validate_try_package_policy`
- `validate_try_manifest_policy`
- M1b script, legacy scriptlet, service, declarative hook, host-root, and
  irreversible-hook refusal messages

`apps/conary/src/commands/try_session/install.rs` owns scratch installation:

- `TryInstallPlan`
- `build_try_install_plan`
- `build_try_transaction_config`
- install of the copied CCS package into the copied DB and scratch root
- transaction config overrides that keep live runtime object/generation paths
  while using the copied DB

`apps/conary/src/commands/try_session/session.rs` owns lifecycle orchestration:

- `begin_try_session`
- one-active-session enforcement
- session row creation and generation recording
- activated and namespace keep/rollback
- DB vacuum/copy/checkpoint/promotion/restore helpers, including recovery that
  restores the previous current-generation link if namespace keep fails after
  publishing the try generation link
- pure active/orphan liveness helpers consumed by dispatch preflight
- cleanup helpers that preserve retryability on failure
- canonical boot-id lookup with the existing `CONARY_TEST_BOOT_ID` test
  override, consumed by session lifecycle, executor liveness, and dispatch
  preflight

`apps/conary/src/commands/try_session/namespace.rs` owns namespace exposure:

- namespace root exposure
- promotable try-hook upperdir creation
- declarative try-hook execution against the exposed namespace root
- generation mount and overlay mount setup
- unmount sequencing
- mountinfo parsing
- test namespace materialization
- safe root-relative path conversion used by hook-effect validation
- symlink, mode, and path-removal helpers that are namespace-specific

`apps/conary/src/commands/try_session/executor.rs` owns command execution:

- optional run command execution
- bubblewrap namespace launcher
- activated launcher path
- child wait behavior
- launcher PID and boot-id recording and clearing
- use of the canonical boot-id helper from `session.rs`, rather than a second
  boot-id implementation

`apps/conary/src/commands/try_session/util.rs` owns shared private helpers when
they are used by more than one sibling module:

- `remove_dir_if_exists`
- `remove_path_if_exists`
- SQLite sidecar path/removal helpers
- DB quarantine path construction

The DB-adjacent helpers may remain in `session.rs` if they are used only by
session lifecycle after the split. General path helpers that would otherwise
create a `session.rs` to `namespace.rs` or `namespace.rs` to `session.rs`
dependency should move to `util.rs` as `pub(super)`.

The implementation should use Rust privacy as the module-boundary check.
Functions should be private by default, `pub(super)` only for sibling-module
collaboration, and `pub(crate)` only for the crate-facing API in `mod.rs` or
dispatch preflight.

## Data Flow

The external command flow does not change:

1. `apps/conary/src/dispatch/root.rs` classifies the parsed `Commands::Try`
   target as package, `status`, `rollback`, or `keep`.
2. Dispatch calls the same command functions re-exported from
   `apps/conary/src/commands/mod.rs`.
3. `cmd_try_package` builds a `TryStartRequest`, calls `begin_try_session`, and
   prints the same package copy, namespace root, generation, and keep/rollback
   guidance.
4. `cmd_try_status`, `cmd_try_rollback`, and `cmd_try_keep` keep their existing
   user-facing behavior.

Inside `begin_try_session`, the internal flow becomes explicit:

1. `session.rs` opens the live DB, rejects an existing active or orphaned
   session, creates runtime paths, copies the package artifact, and parses it.
2. `validation.rs` validates the package for namespace or activated try before
   any active try-session row is created.
3. `session.rs` records the active try-session row and copies the live DB for
   scratch installation.
4. `install.rs` builds the copied DB install plan and installs the package into
   the scratch root without scripts.
5. `session.rs` builds the inactive generation.
6. `namespace.rs` creates the promotable hook upperdir, exposes the namespace
   root, and executes declarative try hooks against that non-host root.
7. `session.rs` records the generation id on both live and copied session rows.
   Activated try then publishes the generation link or records the boot identity
   as existing behavior requires.
8. `executor.rs` optionally launches the requested command and records/clears
   launcher liveness.
9. `session.rs` returns `TryStartOutcome`.

M3c0 should not add a watch API object. The watch-facing seam is the existing
`begin_try_session(TryStartRequest) -> TryStartOutcome` flow plus narrow
lifecycle helpers. M3c can decide whether it needs an additional wrapper after
the watch loop exists.

## Tiny Behavior Fix Policy

Tiny fixes are allowed during M3c0 only when all of these are true:

- The issue is local to try-session behavior.
- A new or existing focused test proves the behavior.
- The fix does not require a schema migration or persisted-state shape change.
- The fix does not broaden or surprise the public CLI contract.
- The implementation note names the fix and why it belongs in the decomposition
  slice.

Allowed examples:

- Preserve retryability for a newly isolated cleanup failure.
- Tighten a misleading context message around an existing failure.
- Move duplicated liveness logic behind one tested helper without changing the
  active/orphan decision.
- Consolidate the duplicate `current_boot_id` implementations into one
  try-session-owned helper that preserves the existing `CONARY_TEST_BOOT_ID`
  test override.
- Add a `TrySession` model method such as `clear_launcher` in
  `crates/conary-core/src/db/models/try_session.rs` when it only centralizes
  duplicated raw SQL status guards and does not change schema or existing method
  signatures.
- Restore the previous current-generation link if namespace keep promotion
  fails after the try generation link has already been published.
- Reduce helper visibility exposed only because of the old monolith.

Not allowed examples:

- Add `--watch`.
- Change status output as a product decision.
- Change keep/rollback semantics.
- Relax active-session refusal.
- Introduce a new persisted status, mode, or schema migration.

## Testing And Verification

M3c0 should be characterization-first. The plan should identify or add parity
tests before major moves, then keep those tests unchanged while modules are
split.

Required behavior gates:

- Begin: package try creates an active session, copied artifact, namespace root,
  and try generation.
- Validation refusal: unsupported hook classes and other policy failures occur
  before any active or orphaned try-session row is created.
- Rollback: namespace rollback marks the session rolled back, cleans workdir,
  unmounts in the expected order, and remains retryable when cleanup fails.
- Keep: namespace keep promotes the try generation, marks the session kept, and
  restores the live DB if promotion fails after backup. A focused regression
  test must prove that a failure after publishing the try generation restores
  the previous current-generation link as well as the live DB checkpoint.
- One-active refusal: starting a second try session reports the active session
  id and does not create another open session.
- Orphan handling: active/orphan liveness helpers preserve dispatch preflight
  behavior for namespace and activated sessions.
- Launcher liveness: command execution records child liveness before wait and
  clears it after exit.
- Boot identity: with `CONARY_TEST_BOOT_ID=boot-a`, activated no-command and
  launcher paths record `boot-a`, and dispatch preflight evaluates against that
  same canonical helper.
- Hook policy: the M1b try hook refusal matrix remains unchanged.

Before the first code move, the implementation plan must map each behavior gate
above to specific existing tests. Any gate without direct coverage needs a
focused characterization test while the monolith still exists.

The split must also preserve all existing test-injection seams, including
`CONARY_TEST_TRY_LAUNCHER`, `CONARY_TEST_TRY_MOUNTINFO_PATH`,
`CONARY_TEST_TRY_UMOUNT_FAIL`, `CONARY_TEST_TRY_UMOUNT_LOG`,
`CONARY_TEST_TRY_SYNC_PARENT_LOG`, `CONARY_TEST_TRY_REMOVE_DIR_FAIL`,
`CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP`, `CONARY_TEST_SKIP_GENERATION_MOUNT`,
and the dispatch-side `CONARY_TEST_BOOT_ID` override after it moves into the
try-session boundary.

Focused proof:

```bash
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
cargo test -p conary --test packaging_m1b
cargo fmt --check
```

When M3c0 changes `crates/conary-core/src/db/models/try_session.rs`, also run:

```bash
cargo test -p conary-core db::models::try_session
```

Any new model helper, such as `TrySession::clear_launcher`, must include a
model unit test proving it updates only active or orphaned sessions and refuses
completed sessions through the same status guard as existing transitions.

Merge gate:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

The M3c0 worktree baseline before writing this spec was:

```bash
cargo build -p conary
cargo test -p conary --lib commands::try_session
```

Both commands passed. The try-session unit-test baseline was 37 tests passing.

## Docs Updates

After implementation lands, update:

- `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
  to mark M3c0 landed.
- `docs/llms/subsystem-map.md` so future assistant routing points to
  `apps/conary/src/commands/try_session/`.
- `docs/modules/feature-ownership.md` so the packaging ownership card names the
  new try-session module directory and proof commands.

Do not update active docs to claim watch behavior until M3c lands.

## Risks And Mitigations

Move-only noise can hide behavior changes. Mitigate by adding or confirming
parity tests before moves, splitting in small ownership commits, and reviewing
with moved-code-aware diffs.

Circular module dependencies can recreate monolith coupling. Mitigate by keeping
shared request/outcome types in `mod.rs`, passing explicit arguments between
modules, and avoiding sideways imports except through narrow `pub(super)`
helpers.

The intentional sibling dependencies should stay small and named:
`session.rs` may call `namespace::root_relative_path` for keep-time hook-effect
verification, `executor.rs` may call the canonical boot-id helper owned by
`session.rs`, and multiple modules may call private `util.rs` helpers. Other
cross-module pulls should be treated as a boundary smell during review.

`session.rs` can become the new large file. Mitigate by moving install mechanics
to `install.rs`, mount mechanics to `namespace.rs`, launcher mechanics to
`executor.rs`, and policy checks to `validation.rs`.

Watch-mode requirements can leak into the refactor. Mitigate by refusing watch
flags, watcher loops, debounce behavior, and streaming event contracts in M3c0.
The only watch preparation is a narrow, private-enough try-session API.

Tiny behavior fixes can drift. Mitigate by requiring a named test, a short note
in the implementation summary, and explicit exclusion of schema or broad CLI
changes.

## Implementation Sequence

The implementation plan should proceed in this order:

1. Map each required behavior gate to existing tests while the monolith still
   exists, then add parity/orphan/liveness characterization tests for any gaps.
2. Convert `try_session.rs` into `try_session/mod.rs` and move validation into
   `validation.rs`.
3. Move install planning and copied-package installation into `install.rs`.
4. Move namespace mount, unmount, path, and test-materialization helpers into
   `namespace.rs`.
5. Move executor and launcher liveness helpers into `executor.rs`.
6. Move session lifecycle orchestration and dispatch-facing liveness helpers
   into `session.rs`, including the canonical test-aware boot-id helper.
7. Move shared private path/SQLite helpers into `util.rs` only where doing so
   avoids module coupling.
8. Reduce visibility and re-export only the narrow crate-facing API from
   `mod.rs`.
9. Implement any accepted tiny behavior fixes in isolated commits with named
   tests, especially launcher SQL centralization and current-link recovery on
   failed namespace keep promotion. The keep recovery test needs a named
   post-current-link failure seam, such as
   `CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP=after-current-link`, injected after
   `publish_generation_link` and before marking the generation or session kept.
10. Update assistant routing docs and run focused proofs plus clippy.
11. Run local agentic review before locking the implementation.

## Completion Criteria

M3c0 is complete when:

- `apps/conary/src/commands/try_session.rs` is retired in favor of the
  `apps/conary/src/commands/try_session/` module directory.
- Public CLI behavior for package try, status, rollback, and keep remains
  covered by existing or added parity tests.
- Dispatch preflight still enforces active/orphan behavior through a narrow
  try-session-owned liveness helper.
- The crate-facing try-session API is narrow enough for M3c watch to call
  without depending on namespace, DB-promotion, or launcher internals.
- Focused tests, formatting, and the workspace clippy merge gate pass.
- Assistant-facing docs route future contributors to the new module directory.
