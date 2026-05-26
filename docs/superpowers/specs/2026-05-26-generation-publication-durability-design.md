---
last_updated: 2026-05-26
revision: 2
summary: Design for Plan B generation publication durability and recovery hardening
---

# Generation Publication Durability: Design Spec

**Date:** 2026-05-26
**Status:** Review-tightened design approved; implementation plans pending
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
   - Add a DB-backed generation publication debt table and typed model APIs.
   - Route install, batch install, and remove through the shared publication
     contract after DB commit.
   - Add an explicit retry command for pending publication intents.
   - Make recovery, history, and status surfaces consult pending publication
     state.
   - Include durable `/conary/current` parent-directory sync because the B1
     phase contract depends on knowing when the selected generation is durable.

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
- `crates/conary-core/src/generation/builder.rs` builds generations from
  `Trove::list_all(conn)` and therefore publishes the current installed DB
  state. It does not replay an isolated historical changeset.
- The current composefs package-mutation path uses a generation build API whose
  default activation mode can mark the DB state active before
  `/conary/current` is durably selected. Plan B must refactor this ordering
  rather than merely record it.
- `apps/conary/src/commands/install/mod.rs`, `apps/conary/src/commands/install/batch.rs`,
  and `apps/conary/src/commands/remove.rs` append free-form deferred follow-up
  metadata when post-commit generation rebuild fails.
- That deferred metadata currently uses string fields such as
  `kind = "generation_rebuild"` and `status = "failed"` and points to
  `system generation build`, which builds a new inactive generation instead of
  clearing the DB-backed publication debt.
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

Use a real SQLite table as the authoritative publication debt ledger, not
changeset metadata and not sidecar JSON.

Plan B adds a `generation_publications` table with typed Rust model APIs. Each
row records that committed DB state still needs to be flushed into a selected
generation, or that such a flush previously failed. Rows may link to the
changeset that created or observed the debt for history and diagnostics, but
they are not independent historical build tasks.

This distinction is important. Conary generation builds are snapshots of the
current DB state. If changeset A and changeset B both have pending publication
debt, a single successful publication of current DB state publishes the effects
of both. It must mark both debts complete instead of rebuilding twice or
pretending that A and B can be replayed separately.

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
| `trigger_changeset_id` | Nullable reference to the changeset that created or observed this publication debt. |
| `published_through_changeset_id` | High-water changeset ID represented by the generation once publication completes. |
| `tx_uuid` | Optional transaction UUID for correlation with existing transaction state. |
| `db_path` | Canonical DB path used by the command. |
| `runtime_root` | Runtime root whose `/conary/current` is being published. |
| `phase` | Typed publication phase as a snake-case string with a CHECK constraint if practical. |
| `status` | Typed status: `pending`, `running`, `failed`, `complete`, or `abandoned`. |
| `state_number` | System state number once the snapshot exists. In Plan B this must equal `generation_number`; keep it only as an explicit cross-check against current builder behavior. |
| `generation_number` | Generation number once the artifact exists. Current Conary generation builds use the generation number as the system state number. |
| `summary` | Human-readable publication summary. |
| `last_error` | Last failure message, if any. |
| `retry_count` | Number of retry attempts. |
| `recoverable` | Boolean flag for whether `generation publish` may retry. |
| `created_at` | Creation timestamp. |
| `updated_at` | Last phase/status update timestamp. |
| `completed_at` | Completion timestamp. |

Constraints:

- publication rows are debts to flush current DB state, not isolated requests to
  rebuild the exact historical state of `trigger_changeset_id`;
- successful publication records the high-water changeset ID represented by the
  selected generation and marks all pending recoverable rows at or below that
  high-water mark complete;
- the high-water changeset ID must count only applied DB state, specifically
  `applied` and `post_hooks_failed` changesets; pending or rolled-back
  changesets are not represented by the published generation and must not be
  swept complete;
- `state_number` and `generation_number` must be equal while the builder uses
  generation numbers as state numbers. A future design that allows divergence
  must rework DB-active lookups before relaxing that invariant;
- `trigger_changeset_id` should use `ON DELETE SET NULL` unless the
  implementation proves changesets are never deleted. Publication debt should
  not disappear silently merely because historical changeset rows are pruned;
- phases and statuses must parse into Rust enums before recovery acts on them;
- unknown phases or statuses fail closed;
- rows must preserve enough identity to reject accidental publication against
  the wrong runtime root. B1 may use canonical DB path plus canonical runtime
  root for the limited preview, but the implementation plan must explicitly
  decide whether to add a stable DB identity UUID in settings. If a DB is moved
  and the derived runtime root no longer matches the row, publication fails
  closed until the operator abandons or recreates the debt.

`tx_uuid` is optional and must not be required for composefs-native recovery.
The generation-aware path usually creates changesets without a transaction UUID.

## Phase Model

