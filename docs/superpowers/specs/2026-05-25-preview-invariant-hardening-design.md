---
last_updated: 2026-05-26
revision: 5
summary: Umbrella design for hardening limited-preview safety and truthfulness invariants
---

# Preview Invariant Hardening: Design Spec

**Date:** 2026-05-25
**Status:** Plan A implemented in `2e294320`; Plans B and C remain open
**Goal:** Turn the overhead safety review into a release-hardening milestone
that makes Conary's limited-preview claims harder to accidentally violate.

---

## Purpose

Conary is already framed as an adoption-led limited public preview. The next
milestone should harden that preview rather than broaden it. The important
work is not another feature surface; it is making core invariants explicit,
testable, and difficult to drift:

- mutating active-host commands require an explicit acknowledgement;
- adopted content is immutable once captured into CAS;
- adoption database state never pretends a package has coverage it does not
  have;
- post-DB generation publication is durable and recoverable;
- active docs, command hints, and CI gates match the behavior in the repo.

This spec is an umbrella. It defines the target hardening program and splits it
into tracks that can become separate implementation plans.

## Current Repo Facts

The review confirmed several concrete issues:

- `apps/conary/src/dispatch.rs` gates install, remove, update, restore,
  unadopt, native-handoff, and takeover, but `SystemCommands::Adopt` dispatches
  directly to package adopt, system adopt, refresh, convert, and sync-hook
  actions.
- `apps/conary/src/commands/adopt/hooks.rs` installs or removes package-manager
  hook files under host paths such as `/usr/lib/rpm`, `/etc/apt`, and
  `/etc/pacman.d`.
- Before this hardening work, the installed hook templates invoked
  `conary system adopt --refresh --quiet` without the preview acknowledgement
  boundary, so Plan A adds a narrow hook-only refresh contract before gating
  general refresh.
- `apps/conary/src/commands/adopt/convert.rs` backfills adopted source identity
  before the `--dry-run` return path, so `system adopt --convert --dry-run` is
  not currently a true non-mutating dry-run.
- `apps/conary/src/commands/adopt/cas_capture.rs` uses
  `CasStore::hardlink_from_existing` for full-adoption regular files.
- `crates/conary-core/src/filesystem/cas.rs` currently hardlinks adopted files
  into the CAS path and has tests that assert shared inode behavior.
- `apps/conary/src/commands/adopt/packages.rs` cleans up a newly inserted trove
  when all child metadata inserts fail, but
  `apps/conary/src/commands/adopt/system.rs` only warns and continues in the
  equivalent bulk-adoption case.
- `apps/conary/src/commands/adopt/refresh.rs` updates adopted package metadata
  and can delete existing child rows before all replacement child rows are
  known to have persisted.
- `crates/conary-core/src/generation/mount.rs` updates `/conary/current` with a
  temp symlink and rename, but the publication path is not yet modeled as a
  typed durable phase sequence.
- `apps/conary/src/commands/install/mod.rs` can commit DB package state, record
  deferred generation-rebuild metadata on post-commit rebuild failure, and
  still return success.
- The deferred rebuild path can be invisible to automation that checks only
  process success or changeset `Applied` status, and the current retry command
  builds another generation instead of completing a specific DB-backed
  publication intent.
- The same inactive-build retry shape appears in batch install and remove paths;
  Track 3 must treat this as a package-mutation publication problem, not a
  single-command install problem.
- Current generation build/state code can mark DB active state before
  `/conary/current` is durably selected, so the publication model must separate
  DB package commit, state snapshot creation, DB active marking, current-link
  publication, and parent sync.
- Current recovery can accept a valid selected `/conary/current` artifact before
  considering a pending package-mutation publication intent.
- `apps/conary/src/commands/live_root.rs` has durable journal writes, but some
  live-root target renames, backup moves, directory creates, and removals lack
  parent-directory sync coverage.
- The review found drift examples such as schema version mismatches, retired
  command hints, stale preview gate wording, and an incomplete conaryd API
  reference path. Track 4 turns those into automated truth checks.
