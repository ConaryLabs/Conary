---
last_updated: 2026-07-15
status: ready
workstream: 0
summary: Implementation plan for the durable development roadmap and the one-time migration away from the repository's former planning system
---

# Development Roadmap and Neutral Planning Migration Implementation Plan

**Status:** Ready for execution
**Design:**
[`docs/designs/2026-07-15-codebase-development-roadmap-design.md`](../designs/2026-07-15-codebase-development-roadmap-design.md)
**First product milestone:** Ten qualifying external tester completions, or an
evidence-backed maintainer pivot

## Goal

Create the repository's durable codebase-development umbrella, make
Workstream 0 the first enabling workstream, preserve only active or canonical
truth in the current tree, and remove the former Superpowers-specific
planning/history system without losing release policy, tester state, or the
resume state of `goal/scriptlet-public-authority`.

This plan implements documentation and repository-process changes only. It
does not change Rust product behavior, merge the authority branch, repair the
RustSec blocker, cut a release, or launch tester outreach. Those are
Workstreams 1 through 3.

This file is itself temporary. Delete it together with the active design when
Workstream 0 meets its closeout gate; Git history remains the record.

## Architecture

The finished repository has one compact public index and one detailed source
of roadmap truth:

```text
ROADMAP.md
    -> docs/roadmaps/development-roadmap.md
       -> active milestone tracker
       -> canonical architecture, module, operations, testing, and release docs

docs/designs/   active decision records only
docs/plans/     active multi-step implementation plans only
docs/specs/     stable public or persisted contracts
Git history     completed, superseded, or abandoned planning history
```

`scripts/check-doc-truth.sh` remains the product-documentation invariant
checker and gains the small neutral-layout invariant at closeout. Feature
ownership cards plus `scripts/agent-context.sh` remain the routing and focused
proof layer. The per-file documentation inventory, documentation audit
ledger, feature-coherency ledger, completed wave registry, and their
structural validators are retired rather than renamed.

## Technologies and Ownership Boundaries

- Markdown and TSV documentation under `docs/` and repository-root guidance.
- Bash validation and routing tools under `scripts/`.
- GitHub Actions YAML under `.github/workflows/`.
- Git for reversible moves, bounded commits, history lookup, and the
  disposable reconciliation rehearsal.
- No Rust source, database schema, persisted product state, CLI surface, or
  service API changes.

## Non-Negotiable Constraints

1. Keep `main` and the linked authority worktree clean at every task boundary.
2. Pin authority work to
   `e6eeb4da9c560c34317b57b2a422717d4d556b37`; stop if the linked worktree or
   its head has changed unexpectedly.
3. Do not merge, rebase, reset, or edit tracked files in
   `.worktrees/scriptlet-public-authority` during Workstream 0.
4. Do not delete the old tracked tree until the replacement roadmap and
   routing exist and the disposable merge rehearsal has passed.
5. Do not move completed history into a newly named archive.
   `docs/designs/archive/` and `docs/plans/archive/` must not be created or
   ignored.
6. Do not mechanically rename old ledgers or approval rituals. Retain only a
   check that protects a current product, routing, release, privacy, or
   documentation invariant.
7. Preserve the active RSA waiver and distinguish it from the unwaived
   `crossbeam-epoch` advisory. Workstream 0 is not release-green.
8. Preserve branch-owned product code and test content exactly in the
   rehearsal. Resolve canonical documentation overlaps deliberately; never
   use a blanket `--ours` or `--theirs` resolution.
9. Local ignored SDD/review scratch may be removed only after its unique
   resume facts are written into the detailed roadmap and the rehearsal is
   recorded.
10. Every destructive step has an immediately preceding stop condition and a
    following stale-reference check.
11. Do not uninstall or modify a contributor's user-local Codex plugins or
    home configuration. Repository-local tracked and ignored process state is
    in scope; user-global tooling is not.

## Known Starting State

Record these facts in the first execution log and recheck them rather than
assuming they remain true:

- `main` was at `ac0ca355` on 2026-07-15, two commits ahead of
  `origin/main` at `ce6841ec`.
- The linked authority worktree was tracked-clean at
  `e6eeb4da9c560c34317b57b2a422717d4d556b37`, 78 commits ahead of its merge
  base `ce6841ec`.
- The old documentation tree contained 176 tracked files: 12 top-level
  artifacts, 3 non-archived plans, 4 non-archived specs, and 157 archived
  plan/spec/history files.
- `docs/llms/archive/` contained one tracked Claude-era note.
- The external tester tracker was at 0/10 with no recorded venue or launch
  date.
- `bash scripts/check-coherency-wave-scopes.sh ...` failed because the
  completed-wave registry omitted `remi-scriptlet-evidence-queue`. Treat this
  as obsolete process drift, not a product regression.
- `bash scripts/test-maintainability-drift-report.sh` could falsely fail its
  federation assertion because `grep -A1 ... | grep -q ...` produces status
  141 under `set -o pipefail`. Fix this retained test in Task 2.
- `bash scripts/release-cargo-audit.sh` failed on unwaived
  `RUSTSEC-2026-0204` through `crossbeam-epoch 0.9.18`; a dry-run resolution to
  0.9.20 succeeded, but no lockfile remediation or green audit was completed.

## Final File and Check Disposition

### Durable and Active Destinations

| Destination | Contents at Workstream 0 closeout | Lifecycle |
| --- | --- | --- |
| `ROADMAP.md` | Public direction, current milestone, preview caveats, detailed-roadmap link, stable non-goals and contribution links | Long-lived, concise |
| `docs/roadmaps/development-roadmap.md` | Maturity baseline, Workstreams 0-3, authority-branch handoff, longer horizons, maintenance rules | Long-lived source of detailed status |
| `docs/roadmaps/external-tester-milestone.md` | 0/10 outcome tracker, qualifying-flow definition, launch dependency, reports, stall/pivot rules | Active until Milestone 1 closes |
| `docs/operations/external-tester-outreach.md` | Current but explicitly not-yet-publishable launch copy; repinned by Workstream 2 | Active through launch; delete after W3 records durable venue, date, release, and findings |
| `docs/operations/release-security-waivers.md` | Current release audit waiver policy and current unwaived blocker | Long-lived operational policy |
| `docs/designs/` | No tracked file after this design closes unless another design is genuinely active | Active-only |
| `docs/plans/` | No tracked file after this plan closes unless another plan is genuinely active | Active-only |

### Old Top-Level Artifacts

| Current path | Disposition and truth to preserve first |
| --- | --- |
| `docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md` | Condense still-current limitations and dated proof into the detailed roadmap; canonical behavior remains in integration and module docs; delete. |
| `docs/superpowers/distro-adoption-gap-analysis-2026-06-10.md` | Carry only current distro breadth, migration, and adoption horizons into the detailed roadmap; delete. |
| `docs/superpowers/documentation-accuracy-audit-inventory.tsv` | Delete generated historical snapshot; do not replace it with another committed inventory. |
| `docs/superpowers/documentation-accuracy-audit-ledger.tsv` | Delete completed per-file bookkeeping; current invariants remain in doc truth, feature cards, tests, and canonical docs. |
| `docs/superpowers/documentation-accuracy-audit-summary.md` | Delete dated audit narrative after the umbrella baseline exists. |
| `docs/superpowers/feature-coherency-ledger.tsv` | Delete closed structural ledger. Preserve `CLI-ADOPT-004` in its existing follow-up document and transfer two authority-branch-only claims as described below. |
| `docs/superpowers/feature-coherency-wave-scopes.tsv` | Delete completed and already-drifting wave registry. |
| `docs/superpowers/first-external-tester-loop-tracker.md` | Move and condense to `docs/roadmaps/external-tester-milestone.md`; retain 0/10 and the exact qualifying flow. |
| `docs/superpowers/limited-preview-release-checkpoint-2026-05-16.md` | Condense current release caveats into the umbrella and release artifact matrix; delete dated checkpoint. |
| `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md` | Move actionable copy to `docs/operations/external-tester-outreach.md`; mark it blocked from publication until Workstream 2 repins it. |
| `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md` | Apply the six-row disposition in Task 1: combine three Remi target-ID/CLI questions under W2 compatibility proof, canonicalize Debian-family fixture ownership, drop the evidence-free dead-helper queue, and satisfy archive normalization in W0; delete the inventory. |
| `docs/superpowers/release-security-waivers-2026-05-06.md` | Move and refresh as `docs/operations/release-security-waivers.md`; never delete active release policy with planning history. |

