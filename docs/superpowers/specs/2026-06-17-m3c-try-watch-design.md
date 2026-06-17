# M3c Try Watch Design

**Date:** 2026-06-17
**Status:** Draft design for user review; implementation plan next
**Parent design:** `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
**Prerequisite milestone:** M3c0 try-session decomposition landed

## Purpose

M3c adds `conary try --watch` as the fast feedback loop for package authors:

```text
source change -> debounce -> cook -> staged namespace try refresh
              -> event output -> wait for next source change
```

The feature should make local package iteration feel immediate without
weakening the safety rules that M1b through M3c0 established for try sessions.
The important product rule is:

> A failed watched rebuild is non-destructive. The current try environment
> stays on the last successful generation, the failure is visible, and the next
> source change retries the refresh.

M3c is not a publish path, not an activated-generation workflow, and not record
mode. It composes existing cook, diagnostics, source identity, and try-session
ownership into one watch loop.

## Current Repo Facts

- `apps/conary/src/commands/try_session/` now owns the decomposed try-session
  command surface from M3c0.
- `apps/conary/src/commands/try_session/mod.rs` exposes the narrow command API:
  `TryStartRequest`, `TryStartOutcome`, `begin_try_session`, status, keep,
  rollback, and dispatch-facing active/orphan liveness helpers.
- `apps/conary/src/commands/try_session/session.rs` is already a large file,
  but it owns the one-active-session invariant, session row updates, generation
  recording, keep, rollback, and cleanup retryability.
- `apps/conary/src/cli/mod.rs`, `apps/conary/src/dispatch/root.rs`,
  `apps/conary/src/commands/cook.rs`, and
  `crates/conary-core/src/recipe/kitchen/cook.rs` are large enough that M3c
  should keep additions there thin and place watch-specific behavior in a
  focused owner.
- `crates/conary-core/src/diagnostics/` owns the M3a packaging diagnostic,
  event, and command-output DTOs. `conary cook --json` and
  `conary publish --json` already use the shared output shape.
- `apps/conary/src/commands/operation_records.rs` owns the local private
  packaging operation record store. Records are JSON files, redacted before
  write, `0600` under a `0700` directory, with newest-50 retention.
- `crates/conary-core/src/recipe/hermetic/source_identity.rs` owns canonical
  local file listing and local tree identity. It uses git-tracked files for git
  worktrees, default filesystem ignore rules for non-git trees, and records
  warnings for weaker identities.
- `crates/conary-core/src/recipe/kitchen/config.rs` already has
  `SourceDownloadPolicy::OfflineCacheOnly`, and `Kitchen::cook_hermetic()`
  prefetches sources before switching the build to offline source policy.
- The CLI intentionally rejects `try --watch`, `try --record`, and `try --json`
  today.

## Scope

In scope:

- Add `conary try --watch` for recipe projects or inferable source trees.
- Keep watch mode namespace-only.
- Preserve the last successful try generation when cook, validation, hooks,
  namespace exposure, or staged cleanup fails.
- Emit M3a structured watch events and diagnostics.
- Reuse canonical source identity after debounce so meaningful-change
  detection follows existing hermetic local-source rules.
- Rerun cook and normal try package validation on every refresh.
- Add a staged try-session refresh API narrow enough that watch does not depend
  on mount, install, DB promotion, or launcher internals.
- Stop cleanly on cancellation and route cleanup through normal try rollback.
- Update assistant-facing docs only after implementation lands.

Out of scope:

- `--watch` with a prebuilt `.ccs` artifact.
- `--watch --activate`.
- `--watch -- <command>`.
- Auto-keep, auto-publish, or direct publish of watch-created artifacts.
- Record-mode tracing or recorded-draft artifacts.
- Incremental build optimization beyond whole-package rebuilds.
- A DB schema migration for watch state.
- A remote build service or MCP watch tool.
- A DB-backed packaging operation store.

## CLI Contract

The M3c CLI surface should parse:

```bash
conary try --watch
conary try --watch .
conary try --watch path/to/project
conary try --watch --recipe path/to/recipe.toml
conary try --watch --json
```

`target` defaults to `.` when `--watch` is set and no package/action target is
provided.

Rejected combinations:

- `conary try --watch pkg.ccs`
- `conary try --watch --activate`
- `conary try --watch --allow-irreversible`
- `conary try --watch status`
- `conary try --watch rollback`
- `conary try --watch keep`
- `conary try --watch -- /usr/bin/demo`

Watch-created sessions still use ordinary `conary try status` and
`conary try rollback` for inspection and cleanup. `conary try keep` should
refuse watch-created sessions in M3c unless the implementation can prove a
watch generation is indistinguishable from a normal non-watch namespace try
generation and the spec is updated before planning. The default M3c design is
no keep for watch-created sessions. The refusal must not require a schema
migration. M3c should write a durable marker file under the active try
`work_dir`, such as `.conary-try-watch-session.json`, and have keep check that
marker before promotion. Rollback treats the marker as ordinary workdir content
and removes it during cleanup.

`--json` for watch is streaming NDJSON, one redacted `PackagingEvent` per line,
not the final-object JSON used by one-shot `cook --json`. The final line should
be an `OperationFinished` event when the loop exits normally or after
cancellation cleanup. Human output should stay terse and event-like:

```text
Watching .
[1] cooking...
[1] refreshed try generation 42
[2] cooking...
[2] cook failed; keeping generation 42 active
```

Human output should not print unbounded logs. Failure details go through
structured diagnostics and the operation record.

## Ownership Boundary

M3c should add a focused watch owner. Preferred path:

- `apps/conary/src/commands/try_session/watch.rs`

This keeps the feature inside the try-session command boundary while separating
watch orchestration from session lifecycle internals. If implementation pressure
makes the file too broad, split into:

- `apps/conary/src/commands/try_session/watch.rs`: CLI-facing orchestration,
  loop state, cancellation, event rendering, and operation record assembly
- `apps/conary/src/commands/try_session/watch_source.rs`: source identity,
  watch roots, polling/watcher abstraction, debounce, and ignored-change tests

Existing large-file changes should stay thin:

- `apps/conary/src/cli/mod.rs`: add flags and parser tests only.
- `apps/conary/src/dispatch/root.rs`: route watch to one command function and
  preserve try-management action handling.
- `apps/conary/src/commands/cook.rs`: expose a narrow internal cook API only
  if needed; do not move watch orchestration into cook.
- `apps/conary/src/commands/try_session/session.rs`: add narrow staged refresh
  helpers and small model-facing state updates, not a watch loop.
- `crates/conary-core/src/diagnostics/mod.rs`: add only additive event kinds or
  diagnostic codes needed by watch.

The watch module may call a narrow try-session API such as:

```rust
pub(crate) struct TryRefreshRequest<'a> {
    pub db_path: &'a str,
    pub session_id: &'a str,
    pub package_path: &'a Path,
}

