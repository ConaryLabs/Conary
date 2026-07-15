---
last_updated: 2026-07-15
revision: 2
summary: Approved design for a durable development roadmap, neutral planning lifecycle, Superpowers retirement, and an external-tester-first delivery sequence
---

# Codebase Development Roadmap and Neutral Planning Migration Design

**Status:** Approved; Workstream 0 implementation plan ready for execution
**Date:** 2026-07-15
**First product milestone:** First external tester loop

## Goal

Create one durable view of where Conary works, where it is limited, where it
is unfinished, and what should happen next. The result must be useful both as
the immediate execution sequence and as a longer-term development umbrella.

The first enabling workstream retires the repository's Superpowers-specific
planning system. Active design and implementation planning remain part of the
repository, but become tool-neutral, proportional to the work, and temporary.
Git history replaces maintained planning archives.

The first product milestone remains external validation: ten people outside
the existing project circle complete the bounded preview loop, or the project
records a deliberate pivot based on a systemic blocker discovered while
trying.

## Why This Is Needed

Conary has substantial implementation and internal proof, but its current
planning surface makes the state harder to see than it should be:

- completed plans, reviews, audits, and historical status reports remain
  mixed with genuinely active work;
- roadmap detail is split across `ROADMAP.md`, active and archived planning
  documents, operational evidence, and branch-local progress state;
- subsystem maturity and workstream execution status are often conflated;
- Superpowers process instructions are embedded in plans, scripts, CI, and
  assistant routing even though that process is no longer desired;
- important current work exists on a long-running isolated branch, so a
  documentation cleanup could accidentally discard its resume state or allow
  its later integration to resurrect retired planning material;
- internal proof has outpaced external feedback. The external tester tracker
  was still at zero completions during the 2026-07-15 review.

The answer is not another archive or a renamed copy of the existing process.
It is a smaller active planning surface, a long-lived roadmap that records
decisions and state, and canonical product documentation that owns enduring
truth.

## Dated Baseline

This baseline records the 2026-07-15 audit. It is evidence for the initial
roadmap, not a claim that the facts remain current indefinitely.

### Repository and Proof State

- `main` was at `ca1039dc`, one local documentation commit ahead of
  `origin/main` at `ce6841ec`.
- The clean `goal/scriptlet-public-authority` worktree was at `e6eeb4da`, 78
  commits ahead of `origin/main`. Its implementation and independent review
  were recorded as complete, but generation file-capability shipping proof
  still depended on a passing TGE05 Group O QEMU run or an explicit maintainer
  risk-acceptance decision.
- `cargo run -p conary-test -- list` parsed 28 suites and 333 declared tests;
  it did not execute those 333 tests.
- The audit recorded passing focused checks from `main` at `ca1039dc`:
  `cargo test -p conary --test cli_daily_ux` (11 tests),
  `cargo test -p conary --test packaging_m3c` (7),
  `cargo test -p remi publication` (13), and
  `cargo test -p conaryd daemon::routes` (54). These focused checks did not
  constitute a whole-workspace or full QEMU readiness run.
- The release cargo-audit gate failed on `RUSTSEC-2026-0204` through
  `crossbeam-epoch 0.9.18`. Dependency resolution for a dry-run update to
  0.9.20 succeeded; the audit was not rerun against a modified lockfile.
- Release `v0.10.1` had published multi-distro artifacts and one documented
  `0.10.0` to `0.10.1` installed-binary update on 2026-07-07 in the
  [release artifact matrix](../operations/release-artifact-matrix.md), but
  current `main` and the isolated authority branch had moved materially beyond
  that release.
- The external tester tracker remained at 0/10, with no recorded launch venue
  or date.

### Subsystem Maturity

The roadmap must keep product maturity separate from whether a particular
workstream is queued, active, or blocked.