### Old Non-Archived Plans and Specs

| Current path | Disposition and truth to preserve first |
| --- | --- |
| `docs/superpowers/plans/2026-07-01-first-external-tester-loop-plan.md` | Reconcile item by item. Tasks 1-8 and 10-11 landed as preparation; Task 7's tracker remains active at 0/10, Task 8's copy needs a fresh repin, and Task 9 is partial because `v0.10.1` evidence landed while post-integration release, prewarm, and manual launch remain. Do not move the 1,094-line plan. |
| `docs/superpowers/plans/2026-07-03-kernel-initramfs-selinux-scriptlet-handling-plan.md` | MVP behavior is canonical in scriptlet/CCS/Remi docs; carry later target-profile, boot, authority, and release proof into longer horizons; delete. |
| `docs/superpowers/plans/2026-07-08-remi-non-public-test-serving-plan.md` | Completed and canonicalized; delete. |
| `docs/superpowers/specs/2026-07-01-first-external-tester-loop-design.md` | Superseded by the current umbrella design; delete. |
| `docs/superpowers/specs/2026-07-03-kernel-initramfs-selinux-scriptlet-handling-design.md` | Canonicalize landed behavior and carry only unimplemented horizons; delete. |
| `docs/superpowers/specs/2026-07-08-remi-non-public-test-serving-design.md` | Completed and canonicalized; delete. |
| `docs/superpowers/specs/2026-07-08-scriptlet-public-authority-roadmap-design.md` | Condense then delete after Task 5 records an exhaustive A-I decision/destination map. A-D and G-H are implemented on the reviewed branch; E retains later boot/security authority outcomes, F remains intentionally blocked/non-public, and I is conditional maintainability guidance. Canonical invariants live in the eight branch docs and owning tests; W1 owns integration, TGE05/risk, and retained minors. Do not keep a second 502-line status umbrella for W1. |

### Whole-Class Deletions

- Delete all 157 tracked files under the old archive, plan archive, and spec
  archive subtrees after exceptional inbound references are repaired.
- Delete `docs/llms/archive/claude-era-notes.md`; its current conclusions are
  already owned by `AGENTS.md`, `CLAUDE.md`, and `docs/llms/README.md`.
- Delete ignored `docs/superpowers/reviews/` and the ignored `.superpowers/`
  SDD trees in the main and authority worktrees only after Task 5 records all
  unique handoff facts.
- Delete this design and this plan in Task 7.
- Leave `recipes/archive/` alone. It is package-domain data, not planning
  history.

### Check Families

| Current check/tool | Final disposition |
| --- | --- |
| `scripts/check-doc-truth.sh`, `scripts/test-doc-truth.sh` | Retain and generalize for neutral roadmap/release paths; add the small no-retired-layout invariant at closeout. |
| `docs/modules/feature-ownership.md`, `scripts/agent-context.sh`, `scripts/test-agent-context.sh` | Retain and generalize to neutral planning roots and canonical proof. |
| `scripts/maintainability-drift-report.sh`, test | Retain changed-path and line-count value; replace ledger health with doc-truth health and fix the pipefail false negative. |
| `scripts/agentic-plan-review.sh`, test | Retain as an optional reviewer convenience, not a required planning ritual; use `target/agent-reviews/` and neutral risk/proof language. |
| `scripts/release-cargo-audit.sh` | Retain; update its waiver-policy path. |
| `scripts/docs-audit-inventory.sh`, `scripts/check-doc-audit-ledger.sh` | Delete with the inventory and ledger. |
| `scripts/check-coherency-ledger.sh`, `scripts/check-coherency-wave-scopes.sh`, `scripts/test-coherency-ledger.sh` | Delete with the closed claim and wave ledgers. |
| Workflow audit/coherency steps | Delete; keep doc truth, doc-truth tests, agent-context tests/validation, and existing product/privacy gates. |

## Authority Branch Reconciliation Contract

The branch delta from merge base `ce6841ec` contains three classes.

### Discard as Process History

- modified documentation inventory;
- modified documentation audit ledger;
- modified feature-coherency ledger; and
- eight added completed plans:
  `file-capability-public-policy`, `generation-file-capability-xattrs`,
  `lsm-policy-semantics-lock`, `network-package-recursion-authority-lock`,
  `pam-authority-lock`, `publication-summary-schema-docs-truth`,
  `remi-local-only-test-serving-alignment`, and
  `sysctl-target-profile-public-policy`.

Do not recreate those plans under `docs/plans/`.

### Preserve as Canonical Product Truth

Resolve the following branch-owned docs into the later integrated result:

- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/SCRIPTLET_SECURITY.md`
- `docs/conaryopedia-v2.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/operations/post-generation-export-follow-up-roadmap.md`

All branch-owned product source and tests outside the retired process paths
must survive byte-for-byte in the rehearsal unless an overlap is explicitly
listed and reviewed. Workstream 0 has no authority to rewrite that product
behavior.

### Transfer Before Ledger Deletion

Ensure canonical docs, feature-card invariants, and existing tests still make
these two branch-only claims discoverable:

1. Network/package recursion remains non-public and blocked.
2. Stale publication summaries fail closed; public responses are sanitized
   while raw review artifacts remain private.

Also transcribe the ignored branch progress state into the umbrella:

- exact head and merge base;
- tracked-clean and independently reviewed/ready-to-integrate state;
- TGE05 Group O QEMU success or explicit shipping-risk decision as the W1
  gate;
- the two retained nonblocking review minors: add one combined
  raw-artifact-writer plus sanitized-response-route workflow regression, and
  decide whether balanced quoted environment assignments should normalize to
  `<path>` or `<env-assignment>` for clustering invariance; and
- the process/canonical/product reconciliation map above.

## Task 0: Recheck and Pin the Execution Baseline

**Files:** none

1. Confirm the main checkout is clean before editing:

   ```bash
   git status --short --branch
   git rev-parse HEAD
   git rev-parse origin/main
   git rev-list --left-right --count HEAD...origin/main
   ```

   The approved design was committed at `ac0ca355`; the committed plan will
   necessarily put execution on a later head. Require `ac0ca355` to remain an
   ancestor, record the actual clean execution head and ahead/behind count,
   and refresh every drifted path or count before continuing. Do not reset new
   work away.

2. Pin the authority worktree:

   ```bash
   goal_wt="$(git rev-parse --show-toplevel)/.worktrees/scriptlet-public-authority"
   goal_head=e6eeb4da9c560c34317b57b2a422717d4d556b37
   test -z "$(git -C "$goal_wt" status --porcelain)"
   test "$(git -C "$goal_wt" rev-parse HEAD)" = "$goal_head"
   git merge-base HEAD "$goal_head"
   git rev-list --left-right --count HEAD..."$goal_head"
   ```

   Stop if the worktree is dirty or its head differs. Re-audit rather than
   deleting or overwriting new state.

3. Capture current old-tree counts and external references for comparison:

   ```bash
   git ls-files 'docs/superpowers/**' | wc -l
   git ls-files 'docs/llms/archive/**'
   git grep -n -I -i -E \
     'superpowers|docs/llms/archive|check-doc-audit-ledger|docs-audit-inventory|check-coherency-(ledger|wave-scopes)|test-coherency-ledger|documentation-accuracy-audit|feature-coherency|coherency-wave' \
     -- . ':!docs/superpowers/**' || true
   ```

4. Run retained baseline checks and record pass/fail honestly:

   ```bash
   bash scripts/check-doc-truth.sh
   bash scripts/test-doc-truth.sh
   bash scripts/test-agent-context.sh
   bash scripts/agent-context.sh --validate
   bash scripts/test-agentic-plan-review.sh
   bash scripts/test-maintainability-drift-report.sh
   bash scripts/release-cargo-audit.sh
   ```

   The last two may show the known pipefail false negative and crossbeam
   blocker. Do not weaken release policy or claim a green baseline.

## Task 1: Establish the Durable Roadmap and Active Milestone Artifacts

**Create:**

- `docs/roadmaps/development-roadmap.md`
- `docs/roadmaps/external-tester-milestone.md`
- `docs/operations/external-tester-outreach.md`

**Modify:**

- `ROADMAP.md`
- `docs/operations/release-artifact-matrix.md` if its current release evidence
  omits a caveat preserved by the old checkpoint
- `scripts/check-doc-truth.sh`
- `scripts/test-doc-truth.sh`
- transitional documentation inventory and ledger rows

### Step 1.1: Write the detailed roadmap

Create the neutral roadmap directory before adding its first file:

```bash
mkdir -p docs/roadmaps
```

Create `docs/roadmaps/development-roadmap.md` with this exact section order:

1. front matter with `last_updated`, `proof_baseline`, `current_milestone`, and
   `active_workstream`;
2. purpose and supported limited-preview scope;
3. Milestone 1 exit condition;
4. principles and safety boundaries;
5. subsystem maturity table using only `solid`, `limited`, `unfinished`, and
   `experimental`;
6. Workstreams 0 through 3;
7. authority-branch reconciliation map;
8. post-milestone horizons;
9. explicit deferrals and non-goals; and
10. roadmap maintenance and closeout rules.

Populate the maturity table from the approved design's dated 2026-07-15
baseline. Do not convert it into percentages or erase scope limitations.

Each workstream record must contain `Outcome`, `Current truth`, `Execution
status`, `Dependencies`, `Next gate`, `Proof`, `Limitations`, and `Non-goals`.
Initial statuses are:

- W0 Neutral Planning Migration: `active`;
- W1 Integrated Release-Green Baseline: `queued` behind W0;
- W2 Preview Release and Remi Readiness: `queued` behind W1; and
- W3 First External Tester Loop: `queued` behind W2, with tracker 0/10.

The W1 record must name the unwaived crossbeam advisory, authority head,
TGE05/risk gate, eight branch canonical docs, discarded process files, and
retained minor review notes. W2 must require a fresh post-integration release;
identity equality among source commit, tag, published metadata, and artifacts;
checksums, signatures, and an explicit SBOM/provenance decision; self-update
proof from the actual preceding preview; canonical installation guidance;
the exact Remi commit, conversion-version handling, prewarming, rollback, and
clean-host smoke; exact repinning of compatibility, tester, feedback, and
launch material; and an explicit Remi operator-artifact versus
service-operator/source-build-preview decision. W3 must retain the exact
external flow and the three-week no-completion review rule.

Map the six deferred candidates from the old dead-surface inventory exactly:

1. Clap-time Remi target rejection, runtime target-ID normalization, and
   `repo add --remi-distro` validation become one W2 compatibility/proof
   question; do not change behavior in W0.
2. Debian identifiers in conversion tests remain internal package-family
   fixture coverage under `docs/modules/test-fixtures.md`; they are not public
   distro claims.
3. The generic “prune dead helper APIs” row is deleted without a roadmap item
   because it names no proven-dead surface; future product slices own concrete
   removals with focused tests.
4. Old assistant/archive references are resolved by this W0 migration.

For the kernel/boot/security follow-up, preserve all five later outcomes:
fail-closed target-profile facts, proof-backed native adapters, explicit CCS
v2 authority, route-derived release validation, and promotion only for rows
backed by the proof corpus.

### Step 1.2: Move and condense the active tester material

Use history-preserving moves as the starting point:

```bash
git mv docs/superpowers/first-external-tester-loop-tracker.md \
  docs/roadmaps/external-tester-milestone.md
git mv docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md \
  docs/operations/external-tester-outreach.md
```

Rewrite the tracker around the outcome, not the old implementation-plan task
list. It must record:

- current result `0/10`;
- a unique outsider as the unit of completion;
- exact sequence `install -> adopt -> list/search -> update --dry-run ->
  unadopt`;
- supported-host and pinned-release fields;
- venue and launch timestamp fields;
- privacy-safe report references, not secrets or broad machine dumps;
- triage status and owner for each qualifying or failed attempt;
- the three-week stall review; and
- the reproducible systemic-blocker threshold for a maintainer pivot.

Rewrite the outreach document as actionable draft copy. Keep `v0.10.1` only as
the current release truth while W2 is queued, and put an unmistakable gate at
the top: **do not publish until W2 records and repins the post-integration
release, compatible Remi commit, prewarmed package set, and clean-host smoke**.
Retain the manual maintainer posting decision.

The W3 closeout rule must delete this outreach draft after durable launch
venue/date, pinned release, and tester findings have moved to the milestone
tracker, release history, and detailed roadmap. Published copy remains on its
venue; the repository does not keep a permanent post archive.

### Step 1.3: Condense the public roadmap

Replace the long completed queues in `ROADMAP.md` with:

- `Direction`: adoption-led limited preview;
- `Current milestone`: ten qualifying external completions or an
  evidence-backed pivot;
- `Enabling sequence`: W0 -> W1 -> W2 -> W3;
- `Current caveats`: supported three-distro preview, x86_64 generation proof,
  and conaryd/federation outside the reliable core path;
- link to `docs/roadmaps/development-roadmap.md`;
- stable links to architecture, changelog, release matrix, and contribution
  guidance; and
- concise `Not planned` boundaries.

Remove dated Group O/P proof, archived plan links, and completed task detail
from the public index.

Compare the old limited-preview release checkpoint against
`docs/operations/release-artifact-matrix.md`. Add any still-current artifact,
signature, checksum, self-update, or caveat evidence missing from the matrix;
if nothing is missing, record `verified-no-change` in the Task 1 execution
note. This explicit comparison is the deletion gate for the checkpoint.

### Step 1.4: Change doc-truth tests first, then the checker

Update the fixture in `scripts/test-doc-truth.sh` to create
`docs/roadmaps/development-roadmap.md`,
`docs/roadmaps/external-tester-milestone.md`, and
`docs/operations/external-tester-outreach.md`. Add negative cases proving the
checker rejects:

- a root roadmap without the detailed-roadmap link;
- a detailed roadmap without first-external-tester milestone wording;
- stale release tags in the tracker or outreach draft; and
- missing remote-Forge and dated Group O/P evidence in the detailed roadmap
  while that evidence remains part of current preview truth.

Run the updated fixture before changing the checker and confirm the intended
new case fails for the old implementation.

Then update `scripts/check-doc-truth.sh`:

- add `docs/roadmaps` to `PRODUCT_DOC_PATHS`;
- require `ROADMAP.md` to contain adoption-led wording, the first external
  tester milestone, and the detailed-roadmap link;
- move the remote-Forge and dated Group O/P roadmap requirements from
  `ROADMAP.md` to `docs/roadmaps/development-roadmap.md`, retaining the
  independent requirements in `docs/INTEGRATION-TESTING.md`;
- replace the two old release-document paths with the new tracker and outreach
  paths; and
- do not add the final no-retired-layout scan yet, because this active design
  and plan still describe the migration literally.

### Step 1.5: Preserve the transitional audit gate only for this commit

Stage or intent-to-add the new/moved documentation, regenerate the current
inventory mechanically, and add/update ledger rows for the roadmap, tracker,
outreach draft, design, and this plan. State in the ledger note that this is a
one-commit transition and the registry is retired after the Task 5 rehearsal,
in Task 6.

### Step 1.6: Verify and commit

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh \
  | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
git status --short
```