- `apps/conaryd/src/daemon/auth.rs` safely denies PolicyKit authorization
  attempts today, but its module docs describe non-root PolicyKit authorization
  as available instead of clearly calling it an unimplemented stub.
- The existing docs-audit scripts are useful but are not yet wired into the
  regular PR gate.

## Decision

Define a single limited-preview hardening milestone with four tracks:

1. **Command Risk And Adoption Gate**
2. **CAS And Adoption State Integrity**
3. **Generation Publication Durability**
4. **Docs, CI, And Public-Surface Truth Checks**

The first implementation plan should cover Tracks 1 and 2. Tracks 3 and 4
should receive follow-on plans unless the user explicitly chooses to batch
them.

Any plan that adds, moves, or archives docs under `docs/superpowers/` must update
the docs-audit inventory and ledger in the same change. That bookkeeping is not
deferred to Track 4.

## Track 1: Command Risk And Adoption Gate

**Plan A status:** Implemented in `2e294320`. Mutating adoption modes now route
through the centralized command-risk policy, installed hooks use the constrained
`--refresh --quiet --from-sync-hook` path, and dry-run conversion no longer
backfills source identity before returning.

### Intent

Live-host mutation policy should be represented as data or a small focused API,
not as scattered match-arm convention. Any future command should be hard to add
without classifying its risk and acknowledgement behavior.

### Required Behavior

Classify the current CLI surface exhaustively into at least these categories:

- `ReadOnly`: no active-host mutation and no DB mutation.
- `DryRunOnly`: allowed without live-host acknowledgement only when the path is
  proven non-mutating.
- `DbMutation`: mutates Conary state but not live host files.
- `ActiveHostMutation`: may mutate live host files, active package-manager
  integration, selected generation state, scriptlet-visible state, or native
  package ownership.
- `AlwaysLive`: generation activation/recovery/switching and similar operations
  that are inherently live regardless of `--root`.

Plan A must define the user-facing acknowledgement boundary for `DbMutation`
commands that target the default live Conary DB. It may use the existing
`--allow-live-system-mutation` preview flag for those paths, but refusal text
must describe the actual risk instead of implying scriptlets, remounts, or
`/etc` rewrites when the command is DB-only.

Adoption modes should be classified explicitly:

| Adopt mode | Classification | Gate |
| --- | --- | --- |
| `system adopt --status` | `ReadOnly` | No live-host acknowledgement. |
| `system adopt --system --dry-run` | `DryRunOnly` | No acknowledgement if no DB or host writes occur. |
| `system adopt --refresh --dry-run` | `DryRunOnly` | No acknowledgement if no DB or host writes occur. |
| `system adopt --convert --dry-run` | `DryRunOnly` | Must not write DB metadata before returning. |
| `system adopt <pkg> --dry-run` | `DryRunOnly` | No acknowledgement if no DB, CAS, or host writes occur; otherwise reject as unsupported. |
| `system adopt <pkg> --full --dry-run` | `DryRunOnly` | Must not capture files into CAS or mutate DB. |
| `system adopt --system --full --dry-run` | `DryRunOnly` | Must not capture files into CAS or mutate DB. |
| `system adopt <pkg>` | `DbMutation` | Requires the Plan A acknowledgement chosen for live-DB adoption. |
| `system adopt <pkg> --full` | `DbMutation` | Requires acknowledgement; CAS capture must satisfy Track 2. |
| `system adopt --system` | `DbMutation` | Requires acknowledgement; bulk insert must satisfy Track 2. |
| `system adopt --system --full` | `DbMutation` | Requires acknowledgement; full CAS capture must satisfy Track 2. |
| `system adopt --refresh` | `DbMutation` | Requires acknowledgement unless invoked through the narrow hook-refresh contract below. |
| `system adopt --refresh --quiet --from-sync-hook` | `HookRefreshDbMutation` | Hidden installed-hook path; no interactive acknowledgement, root-owned hook context only, no `--full`. |
| `system adopt --convert` | `DbMutation` | Requires acknowledgement for DB writes; `--dry-run` must be non-mutating. |
| `system adopt --sync-hook` | `ActiveHostMutation` | Requires `--allow-live-system-mutation`. |
| `system adopt --sync-hook --remove-hook` | `ActiveHostMutation` | Requires `--allow-live-system-mutation`. |

