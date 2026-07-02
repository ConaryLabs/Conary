---
last_updated: 2026-07-01
revision: 1
summary: Umbrella design for shifting the limited preview from internal-proof posture to external tester contact
---

# First External Tester Loop: Design Spec

**Date:** 2026-07-01
**Status:** Approved design; child plans pending
**Goal:** Turn the 2026-07-01 big-picture review findings into a sequenced
program that gets Conary its first structured external tester feedback, with
a falsifiable milestone that ends the internal-proof era.

---

## Purpose

The 2026-05-26 release-hardening queue is complete and archived. Since then,
every near-term roadmap priority has been a "keep X green" maintenance item,
and the repository contains exactly one external-feedback artifact: the
unposted tester-post draft at
`docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`.

The project has proved, repeatedly and to itself, that Conary is safe to try.
It has never tested whether strangers will try it and succeed. Every
additional internal gate now builds confidence in things testers may not care
about, while the real unknowns — does adopt/unadopt survive a messy real
laptop, does the mutation acknowledgement read as a guard or as friction, is
Remi cold-start a dealbreaker — can only be answered by people who are not us.

This umbrella shifts the project from internal-proof posture to
external-contact posture.

## Inputs

This design is based on:

- the Claude Fable 5 big-picture review of 2026-07-01 (strategy, CLI surface,
  meta-layer analysis, code-health assessment);
- the Codex verification pass of the same date, which confirmed the roadmap
  and feedback-artifact claims against the checkout and corrected one
  staleness: `conary new`, `cook`, `publish`, and `ccs keygen` now exist, so
  the June 10 gap-analysis keystone is no longer missing code — the remaining
  gap is stranger-usable operator documentation;
- `docs/superpowers/distro-adoption-gap-analysis-2026-06-10.md`;
- the archived precedent umbrella,
  `docs/superpowers/specs/archive/2026-05-26-limited-preview-release-hardening-design.md`;
- `ROADMAP.md`, `AGENTS.md`, and the current `conary --help` surface.

## Strategic Thesis

The wedge is unchanged: Nix-like safety on the Linux distribution you already
use, adoption-led, with a clean escape hatch. What changes is the definition
of progress. Until the milestone below is hit, progress means external
contact, not internal proof.

**Milestone (falsifiable):** 10 people we do not know complete the loop
install → adopt → list/search → dry-run → unadopt on their own machines and
report friction through the beta feedback template.

The program optimizes for:

- a stranger succeeding unassisted on a pinned release;
- a first-contact CLI and docs surface that reads "safe package manager,"
  not "research platform";
- tester-reported friction outranking the "keep green" rotation while the
  loop runs;
- honest recording of negative findings — a systemic blocker discovered by
  testers is a successful outcome of this program, not a failure of it.

The program does not optimize for:

- new distros, architectures, or platform surface;
- conaryd remote/fleet features;
- meta-layer (ledger/card/gate/agent-tooling) expansion;
- command renames or removals.

## Current Repo Facts

- Adoption, unadoption, native handoff, generation backups, scriptlet
  sandboxing, release evidence, and the support bundle are landed and gated —
  the safety prerequisites for a public ask are met.
- `conary --help` exposes roughly 44 top-level commands, most of them
  builder/platform surface irrelevant to a preview tester.
- The June 2026 commit history shows the meta-layer competing with product
  work: 511 commits, with `docs/` touched about as often as `apps/`, and the
  most recent batches dominated by agent-context/card/gate work.
- `.github/ISSUE_TEMPLATE/beta_feedback.md` exists; `remi prewarm` exists;
  the `release.sh` → GitHub Actions pipeline exists, but
  `docs/operations/release-artifact-matrix.md` still marks every product row
  source-build-only, with binary checksums, signatures, and SBOM/provenance
  evidence pending until a preview release links concrete artifact URLs;
  Remi cold-start latency is a documented preview risk.
- The recipe → `.ccs` build/publish toolchain exists (`conary new`, `cook`,
  `publish`, `ccs keygen`), but no operator-facing tutorial shows a stranger
  how to run their own Remi or check host compatibility before trying the
  generation model.

## Decision

Create four sequenced slices under this umbrella. Slices 1–3 are strictly
ordered; slice 4 runs parallel after slice 2. Each slice becomes a child
implementation plan.

### Slice 1: Meta-Layer Budget Policy

Add a short "Meta-layer budget" paragraph to the Maintainability section of
`AGENTS.md`:

- ledger, card, gate, and agent-tooling changes are allowed only when product
  work forces them — a touched path, a failing gate, or a factual drift;
- at most one discretionary meta slice per four product slices;
- the policy holds at least until the tester milestone is hit.

`ROADMAP.md` gets a one-line pointer to the policy. The evidence basis is the
June 2026 commit distribution recorded above. This slice is a policy commit
with no code; it is the first child implementation slice and should be the
first change executed after this design locks in.

### Slice 2: Preview CLI Surface Tiering

Mark builder/platform subcommands hidden in clap so the default `conary
--help` shows only the daily-driver set, with an epilogue pointing at the
full surface. Nothing is removed or renamed; every existing command path
keeps working.

Visible (daily-driver): `install`, `remove`, `update`, `search`, `list`,
`autoremove`, `pin`, `unpin`, `try`, `system`, `repo`, `config`, `distro`,
`self-update`.

