# Live-System Mutation UX Redesign

## Status

Reviewed design; implementation planning pending.

## Goal

Replace Conary's one-size-fits-all live-system mutation acknowledgement UX with
a risk-tiered intent model that lowers everyday CLI friction while preserving
explicit safety for commands that can mutate the active host, boot selection,
generation state, or recovery state.

## Background

The current CLI exposes a global `--allow-live-system-mutation` flag. It is
intentionally long and explicit, but the same wording now covers several
different risks:

- metadata-only Conary DB or CAS updates;
- package, file, scriptlet, and ownership mutations on the active root;
- generation build, switch, rollback, garbage collection, publication, and
  recovery flows;
- conaryd package jobs that reuse the CLI package-operation contracts.

That made sense while the live-host guard was first being introduced. It is now
too blunt. The flag is noisy in everyday examples, makes safe workflows look
more frightening than they are, and spreads a single policy phrase through CLI
help, docs, tests, conaryd request payloads, and integration manifests.

The redesign should use the existing command-risk model rather than invent a
second safety system. `apps/conary/src/command_risk.rs` already classifies
commands as read-only, local-state mutation, dry-run-only, hook-refresh DB
mutation, DB mutation, active-host mutation, or always-live. The follow-up
implementation should make the UX match those categories.

## Current Repo-Grounded Surface

Primary policy files:

- `apps/conary/src/cli/mod.rs` defines the global
  `allow_live_system_mutation` flag and the root `after_help` examples.
- `apps/conary/src/command_risk.rs` classifies CLI command risk and calls the
  live-host acknowledgement helper.
- `apps/conary/src/live_host_safety.rs` renders current refusal messages.
- `apps/conary/src/dispatch.rs` still has deeper per-dispatch live-mutation
  checks for command groups.
- `apps/conaryd/src/daemon/routes.rs`,
  `apps/conaryd/src/daemon/routes/transactions.rs`,
  `apps/conaryd/src/daemon/package_ops.rs`, and
  `apps/conaryd/src/daemon/client.rs` carry the daemon-side request field and
  execution gate.

Primary user-facing source strings and persisted hints:

- `apps/conary/src/commands/adopt/refresh.rs`,
  `apps/conary/src/commands/generation/builder.rs`,
  `apps/conary/src/commands/generation/publication.rs`,
  `apps/conary/src/commands/install/mod.rs`,
  `apps/conary/src/commands/install/batch.rs`,
  `apps/conary/src/commands/query/history.rs`, and
  `apps/conary/src/commands/update.rs` contain user-facing hints that mention
  the old global flag.
- `apps/conary/src/commands/changeset_metadata.rs` serializes
  `ChangesetMetadataEnvelope` records with `DeferredFollowUp.retry_command`.
  Existing databases may already contain retry commands with the old global
  flag, so the CLI parser must keep accepting that flag as a deprecated
  compatibility alias or fallback during the redesign. Do not hard-reject
  persisted retry commands in the first implementation.

Primary tests:

- `apps/conary/tests/live_host_mutation_safety.rs` verifies CLI refusal and
  dry-run bypass behavior.
- `apps/conary/tests/cli_daily_ux.rs` verifies daily workflow wording and
  refusal routing.
- `apps/conary/tests/component.rs`,
  `apps/conary/tests/live_host_mutation_readiness.rs`,
  `apps/conary/tests/model_apply.rs`, `apps/conary/tests/query.rs`, and
  `apps/conary/tests/workflow.rs` contain hardcoded active-mutation
  invocations that must be migrated or kept compatible.
- `apps/conary/tests/native_pm_live_root.rs`,
  `apps/conary/tests/native_pm_daily_driver.rs`, and
  `apps/conary/tests/bundle_replay.rs` contain active mutation invocations that
  must move with the new apply-intent wording.
- `apps/conary/src/command_risk.rs` unit tests verify risk classification.
- `apps/conary/src/live_host_safety.rs` unit tests verify refusal wording.
- `apps/conary/src/cli/mod.rs` unit tests verify global flag parsing.
- `apps/conaryd/src/daemon/package_ops.rs` tests verify daemon package-job
  refusal without acknowledgement.