Plan A chooses a narrow installed-hook refresh path rather than embedding the
global acknowledgement flag in generated hook scripts. The hidden
`--from-sync-hook` flag must require `--refresh --quiet`, conflict with `--full`
and all other adoption modes, and be accepted only for the installed
package-manager hook contract. Hook install/remove remains gated. Generated hook
scripts must call `conary system adopt --refresh --quiet --from-sync-hook`, and
operator-facing hook text must say that installing hooks enables a constrained
hook-only refresh of Conary's adopted-package DB metadata after native package
manager transactions. The hidden flag must not become a general user-facing
bypass for interactive refresh.

The `--root` option must not be presented as active-host isolation for commands
whose runtime state still resolves to live Conary locations. If a future
offline-root mode is added, it should use a distinct name and a separate
runtime-root contract.

### Acceptance Criteria

- The dispatcher has one visible policy path for live-host acknowledgement.
- Every Clap command variant has an explicit `CommandRisk` classification; tests
  fail when a new command variant is added without a policy entry.
- `system adopt` mutating modes refuse without the acknowledgement chosen for
  active live-DB adoption.
- `system adopt --status` remains ungated.
- Dry-run adopt modes either remain non-mutating or are rejected as unsupported
  dry-runs.
- `system adopt --convert --dry-run` has a regression test proving it does not
  write DB metadata before returning.
- Sync-hook install/remove is gated.
- Installed sync-hook refresh behavior has a concrete contract: hook
  install/remove remains gated, generated hook scripts use only the constrained
  `--refresh --quiet --from-sync-hook` path, that path refuses unsupported
  combinations such as `--full`, and the operator-facing hook text says hook
  installation enables constrained hook-only refresh after native package
  manager transactions.
- Table-driven tests enumerate the full command-risk registry and expected gate
  behavior.

## Track 2: CAS And Adoption State Integrity

**Plan A status:** Implemented in `2e294320`. Full adoption now copies mutable
live files into private CAS inodes, touched legacy shared CAS objects are
repaired on copy, adoption warnings extend the existing changeset metadata
envelope, ghost troves are removed, and refresh replacement uses per-package
savepoints.

### Intent

Full adoption should produce private immutable CAS objects and truthful
adoption database state. Native package-manager files are mutable from
Conary's perspective, so they must not share inodes with CAS objects.

### Required Behavior

Regular-file full adoption must store content into CAS by copy, reflink-copy,
or another private-inode path. Hardlinking from mutable live roots is allowed
only when the source root is explicitly proven immutable or sealed. The default
live adoption path must not use shared-inode CAS objects.

Any remaining hardlink-based CAS API should be named or typed so it cannot be
called from live adoption by accident. Plan A should decide whether legacy
already-hardlinked CAS objects are out of scope, detected and warned about, or
repaired during refresh.

Plan A chooses a bounded legacy-CAS repair: the mutable-source copy API must
not trust an existing hash path blindly. If the target CAS object already exists
and appears to be a shared hardlink on Unix, the copy API must replace that CAS
directory entry with a private inode by temp-write, fsync, atomic rename, and
parent-directory fsync. A repo-wide sweep of every historical CAS object is out
of scope for Plan A and should be recorded as a follow-on audit/repair task.

Package-level adoption should be all-or-clean:

- If all child metadata inserts fail for a new package, the trove must not
  persist.
- Partial child metadata success may persist only if the command surfaces a
  degraded package result and records package names plus reasons in durable,
  operator-visible state. Adoption warning metadata must extend the existing
  `conary.changeset.metadata.v1` envelope instead of overwriting
  `changesets.metadata` with an unrelated JSON shape.
- Refresh must preserve old child metadata unless the replacement package
  metadata has been fully prepared and successfully committed.
- Empty package metadata is allowed only when it is a real package-manager fact,
  not a consequence of swallowed insert failures.

### Acceptance Criteria

- A mutation-after-capture regression test proves that modifying the original
  source file in place after full adoption does not change the CAS object.