| Subsystem | Maturity on the audited state | Current limitation or next proof |
| --- | --- | --- |
| CLI and core package operations | Solid | The scope is the limited preview; external use and fresh combined proof matter more than more surface area. |
| Adoption, unadoption, and native handoff | Solid | Proof covers the current three-distro scope and is dated; revalidate on the integrated release candidate. |
| Database, CAS, native parsing, and resolution | Solid | Some advanced repository-policy abstractions and integration edges remain incomplete. |
| Packaging, static repositories, trust, and self-update | Solid | The scope is preview use; refresh release evidence and keep supply-chain caveats explicit. |
| CCS conversion and scriptlet authority | Limited | The isolated branch is materially stronger than the integrated tree; integrate it and resolve the QEMU shipping-proof gap. |
| Generation build and export | Limited | Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | Limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | Limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | Solid | The scope is an operator-run limited preview; distribution, cold-start readiness, and a wider stranger-operated path remain limited. |
| conaryd package and query service | Limited | System routes, authorization policy, restart semantics, dry-run behavior, deployment, and PolicyKit remain incomplete. |
| Federation | Experimental | Coordinator and fetch paths are not wired into serving; TLS identity documentation and enforcement do not yet agree. |
| Advanced derivation, lock, and reproducibility flows | Unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | Unfinished | The prepared tester loop has not been launched or completed. |

The initial umbrella should link to canonical evidence rather than duplicating
implementation detail from this table. As work changes, it updates the live
truth and proof date while retaining this design as the decision record only
for as long as implementation remains active.

## Documentation Architecture

The repository adopts the following canonical split:

| Location | Purpose | Lifecycle |
| --- | --- | --- |
| `ROADMAP.md` | Concise public direction, current milestone, and link to the detailed roadmap | Long-lived and intentionally short |
| `docs/roadmaps/development-roadmap.md` | Detailed development umbrella, maturity state, ordered workstreams, gates, and longer horizons | Long-lived source of roadmap truth |
| `docs/designs/` | Decision records for active work that needs durable design | Deleted after implementation closeout and canonicalization |
| `docs/plans/` | Executable implementation plans for active multi-step work | Deleted after implementation closeout and canonicalization |
| `docs/specs/` | Stable public or persisted contracts such as file formats and protocols | Long-lived and versioned as needed |
| `docs/ARCHITECTURE.md`, `docs/modules/`, `docs/operations/`, and guides | Enduring architecture, subsystem, operator, and user truth | Long-lived canonical documentation |

There is no replacement archive tree. Completed designs, plans, reviews,
progress logs, and dated audits are removed from the current branch once their
enduring truth has moved to the appropriate canonical documentation. Git
history remains the historical record. The default Workstream 0 scope is
`docs/superpowers/` and its plan/spec/review archives plus
`docs/llms/archive/`, which contains retired assistant-process history. Any
additional tracked documentation archive requires an explicit disposition in
the reviewed migration inventory. Domain-owned compatibility fixtures or
package data merely named `archive` are outside this policy unless their owner
separately decides otherwise.

Old material is retrieved with normal Git history tools or the GitHub commit
and file history. Current documentation must not link to a deleted historical
path as if it were live supporting evidence.

This deliberately changes the current `AGENTS.md` rule that sends finished
designs and reviews into archive subdirectories. Workstream 0 replaces that
rule with the lifecycle above.

### Closeout Rule

An active design or plan can be deleted only after all of the following are
true:

1. implementation has reached its stated acceptance condition;
2. enduring behavior and architecture are represented in canonical docs;
3. the detailed roadmap reflects the resulting maturity and next gate;
4. release or change history is updated when the user-visible product changed;
5. verification evidence is recorded in the durable place that owns it; and
6. no active branch, worktree, or handoff depends on the working document for
   resume state.

The deletion is part of closeout, not a later cleanup promise.

## Roadmap Model

`docs/roadmaps/development-roadmap.md` is the single owner of detailed status.
Active designs explain decisions and active plans explain execution, but they
do not maintain competing workstream status summaries. The detailed roadmap
is structured around decisions and delivery state rather than chronology. It
contains:

1. purpose and scope;
2. the current codebase maturity baseline;
3. product and safety principles;
4. the current milestone and its falsifiable exit condition;
5. ordered enabling and product workstreams;
6. longer-term horizons;
7. explicit deferrals and non-goals; and
8. maintenance and closeout rules.

`ROADMAP.md` becomes a compact public index: product direction, the current
milestone and exit condition, the few preview caveats a prospective user must
see, a link to the detailed roadmap, and stable contributing/non-goal links.
It does not retain completed queues, dated proof narratives, or task-level
status.

Each workstream uses the same compact record:

- **Outcome:** the user- or operator-visible result;
- **Current truth:** what is implemented and what is not;
- **Execution status:** queued, active, blocked, complete, or deferred;
- **Dependencies:** earlier work or external decisions;
- **Next gate:** the next falsifiable decision or proof point;
- **Proof:** focused tests, release evidence, integration runs, or external
  outcomes, with a date or freshness marker;
- **Limitations:** risks and missing cases that must remain visible; and
- **Non-goals:** attractive adjacent work intentionally outside the slice.

### Separate Status Dimensions

Subsystem maturity uses these labels:

- **solid:** reliable within a stated supported scope;
- **limited:** useful implementation with material scope or operational gaps;
- **unfinished:** important paths are missing or deliberately return an
  incomplete result; and
- **experimental:** implementation or scaffolding exists, but the runtime,
  trust, or support contract is not ready to rely on.

Workstream execution uses different labels:

- **queued:** accepted work waiting on its dependency or turn;
- **active:** work is being implemented or externally exercised now;
- **blocked:** progress requires a named decision, environment, or external
  state change;
- **complete:** the acceptance condition and closeout rule are satisfied; and
- **deferred:** intentionally not being scheduled in the current horizon.

Evidence freshness is a third, independent fact. A subsystem can be solid
with stale proof, or an active workstream can target an unfinished subsystem.
The roadmap must not collapse those facts into a single percentage or color.

## Delivery Sequence

The initial order is deliberate. Enabling work and release readiness precede
the first product milestone, but do not displace it.

### Workstream 0: Neutral Planning Migration

**Outcome:** Active repository planning is concise, tool-neutral, and located
under the new documentation architecture. Historical process material is
gone from the current tree without losing active resume state or canonical
product truth.

**Initial status:** queued immediately after approval of this design.

Workstream 0 is a one-time, explicitly approved exception to the current
pre-tester meta-layer budget. It is product-forced simplification: the existing
planning system obscures release and tester readiness and is itself being
removed. The exception is bounded to migration, consolidation, and deletion;
it does not authorize an open-ended replacement tooling program.

**Scope:**

- freeze creation of new content under `docs/superpowers/`;
- create `docs/roadmaps/`, `docs/designs/`, and `docs/plans/`;
- classify every Superpowers-era artifact as active, enduring truth, useful
  verification, completed history, or stale/redundant material;
- move only genuinely active designs, plans, and trackers into the neutral
  structure;
- condense architectural and operational decisions into canonical docs;
- generalize useful tests and checks, consolidate overlapping coherency or
  audit machinery, and delete process-only waves, reviews, and ledgers that
  no longer catch meaningful drift;
- update `AGENTS.md`, `docs/llms/`, ownership routing, scripts, workflows, and
  ignore rules so no Superpowers skill or path is required;
- remove tracked `docs/superpowers/` and tracked `.superpowers/` material;
- run a repository-wide stale-reference and terminology sweep; and
- create an explicit path and resume-state mapping for every active branch or
  worktree, especially `goal/scriptlet-public-authority`.

Before deleting paths used by `goal/scriptlet-public-authority`, Workstream 0
must pin the mapping to audited head `e6eeb4da` and perform a disposable
merge/replay rehearsal against the migrated tree. The rehearsal must show that
retired paths do not return, every branch-owned current document has a
destination, and the linked worktree remains clean and resumable. The
rehearsal is proof for the later integration plan, not the production merge.