- `apps/conary-test/src/config/mod.rs` asserts integration manifest mutation
  commands acknowledge live mutation unless they are dry runs; update this test
  in the same patch as the integration manifests.

Primary docs and generated surfaces:

- `README.md`
- `ROADMAP.md`
- `docs/ARCHITECTURE.md`
- `docs/conaryopedia-v2.md`
- `apps/conary/man/conary.1`
- `man/conary.1`
- `docs/operations/daily-driver-ux-matrix.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/conaryd.md`
- `docs/modules/ccs.md`
- `docs/operations/bootstrap-selfhosting-vm.md`
- `docs/operations/live-mutation-backup-inventory.md`
- `docs/operations/post-generation-export-follow-up-roadmap.md`
- `docs/operations/release-artifact-matrix.md`
- `apps/conary/tests/integration/remi/manifests/*.toml`

## Non-Goals

- Do not remove live-host safety entirely.
- Do not weaken scriptlet replay gates, trust gates, selected-generation
  safeguards, rollback/recovery safeguards, or active-root mutation preflights.
- Do not change SQLite schemas, `.ccs` formats, Remi persisted state, conaryd
  job schemas, or integration manifest schemas in the first slice.
- Do not make conaryd API clients migrate immediately unless the child plan
  proves a compatibility path.
- Do not hide broad manifest rewrites inside an unrelated behavior change.
- Do not make old persisted `DeferredFollowUp.retry_command` strings fail to
  parse. Old databases can contain retry commands with the current global flag.

## Design Principles

1. **Intent should match risk.** A metadata-only Conary DB update should not
   require the same phrase as a generation rollback.
2. **Dry-run remains the safest first path.** Where a command supports dry-run,
   refusal messages should prefer preview first.
3. **`--yes` can mean "apply the described plan"; it should not silently mean
   "ignore every active-host risk."** The implementation must decide which risk
   tiers accept `--yes` and which still need a stronger operation-specific
   acknowledgement.
4. **No accidental active-host mutation.** A command that mutates the active
   host, active boot selection, or recovery state must require explicit operator
   intent in non-interactive contexts.
5. **The daemon mirrors the policy.** conaryd should not preserve old wording
   forever just because its request field already exists, but API compatibility
   can be staged.

## Proposed Risk-Tier UX

### Tier 0: Read-Only, Local-State, And Dry-Run-Only

Examples include search, list, status, dry-run adoption, dry-run restore, and
local repo or pin metadata where the command already expresses the mutation.

UX rule:

- no live-system acknowledgement;
- no new global flag;
- dry-run commands continue to bypass active-host refusal.

### Tier 1: Conary DB/CAS Metadata Mutation

Examples include single-package adoption tracking, full adoption metadata/CAS
capture, adoption refresh, and other commands classified as `DbMutation`.

UX rule:

- remove the global live-system acknowledgement requirement;
- rely on the command name, existing command-specific options, and focused
  refusal wording;
- require `--yes` only where the command already needs non-interactive
  confirmation or deletes Conary tracking.

Reasoning:

These commands can update Conary's view of the machine, so they are not
read-only. But they are not the same as package file mutation, generation
activation, or recovery. Treating them as active-host mutation makes the
day-to-day adoption and refresh path more awkward than useful.

Implementation note:

Adopt commands currently have no secondary live-mutation refusal check in
`apps/conary/src/dispatch.rs`; the central `enforce_cli_policy` call is their
only gate. The implementation plan must explicitly choose one of these paths:

- add `--yes` or another command-scoped confirmation to mutating adopt variants
  in the same slice;
- keep a scoped continue/apply flag on adopt until a command-specific
  confirmation exists;
- document that `DbMutation` commands genuinely need no gate beyond the command
  name and existing options such as `--dry-run`, `--status`, `--system`, and
  `--refresh`.

### Tier 2: Active Package, File, Scriptlet, Model, And Automation Mutation

Examples include install, remove, update, autoremove, ccs install, model apply,
automation apply, state revert, state rollback, package restore, sync-hook
install/remove, db-backup recover, and unadopt flows that change active package
ownership, files, or the live Conary database.

UX rule:

- replace the global live-system phrase with normal apply intent;
- prefer `--yes` for non-interactive apply when the command already supports a
  meaningful plan, prompt, or destructive confirmation;