- On Unix, CAS capture tests assert that default live adoption does not share an
  inode with the source file, or they prove that the only remaining hardlink API
  is unreachable from live adoption.
- Bulk adoption cannot leave ghost troves when all child inserts fail.
- Refresh cannot hollow out an existing adopted package when replacement child
  insertions fail; injected failures after child-row deletion must leave the old
  version plus old files/dependencies/provides intact. If refresh continues past
  a failed package, that package must be recorded as degraded or skipped in
  durable adoption warning metadata.
- Adoption summaries distinguish adopted, skipped, degraded, and failed
  packages.
- Degraded adoption persists package names and warning reasons in a durable
  operator-visible record. Plan A uses the existing versioned changeset
  metadata envelope with an `adoption_warnings` field; it must preserve
  rollback snapshots and deferred follow-up metadata.

## Track 3: Generation Publication Durability

### Intent

The composefs-native model treats the DB commit as the point of no return, with
generation build and selection re-derivable afterward. Publication should be
modeled and persisted so recovery can complete or fail closed from every
important phase.

### Required Behavior

Introduce a typed publication/recovery contract around generation publication.
The exact storage can be decided in the implementation spec, but Plan B should
define a persisted publication intent before artifact build starts. That intent
should record at least the package changeset, runtime root/DB identity, intended
generation or state number once reserved, current phase, failure status, and
retry command semantics.

The phase model must separate package DB commit, system-state snapshot creation,
DB active marking, current-link publication, and parent sync. It should be able
to represent phases such as:

- `DbCommitted`
- `StateSnapshotStarted`
- `StateSnapshotRecorded`
- `DbActiveMarkStarted`
- `DbActiveMarked`
- `ArtifactBuildStarted`
- `ArtifactBuilt`
- `CurrentTmpCreated`
- `CurrentRenamed`
- `CurrentSynced`
- `Complete`
- `Failed`

Use a shared durable filesystem helper for temp-write, content fsync, atomic
rename, and parent-directory fsync. It should propagate fsync errors instead of
silently treating them as best effort where crash consistency matters.

Apply the helper or equivalent durable behavior to:

- `/conary/current` updates;
- generation metadata;
- `.conary-gen.sig` writes when generation signing is active;
- artifact and boot manifests;
- boot asset copies where the artifact contract depends on them;
- operation records that participate in generation publication, takeover, or
  recovery; broad operation-record hardening can be factored into a shared
  durable-filesystem helper task;
- live-root journal-adjacent target renames, backup moves, directory creates,
  and removals.

Deferred generation rebuild metadata must tell the operator to run a command
that actually completes the DB-backed publication path. Building an inactive
generation is not enough when the selected boot state remains behind the DB.
Plan B must also define automation-facing semantics for this state: either the
CLI/daemon command exits or reports a typed partial-success result, or success
includes an explicit machine-readable needs-publication record. A plain
successful command plus `Applied` changeset with only warning text is not enough.

### Acceptance Criteria

- Recovery has tests for each publication phase that can be represented without
  destructive host mutation.
- Recovery consults pending publication intents before accepting an otherwise
  valid selected `/conary/current` artifact as complete.
- A forced post-DB generation rebuild failure leaves an observable
  needs-publication state.
- CLI, daemon, and changeset/status surfaces distinguish fully published
  mutations from DB-committed-but-publication-pending mutations in a way scripts
  can detect.
- The retry path rebuilds and selects or recovers the DB-backed generation
  according to the publication record.
- Single install, batch install, and remove all use the same post-DB
  publication failure contract.
- Unknown publication phases fail closed.
- Live-root journal state is typed or otherwise rejects unknown states instead
  of silently choosing rollback or cleanup.
- Deferred follow-up kind/status values are typed or validated before use.

## Track 4: Docs, CI, And Public-Surface Truth Checks

### Intent

Docs should not be a parallel product. Active docs, examples, and release
status should be cheap to check against code and generated command surfaces.

### Required Behavior

Add lightweight truth checks for:

- schema version mentions in active docs versus
  `crates/conary-core/src/db/schema.rs::SCHEMA_VERSION`;