Publication phases are typed, but they should represent recovery decision
points rather than every syscall. Failure is represented by `status = failed`
plus the last safe phase and `last_error`.

| Phase | Meaning |
| --- | --- |
| `PendingBuild` | DB committed; no reusable validated generation artifact is known. |
| `Building` | Generation build is in progress. A crash here may require rebuilding from current DB state. |
| `ArtifactReady` | Artifact, metadata, manifest, and state snapshot exist and validate; generation/state number is known. |
| `CurrentPublished` | `/conary/current` points to the generation and its parent directory has been synced. |
| `ActiveMarked` | Matching DB state has been marked active. This is the complete phase. |

The implementation may add more granular internal trace events, but persisted
recovery state should stay at these semantic boundaries unless a finer phase is
needed for a concrete recovery decision. In particular, `CurrentLinkStarted`,
`CurrentRenamed`, and `CurrentSynced` should be one durable helper outcome:
`CurrentPublished`.

## Ordering Rule

Plan B changes the publication ordering so durable boot selection happens
before DB active-state marking:

1. Package DB commit.
2. Publication debt row created or updated.
3. Current DB state high-water changeset ID captured while holding the package
   transaction/publication lock.
4. Generation artifact built from current DB state with the DB snapshot left
   inactive.
5. Artifact, metadata, manifest, and state snapshot validated.
6. `/conary/current` updated and parent directory synced.
7. Matching system state marked active in the DB.
8. Publication debt rows at or below the high-water changeset ID marked
   complete.

This ordering avoids the current unsafe shape where DB state can claim a
generation is active before `/conary/current` durably points at it. The reverse
is easier to recover: if `/conary/current` is durable but DB active marking
failed, recovery can validate the artifact and mark the matching state active.

This requires a real refactor. `rebuild_and_mount` and the builder path must use
inactive generation creation for package-mutation publication, then activate the
state only after current-link publication succeeds. The implementation plan
must name the exact call sites that currently use active generation creation and
move them behind the new publication helper.

## Package Mutation Flow

Install, batch install, and remove should call one shared post-commit
publication helper after the package DB transaction commits.

Expected behavior:

1. Commit the package mutation transaction.
2. Create or update a `generation_publications` debt row for the triggering
   changeset or command.
3. Attempt publication through the phase model while holding the same
   transaction/publication lock used to prevent concurrent package mutations.