Hidden (advanced; exact list settled in the child plan): `cook`, `new`,
`publish`, `convert-pkgbuild`, `recipe-audit`, `ccs`, `derive`, `derivation`,
`model`, `collection`, `groups`, `automation`, `bootstrap`, `cache`,
`profile`, `provenance`, `capability`, `trust`, `verify-derivation`, `sbom`,
`federation`, `export`, `canonical`, `registry`, `query`.

Discovery path: a root-level `--help-advanced` flag whose listing is
rendered from the same clap command tree, hidden subcommands included, so
the advanced list cannot drift from the real surface. Clap's generated help
and its built-in `help` subcommand do not list hidden commands, which is why
this is a dedicated flag rather than a `help` variant. The default help
epilogue gains one line: "Advanced packaging and platform commands: run
`conary --help-advanced`." A docs page lists the same advanced surface.

Proof:

- a focused `cli_daily_ux` test asserting the default help shows exactly the
  daily-driver set and `--help-advanced` shows the remainder;
- an intentional update to `root_help_includes_daily_workflow_examples`
  (`apps/conary/tests/cli_daily_ux.rs`), whose epilogue assertions the new
  help text will touch — the child plan must list every help-content test it
  rewrites and why;
- README and site quickstart re-checked against the docs-truth gate;
- feature-coherency ledger rows for touched help surfaces rerun (this is a
  product-forced meta update, allowed under slice 1's policy).

This slice must land before the tester post goes out.

### Slice 3: First External Tester Loop

The milestone slice. Three phases:

**Pre-launch gate (all items complete before posting):**

- a pinned release cut via `./scripts/release.sh conary`: tag pushed,
  `release-build.yml` green, GitHub release assets published, checksum and
  signature verification evidence recorded, and the
  `docs/operations/release-artifact-matrix.md` `conary` row moved off
  source-build-only by linking the concrete artifact URLs — the tester post
  pins this exact tag;
- `remi prewarm` run against the tested package set on the public Remi, so
  first-install latency does not masquerade as package-manager slowness;
- a published compatibility checklist linked from the quickstart, split into
  two tiers so it does not overstate requirements: the basic
  adopt/unadopt/install loop (supported distro versions: Fedora 44,
  Ubuntu 26.04 LTS, Arch, on their stock kernels) and the generation-model
  features (composefs-capable kernel, systemd, UEFI boot stack) — only the
  second tier carries the stricter requirements;
- `.github/ISSUE_TEMPLATE/beta_feedback.md` narrowed or supplemented for
  this loop: today it invites six preview lanes including generation export
  and conaryd, which cuts against the narrow milestone; the loop needs an
  intake that foregrounds the install → adopt → dry-run → unadopt path and
  captures an explicit "completed the full loop: yes/no" signal the tracker
  can count;
- the 2026-05-19 subreddit tester post refreshed to reference the pinned
  release tag and artifact URLs, the tiered CLI, the compatibility
  checklist, and the feedback template link.

**Launch:** posting is Peter's manual action. The child plan prepares
everything up to "click post" and records the posting venues.

**Loop:**

- feedback arrives as GitHub issues via the beta feedback template;
- each report is triaged into fix-now, next-slice, or declined-with-reason;
- `docs/superpowers/first-external-tester-loop-tracker.md` tracks completions
  toward the milestone and links each report;
- while the loop runs, tester-reported friction outranks the roadmap's
  "keep green" rotation.

**Exit criteria:** 10 stranger completions recorded in the tracker, or a
documented pivot decision if a systemic blocker emerges. A blocker discovered
this way (for example, cold-start latency proving fatal in practice) is a
finding, and the pivot document must say what changes because of it.

### Slice 4: Self-Hosted Remi Tutorial

A "run your own Remi in 30 minutes" guide under `docs/guides/`: single
binary, minimal `remi.toml`, optional S3/R2 chunk storage, systemd unit,
health check. Written for a stranger's host, not our deployment.

Proof: the tutorial is executed start-to-finish on a fresh VM, and that
execution is the acceptance test. Drift between the tutorial and the actual
binary behavior is a docs-truth failure.

This slice unblocks the org/self-hoster persona from the June 10 gap
analysis but does not gate the tester launch; it runs parallel after
slice 2.

## Sequencing

1. Slice 1 (policy) — first child slice, executed immediately after design
   lock-in.
2. Slice 2 (CLI tiering) — small, must precede launch.
3. Slice 3 (tester loop) — pre-launch gate, launch, loop to milestone.
4. Slice 4 (Remi tutorial) — parallel after slice 2; does not block launch.

## Error Handling

Failure handling is the product in this program. Negative tester feedback,
incomplete runs, and confusion reports are recorded in the tracker with the
same weight as completions. If the milestone stalls (no new completions for
three consecutive weeks after launch), the loop exits via the
documented-pivot path rather than silently reverting to internal-proof work.

## Non-Goals

- No command renames or removals; tiering is help-visibility only.
- No new distros or architectures this round (openSUSE, aarch64 remain
  roadmap follow-ups gated on tester demand).
- No conaryd remote/fleet surface.
- No meta-layer expansion beyond product-forced updates and the slice 1
  budget.
- No migration runbooks or trust/key lifecycle guide this round; both are
  recorded here as the next documentation round after the milestone.