- keep dry-run as the primary preview path;
- refusal messages should say which command is about to mutate the active
  system and which apply flag or prompt is required.

Candidate CLI wording:

- interactive: prompt with a short operation-specific message after showing the
  plan, when stdin is a terminal and the command supports prompting;
- non-interactive: require `--yes` or a scoped apply flag chosen by the child
  implementation plan;
- docs/examples: show `conary install nginx --dry-run`, then
  `conary install nginx --yes` only when apply is intended.

The implementation plan must verify which commands already have `--yes`.
Commands without `--yes` should either gain it in the same slice or retain a
scoped explicit flag until a prompt/apply flow exists.

Current `--yes` inventory to resolve in the implementation plan:

| Command | Current confirmation | Action needed |
|---|---|---|
| `install` | `--yes` | Wire to the new apply-intent policy. |
| `update` | `--yes` | Wire to the new apply-intent policy. |
| `remove` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `autoremove` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `system restore` | `--force`, no `--yes` | Decide whether overwrite intent remains `--force` or gains `--yes`. |
| `system unadopt` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `system native-handoff` | `--yes` | Wire to the new apply-intent policy. |
| `system state revert` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `system state rollback` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `system db-backup recover` | `--yes` | Wire to the new apply-intent policy; this is `ActiveHostMutation`, not `AlwaysLive`. |
| `ccs install` | none | Add `--yes` or keep a scoped explicit apply flag. |
| `model apply` | `--force`, no `--yes` | Decide whether model apply intent remains `--force` or gains `--yes`. |
| `automation apply` | `--yes` | Wire to the new apply-intent policy. |
| `self-update` | `--force`, no `--yes` | Decide whether update intent remains `--force` or gains `--yes`. |

### Tier 3: Always-Live Generation, Boot, Publication, And Recovery Operations

Examples include generation build, switch, rollback, garbage collection,
generation publish follow-up, generation recovery, and generation-bound DB
recovery apply through `system generation recover-db`.

UX rule:

- keep the strongest acknowledgement behavior, but make it precise and
  operation-specific;
- do not let a generic `--yes` alone obscure high-risk generation or recovery
  actions unless the command already prints a concrete plan and the tests prove
  refusal-before-mutation;
- refusal messages should name the exact active asset: boot selection,
  `/conary/current`, generation-bound DB recovery target, generation
  publication follow-up, or rollback target.
- interactive generation switch, rollback, garbage collection, publish, and
  recovery flows should print the concrete generations, boot assets, DB backup,
  or publication debt they will affect before accepting confirmation;
- non-interactive generation switch, rollback, and garbage collection should
  reject a generic `--yes` unless the command has a specific target or a scoped
  confirmation flag that names the operation's real risk.

Candidate CLI wording:

- for non-interactive apply, accept `--yes` plus the operation's required
  identifier when the command already has a concrete target;
- for broad operations like generation garbage collection or recovery, keep a
  scoped flag or explicit prompt if `--yes` would be ambiguous.

The implementation plan should inventory each always-live command and decide
case-by-case. This is still one UX redesign, but the high-risk tier should not
be flattened into a single shortcut.

### Tier 4: conaryd Package Jobs

conaryd request bodies currently carry `allow_live_system_mutation`. A first
implementation slice should preserve backward compatibility for this field
while introducing the new semantic wording.

UX rule:

- daemon package jobs should follow the Tier 2 package apply policy;
- dry-run transaction routes stay dry-run-first and do not need live
  acknowledgement;
- existing request bodies with `allow_live_system_mutation: true` should keep
  working during the transition;
- docs and responses should stop requiring the old phrase once the CLI policy
  changes.
- `apps/conaryd/src/daemon/client.rs` should forward the preferred new intent
  once it exists, while still serializing the old compatibility field during the
  transition.
- daemon refusal wording should move with the shared helper so background jobs
  do not keep obsolete "early software" or global-flag language after the CLI
  changes.

Potential staging:

1. accept both the old request field and a new request intent field, if the
   implementation plan decides an API shape is needed;
2. update daemon tests and docs to prefer the new intent;
3. defer removal of the old field until a separate API cleanup plan.

## Rejected Approaches

### Short Alias Only

