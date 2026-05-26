---
last_updated: 2026-05-26
revision: 1
summary: Design for Plan B generation publication durability and recovery hardening
---

# Generation Publication Durability: Design Spec

**Date:** 2026-05-26
**Status:** Design approved; implementation plans pending
**Parent:** `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`
**Goal:** Make post-DB generation publication explicit, durable, recoverable,
and visible to operators and automation.

---

## Purpose

Plan B implements Track 3 from the preview invariant hardening umbrella.
Conary's composefs-native model treats the package database commit as the point
of no return. Generation build, selection, and active-state marking are meant to
be re-derivable after that point, but the current implementation does not record
the post-commit publication path as a typed durable state machine.

That creates three safety gaps:

- recovery can accept a valid `/conary/current` artifact before checking whether
  a newer package mutation is still waiting to be published;
- install, batch install, and remove can commit DB state, record a deferred
  warning, return success, and leave automation without a structured
  "publication still needed" signal;
- filesystem publication steps such as current-link renames and operation
  records do not consistently fsync destination parent directories.

Plan B closes those gaps without broadening the public preview feature surface.

## Scope And Sequencing

Plan B is one Track 3 design split into two implementation slices:

1. **Plan B1: Publication Intent And Recovery Contract**
   - Add a DB-backed generation publication intent table and typed model APIs.
   - Route install, batch install, and remove through the shared publication
     contract after DB commit.
   - Add an explicit retry command for pending publication intents.
   - Make recovery, history, and status surfaces consult pending publication
     state.

2. **Plan B2: Durable Filesystem Helper And Fsync Sweep**
   - Add shared durable filesystem helpers for temp-write, sync, rename, and
     parent-directory sync.
   - Apply them to generation publication and recovery paths that depend on
     durable directory entries.

B1 should land before B2 because it fixes the semantic hole first. B2 then
tightens crash consistency around the now-explicit state machine.

## Non-Goals

- Do not add new package-manager features unrelated to publication recovery.
- Do not make broad conaryd API additions in B1 beyond preventing false
  "fully published" reporting in existing status/history paths.
- Do not convert every JSON write in the repository to durable writes. B2 is
  targeted at generation publication, recovery, selected boot state, takeover
  or bootstrap operation records that affect recovery, and live-root
  journal-adjacent mutation repair.
- Do not change the package DB commit rule: after commit, Conary must recover
  forward or fail closed rather than pretending the package mutation did not
  happen.
- Do not use destructive host crash tests as the acceptance gate. Use temp-root,
  injected-failure, and phase-model tests.

## Current Repo Facts

The design is based on these current code paths:

- `apps/conary/src/commands/composefs_ops.rs::rebuild_and_mount` builds a
  generation, may update `/conary/current`, and is called after package DB
  commit.
- `apps/conary/src/commands/install/mod.rs`, `apps/conary/src/commands/install/batch.rs`,
  and `apps/conary/src/commands/remove.rs` append free-form deferred follow-up
  metadata when post-commit generation rebuild fails.
- That deferred metadata currently uses string fields such as
  `kind = "generation_rebuild"` and `status = "failed"` and points to
  `system generation build`, which builds a new inactive generation instead of
  completing a specific DB-backed publication intent.
- `crates/conary-core/src/transaction/recovery.rs` accepts a valid selected
  `/conary/current` artifact in selected-generation recovery without consulting
  pending publication work.
- `crates/conary-core/src/generation/mount.rs::update_current_symlink` creates a
  temporary symlink and renames it over `current`, but it does not fsync the
  parent directory afterward.
- `apps/conary/src/commands/operation_records.rs::write_json_record` writes and
  renames JSON records without durable file or parent-directory sync.
- `apps/conary/src/commands/live_root.rs` has durable journal writes, but some
  target renames, backup moves, directory creates, and removals still need
  parent-directory sync coverage.

## Architecture Decision

Use a real SQLite table as the authoritative publication state, not changeset
metadata and not sidecar JSON.