The removal scope is tracked repository content and repository requirements.
It removes the path and branding, mandatory `superpowers:*` directives,
hard-coded defaults, ignored repository-local review conventions, and process
scaffolding. It does not uninstall a contributor's user-local Codex plugins,
rewrite Git history, or destroy active worktrees.

Mixed-purpose artifacts require deliberate destinations. In particular:

- the tester tracker becomes an active roadmap artifact until Milestone 1;
- tester-post copy and release-security decisions move to a current
  operations, release, or outreach location only while still actionable;
- stable format contracts remain under `docs/specs/`;
- useful audit or coherency checks move to neutral paths only if they catch a
  concrete class of current drift;
- redundant ledgers, completed audit waves, review packets, local SDD state,
  and old planning history are deleted after current truth is preserved; and
- the old tester-loop design and unchecked plan are reconciled item by item,
  not moved verbatim. The meta-layer budget, CLI tiering,
  `--help-advanced`, compatibility guidance, tracker, `v0.10.1` release, and
  fresh-VM Remi tutorial had landed by the audit; release re-pinning, Remi
  prewarming, manual launch, and completion tracking remained current work.

Every retired check family gets a short coverage disposition: redundant with
a named retained check, replaced by a named neutral check, or intentionally
removed because its invariant is process-only or obsolete. Non-Superpowers
archive paths and checks require explicit approval in the migration inventory
rather than disappearing as a side effect of a broad delete.

The currently active material includes at least the external tester loop and
the remaining scriptlet public-authority integration decision. The
kernel/initramfs/SELinux and Remi non-public-serving documents must be judged
by current implementation truth: landed behavior moves to canonical docs,
while only genuinely unimplemented follow-up remains active. Completed
checkboxes and an active-directory location are not sufficient reasons to
retain a document.

**Dependency:** this approved design.

**Next gate:** an explicit keep, move, condense, or delete inventory reviewed
against active branches before the first destructive commit.

**Acceptance:**

- no tracked `docs/superpowers/` or `.superpowers/` paths;
- no required Superpowers skill, plugin, worker instruction, approval ritual,
  or SDD progress scaffold;
- no replacement planning archive;
- all active work is discoverable from the detailed roadmap and neutral
  active directories;
- enduring decisions are present in canonical docs;
- repository routing, link, documentation-truth, and CI checks pass; and
- reconciliation guidance prevents an active branch from reintroducing
  removed planning material.

The branch requirement is satisfied only by the pinned path/state map and the
successful disposable reconciliation rehearsal described above; written
guidance alone is insufficient.

Mechanical proof includes an empty
`git ls-files 'docs/superpowers/**' 'docs/llms/archive/**'` result, no live
`docs/superpowers` or mandatory `superpowers:*` references, no ignored
replacement `docs/designs/archive/` or `docs/plans/archive/` convention, and
green neutral replacements for the repository's documentation-truth, routing,
link, and coherency checks.

**Non-goal:** product behavior changes. If migration exposes a product defect,
it becomes a later workstream item unless it is necessary to keep the
documentation migration safe.

### Workstream 1: Integrated Release-Green Baseline

**Outcome:** one integrated tree is safe to treat as the release candidate,
with the isolated authority work reconciled and current release gates green.

**Initial status:** queued behind Workstream 0. The authority implementation
is substantially complete on its branch, but integration and shipping proof
are not complete.

**Scope:**

- update `crossbeam-epoch` to a non-vulnerable version and rerun the release
  audit;
- reconcile the scriptlet public-authority branch at audited head `e6eeb4da`
  (78 commits ahead of `origin/main` on 2026-07-15) with the migrated
  documentation layout without resurrecting retired process files;
- repair the `minimal-boot-v3` path and pass TGE05, or record an explicit
  maintainer shipping-risk decision that names the unproven behavior;