Expected commit:

```text
docs: establish development roadmap
```

## Task 2: Replace Live Repository Routing and Planning Conventions

**Modify:**

- `AGENTS.md`
- `CONTRIBUTING.md`
- `docs/conaryopedia-v2.md`
- `docs/llms/README.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/test-fixtures.md`
- `docs/operations/agent-mcp-adapter-decision.md`
- `docs/operations/infrastructure.md`
- `docs/operations/system-adopt-single-package-dry-run-follow-up.md`
- `docs/specs/static-repo-format-v1.md`
- `scripts/agent-context.sh`
- `scripts/test-agent-context.sh`
- `scripts/agentic-plan-review.sh`
- `scripts/test-agentic-plan-review.sh`
- `scripts/maintainability-drift-report.sh`
- `scripts/test-maintainability-drift-report.sh`
- `.github/workflows/merge-validation.yml`
- `.github/workflows/pr-gate.yml`
- `.gitignore`

### Step 2.1: Make the repository contract active-only and neutral

In `AGENTS.md`:

- replace the old tester-design link in the meta-layer budget with
  `docs/roadmaps/development-roadmap.md`;
- replace the documentation-ledger/coherency-ledger paragraph with the
  retained model: doc-truth checks for public claims, feature cards for owner
  and interaction proof, and focused tests for behavior;
- state that active designs live in `docs/designs/`, active plans in
  `docs/plans/`, and roadmap state in `docs/roadmaps/`;
- state that completed/superseded planning is deleted after canonicalization
  and recovered through Git history; and
- retain the existing proportional planning, safety, Rust, output, and
  maintainability rules.

In `docs/llms/README.md`, make the same location/lifecycle model the routing
layer. Replace the old proof floor with:

```bash
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
git diff --check
```

Keep feature-specific commands additive when a card names them. Remove the
assistant archive convention and its link.

Change `CONTRIBUTING.md` from “docs-audit health” to documentation truth and
feature-card routing. Replace its instruction to move completed plans/specs
into archive subtrees with the active-only lifecycle and Git-history rule.
Repair its broken relative MIT license link to the tracked `LICENSE-MIT` file.

### Step 2.2: Repair canonical inbound references

- In `docs/modules/feature-ownership.md`, remove archived plan/spec links.
  The CCS card keeps current CCS, test-fixture, and static-format docs. The
  packaging card keeps current static-format, recipe, Remi, and first-package
  docs. Replace “docs-audit truth gates” with doc-truth and owning-card proof.
- In `docs/modules/test-fixtures.md`, replace docs-audit instructions with
  doc-truth and diff hygiene.
- In `docs/operations/system-adopt-single-package-dry-run-follow-up.md`, state
  directly that this document owns the deferred dry-run surface; remove the
  wave-ledger dependency.
- In `docs/operations/agent-mcp-adapter-decision.md`, remove the five archived
  source-spec links. Keep the operational decision self-contained and point
  at current code/tests for evidence.
- In `docs/specs/static-repo-format-v1.md`, remove the archived parent-design
  dependency so the stable spec stands alone.
- In `docs/operations/infrastructure.md`, remove the Claude-era archive link;
  current assistant guidance owns the workflow.
- In `docs/conaryopedia-v2.md`, remove the dead local link to the absent
  rPath-era `conaryopedia.md`; keep the historical sentence as plain text.

Do not replace any of these with GitHub URLs to deleted files.

### Step 2.3: Generalize owner-card routing

Change `scripts/agent-context.sh` fallback routing to:

- assistant/contributor guidance: root contracts, `docs/llms/*`, feature
  ownership, and routing/maintainability scripts; focused proof is doc truth
  plus agent-context validation;
- canonical docs: `docs/modules/*`, `docs/operations/*`, architecture, and
  integration testing; focused proof is doc truth plus affected feature-card
  proof; and
- planning/roadmap docs: `docs/designs/*`, `docs/plans/*`, and
  `docs/roadmaps/*`; focused proof is doc truth and the risk-proportional
  review gate.

In `scripts/test-agent-context.sh`, replace the old planning fixture and add
one fixture for each neutral root. Require all three to resolve to the
planning-docs fallback.

### Step 2.4: Keep optional review tooling optional and neutral

In `scripts/agentic-plan-review.sh`:

- default review output to `target/agent-reviews/`;
- say that output is ignored because `target/` is ignored;
- replace docs-audit/coherency rubric language with canonical truth, focused
  proof, migration/rollback, and closeout expectations; and
- keep the helper optional. Do not make it a CI requirement or a mandatory
  plan directive.

Update `scripts/test-agentic-plan-review.sh` to assert the new help text and
default path while retaining its dry-run and reviewer-stub coverage.

### Step 2.5: Simplify maintainability reporting

Replace the documentation inventory/ledger section in
`scripts/maintainability-drift-report.sh` with one documentation-truth section
that runs `bash scripts/check-doc-truth.sh`. Keep changed-path owner hints and
line-count evidence.

In `scripts/test-maintainability-drift-report.sh`, fix the federation routing
assertion so `pipefail` cannot turn a successful early `grep -q` exit into
status 141. Use a bounded producer such as `grep -m1 -A1` or one `awk`
expression, then retain the semantic assertion.

### Step 2.6: Simplify CI callers before deleting old scripts

In both `.github/workflows/merge-validation.yml` and
`.github/workflows/pr-gate.yml`, remove steps for:

- documentation audit ledger;
- generated documentation inventory;
- feature-coherency validator fixture;
- feature-coherency live-ledger validator; and
- completed coherency-wave registry.

Retain doc truth and its fixture test. Retain agent-context test/validation in
the PR gate and add both to merge validation so owner-card routing has the same
merge proof. Do not add another inventory or wrapper script.

### Step 2.7: Remove archive/review ignore conventions

From `.gitignore`, remove only:

```text
docs/plans/archive/
docs/superpowers/plans/archive/*
docs/superpowers/specs/archive/*
docs/superpowers/reviews/
```

Do not touch `recipes/archive/`, `.worktrees/`, or the existing `/target/`
ignore.

### Step 2.8: Verify and commit

```bash
bash -n scripts/agent-context.sh scripts/test-agent-context.sh \
  scripts/agentic-plan-review.sh scripts/test-agentic-plan-review.sh \
  scripts/maintainability-drift-report.sh \
  scripts/test-maintainability-drift-report.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
bash scripts/agent-context.sh --feature ccs
bash scripts/agent-context.sh --feature packaging
bash scripts/test-agentic-plan-review.sh
bash scripts/test-maintainability-drift-report.sh
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
if command -v actionlint >/dev/null 2>&1; then actionlint; fi
git check-ignore -v target/agent-reviews/example.md
if rg -n 'docs/(plans|designs)/archive' .gitignore .git/info/exclude; then
  echo 'repository-local planning archive ignore remains' >&2
  exit 1
fi
if rg -n -i 'completed (plans|specs|designs|reviews).*(archive|archived)|move completed.*archive' \
  AGENTS.md CONTRIBUTING.md docs/llms/README.md; then
  echo 'active-only planning lifecycle is contradicted by archive guidance' >&2
  exit 1
fi
git diff --check
```

Expected commit:

```text
docs: adopt neutral planning contract
```

## Task 3: Move and Refresh Release Security Policy

**Move:**

- `docs/superpowers/release-security-waivers-2026-05-06.md`
  -> `docs/operations/release-security-waivers.md`

**Modify:**

- `scripts/release-cargo-audit.sh`

### Step 3.1: Preserve current policy and distinguish the blocker

Start with `git mv`, then make the document date-neutral. Preserve:

- active `RUSTSEC-2023-0071` RSA waiver, reachability rationale, sign-off, and
  expiry condition;
- resolved quick-xml follow-up and local-patch removal condition; and
- nonblocking `RUSTSEC-2026-0173` proc-macro warning.

Add a `Current Unwaived Blocker` section dated 2026-07-15 for
`RUSTSEC-2026-0204`, `crossbeam-epoch 0.9.18`, fixed in 0.9.20. State that it
blocks W1 release-green acceptance and is not waived.

Update the comment in `scripts/release-cargo-audit.sh` to the new path. Do not
add the crossbeam advisory to `--ignore`.

### Step 3.2: Verify and commit

```bash
rg -n 'RUSTSEC-2023-0071|RUSTSEC-2026-0204|RUSTSEC-2026-0173' \
  docs/operations/release-security-waivers.md scripts/release-cargo-audit.sh
bash -n scripts/release-cargo-audit.sh
audit_out="$(mktemp)"
if bash scripts/release-cargo-audit.sh >"$audit_out" 2>&1; then
  cat "$audit_out"
  if rg -q -U 'name = "crossbeam-epoch"\nversion = "0\.9\.18"' Cargo.lock; then
    echo 'audit is green while the authored vulnerable lock entry remains; investigate' >&2
    exit 1
  fi
  echo 'audit is now green; remove the stale blocker from policy and roadmap before commit'
else
  cat "$audit_out"
  rg -q 'RUSTSEC-2026-0204' "$audit_out"
  if rg -qi 'failed to (fetch|download|update)|network error|command not found' "$audit_out"; then
    echo 'release audit failed for infrastructure rather than policy' >&2
    exit 1
  fi
  advisories="$(rg -o 'RUSTSEC-[0-9]{4}-[0-9]{4}' "$audit_out" | sort -u)"
  printf 'reported advisories:\n%s\n' "$advisories"
  unexpected="$(printf '%s\n' "$advisories" \
    | rg -v '^(RUSTSEC-2023-0071|RUSTSEC-2026-0173|RUSTSEC-2026-0204)$' \
    || true)"
  if [[ -n "$unexpected" ]]; then
    printf 'additional advisory requires explicit disposition:\n%s\n' "$unexpected" >&2
    exit 1
  fi
fi
rm -f "$audit_out"
git diff --check
```

Do not blanket-ignore the audit command's status. The narrowly scoped
`rg -v ... || true` above only permits an empty unexpected-advisory set, which
is inspected immediately. At the authored baseline the expected audit result
is failure only on the unwaived crossbeam vulnerability plus nonblocking
warnings.

Expected commit:

```text
docs: move release security policy
```

## Task 4: Prepare Structural Check Retirement Without Deleting It

**Modify:** none

**Inspect:** the five old data files, five validator/generator scripts, their
workflow callers, and the branch-only claims described below.

This task is deliberately non-destructive. The authority branch modifies
three of these data files, so the approved design requires the Task 5
reconciliation rehearsal before any of them are removed on `main`.

### Step 4.1: Prove callers and live claims have destinations

Require these searches to return no caller outside the files slated for
retirement and the still-active migration design/plan:

```bash
rg -n 'check-doc-audit-ledger|docs-audit-inventory|check-coherency-ledger|check-coherency-wave-scopes|test-coherency-ledger' \
  .github scripts docs AGENTS.md CONTRIBUTING.md \
  -g '!scripts/check-doc-audit-ledger.sh' \
  -g '!scripts/docs-audit-inventory.sh' \
  -g '!scripts/check-coherency-ledger.sh' \
  -g '!scripts/check-coherency-wave-scopes.sh' \
  -g '!scripts/test-coherency-ledger.sh' \
  -g '!docs/superpowers/**' \
  -g '!docs/designs/2026-07-15-codebase-development-roadmap-design.md' \
  -g '!docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md'
```

Confirm `docs/operations/system-adopt-single-package-dry-run-follow-up.md`
owns the old deferred `CLI-ADOPT-004` claim.

### Step 4.2: Lock the retained coverage map

The retained coverage disposition is:

- product/public claim drift -> doc-truth checker and focused tests;
- feature owner/interaction gate -> feature ownership cards and
  agent-context;
- release vulnerability policy -> release audit plus operations waiver doc;
- completed per-file/wave bookkeeping -> intentionally removed as process
  history.

Do not rename the data into `docs/quality/`, create compatibility shims, or
delete any of it in this task. Task 5 simulates deletion and branch replay;
Task 6 performs the reviewed deletion on `main` only after that proof passes.

## Task 5: Record and Rehearse Authority-Branch Reconciliation

**Modify:**

- `docs/roadmaps/development-roadmap.md`

**Read only:**

- `.superpowers/sdd/progress.md`
- `.worktrees/scriptlet-public-authority/.superpowers/sdd/progress.md`
- inventories of every ignored file under both `.superpowers/` trees and
  `docs/superpowers/reviews/`
- branch diff and canonical docs at the pinned authority head

### Step 5.1: Complete the durable handoff

Set the live roots for this task:

```bash
repo="$(git rev-parse --show-toplevel)"
goal_wt="$repo/.worktrees/scriptlet-public-authority"
```

Read the pinned branch progress file and copy only durable resume facts into
the W1 record. Name the two nonblocking review minors exactly. Do not copy
task reports, review transcripts, worker topology, or per-commit activity
logs.

Read the main checkout's progress file as well. Inventory every ignored local
file by relative path, class, size, and SHA-256 before deletion. Classify the
inventory into progress state, task briefs/reports, review diffs, final
verification reports, local plan reviews, and ignore scaffolding. Progress
files receive a full actionable-fact review; the other classes may be deleted
as process evidence only after every referenced implementation commit exists
and no unique blocker, decision, or follow-up is absent from the roadmap.
Record the class counts and the conclusion in the W0 proof; do not preserve
the full local inventory as a new tracked ledger.

Use a NUL-safe read-only inventory command and retain its output in the
execution log until closeout:

```bash
for local_root in \
  "$repo/.superpowers" \
  "$goal_wt/.superpowers" \
  "$repo/docs/superpowers/reviews"
do
  [[ -d "$local_root" ]] || continue
  find "$local_root" -type f -print0 \
    | sort -z \
    | while IFS= read -r -d '' file; do
        printf '%s\t%s\t' "$(stat -c '%s' "$file")" "$(sha256sum "$file" | cut -d' ' -f1)"
        printf '%s\n' "$file"
      done
done
```

Add a table to the detailed roadmap with:

- the three discarded modified ledger files;
- the eight discarded completed branch-only plans;
- the eight canonical docs to reconcile;
- product source/tests preserved from the branch;
- TGE05/risk decision as the remaining shipping gate; and
- the later production integration rule that retired paths may not return.

Add an exhaustive disposition for the old authority umbrella:

- A file-capability policy precision: implemented on the pinned branch;
  canonical owner `docs/SCRIPTLET_SECURITY.md` and `docs/modules/ccs.md`;
- B generation xattr propagation: implemented on the branch, with TGE05 or a
  named risk decision still required by W1;
- C sysctl target-profile policy: implemented; canonical owner
  `docs/modules/ccs.md` and target-profile tests;
- D LSM policy semantics: implemented; canonical owners
  `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and `docs/modules/remi.md`;
- E PAM/kernel/initramfs/bootloader: PAM remains deliberately non-public;
  preserve later boot/security outcomes in the post-M1 horizon;
- F network/package-manager recursion: deliberately blocked and non-public;
- G Remi non-public test serving: implemented and canonical in
  `docs/modules/remi.md`;
- H schema/docs truth: implemented, with the two retained review minors owned
  by W1/future follow-up; and
- I enabling refactors: no standalone work item; apply the existing
  maintainability boundary only when a product slice touches the hotspot.

Transfer the two branch-only ledger claims into durable W1 handoff rows with
their exact owners and proof:

1. Network/package recursion remains non-public and blocked. Canonical owners:
   `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and the CCS/Remi feature
   cards. Proof includes
   `blocked_classes_block_live_fetch_and_package_manager_recursion`, the
   support-matrix Known-row refusal, Remi scriptlet-corpus classification, and
   publication refusal tests.