4. If publication completes, mark all pending recoverable rows covered by the
   published DB high-water mark `complete`.
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
Run: conary --allow-live-system-mutation system generation publish
```

If a later package mutation successfully publishes current DB state, it must
automatically resolve earlier pending debts covered by that generation. Required
test shape: install A commits and publication fails; install B commits and
publication succeeds; A's prior publication debt is marked complete because the
selected generation now represents the current DB state including A and B.

## Retry Command

Plan B1 adds an explicit retry command whose primary form flushes current DB
state:

```bash
conary --allow-live-system-mutation system generation publish
```

Behavior:

- If no pending recoverable publication debt exists, report that publication is
  already current and exit successfully.
- If pending or failed recoverable debt exists, publish current DB state once
  and mark every covered debt complete.
- If an existing artifact recorded in a debt row still validates and represents
  the current high-water DB state, reuse it rather than rebuilding.
- If the phase/status is unknown, fail closed.
- If all matching rows are `abandoned`, fail with a diagnostic that manual
  intervention is required.

Optional selector:

```bash
conary --allow-live-system-mutation system generation publish --changeset <id>
```

`--changeset <id>` is an assertion/filter, not a request to rebuild that
historical changeset in isolation. It should fail if the changeset has no
pending recoverable publication debt. If it succeeds, publication still flushes
current DB state and may complete other pending debts covered by the same
generation.

`system generation recover --publish-pending` is an allowed convenience flag,
but it is optional for B1. The required B1 surface is parameterless `publish`
plus the optional `--changeset` guard if the implementation plan includes it.

## Recovery Behavior

Recovery must consult pending publication intents before accepting a valid
selected `/conary/current` artifact as complete.

Required behavior:

- If no pending/failed recoverable publication debts exist, current recovery
  may continue with selected-generation validation.
- In pre-command transaction recovery, recoverable debt must stay visible but
  must not block a later package mutation from publishing current DB state and
  sweeping older debt complete. If `/conary/current` already points at the debt
  generation, recovery may catch the DB active marker up; otherwise it should
  warn and continue with the valid selected generation.
- Manual `system generation publish` is the fail-closed remediation path: it
  must complete recoverable debt or return a clear diagnostic.
- In boot-selection recovery contexts, if publication cannot be completed with
  the available runtime facilities, recovery should prefer booting the last
  complete published generation and leave the debt visible for later retry
  rather than making the system unnecessarily unbootable. If no complete
  generation exists, fail with a clear manual-intervention diagnostic.
- If multiple pending debts exist, recovery/publish should perform one
  publication of current DB state and complete all covered debts rather than
  rebuilding once per changeset.
- If a crash occurs after `/conary/current` is durably renamed but before the
  row advances to `CurrentPublished`, recovery may complete an `ArtifactReady`
  debt when its `generation_number` matches the selected current generation and
  the artifact validates.
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
  debt. B1 should review at least `/v1/history`, `/v1/transactions`,
  `/v1/transactions/{id}`, and `/v1/system/states`. A dedicated conaryd
  endpoint can be deferred if existing route output can truthfully surface the
  state.

The exit-code contract must be documented in CLI help or operator docs:
exit `0` after a package mutation means the package DB commit succeeded. Scripts
that need boot-state convergence must check publication status separately.

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

Legacy metadata cannot be fully migrated by a schema migration alone because
old follow-up JSON does not reliably contain runtime-root identity. Plan B must
still prevent old hints from misleading operators. The minimum requirement is:
when rendering or appending follow-up metadata, recognize legacy
`generation_rebuild` records, suppress the old `system generation build` retry
hint, and point operators at `system generation publish`. If practical, B1 may
perform lazy backfill of publication debt rows when opening a DB with known
runtime root information.

## Durable Filesystem Helper

Plan B1 must make `/conary/current` publication durable enough for
`CurrentPublished` to mean what it says: temp symlink, atomic rename, and
successful parent-directory sync. Plan B2 then adds shared durable filesystem
helpers, preferably in `conary-core`, for the broader publication pattern Conary
repeats today:

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
- `.conary-gen.pending` writes/removals;
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

Operation records should be split into "must sync" and "best effort" classes.
Records used for recovery decisions are B2 targets. Informational-only records
may use the helper for consistency, but they should not expand Plan B's
acceptance surface.

## Testing Strategy

B1 tests:

- migration creates `generation_publications` and increments `SCHEMA_VERSION`;
- phase/status parsing rejects unknown values;
- forced post-DB rebuild failure creates a pending or failed publication intent
  and exits successfully;
- install, batch install, and remove share the same publication failure
  contract;
- multiple pending debts are resolved by one successful publication of current
  DB state;
- a later successful install publication marks prior covered debts complete;
- pending debt for a changeset whose packages were later removed is resolved by
  publishing current DB state, not by trying to reconstruct removed troves;
- `system history` marks incomplete publications;
- the pending publication query surface includes changeset ID, phase, status,
  and retry command;
- `system generation publish` completes pending publication debt;
- `system generation publish --changeset <id>`, if implemented, acts as an
  assertion/filter and still completes all covered debts;
- publish is idempotent for completed publications;
- recovery checks pending publication intents before accepting a valid selected
  `/conary/current`;
- recovery handles the crash shape where current-link publication succeeded but
  DB active marking did not;
- retry validates and reuses an existing artifact at `ArtifactReady` when it
  still represents the current DB high-water mark;
- recovery fails closed on unknown publication phase/status;
- deferred follow-up kind/status values are typed or validated.
- `update_current_symlink` reports parent-directory sync errors through the B1
  publication path.

B2 tests:

- generation metadata/signature writers propagate parent-sync errors;
- operation record writes use durable temp-write and rename helpers;
- live-root rename/remove/directory helpers call parent sync after target
  changes;
- temp-root recovery tests cover representative publication phases without
  destructive host mutation;
- generation GC refuses to remove a generation referenced by an incomplete
  publication debt row;
- concurrent `generation publish` and package mutation attempts serialize on
  the transaction/publication lock.

## Rollout

Plan B should become one implementation plan with two slices or two tightly
linked plans:

1. **B1 commit path:** schema migration, model APIs, publication helper,
   install/batch/remove integration, `rebuild_and_mount` activation-order
   refactor, durable current-link parent sync, retry command,
   recovery/status/history behavior, focused tests.
2. **B2 durability path:** shared durable filesystem helper,
   metadata/signature/pending-marker/operation-record durability, live-root
   parent-sync coverage, GC coordination, focused tests.

The implementation plan must stage exact changed paths only. Broad `git add`
of source or docs directories is not allowed.

## Acceptance Criteria

- A DB-backed publication debt row exists before post-commit generation
  publication begins.
- Post-DB generation publication phases are typed and persisted.
- Unknown publication phases or statuses fail closed.
- Recovery checks pending publication intents before accepting a valid selected
  `/conary/current`.
- A forced post-DB publication failure leaves an observable
  `needs_publication` state and still exits `0`.
- `system generation publish` flushes current DB state, completes all covered
  pending publication debts, and is idempotent when already complete.
- `system generation publish --changeset <id>`, if implemented, does not claim
  to replay historical changeset state; it is an assertion/filter over pending
  debt.
- Single install, batch install, and remove use the same failure and retry
  contract.
- A subsequent successful publication resolves earlier pending debts covered by
  the published DB high-water mark.
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