pub(crate) struct TryRefreshOutcome {
    pub previous_generation_id: i64,
    pub try_generation_id: i64,
    pub namespace_root: PathBuf,
    pub copied_package_path: PathBuf,
}
```

The exact names can change in the plan, but the boundary must preserve this
shape: watch provides a cooked package; try-session owns validation, copied DB
installation, generation build, namespace exposure, hook execution, session row
update, and cleanup.

## Watch Lifecycle

Startup:

1. Resolve the watch target using the same recipe/inference rules as cook.
2. Refuse unsupported CLI combinations before opening a try session.
3. Check for an existing active or orphaned try session using the normal
   preflight path.
4. Compute the initial source identity.
5. Cook the initial package.
6. Start a normal namespace try session through the try-session boundary.
7. Write the watch-session marker under the try session workdir.
8. Record the session id, generation id, source identity hash, and operation id
   in in-memory watch state.
9. Emit `OperationStarted`, `PhaseStarted`, cook/try events, and a successful
   refresh event.

Loop:

1. Wait for a filesystem wakeup or polling tick.
2. Debounce for a fixed initial delay, recommended default 750 ms.
3. Recompute canonical local source identity.
4. If the identity is unchanged, emit no refresh and continue waiting.
5. Cook the package into a watch-owned output directory.
6. If cook fails, emit a diagnostic and keep the last successful generation.
7. If cook succeeds, stage a try refresh.
8. Commit the new generation to the active watch session only after validation,
   copied DB installation, generation build, namespace exposure, and hook
   execution all succeed.
9. Tear down and remove the previous generation/workdir only after the new
   generation is the recorded active generation.

Shutdown:

1. On Ctrl-C or process termination handled by the loop, emit cancellation.
2. Roll back the active watch session through normal try rollback.
3. If rollback succeeds, emit `OperationFinished` with a cancellation summary.
4. If rollback fails, emit a cleanup diagnostic, leave the session active or
   orphaned according to existing try-session rules, and exit non-zero.

M3c should not keep a watch session alive after the watch process exits. The
user-facing promise is fast iteration during the process, then cleanup. If a
future slice wants persistent watch sessions, it needs a separate persisted
state design.

## Staged Refresh Semantics

The central M3c behavior is staged refresh with last-good preservation.

The active try-session row remains the single open session for the watch loop.
Each refresh creates temporary staging paths under the watch work directory,
for example:

```text
try/<session-id>/
  current/
  refresh-0002/