2. Stale publication summaries fail closed; public responses are sanitized
   while raw review artifacts remain private. Canonical owners:
   `docs/modules/remi.md` and `docs/SCRIPTLET_SECURITY.md`. Proof includes
   `converted_ccs_path_for_download_rejects_stale_conversion_records`,
   publication report sanitization/raw-report tests, stale public-route tests,
   and the non-public/admin response sanitization tests.

Verify the handoff, then commit it before creating the rehearsal so
`migration_head` is a clean, reproducible commit:

```bash
bash scripts/check-doc-truth.sh
git diff --check
git add docs/roadmaps/development-roadmap.md
git commit -m 'docs: record authority branch handoff'
```

### Step 5.2: Create a disposable clone at the committed migration head

Do not run the rehearsal in either live worktree. Use a clone so index and
merge state cannot leak into them:

```bash
repo="$(git rev-parse --show-toplevel)"
goal_wt="$repo/.worktrees/scriptlet-public-authority"
goal_head=e6eeb4da9c560c34317b57b2a422717d4d556b37
migration_head="$(git rev-parse HEAD)"
test -z "$(git status --porcelain)"
test -z "$(git -C "$goal_wt" status --porcelain)"
test "$(git -C "$goal_wt" rev-parse HEAD)" = "$goal_head"

scratch_root="$(mktemp -d)"
scratch="$scratch_root/repo"
git clone --no-local "$repo" "$scratch"
git -C "$scratch" config user.name 'Conary reconciliation rehearsal'
git -C "$scratch" config user.email 'rehearsal@invalid'
git -C "$scratch" switch -c reconciliation-rehearsal "$migration_head"
```

Record `scratch_root`. On failure, retain it and report the path for diagnosis.
On success, remove it after the proof summary is committed.

### Step 5.3: Simulate old-tree deletion, then merge the authority head

```bash
git -C "$scratch" rm -r docs/superpowers docs/llms/archive
git -C "$scratch" rm \
  scripts/docs-audit-inventory.sh \
  scripts/check-doc-audit-ledger.sh \
  scripts/check-coherency-ledger.sh \
  scripts/check-coherency-wave-scopes.sh \
  scripts/test-coherency-ledger.sh
git -C "$scratch" commit -m 'rehearsal: retire former planning history'
if GIT_MERGE_AUTOEDIT=no \
  git -C "$scratch" merge --no-commit --no-ff "$goal_head"; then
  :
else
  git -C "$scratch" rev-parse -q --verify MERGE_HEAD >/dev/null
  test -n "$(git -C "$scratch" diff --name-only --diff-filter=U)"
  echo 'merge stopped on expected content conflicts; resolve by ownership'
fi
```

The merge may stop for conflicts. Resolve by ownership:

- old planning/audit paths: delete;
- W0 roadmap, assistant contract, routing, CI, and neutral lifecycle: keep W0
  semantics;
- authority branch product source and tests: keep branch content;
- eight canonical docs: integrate branch product truth, then reapply only
  W0's neutral path/proof wording where those concerns overlap.

Never resolve all conflicts with one side wholesale.

After deliberate resolution:

```bash
git -C "$scratch" rm -r --ignore-unmatch \
  docs/superpowers .superpowers docs/llms/archive
git -C "$scratch" add -A
test -z "$(git -C "$scratch" diff --name-only --diff-filter=U)"
test -z "$(git -C "$scratch" ls-files \
  'docs/superpowers/**' '.superpowers/**' 'docs/llms/archive/**')"
if git -C "$scratch" grep -n -I -i -E \
  'superpowers|docs/llms/archive|check-doc-audit-ledger|docs-audit-inventory|check-coherency-(ledger|wave-scopes)|test-coherency-ledger|documentation-accuracy-audit|feature-coherency|coherency-wave' \
  -- . \
  ':!docs/designs/2026-07-15-codebase-development-roadmap-design.md' \
  ':!docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md'; then
  echo 'rehearsal resurrected a retired path, token, or caller' >&2
  exit 1
fi
```

### Step 5.4: Compare branch-owned content and run focused proof

Enter the rehearsal clone before any unqualified Git command, then build the
comparison set:

```bash
cd "$scratch"
git diff --name-status ce6841ec.."$goal_head"
```

For every changed product source/test path outside the eleven discarded
process paths and the eight explicitly reconciled canonical docs, require the
rehearsed index blob or deletion state to match the goal head:

```bash
while IFS= read -r -d '' path; do
  case "$path" in
    docs/superpowers/*|\
    docs/ARCHITECTURE.md|\
    docs/INTEGRATION-TESTING.md|\
    docs/SCRIPTLET_SECURITY.md|\
    docs/conaryopedia-v2.md|\
    docs/modules/ccs.md|\
    docs/modules/remi.md|\
    docs/modules/test-fixtures.md|\
    docs/operations/post-generation-export-follow-up-roadmap.md)
      continue
      ;;
  esac

  if ! git diff --cached --quiet "$goal_head" -- "$path"; then
    printf 'branch-owned content differs: %s\n' "$path" >&2
    exit 1
  fi
done < <(git diff --name-only -z ce6841ec.."$goal_head")
```

Review the eight canonical-doc diffs individually against the branch head:

```bash
git diff "$goal_head" -- \
  docs/ARCHITECTURE.md \
  docs/INTEGRATION-TESTING.md \
  docs/SCRIPTLET_SECURITY.md \
  docs/conaryopedia-v2.md \
  docs/modules/ccs.md \
  docs/modules/remi.md \
  docs/modules/test-fixtures.md \
  docs/operations/post-generation-export-follow-up-roadmap.md
```

Then run in the scratch clone:

```bash
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
cargo fmt --all -- --check
cargo test -p conary-core public_policy
cargo test -p conary-core file_capability
cargo test -p conary generation_file_capabilities
cargo test -p remi publication
cargo test -p remi non_public_test_serving
cargo test -p remi scriptlet_corpus
cargo run -p conary-test -- list
git diff --check

git add -A
git diff --cached --check
git commit -m 'rehearsal: reconcile authority branch after planning migration'
git merge-base --is-ancestor "$goal_head" HEAD
```

If a focused test selector matches zero tests, replace it with the exact
owning test command printed by `scripts/agent-context.sh` and record the
correction. Do not report a zero-match cargo invocation as proof.

### Step 5.5: Record proof and clean up

Before editing the proof record, verify that the live worktrees were
untouched:

```bash
cd "$repo"
test -z "$(git -C "$repo" status --porcelain)"
test -z "$(git -C "$goal_wt" status --porcelain)"
test "$(git -C "$goal_wt" rev-parse HEAD)" = "$goal_head"
```

Then record in the detailed roadmap:

- migration head and authority head;
- merge result and conflict paths;
- discarded process paths;
- canonical-doc resolutions;
- exact commands and counts;
- any zero-match selector correction;
- confirmation that the original worktrees remained clean; and
- the date.

Commit the proof:

```text
docs: record authority branch rehearsal
```

After that commit succeeds, remove only the successful scratch clone
directory. Retain a failed clone until the failure is resolved.

## Task 6: Remove Former Planning History, Structural Checks, and Local Scratch

**Delete:**

- every remaining tracked file under `docs/superpowers/`;
- `docs/llms/archive/`;
- `scripts/docs-audit-inventory.sh`;
- `scripts/check-doc-audit-ledger.sh`;
- `scripts/check-coherency-ledger.sh`;
- `scripts/check-coherency-wave-scopes.sh`;
- `scripts/test-coherency-ledger.sh`;
- ignored `docs/superpowers/reviews/`;
- ignored main and authority-worktree `.superpowers/` SDD state; and
- the `.superpowers/` local exclude entry and its process-specific comment in
  `.git/info/exclude`.