- integrate the branch as one coherent authority boundary;
- resolve public truth contradictions exposed by the audit, including version
  and support claims; and
- run focused, package, workspace, documentation, and applicable interaction
  gates from the integrated head.

The Workstream 1 plan must make "release-green" mechanical. At minimum it
binds `cargo fmt --check`, workspace Clippy with warnings denied, the owning
Conary/core/Remi/conaryd tests, `conary-test` inventory parsing, release cargo
audit, release-matrix validation, the neutral documentation/coherency gates,
and the scriptlet/generation interaction gates selected by ownership routing.
It names the exact commands in effect after Workstream 0 rather than relying
on a general statement that tests passed.

**Next gate:** an integrated candidate with the RustSec blocker resolved and
the generation file-capability shipping decision recorded.

If TGE05 remains blocked and risk is accepted, the roadmap and release
evidence must continue to name the missing real-VM proof. Focused unit and
integration coverage must not be presented as equivalent to that QEMU gate.

**Acceptance:**

- one exact integrated commit is named and the worktree is clean;
- the authority branch has a recorded integrated, superseded, or retained
  disposition;
- the release audit no longer reports the unwaived crossbeam advisory;
- every command named by the Workstream 1 plan passes from that integrated
  commit; and
- TGE05 passes, or the maintainer records the precise missing proof, affected
  behavior, rationale, and expiration condition for accepting the risk.

**Non-goals:** expanding the scriptlet public allowlist, enabling federation,
or adding unrelated package-manager features.

### Workstream 2: Preview Release and Remi Readiness

**Outcome:** external testers have one current, pinned release and a prepared
service path that match the integrated code and documentation.

**Initial status:** queued behind Workstream 1.

**Scope:**

- identify the actual preceding preview when this workstream starts, then cut
  and publish a new post-Workstream-1 Conary preview tag from the integrated
  tree rather than relabeling `v0.10.1`, which was current at the audit;
- prove that the release tag, source commit, published metadata, and built
  artifacts identify the same tree;
- verify distro artifacts, checksums, signatures, release metadata, and an
  actual installed-binary self-update from the preceding preview;
- publish SBOM and provenance evidence where available, or retain a precise
  limited-preview caveat rather than implying it exists;
- update the release artifact matrix and canonical installation guidance;
- deploy or verify the public Remi service from an explicitly compatible
  commit, then record health plus representative conversion and install smoke;
- handle rows stale under the integrated `CONVERSION_VERSION` (version 6 at
  audited branch head `e6eeb4da`) before prewarming publication-policy
  packages;
- prewarm the public Remi package set used by the tester loop;
- re-pin compatibility, tester, feedback, and launch material to the exact
  release; and
- verify the stranger-facing path from a clean supported system.

The release record includes the rollback procedure and where its evidence
lives. It also makes an explicit operator-distribution decision for Remi: ship
an operator artifact with proof, or keep Remi honestly labeled as a
service-operator/source-build preview for this milestone.

**Next gate:** a pinned release and prewarmed public path that a maintainer can
hand to a tester without referencing an unmerged commit or private setup.

**Acceptance:**

- the exact source commit, release tag, supported-distro artifact set,
  checksums, signature, and SBOM/provenance decision are recorded;
- an installed binary from the actual preceding preview updates to the new
  release successfully;
- the exact compatible Remi commit and prewarmed package set are recorded;
- a clean supported host completes representative conversion, install, and
  preview-loop smoke without private setup;
- rollback is exercised or otherwise proved by the release runbook; and
- tester-facing documentation names only the new release and current service
  path.

**Non-goals:** distributing every workspace binary or claiming broad
production support.

### Workstream 3: First External Tester Loop

**Outcome:** Conary has external evidence about whether its adoption-led
preview works for strangers on supported systems.

**Initial status:** queued behind Workstream 2. Preparation exists, but the
loop has not launched.

**Scope:**