Adding a shorter alias for the existing global flag would reduce typing but
would not fix the core mismatch. DB/CAS metadata updates would still be framed
like package mutation, and generation recovery would still share wording with
ordinary install.

### Delete The Guard Entirely

Removing the guard from every command would make early workflows smoother but
would weaken the safety property for active package mutation, generation
selection, rollback, recovery, and daemon jobs. That is too blunt for the
current safety posture.

### Keep The Existing Flag Forever

The current flag is clear, but it is also broad enough to become background
noise. A safety acknowledgement that appears everywhere eventually stops
communicating anything useful.

## Implementation Packet Shape

The child implementation plan should be a full behavior plan, not a docs-only
cleanup. It should tackle the whole old-flag surface in one reviewed packet, but
the packet should stage compatibility and migration instead of doing one
all-at-once rename.

Suggested staging:

1. Add the tiered UX and command-scoped confirmations while keeping
   `--allow-live-system-mutation` accepted as a deprecated compatibility alias;
   update the conary-test manifest validator to accept either the old alias or
   the new apply-intent form.
2. Migrate active tests, integration manifests, user-facing source hints, docs,
   and generated manpages to the new wording.
3. Only after active surfaces are migrated, decide whether to hide, warn on, or
   eventually remove the old global flag. Hard removal requires a separate
   compatibility plan for old databases with persisted retry commands.

The implementation packet should likely split into these tasks:

1. Add failing tests for the new Tier 1/Tier 2/Tier 3 UX expectations.
2. Run an `rg` inventory of `allow-live-system-mutation`,
   `allow_live_system_mutation`, and `LiveMutationRequest`, then turn the
   results into an explicit implementation checklist.
3. Decide the Tier 1 `DbMutation` confirmation policy before removing the
   central gate for those commands.
4. Introduce a new mutation-intent helper or refactor the existing
   `LiveMutationRequest` API so risk class controls the refusal policy.
5. Update CLI parsing and help examples, including the current root
   `after_help` examples.
6. Update dispatch and command-risk enforcement while preserving deeper
   refusal-before-mutation checks.
7. Update conaryd package-job request handling with a compatibility path.
8. Update integration manifests and manifest validation tests while temporarily
   accepting both old and new intent forms.
9. Update docs, user-facing source hints, persisted follow-up generators, and
   generated manpages.
10. Run focused and medium gates.

The implementation plan should still avoid a partial user-visible rename. The
old phrase is spread through tests, docs, manifests, persisted follow-up hints,
and daemon behavior; any staged compatibility path must be explicit about which
surfaces still accept old input and which surfaces now prefer the new wording.

## Verification Gates

Minimum fast gates:

- `cargo test -p conary --lib command_risk`
- `cargo test -p conary --lib live_host_safety`
- `cargo test -p conary --test live_host_mutation_safety`
- `cargo test -p conary --test cli_daily_ux live_mutation`
- `cargo test -p conaryd package_executor_refuses_live_mutation_without_ack`
- `cargo run -p conary-test -- list`
- `cargo test -p conary-test config::tests::active_manifest_live_mutation_commands_acknowledge_live_mutation`
- `cargo fmt --check`

Medium gates when command examples, integration manifests, or conaryd request
semantics change:

- `cargo test -p conary --test native_pm_daily_driver`
- `cargo test -p conary --test native_pm_live_root`
- `cargo test -p conary --test bundle_replay`
- `cargo test -p conaryd daemon::routes`
- `cargo test -p conary-test suite_inventory`

Docs and hygiene gates:

- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -`
- `git diff --check`
- an added-line stale-term sweep over touched docs and active plans/specs.

## Acceptance Criteria

- DB/CAS-only mutation commands no longer require the global live-system
  acknowledgement.
- Active package/file/scriptlet mutation commands require explicit apply intent
  without relying on the old global phrase in user-facing examples.
- Always-live generation, boot, publication, and recovery commands still require
  explicit operator intent that names the real risk.
- conaryd package jobs preserve compatibility while aligning new docs/tests
  with the redesigned intent model.
- Old persisted follow-up retry commands containing the current global flag
  still parse during the compatibility window.
- Docs, manpage, CLI help, tests, and integration manifests no longer disagree
  about the preferred apply path.
- Refusal tests still prove that unsafe commands fail before mutation when
  intent is missing.