```

The staging area gets its own copied package and copied DB. The staged refresh
uses the same package policy validation and no-script scratch installation as a
normal namespace try. It builds the new inactive generation under the live
runtime artifact roots, exposes a staged namespace root, and runs declarative
try hooks against that staged non-host root.

Only after all staged work succeeds does try-session update the open session to
the new package path, copied DB path if needed, generation id, and namespace
root/workdir pointers. The implementation may either keep the existing
`work_dir` stable and move staged content into a stable `current/` subdirectory,
or keep generation-specific staging directories and make rollback aware of the
recorded current staging directory. It must not require a schema migration.

If staging fails before commit:

- the active session row still points at the last successful generation
- the last successful namespace root remains available
- the failed staged generation and staging directory are removed when possible
- cleanup failure stops the loop and surfaces an orphan/cleanup diagnostic

If commit succeeds but previous-generation cleanup fails:

- the new generation remains active for the watch session
- the loop stops and reports cleanup failure rather than piling up state
- normal rollback remains retryable

The try-session model may need a small open-session update method, for example
`replace_try_generation`, that updates only `active` or `orphaned` sessions and
preserves the existing completed-session guard. It must not add a new status,
mode, or table.

## Source Watching And Debounce

M3c should prefer deterministic source identity over raw filesystem event
interpretation. The watcher or poller wakes the loop; the canonical identity
decides whether a rebuild is meaningful.

Initial implementation options:

- Poll canonical source identity on an interval after filesystem wakeups.
- Add a file watcher dependency and still verify with canonical identity.

The implementation plan should choose the smallest reliable option. Since the
workspace already depends on `walkdir` and does not currently depend on a watch
crate, a polling-first implementation is acceptable for M3c if tests prove
debounce and unchanged-identity suppression. A later slice can replace the wake
mechanism with `notify` without changing the refresh contract.

Identity rules:

- Git worktrees use git-tracked files, matching
  `canonical_local_file_list`.
- Non-git source trees use the existing filesystem walk ignore rules.
- Untracked git files do not trigger refresh unless they become tracked.
- Dirty tracked files do trigger refresh outside CI.
- CI dirty-tree refusal remains owned by hermetic source identity and should
  surface as a diagnostic rather than silently watching a different file set.

## Cook And Source Policy

Watch mode is a project workflow, not an artifact workflow. It cooks each
refresh from the recipe or inferred source tree and then tries the resulting
CCS artifact.

M3c should use hermetic cook behavior when the existing cook defaults or flags
select it. It must not claim stronger provenance than the underlying cook
produced. Source fetching follows M2a expectations:

- initial startup may use the normal prefetch path
- rebuilds should prefer offline-cache-only behavior after startup when the
  existing cook API can express it
- if the existing cook API cannot express "prefetch once, rebuild offline" in
  the first slice without broad cook refactoring, the implementation plan must
  either add a narrow cook-run option or explicitly limit M3c to the current
  cook source policy and keep the provenance text honest

Every refresh must rerun try package validation after cook. A source edit that
introduces unsafe scriptlets, unsupported declarative hooks, irreversible
activated behavior, or other try policy failures fails that refresh closed and
preserves the last successful generation.

## Events, Diagnostics, And Records

M3c should extend the M3a event model additively. Recommended new event kinds:

- `WatchStarted`
- `WatchDebounced`
- `WatchRefreshStarted`
- `WatchRefreshSkipped`
- `WatchRefreshSucceeded`
- `WatchRefreshFailed`
- `WatchCancelled`

If adding event kinds is too broad, these can be represented initially as
`PhaseStarted`, `PhaseFinished`, `DiagnosticEmitted`, and `OperationFinished`
with watch-specific messages. The implementation plan should choose one
approach and include serialization tests.

Recommended diagnostic codes:

- `WatchCookFailed`
- `WatchTryRefreshFailed`
- `WatchCleanupFailed`
- `WatchSourceIdentityFailed`
- `TryWatchUnsupported`

Diagnostics should use existing phases when possible:

- source identity and debounce: `Inference` or a watch-specific phase only if a
  phase addition is justified
- cook: `Build`
- try refresh: `TrySession`
- operation record writes: `OperationRecord`

Operation records:

- Write one operation record per watch process, not one record per source
  event.
- Include a bounded event list. The implementation should cap stored events,
  recommended newest 500, before writing the final record.
- Redact before write through existing diagnostics helpers.
- Use the existing private file store and newest-50 retention.
- If record writing fails, print/log a warning event but do not fail an
  otherwise successful rollback on shutdown.

Streaming JSON:

- `--json` emits one redacted `PackagingEvent` per line.
- Each line includes `schema_version`, `operation_id`, `sequence`, `phase`, and
  `kind`.
- Event sequences are monotonic for the whole watch process.
- The stream must not print human text mixed into stdout.
- Diagnostics with secret-bearing logs go through the existing redactor before
  serialization.

## Failure Behavior

Startup failures:

- CLI conflict or unsupported target: fail before opening a try session.
- Initial cook failure: fail without opening a try session.
- Initial try validation or setup failure: fail without leaving an active
  session when possible; if cleanup fails after a session opens, mark/report the
  existing active/orphan state through the normal try-session path.

Refresh failures:

- Cook failure: keep last successful generation and continue watching.
- Source identity failure: keep last successful generation and continue unless
  the source root is gone; if the root is gone, stop after reporting the error.
- Try validation failure: keep last successful generation and continue
  watching.
- Namespace/hook failure during staging: keep last successful generation and
  continue if staging cleanup succeeds.
- Staging cleanup failure: stop, report cleanup failure, and require
  `conary try status` plus `conary try rollback`.
- Commit failure after the session row changes: stop, report the session as
  needing rollback, and do not attempt another refresh.

Cancellation:

- Ctrl-C triggers rollback.
- A second Ctrl-C may exit immediately after printing that manual rollback may
  be required.
- Successful cancellation cleanup exits zero.
- Failed cancellation cleanup exits non-zero and leaves normal try status
  guidance.

## Testing And Verification

M3c should be tested at the unit boundary first, then through focused CLI
integration.

Unit tests:

- CLI parses `try --watch`, default target, `--recipe`, and `--json`.
- CLI rejects watch with `.ccs`, `--activate`, `--allow-irreversible`, action
  words, and trailing run commands.
- Debounce coalesces rapid changes into one refresh.
- Unchanged canonical source identity skips refresh.
- Git untracked files do not trigger refresh; tracked dirty files do.
- Filesystem identity ignores the existing default ignored directories.
- Event sequences are monotonic and JSON lines serialize with schema version.
- Bounded operation records drop older watch events before write.
- Watch-created sessions write a workdir marker, and `conary try keep` refuses
  marked sessions without changing the `try_sessions` schema.
- Refresh success updates the active session generation only after staging
  succeeds.
- Cook failure preserves the previous generation id and namespace root.
- Try policy failure on the second refresh preserves the previous generation id.
- Staging cleanup failure stops the loop and leaves rollback retryable.
- Cancellation calls normal try rollback.

Focused CLI/integration tests:

- `conary try --watch` startup creates one active namespace try session.
- A source edit produces a new try generation and replaces the active watch
  generation.
- A failing rebuild reports failure while `conary try status` still shows the
  last successful generation.
- `conary try rollback` after a failed refresh cleans the active session.
- `conary try --watch --json` emits NDJSON events and no human stdout.

Required focused proof for implementation:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
cargo test -p conary --test packaging_m1b
cargo test -p conary --test packaging_m3a
cargo fmt --check
```