- record the manual launch venue and date;
- stop at the maintainer posting gate: automation prepares and verifies the
  material, while a maintainer chooses when and where to publish it;
- ask testers to complete the bounded install, adopt, list/search,
  `update --dry-run`, and unadopt flow;
- use privacy-safe feedback intake that does not request secrets or broad host
  data by default;
- track completion separately from general interest or partial attempts;
- triage feedback into fix-now, next-slice, or declined-with-reason;
- prioritize systemic tester blockers over speculative roadmap work while the
  loop runs; and
- keep negative results as useful evidence rather than silently redefining
  success.

**Milestone 1 exit condition:** ten people outside the existing project circle
complete the full loop and report friction, or the roadmap records a
maintainer-approved pivot that names the systemic blocker, the evidence, and
what changes next.

A blocker-based pivot requires a reproducible failure within the stated
supported scope, the number and shape of affected attempts, and a chosen
remediation or explicit support-scope change. Ordinary outreach difficulty or
one unexplained partial attempt is not enough.

Launching the post, completing Workstream 2, or receiving one partial report
does not achieve Milestone 1. The Workstream 3 plan assigns triage ownership
and a response target. If the loop records no qualifying completion for three
weeks after launch, the maintainer reviews venue reach, onboarding friction,
and observed failures, then records whether to revise outreach, repair a
blocker, or invoke the pivot exit.

The tracker is an active roadmap artifact, not a permanent status log. At
milestone closeout, its durable findings move into the development roadmap,
release history, or relevant canonical product docs and the tracker is
deleted.

**Acceptance:**

- the launch venues and timestamp, pinned release, supported-host scope, and
  privacy-safe feedback path are recorded;
- each qualifying completion belongs to a unique external person and includes
  the full install, adopt, list/search, `update --dry-run`, and unadopt flow;
- ten qualifying completions and their friction records exist, or the pivot
  evidence meets the reproducible-blocker threshold above; and
- triage decisions and durable findings are reflected in the detailed roadmap
  before the temporary tracker closes.

## Post-Milestone Horizons

After Milestone 1, tester evidence determines the detailed order. The roadmap
still carries the following longer-term outcomes so they are not lost:

### Feedback-Driven Compatibility and Authority

- repair the highest-impact adoption, resolution, scriptlet, or service
  friction found by real testers;
- expand scriptlet public authority only through positive target policy and
  end-to-end preservation proof;
- preserve fail-closed public serving and explicit native-package-manager
  authority; and
- refresh supported distro proof before adding breadth.

### Native Packaging and System-Building Completeness

- close explicit CCS v2 dependency-authoring and lock/reproducibility gaps;
- connect model, source-selection, builder, and derivation inputs end to end;
- strengthen bootstrap and self-host repeatability; and
- mature recipes, groups, and composition based on demonstrated users rather
  than adding surface speculatively.

### Service and Operator Maturity

- resolve conaryd's root-only versus read-only authorization contract;
- implement or remove unreachable PolicyKit behavior;
- replace echo-style dry runs with real resolver plans;
- prove crash/restart safety before adding more destructive routes;
- decide distribution and deployment support for Remi, conaryd, and the test
  harness; and
- improve operator observability, migration, and recovery guidance.

### System Artifacts and Platform Breadth

- make x86_64 generation boot proof repeatable on maintained fixtures;
- add signed boot-artifact authority and portable generation-bundle trust;
- add aarch64 or other architectures only with owned boot assets and proof;
- add distros only after the existing three-distro path is boringly reliable;
  and
- address persistent scriptlet-effect rollback limitations explicitly.

### Federation Runtime and Trust Closure

Federation remains experimental until its code and public contract agree.
Before enabling it, choose and document one peer identity model, enforce that
model in the TLS client, wire the coordinator and federated fetcher into Remi
startup and serving, connect admin changes to runtime state, define public
chunk reachability for federated content, and prove a two-node failure matrix.