### Step 6.1: Run the destructive stop gate

Proceed only if all are true:

- all tracked changes from Tasks 1-5 are committed and Task 4's
  non-destructive retirement preflight is recorded in the execution log;
- detailed roadmap links and tester/release destinations exist;
- release waiver lives under `docs/operations/`;
- branch head and resume state are recorded;
- disposable reconciliation rehearsal passed;
- both live worktrees are tracked-clean; and
- no current canonical doc links to an old path.

Check exceptional live references explicitly:

```bash
if rg -n -i \
  'superpowers|docs/llms/archive|check-doc-audit-ledger|docs-audit-inventory|check-coherency-(ledger|wave-scopes)|test-coherency-ledger|documentation-accuracy-audit|feature-coherency|coherency-wave' \
  AGENTS.md CONTRIBUTING.md ROADMAP.md .github scripts docs \
  -g '!scripts/check-doc-audit-ledger.sh' \
  -g '!scripts/docs-audit-inventory.sh' \
  -g '!scripts/check-coherency-ledger.sh' \
  -g '!scripts/check-coherency-wave-scopes.sh' \
  -g '!scripts/test-coherency-ledger.sh' \
  -g '!docs/superpowers/**' \
  -g '!docs/designs/2026-07-15-codebase-development-roadmap-design.md' \
  -g '!docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md'; then
  echo 'retired reference remains outside the files slated for deletion' >&2
  exit 1
fi
```

Expected: no output. The two excluded active migration documents are removed
in Task 7.

### Step 6.2: Delete tracked history as one reviewed class

```bash
git rm -r docs/superpowers docs/llms/archive
git rm \
  scripts/docs-audit-inventory.sh \
  scripts/check-doc-audit-ledger.sh \
  scripts/check-coherency-ledger.sh \
  scripts/check-coherency-wave-scopes.sh \
  scripts/test-coherency-ledger.sh
```

Do not individually rewrite the 157 archive files. Their durable truth has
already been classified; Git history preserves them. The structural scripts
are removed in the same post-rehearsal slice because Task 2 removed every
caller and Task 5 proved the branch merge without them.

### Step 6.3: Remove ignored process output after handoff preservation

Review the Task 5 ignored-file inventory and its class counts, then remove the
ignored old review directory and both ignored SDD trees. These deletions are
local cleanup, not part of the Git commit. Before each deletion, reconfirm
that both progress files were classified, every task/report/review class maps
to an existing implementation commit or completed review, and no unique
blocker or resume fact is absent from the detailed roadmap.

Edit `.git/info/exclude` to remove the `.superpowers/` rule and its
process-specific comment. Do not commit `.git/info/exclude`; it is local Git
metadata.

### Step 6.4: Verify and commit

The active migration design and plan still contain the old token, so exclude
only those two files from this intermediate content scan:

```bash
test -z "$(git ls-files 'docs/superpowers/**' 'docs/llms/archive/**')"
if git grep -n -I -i -E \
  'superpowers|docs/llms/archive|check-doc-audit-ledger|docs-audit-inventory|check-coherency-(ledger|wave-scopes)|test-coherency-ledger|documentation-accuracy-audit|feature-coherency|coherency-wave' \
  -- . \
  ':!docs/designs/2026-07-15-codebase-development-roadmap-design.md' \
  ':!docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md'; then
  echo 'retired planning reference remains outside the active migration docs' >&2
  exit 1
fi
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
bash scripts/test-agentic-plan-review.sh
bash scripts/test-maintainability-drift-report.sh
if command -v actionlint >/dev/null 2>&1; then actionlint; fi
git diff --check
```

Expected commit:

```text
docs: remove retired planning history
```

## Task 7: Install the Neutral Closeout Invariant and Self-Delete

**Modify:**

- `scripts/check-doc-truth.sh`
- `scripts/test-doc-truth.sh`
- `docs/roadmaps/development-roadmap.md`

**Delete:**

- `docs/designs/2026-07-15-codebase-development-roadmap-design.md`
- `docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md`

### Step 7.1: Add a focused permanent regression check

Add a neutral-layout function to `scripts/check-doc-truth.sh` and fixture
coverage to `scripts/test-doc-truth.sh`. Construct the retired brand/path from
split shell fragments so the checker does not contain the forbidden literal
that it scans for.

The function must fail when:

- Git tracks anything under the former planning root, former local SDD root,
  or `docs/llms/archive/`;
- tracked text contains the former brand, planning path, local SDD path,
  mandatory provider-skill directive, or provider-prefixed skill token;
- tracked text refers to `docs/llms/archive/` or to a deleted audit,
  inventory, coherency-ledger, wave-registry, or validator name;
- `docs/designs/archive/` or `docs/plans/archive/` exists as tracked/current
  planning structure; or
- the required public and detailed roadmap entrypoints are missing.

It must not scan `.git/`, `target/`, ignored local files, Git history, package
data under `recipes/archive/`, or arbitrary contributor home configuration.
It is a current-tree invariant, not another inventory.

Because `scripts/test-doc-truth.sh` currently builds a filesystem-only fixture,
initialize a temporary Git repository and `git add -A` the good fixture before
running the new tracked-layout assertions. Run `git add -A` after every
negative mutation as well, so the test is independent of working-tree versus
index grep semantics. Construct the forbidden strings from split fragments in
both the checker and its test source so neither retained script contains the
literal brand that final acceptance scans for.

Add fixture cases for one retired tracked path, one live branded reference,
one assistant-history archive link, one deleted validator name, one mandatory
skill directive, and one planning archive directory. Each case must fail for
the intended reason; the clean neutral fixture must pass. Keep the fixture
repository local and uncommitted; no network or GitHub access is needed.

### Step 7.2: Prepare the closeout record without declaring completion

Change W0 from `active` to `closing`. Record the evidence that is already
complete and leave the final acceptance result pending:

- committed head immediately before self-deletion;
- old tracked file count removed;
- permanent check disposition;
- authority rehearsal result;
- confirmation that no replacement archive exists;
- the clean-worktree check still to run after the closeout commit; and
- W1 as the next queued workstream with crossbeam, integration, and TGE05/risk
  gates still explicit.

Do not mark W0 complete, W1 active, or the release green yet.

### Step 7.3: Delete the active design and plan

After the roadmap contains the closeout result:

```bash
git rm docs/designs/2026-07-15-codebase-development-roadmap-design.md
git rm docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md
```

Empty active directories are acceptable; Git simply does not track empty
directories.

Before and after the deletion, replace any inbound link with the durable
roadmap closeout. After the deletion, require both exact paths to have no live
reference:

```bash
if git grep -n -F \
  -e 'docs/designs/2026-07-15-codebase-development-roadmap-design.md' \
  -e 'docs/plans/2026-07-15-development-roadmap-and-neutral-planning-migration-plan.md' \
  -- .; then
  echo 'live link to the closed migration design or plan remains' >&2
  exit 1
fi
```

### Step 7.4: Run pre-commit acceptance on the closeout tree