Plan B adds a `generation_publications` table with typed Rust model APIs. Each
row represents the publication of one package-mutation changeset into a
selected generation. The row links to `changesets.id`, records runtime identity,
tracks phase transitions, captures target state/generation numbers as they
become known, and stores failure/retry metadata.

Changeset metadata may keep a compatibility follow-up entry, but it must become
secondary. The DB publication row is the source of truth used by recovery,
history/status rendering, retry commands, and daemon-facing status.

This choice matches Conary's architecture: once the package DB commits, the DB
is the durable authority and generation publication is re-derivable from it.

## Publication Intent Schema

Plan B1 should add a migration that increments `SCHEMA_VERSION` and creates
`generation_publications`.

Proposed columns:

| Column | Purpose |
| --- | --- |
| `id` | Primary key. |
| `changeset_id` | Required reference to the package mutation changeset. |
| `tx_uuid` | Optional transaction UUID for correlation with existing transaction state. |
| `db_path` | Canonical DB path used by the command. |
| `runtime_root` | Runtime root whose `/conary/current` is being published. |
| `phase` | Typed publication phase as a snake-case string with a CHECK constraint if practical. |
| `status` | Typed status: `pending`, `running`, `failed`, `complete`, or `abandoned`. |
| `state_number` | System state number once the snapshot exists. |
| `generation_number` | Generation number once the artifact exists. |
| `summary` | Human-readable publication summary. |
| `last_error` | Last failure message, if any. |
| `retry_count` | Number of retry attempts. |
| `recoverable` | Boolean flag for whether `generation publish` may retry. |
| `created_at` | Creation timestamp. |
| `updated_at` | Last phase/status update timestamp. |
| `completed_at` | Completion timestamp. |

Constraints:

- one active non-complete publication should exist per `changeset_id`;
- phases and statuses must parse into Rust enums before recovery acts on them;
- unknown phases or statuses fail closed;
- rows must preserve enough identity to reject accidental publication against
  the wrong DB/runtime root.

## Phase Model

Publication phases are typed and intentionally finer-grained than a generic
"generation rebuild failed" marker:

| Phase | Meaning |
| --- | --- |
| `DbCommitted` | Package mutation committed; publication not started or needs retry. |
| `StateSnapshotStarted` | System state snapshot creation started. |
| `StateSnapshotRecorded` | State row and members exist; `state_number` is known. |
| `ArtifactBuildStarted` | Generation artifact build started. |
| `ArtifactBuilt` | Artifact exists and validates; `generation_number` is known. |
| `CurrentLinkStarted` | Current-link publication started. |
| `CurrentRenamed` | `/conary/current` rename completed; parent sync is not confirmed. |
| `CurrentSynced` | Current-link parent directory sync completed. |
| `DbActiveMarkStarted` | DB active-state update started. |
| `DbActiveMarked` | DB active-state update completed. |
| `Complete` | Publication finished successfully. |
| `Failed` | Last attempt failed; row remains recoverable unless abandoned. |

The implementation may merge adjacent phases only if tests still cover the
important recovery decisions. It must not collapse the whole post-commit path
back into an opaque warning string.

## Ordering Rule

Plan B changes the publication ordering so durable boot selection happens
before DB active-state marking:

1. Package DB commit.
2. Publication intent row created.
3. System state snapshot recorded.
4. Generation artifact built and validated.
5. `/conary/current` updated and parent directory synced.
6. Matching system state marked active in the DB.
7. Publication intent marked complete.

This ordering avoids the current unsafe shape where DB state can claim a
generation is active before `/conary/current` durably points at it. The reverse
is easier to recover: if `/conary/current` is durable but DB active marking
failed, recovery can validate the artifact and mark the matching state active.

## Package Mutation Flow

Install, batch install, and remove should call one shared post-commit
publication helper after the package DB transaction commits.

Expected behavior:

1. Commit the package mutation transaction.
2. Create or update a `generation_publications` row for the changeset.
3. Attempt publication through the phase model.
4. If publication completes, mark the row `complete`.
5. If publication fails after DB commit, leave the row `failed` or `pending`
   with `recoverable = true`, record `last_error`, and keep the command exit
   code `0`.