Federation tuning, larger peer topologies, and alternative transports are
later concerns. They do not precede basic runtime and trust closure.

## Proportional Planning Contract

The new structure keeps durable design and implementation planning without
requiring uniform ceremony for every change.

Each design states whether it is proposed or approved. Approval and
independent review follow the risk rubric below; merely using the directory
does not impose a universal approval workflow.

### When a Tracked Design Is Expected

A design is expected when work changes one or more of:

- architecture or subsystem ownership;
- persisted or publicly exchanged formats;
- security, authority, or trust boundaries;
- public CLI, API, or compatibility contracts;
- behavior spanning multiple subsystems; or
- the roadmap or product contract itself.

### When a Tracked Plan Is Expected

An implementation plan is expected when work:

- has dependent tasks whose order matters;
- crosses packages, services, or ownership boundaries;
- needs migration, compatibility, rollback, or failure handling;
- depends on expensive or environment-specific proof; or
- is expected to span sessions, agents, branches, or worktrees.

Small, localized, reversible fixes can be fully described by an issue, commit,
or pull request when the repository contract and focused test make the intent
clear.

The practical rubric is:

- trivial, local, and reversible: no tracked design or plan;
- bounded multi-file work with known behavior: a short checklist or plan when
  sequencing helps;
- architecture, persisted state, security, public contract, or
  multi-workstream work: design first, then an implementation plan;
- release or operator execution: use a current runbook or checklist rather
  than a speculative product implementation plan; and
- urgent security repair: an abbreviated decision record is acceptable before
  the fix, but regression proof and canonical retrospective documentation are
  required at closeout.

Independent plan review is expected for security or trust boundaries,
persisted-state migrations, irreversible operator actions, release gates, and
large cross-subsystem changes. It is optional for ordinary bounded work when
the owner, proof, and rollback are straightforward. Review expectations are a
repository risk decision, not an agent-tool ritual.

### Minimum Design Contents

- goal and user-visible outcome;
- relevant repository facts;
- decisions and rejected alternatives;
- security, compatibility, and failure implications;
- scope and explicit non-goals; and
- falsifiable acceptance conditions.

### Minimum Plan Contents

- ordered tasks and dependencies;
- files and ownership boundaries;
- migration and failure handling;
- focused and interaction proof; and
- documentation and temporary-artifact cleanup.

Plans remain tool-neutral. They do not mandate a particular agent skill,
worker topology, approval ritual, task-report scaffold, or local progress-file
format.

### Lifecycle

The normal lifecycle is:

1. a roadmap workstream becomes active;
2. an active design is created if the decision warrants one;
3. an active implementation plan is created if execution warrants one;
4. implementation and verification occur;
5. canonical docs, release history, and the umbrella are updated;
6. the design and plan are deleted; and
7. Git history remains available when historical detail is needed.

Abandoned or superseded work follows the same rule: record any still-relevant
decision, limitation, or future outcome in the detailed roadmap or canonical
docs, then delete the inactive design or plan. Its abandoned state is not a
reason to leave it in the active tree.

`ROADMAP.md` changes only when public direction or the current milestone
changes. Routine execution status belongs in the detailed roadmap.

## Migration and Failure Handling

Workstream 0 uses an explicit inventory before deletion. Each artifact gets
one disposition:

- **move active:** still needed to decide or execute current work;
- **move to roadmap:** long-lived workstream or milestone state;
- **canonicalize:** enduring behavior, architecture, operations, or contract
  truth moves to its owning document;
- **generalize:** a useful verification check is retained without
  Superpowers-specific naming or process;
- **condense then delete:** a historical audit contains a small amount of
  current truth that must first be incorporated elsewhere; or
- **delete:** completed, stale, redundant, process-only, or review-history
  material already recoverable from Git.

Ambiguity defaults temporarily to active retention, with the unresolved owner
or decision named. It does not justify a permanent archive.

The migration is split into bounded commits:

1. establish the neutral directories and detailed umbrella;
2. migrate active and canonical material;
3. generalize or remove scripts and CI checks;
4. delete historical and process-only trees;
5. update assistant and contributor guidance;
6. add active-branch reconciliation mappings; and
7. run final stale-reference, routing, truth, link, and CI sweeps.

Reference rewrites and active-state moves precede destructive deletion. Each
commit remains independently reviewable and revertible. If a check reveals
orphaned current truth, the destructive phase stops and the item returns to
the inventory; the response is not to preserve the entire old hierarchy.

## Roadmap Maintenance

The detailed roadmap changes when material implementation truth changes, a
workstream changes state, proof is refreshed or becomes stale, a blocker is
identified, or a milestone transitions. It is not a daily activity log.

- An active item names its next gate.
- A blocked item names the blocker and the decision or state change that would
  unblock it.
- A proof claim includes enough source and date context to judge freshness.
- A completed item leaves active planning only after the closeout rule is met.
- GitHub issues and pull requests can be linked as evidence, but they do not
  replace repository roadmap truth.
- New work enters the ordered sequence only when it advances the current
  milestone, resolves a safety or release blocker, or is explicitly accepted
  as a longer-horizon change.

## Initial Implementation Deliverables

The first implementation plan produces, in order:

1. `docs/roadmaps/development-roadmap.md` with the dated baseline, current
   milestone, ordered workstreams, longer horizons, and maintenance rules;
2. a condensed `ROADMAP.md` that links to the detailed umbrella;
3. `docs/plans/` and the bounded active Workstream 0 implementation plan;
4. neutral destinations for every genuinely active design, plan, tracker, and
   branch-resume fact;
5. canonical updates that preserve enduring architecture and operations
   truth; and
6. tool-neutral assistant routing, contributor guidance, scripts, and CI
   checks, followed by deletion of the retired process tree.

The migration inventory may live in the Workstream 0 plan while the work is
active, but it is deleted at closeout rather than becoming a new permanent
ledger.

## Alternatives Considered

### Keep the Existing Superpowers Tree and Add an Umbrella

Rejected. It would improve discovery while preserving the duplication,
process coupling, active-versus-historical ambiguity, and maintenance burden
that prompted this work.

### Rename the Existing Tree and Preserve Its Archives

Rejected. A mechanical rename would carry the old lifecycle and ceremony into
new paths. It would remove a brand name without changing how planning works.

### Keep Only `ROADMAP.md`

Rejected. A single concise public file cannot carry honest subsystem maturity,
workstream dependencies, proof freshness, blockers, and long-term horizons
without becoming another unmaintainable status report.

### Canonical Split With Git as the Archive

Chosen. It keeps public direction concise, preserves a detailed current
umbrella, retains stable specifications and canonical product truth, and
limits active planning documents to work that is actually active.

## Non-Goals

This design does not:

- implement the roadmap or change product behavior;
- integrate the scriptlet authority branch;
- decide risk acceptance for the outstanding QEMU proof;
- cut or publish a release;
- launch the tester outreach;
- enable federation or complete conaryd;
- require GitHub issues to mirror every roadmap item;
- assign fixed dates to uncertain long-term work; or
- preserve process history in a new archive.

## Design Acceptance

The design is complete when it records:

- the dated codebase maturity baseline;
- the neutral documentation architecture and deletion-based lifecycle;
- the separate maturity, execution-status, and evidence-freshness dimensions;
- Workstream 0 as the first enabling workstream;
- the integrated release baseline and preview release as prerequisites;
- the external tester loop as the first product milestone;
- the feedback-led longer-term horizons;
- proportional design and planning expectations;
- migration safety for active branches and ambiguous artifacts; and
- objective acceptance conditions for removing Superpowers from the repo.

Implementation begins only after this committed design is reviewed. The first
implementation plan covers the detailed umbrella plus Workstream 0; later
product workstreams receive their own plans when they become active.