```bash
repo="$(git rev-parse --show-toplevel)"
goal_wt="$repo/.worktrees/scriptlet-public-authority"
goal_head=e6eeb4da9c560c34317b57b2a422717d4d556b37

bash -n \
  scripts/check-doc-truth.sh \
  scripts/test-doc-truth.sh \
  scripts/agent-context.sh \
  scripts/test-agent-context.sh \
  scripts/agentic-plan-review.sh \
  scripts/test-agentic-plan-review.sh \
  scripts/maintainability-drift-report.sh \
  scripts/test-maintainability-drift-report.sh \
  scripts/release-cargo-audit.sh

bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
bash scripts/agent-context.sh --path docs/roadmaps/development-roadmap.md
bash scripts/agent-context.sh --feature ccs
bash scripts/agent-context.sh --feature packaging
bash scripts/test-agentic-plan-review.sh
bash scripts/test-support-bundle.sh
bash scripts/test-line-count-report.sh
bash scripts/test-maintainability-drift-report.sh
bash scripts/check-github-action-runtimes.sh
bash scripts/test-github-action-runtimes.sh
cargo fmt --all -- --check
cargo run -p conary-test -- list

python3 - <<'PY'
import pathlib
import re
import subprocess
import sys
import urllib.parse

root = pathlib.Path.cwd()
files = subprocess.check_output(
    ["git", "ls-files", "*.md", "*.mdx"], text=True
).splitlines()
pattern = re.compile(r"!?\[[^\]\n]*\]\(([^)\n]+)\)")
errors = []

for name in files:
    path = pathlib.Path(name)
    text = path.read_text(encoding="utf-8")
    for match in pattern.finditer(text):
        raw = match.group(1).strip()
        if raw.startswith("<") and ">" in raw:
            raw = raw[1 : raw.index(">")]
        else:
            raw = raw.split()[0]
        if (
            not raw
            or raw.startswith("#")
            or re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", raw)
        ):
            continue
        target = urllib.parse.unquote(raw.split("#", 1)[0])
        resolved = (
            root / target.lstrip("/")
            if target.startswith("/")
            else path.parent / target
        )
        if not resolved.exists():
            line = text.count("\n", 0, match.start()) + 1
            errors.append(f"{name}:{line}: missing local link target {raw}")

if errors:
    print("\n".join(errors), file=sys.stderr)
    raise SystemExit(1)
print("Tracked Markdown local-link check passed.")
PY

test -z "$(git ls-files \
  'docs/superpowers/**' '.superpowers/**' 'docs/llms/archive/**')"
if git grep -n -I -i -E \
  'superpowers|docs/llms/archive|check-doc-audit-ledger|docs-audit-inventory|check-coherency-(ledger|wave-scopes)|test-coherency-ledger|documentation-accuracy-audit|feature-coherency|coherency-wave' \
  -- .; then
  echo 'retired planning/audit reference remains' >&2
  exit 1
fi
test ! -e "$repo/.superpowers"
test ! -e "$goal_wt/.superpowers"
test ! -e "$repo/docs/superpowers/reviews"
test ! -d docs/plans/archive
test ! -d docs/designs/archive
if rg -n '^\.superpowers/$|Local agent workflow scratch' \
  .gitignore .git/info/exclude; then
  echo 'repository-local ignore for retired SDD state remains' >&2
  exit 1
fi
if rg -n 'docs/(plans|designs)/archive' .gitignore .git/info/exclude; then
  echo 'repository-local replacement planning archive ignore remains' >&2
  exit 1
fi

if command -v actionlint >/dev/null 2>&1; then actionlint; fi
git diff --check
git diff --cached --check
test -z "$(git -C "$goal_wt" status --porcelain)"
test "$(git -C "$goal_wt" rev-parse HEAD)" = "$goal_head"
```

Run the release audit separately and record the expected W1 blocker without
making W0 depend on product remediation:

```bash
audit_out="$(mktemp)"
if bash scripts/release-cargo-audit.sh >"$audit_out" 2>&1; then
  cat "$audit_out"
  echo 'release audit is green; update the roadmap proof to the current fact'
else
  cat "$audit_out"
  rg -q 'RUSTSEC-2026-0204' "$audit_out"
  if rg -qi 'failed to (fetch|download|update)|network error|command not found' "$audit_out"; then
    echo 'release audit failed for infrastructure rather than policy' >&2
    exit 1
  fi
  advisories="$(rg -o 'RUSTSEC-[0-9]{4}-[0-9]{4}' "$audit_out" | sort -u)"
  unexpected="$(printf '%s\n' "$advisories" \
    | rg -v '^(RUSTSEC-2023-0071|RUSTSEC-2026-0173|RUSTSEC-2026-0204)$' \
    || true)"
  test -z "$unexpected"
fi
rm -f "$audit_out"
```

At the authored baseline this is expected to fail on
`RUSTSEC-2026-0204`. If the dependency changed during execution, inspect and
record the actual result; never normalize an unexpected advisory into a
waiver as part of W0.

### Step 7.5: Mark complete, commit, and prove the live state

Only after Step 7.4 passes, change W0 from `closing` to `complete` in the
detailed roadmap and record the dated acceptance command set. Keep W1 queued.
Stage the final roadmap/checker/test/deletions, then rerun the checks affected
by the status-only edit:

```bash
repo="$(git rev-parse --show-toplevel)"
goal_wt="$repo/.worktrees/scriptlet-public-authority"
goal_head=e6eeb4da9c560c34317b57b2a422717d4d556b37
git add -A
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
git diff --cached --check
git commit -m 'docs: complete neutral planning migration'

test -z "$(git status --porcelain)"
test -z "$(git -C "$goal_wt" status --porcelain)"
test "$(git -C "$goal_wt" rev-parse HEAD)" = "$goal_head"
git status --short --branch
```

Expected final Workstream 0 commit:

```text
docs: complete neutral planning migration
```

## Rollback and Failure Handling

- Before Task 5, every tracked change is ordinary additive/move/generalize
  work and can be reverted commit by commit.
- If Task 5 finds a conflict outside the enumerated process or canonical-doc
  paths, stop, retain the scratch clone, add the path to the reconciliation
  map, and review ownership before continuing.
- If a branch-owned product blob differs after rehearsal, treat that as lost
  implementation, not an acceptable merge variation.
- If old history contains current truth with no destination, restore only the
  affected file from the prior commit, classify it, and resume after its truth
  is canonicalized. Do not restore the entire old hierarchy by default.
- If preparing or removing a structural checker exposes a real invariant with
  no retained owner, stop Task 4 or Task 6 and add that invariant to the
  smallest existing product, doc-truth, routing, release, or privacy check.
  Do not recreate a ledger.
- If final stale-reference proof fails, repair the live reference or routing
  call site; do not add an allowlist for ordinary current documentation.
- If local ignored SDD state contains an unrecorded blocker, postpone its
  deletion, record the blocker in the umbrella, and rerun the handoff review.
- W0 can complete with the known crossbeam advisory still failing only because
  W1 explicitly owns remediation and the operations policy names it as an
  unwaived blocker.

## Workstream 0 Acceptance

Workstream 0 is complete only when all of the following are true:

- `ROADMAP.md` is concise and links to the detailed umbrella;
- the detailed roadmap carries honest maturity, ordered W0-W3 status, branch
  handoff, current blockers, and longer horizons;
- the tester milestone and outreach draft are active and discoverable;
- the release waiver is preserved under operations;
- repository contracts, ownership routing, required scripts/workflows, and
  ignore rules are tool-neutral; optional reviewer adapters may name their
  providers but are not required by a plan or gate;
- per-file audit/coherency/wave bookkeeping is gone without losing a current
  invariant;
- no tracked old planning or assistant-history path remains;
- no live old path, mandatory worker-skill directive, or provider-prefixed
  skill token remains;
- no replacement planning archive exists;
- the pinned authority merge rehearsal passes and records all resolutions;
- ignored local SDD/review state is removed only after its unique resume facts
  are durable;
- the retained documentation, routing, review-helper, maintainability, shell,
  and workflow tests pass;
- release audit status is recorded honestly as W1 input; and
- this design and plan are deleted after the umbrella records W0 closeout.

The next slice after this plan is Workstream 1: repair the unwaived dependency
advisory, reconcile and integrate the pinned authority branch without
resurrecting retired process files, resolve TGE05 or record the exact shipping
risk decision, and prove one clean release candidate.