If the implementation touches try-session model methods:

```bash
cargo test -p conary-core db::models::try_session
```

If the implementation touches hermetic source identity or Kitchen policy:

```bash
cargo test -p conary-core recipe::hermetic
cargo test -p conary-core recipe::kitchen
```

Merge gate:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## Implementation Plan Shape

The implementation plan should split M3c into these reviewable tasks:

1. Add CLI parsing and routing for watch with unsupported-combination tests.
2. Add watch source identity/debounce units using canonical local source
   identity.
3. Expose a narrow internal cook function for watch if the current cook command
   API cannot be reused without stdout coupling.
4. Add staged namespace refresh helpers under `try_session/session.rs` and keep
   watch orchestration outside that file.
5. Add watch loop orchestration, cancellation, and human rendering.
6. Add structured events, JSON streaming, bounded operation record writing, and
   redaction tests.
7. Add failure recovery tests for last-good preservation and cleanup stop
   behavior.
8. Update `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`,
   `docs/modules/feature-ownership.md`, and `docs/llms/subsystem-map.md` only
   after implementation proves the active watch behavior.

The plan should not start by adding a dependency. If a file watcher crate is
chosen, the plan must justify why polling canonical identity is insufficient
for M3c and include deterministic tests that do not depend on platform-specific
event timing.

## Completion Criteria

M3c is complete when:

- `conary try --watch` runs a namespace-only project watch loop.
- Each successful source change cooks and stages a new try generation.
- Failed refreshes preserve the last successful generation and remain visible.
- Cleanup failures stop the loop and leave normal try rollback guidance.
- `--json` watch output emits redacted NDJSON packaging events.
- Operation records remain private, redacted, bounded, and file-backed.
- Keep, activation, publish, record mode, and run-command watch semantics remain
  out of scope.
- Focused tests, formatting, and workspace clippy pass.