- retired command names, including the old one-word system-adoption spelling;
- impossible takeover hints in adoption command examples;
- stale preview-gate wording that contradicts current README, ROADMAP, and
  integration-testing evidence;
- conaryd API reference links that point to the actual daemon route surface;
- daemon/auth docs or comments that claim stubbed authorization paths are
  implemented, especially PolicyKit;
- docs-audit inventory and ledger consistency.

Each truth check should have a named script or test output, not just a one-time
cleanup checklist entry. Schema drift, retired command names, impossible command
hints, preview-gate contradictions, conaryd API route drift, stubbed auth
overclaims, and docs-audit inventory/ledger drift should each fail with a
message pointing to the offending file or generated help surface.

Decide whether `conary-core` is currently an internal crate with broad public
modules for workspace convenience, or whether it is a public API crate that
needs a curated facade and API baseline. The default for this milestone is to
record the decision and avoid a broad facade refactor unless it becomes needed
for a concrete invariant.

### Acceptance Criteria

- `.github/workflows/pr-gate.yml` or a dedicated docs workflow runs the
  docs-audit inventory and ledger checks on every pull request that touches
  tracked docs or audit tooling. Plan C must choose the workflow explicitly.
- A docs truth-check script fails on active-doc schema drift.
- Docs truth-check coverage fails on retired adoption commands, impossible
  adoption hints, stale preview-gate contradictions, conaryd API route drift, and
  daemon/auth docs that claim unsupported PolicyKit authorization works.
- Active docs and app strings no longer recommend retired adoption commands.
- README, ROADMAP, and integration docs do not simultaneously describe a gate
  as both passed and still blocked.
- conaryd API docs point to a maintained endpoint list.
- conaryd auth docs say PolicyKit authorization is unavailable or stubbed until
  a real DBus check and policy file contract exist.
- The `conary-core` public-surface decision is documented without claiming a
  smaller API than the code exposes.

## Implementation Sequencing

The umbrella milestone should be implemented as multiple plans:

1. **Plan A: Adoption Safety And Integrity**
   - Track 1 plus Track 2.
   - This is the recommended first plan because it closes the highest-risk
     active-host mutation and CAS immutability gaps.
   - Include any docs, integration manifests, and app-string touchups required
     by changed adoption acknowledgement behavior. Broader docs truth-check
     automation stays in Plan C.

2. **Plan B: Generation Publication Durability**
   - Track 3.
   - This needs a focused design for phase persistence, durable filesystem
     helpers, and failure injection.

3. **Plan C: Docs And CI Truth Checks**
   - Track 4.
   - This can run in parallel with Plan B if desired, but it should not delay
     Plan A.

## Lifecycle

This umbrella remains active only while Plans A, B, and C are being split out or
executed. When the follow-on plans either land or explicitly defer a track, move
this file to `docs/superpowers/specs/archive/` and update the docs-audit
inventory, ledger, and summary in the same change.

## Out Of Scope

- No new package-manager features.
- No expansion of the limited public preview surface.
- No schema migration unless a specific track requires persisted state.
  Track 3's publication intent and Track 2's degraded-adoption warning record are
  the likely reasons to introduce one; Plan A or B must say so explicitly before
  doing it.
- No broad `conary-core` facade refactor in Plan A.
- No destructive host tests.
- No QEMU release rerun as part of writing this spec.

## Verification Strategy

Each plan should include focused verification rather than waiting for a full
workspace gate every few minutes. The expected final gate for implementation
work is still:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
git diff --check
```

Track-specific verification should include:

- `cargo test -p conary --test live_host_mutation_safety -- --nocapture`
- focused adoption/CAS unit tests under `apps/conary/src/commands/adopt/` and
  `crates/conary-core/src/filesystem/cas.rs`;
- targeted generation publication/recovery tests once Track 3 is planned;
- docs-audit inventory and ledger scripts whenever a tracked doc is added, moved,
  or archived, plus broader docs truth-check scripts once Track 4 is planned.

## Review Notes

This spec intentionally preserves the overhead review's main conclusion: keep
going, but stop expanding for a bit. The next milestone is invariant hardening
for a serious preview package manager, not a feature release.