The successful process exit is intentional. The package DB mutation committed,
so automation should not be encouraged to retry the original install/remove as
though nothing happened. Instead, automation gets a first-class
`needs_publication` state that can be queried and retried.

The command should still print a clear warning on stderr when publication fails:

```text
WARNING: package mutation committed, but generation publication is pending for changeset <id>.
Run: conary --allow-live-system-mutation system generation publish --changeset <id>
```

## Retry Command

Plan B1 adds an explicit retry command:

```bash
conary --allow-live-system-mutation system generation publish --changeset <id>
```

Behavior:

- If the publication is `complete`, report that it is already complete and exit
  successfully.
- If the publication is pending or failed but recoverable, resume from the
  recorded phase or rebuild idempotently from the earliest safe phase.
- If the changeset has no publication intent, fail with a clear diagnostic.
- If the intent belongs to a different DB path or runtime root, fail closed.
- If the phase/status is unknown, fail closed.
- If the row is `abandoned`, fail with a diagnostic that manual intervention is
  required.

The retry command must complete the specific DB-backed publication intent. It
must not merely call `system generation build` and create an unrelated manual
generation.

`system generation recover --publish-pending` is an allowed convenience flag,
but it is optional for B1. The required B1 surface is `publish --changeset`.

## Recovery Behavior

Recovery must consult pending publication intents before accepting a valid
selected `/conary/current` artifact as complete.

Required behavior:

- If no pending/failed recoverable publication intents exist, current recovery
  may continue with selected-generation validation.
- If a pending/failed recoverable intent exists, recovery must try to complete
  it or fail closed with a diagnostic explaining which changeset needs
  publication.
- If multiple pending intents exist, recovery must process them in changeset or
  creation order and avoid selecting a later generation before earlier
  publications are resolved or explicitly abandoned.
- Unknown phases or statuses must fail closed.
- Recovery must not mark a system state active unless the selected generation
  artifact validates and `/conary/current` is durably selected.

This rule closes the current short-circuit where a valid older
`/conary/current` can hide a newer DB-committed package state.

## Operator And Automation Surfaces

Plan B keeps the command exit code `0` for post-commit publication failure, but
it must make the partial state visible and machine-readable enough for scripts.

Required surfaces:

- `system history` marks applied changesets with incomplete publication as
  `[publication-pending]` or `[publication-failed]`.
- Deferred follow-up metadata stops accepting arbitrary generation rebuild
  strings. It should use typed/validated values, or it should point at the
  publication intent while treating the DB row as the source of truth.
- A status/query surface lists pending publication intents. The implementation
  may add `system generation pending`, extend `generation list/info`, or expose
  another focused query path. The path must be script-friendly and must include
  changeset ID, status, phase, generation/state numbers when known, and retry
  command.
- Existing conaryd history/status routes must not report a mutation as fully
  published when the DB contains a pending or failed recoverable publication
  intent. A dedicated conaryd endpoint can be deferred if existing route output
  can truthfully surface the state.

## Deferred Follow-Up Metadata

The existing `DeferredFollowUp` shape is stringly typed. Plan B should either:

- replace generation rebuild follow-ups with a typed enum-backed
  `PublicationPending` follow-up that references `generation_publications.id`;
  or
- keep the existing JSON envelope but validate `kind` and `status` against Rust
  enums before rendering or writing them.

The preferred implementation is to make changeset metadata a compatibility
display layer and rely on `generation_publications` for recovery. Metadata must
continue to preserve rollback snapshots and adoption warnings in the existing
`conary.changeset.metadata.v1` envelope.

## Durable Filesystem Helper

Plan B2 adds shared durable filesystem helpers, preferably in `conary-core`, for
the publication pattern Conary repeats today:

1. create/write a temp file or temp symlink next to the destination;
2. fsync file contents when applicable;
3. atomically rename;
4. fsync the destination parent directory;
5. propagate fsync errors where crash consistency matters.

At minimum, helpers should cover:

- durable file write and rename;
- durable JSON write and rename;
- durable symlink replacement;
- parent-directory fsync after file or symlink rename;
- parent-directory fsync after file removal, directory creation, and directory
  removal where recovery depends on the entry.

## B2 Application Targets

Apply the durable helper or equivalent behavior to:

- `/conary/current` updates in `crates/conary-core/src/generation/mount.rs`;
- generation metadata writes in `crates/conary-core/src/generation/metadata.rs`;
- `.conary-gen.sig` writes when generation signing is active;
- artifact and boot manifests where the selected-generation contract depends on
  them;
- boot asset copies where the artifact contract depends on the copied entry;
- operation records used by takeover, bootstrap, native handoff, or recovery;
- live-root journal-adjacent target renames, backup moves, directory creates,
  and removals.

Errors from parent-directory sync must not be silently ignored in these paths.
If a platform does not support a specific sync operation, the helper should
return a typed unsupported/ignored result only where the caller explicitly
chooses that policy.

## Testing Strategy

B1 tests:

- migration creates `generation_publications` and increments `SCHEMA_VERSION`;
- phase/status parsing rejects unknown values;
- forced post-DB rebuild failure creates a pending or failed publication intent
  and exits successfully;
- install, batch install, and remove share the same publication failure
  contract;
- `system history` marks incomplete publications;
- the pending publication query surface includes changeset ID, phase, status,
  and retry command;
- `system generation publish --changeset <id>` completes a pending publication;
- publish is idempotent for completed publications;
- recovery checks pending publication intents before accepting a valid selected
  `/conary/current`;
- recovery fails closed on unknown publication phase/status;
- deferred follow-up kind/status values are typed or validated.

B2 tests:

- `update_current_symlink` fsyncs the parent directory, using injectable failure
  or a focused helper test where direct observation is impractical;
- generation metadata/signature writers propagate parent-sync errors;
- operation record writes use durable temp-write and rename helpers;
- live-root rename/remove/directory helpers call parent sync after target
  changes;
- temp-root recovery tests cover representative publication phases without
  destructive host mutation.

## Rollout

Plan B should become one implementation plan with two slices or two tightly
linked plans:

1. **B1 commit path:** schema migration, model APIs, publication helper,
   install/batch/remove integration, retry command, recovery/status/history
   behavior, focused tests.
2. **B2 durability path:** shared durable filesystem helper, current-link
   durability, metadata/signature/operation-record durability, live-root
   parent-sync coverage, focused tests.

The implementation plan must stage exact changed paths only. Broad `git add`
of source or docs directories is not allowed.

## Acceptance Criteria

- A DB-backed publication intent exists before post-commit generation
  publication begins.
- Post-DB generation publication phases are typed and persisted.
- Unknown publication phases or statuses fail closed.
- Recovery checks pending publication intents before accepting a valid selected
  `/conary/current`.
- A forced post-DB publication failure leaves an observable
  `needs_publication` state and still exits `0`.
- `system generation publish --changeset <id>` completes the specific
  DB-backed publication intent and is idempotent when already complete.
- Single install, batch install, and remove use the same failure and retry
  contract.
- CLI history/status surfaces distinguish fully published mutations from
  DB-committed but publication-pending mutations.
- Existing daemon status/history surfaces do not falsely report pending
  publications as fully published.
- The old retry hint that told users to run `system generation build` is
  replaced by the explicit publication retry command.
- Deferred generation follow-up metadata is typed or validated and preserves the
  existing changeset metadata envelope.
- `/conary/current` publication includes parent-directory fsync coverage.
- Generation metadata/signature writes and publication-relevant operation
  records propagate parent sync failures.
- Live-root journal-adjacent renames, backup moves, creates, and removals have
  parent-directory sync coverage where recovery depends on the target entry.

## Verification Commands For Implementation

The implementation plan should include focused gates before the full workspace
gate:

```bash
cargo test -p conary-core generation_publication
cargo test -p conary generation_publication
cargo test -p conary --test live_host_mutation_safety
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
git diff --check
```

Exact test names can change during planning, but the final implementation gate
must prove both publication intent semantics and durable filesystem behavior.
