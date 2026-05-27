---
last_updated: 2026-05-26
revision: 1
summary: Umbrella design for turning external model review findings into the next limited-preview hardening queue
---

# Limited Preview Release Hardening: Design Spec

**Date:** 2026-05-26
**Status:** Active design; split into four implementation plans
**Goal:** Turn the DeepSeek, Gemini, GPT Pro deep research, and Codex review
findings into a sequenced release-hardening program that protects Conary's
limited-preview promise.

---

## Purpose

The reviews converged on the same strategic answer: Conary's best near-term
position is not "universal Linux package-manager replacement." It is a
reversible adoption path that gives testers Nix-like safety on Fedora, Ubuntu,
and Arch without switching distributions or learning a new package language.

That wedge is valuable, but it is also fragile. A limited preview needs boring
truthfulness, trustworthy scriptlet boundaries, supportable release artifacts,
and a recovery story that survives live-state damage. This spec turns the
review findings into a queue that can be executed one plan at a time.

## Inputs

This design is based on:

- DeepSeek's ecosystem and product-positioning review;
- Gemini's architectural gap review;
- GPT Pro deep research supplied during the 2026-05-26 review thread;
- Codex's local code and docs review on 2026-05-26;
- current repository docs, especially `README.md`, `ROADMAP.md`,
  `docs/INTEGRATION-TESTING.md`, `docs/SCRIPTLET_SECURITY.md`,
  `docs/modules/source-selection.md`, and the docs-audit ledger.

## Strategic Thesis

Conary should say the quiet part clearly:

> Conary gives you Nix-like safety on the Linux distribution you already use,
> starting with native package-manager adoption and a clean escape hatch.

The preview should optimize for:

- adoption before takeover;
- dry-run and unadopt flows before generation switching;
- native package-manager authority until the user explicitly changes it;
- truth in public docs and CLI output over bigger promises;
- repeatable evidence before broader distro and architecture claims.

The preview should not optimize for:

- racing Nix on package count or purity;
- claiming GA-grade package-manager replacement status;
- broadening distro/architecture coverage before Fedora, Ubuntu, and Arch are
  consistently boring;
- adding service/fleet features before the local safety story is simpler.

## Current Repo Facts

The review found a strong base:

- adoption/unadoption and selected-generation native handoff are already the
  core differentiator;
- generation export has real x86_64 raw/qcow2/ISO QEMU evidence;
- Remi conversion makes Conary useful before a native CCS corpus exists;
- docs-audit and docs-truth gates are already unusually serious for a young
  package manager;
- conaryd package jobs, publication debt, and docs truth checks have moved
  beyond stub status.

It also found current risks that should shape the next queue:

- `CHANGELOG.md` still contains a stale conaryd `501 Not Implemented` claim
  while the active docs describe queued package execution;
- `README.md` says every install/remove/update is all-or-nothing, but legacy
  direct post-install/post-remove scriptlet failures can be warning-only after
  package files are installed or removed;
- docs-truth does not currently scan `CHANGELOG.md` or the public site;
- single-package adoption dry-run currently refuses instead of previewing;
- the five-minute tester path currently depends on build-from-source setup in
  public docs, which is not a true five-minute path for most testers;
- the explicit `--allow-live-system-mutation` acknowledgement is correct for
  safety, but the quickstart must explain why it exists so it reads as a guard
  rather than unexplained ceremony;
- `docs/SCRIPTLET_SECURITY.md` still names `chroot` as filesystem isolation
  while other code/docs imply stronger protected sandboxing and the public
  security review raised `chroot` escape as a P0 risk class;
- scriptlets that need durable host integration can be forced toward
  `--sandbox=never` instead of declaring narrow integration capabilities;
- Remi on-demand conversion creates first-use latency that may look like
  package-manager slowness to testers;
- the live SQLite DB is the operational authority, but generation artifacts do
  not yet carry enough state to rebuild the manager's DB after corruption;
- public docs explain many facts, but a tester still has to assemble the
  five-minute safe path from several places.

## Decision

Create four active implementation plans and keep broader ecosystem growth as
roadmap follow-up:

1. **Preview Truth And Onboarding**
2. **Scriptlet Trust Assurance**
3. **Release Evidence And Supportability**
4. **Generation State Resilience**

The first two plans are release hardening. The third makes the preview easier
to support and contribute to. The fourth can land shortly after the first
tester wave if the preview remains package-manager focused, but it becomes a
release blocker before generation switching is marketed as the primary ask.

## Plan A: Preview Truth And Onboarding

The public surface should be coherent enough that a careful tester understands
what to try, what not to try, and how to back out.

Required outcomes:

- fix active truth drift in `CHANGELOG.md`, `README.md`, and site copy;
- revise atomicity wording so it distinguishes package DB/file transaction
  atomicity, generation atomicity, and best-effort legacy scriptlet side
  effects;
- add a five-minute adoption/unadoption quickstart that is not build-from-source
  as the main public tester path;
- make the unadopt and native-handoff recovery paths prominent;
- explain the live-mutation acknowledgement flag at first use and decide
  whether a shorter preview-safe alias or session acknowledgement is needed;
- document Remi cold-start conversion latency and either pre-warm a small tested
  package set or provide an explicit tester pre-warm command;
- audit `conary system init` first-run failure modes such as existing state,
  low disk space, and missing kernel features;
- sharpen the Nix comparison into one honest paragraph;
- include `CHANGELOG.md` and site copy in docs-truth checks;
- either implement true single-package adoption dry-run or make the unsupported
  route and first-run UX deliberate.

Success means a tester can read one page, try the safe lane, and understand
that adopted packages remain owned by dnf, apt, or pacman until explicit
takeover.

## Plan B: Scriptlet Trust Assurance

Conary cannot ask for package-manager trust while leaving ambiguity around
scriptlet containment.

Required outcomes:

- audit protected live-root scriptlet execution and document the exact root
  transition and namespace guarantees;
- replace stale or ambiguous `chroot` wording if protected live-root execution
  no longer depends on it, or replace the implementation if it still does;
- add a regression gate that fails if protected scriptlets regain the unsafe
  `chroot` capability;
- make legacy post-install/post-remove warning-only failures visible through
  structured changeset metadata, status, or history;
- define a small capability declaration model for scriptlets that need system
  integration, instead of telling operators to disable the sandbox wholesale.

Success means the sandbox docs, code, and tests tell the same story, and
operators can see when scriptlet side effects degraded a transaction.

## Plan C: Release Evidence And Supportability

The preview needs evidence and support loops more than new feature surface.

Required outcomes:

- publish a product/artifact/provenance matrix for `conary`, `remi`,
  `conaryd`, and `conary-test`;
- make SBOM/provenance expectations explicit for every release artifact;
- keep local QEMU/KVM evidence honest while remote Forge validation is paused;
- add a privacy-preserving beta support bundle flow or script;
- add a beta feedback issue template and contributor-onboarding guide;
- make "good first validation tasks" visible for testers who want to help.

Success means a release artifact can be inspected, a beta bug report can be
useful without leaking secrets, and a motivated tester can become a
contributor.

## Plan D: Generation State Resilience

Gemini's SQLite SPOF finding is strategically important because Conary is
database-first while selling generation-backed safety.

Required outcomes:

- write a compact generation-bound state snapshot next to each selected
  generation artifact;
- land a minimal forward-compatible marker writer before the first tester wave
  so early preview generations can be distinguished from pre-snapshot
  generations;
- include enough package, trove, generation, publication-debt, and provenance
  state to rebuild the live SQLite DB for manager visibility;
- add verification commands for snapshot integrity;
- add an explicit dry-run recovery command that reconstructs into a temporary
  DB before touching the live DB;
- fail closed when generation metadata and live DB disagree.

Success means a damaged `conary.db` is a recoverable state-management problem,
not a blind-manager disaster.

## Deferred Roadmap Items

These remain important but should not crowd the release-hardening queue:

- a broader pre-converted Remi base repository beyond the small preview
  warm-set;
- a small CCS-native signed reference corpus;
- openSUSE/Tumbleweed support after Fedora/RPM behavior is stable;
- aarch64 generation boot assets;
- recipe/bootstrap/model polish for system-builder use cases;
- conaryd remote/fleet management;
- signed portable generation bundles;
- governance-lite and contributor review rules.

## Release Sequencing

Before the limited tester post:

- land Plan A;
- land the Plan D forward-compatible generation-state marker;
- land the documentation and assurance portions of Plan B or explicitly narrow
  the preview if any scriptlet assurance work remains open.

Before widening beta:

- complete Plan B;
- complete Plan C;
- rerun local QEMU/KVM evidence and publish artifact/provenance status.

Before making generation switching the headline ask:

- complete Plan D;
- restore scheduled remote KVM validation or document a replacement gate with
  equivalent evidence quality.

## Non-Goals

- Do not add a new distro target in this queue.
- Do not add aarch64 boot assets in this queue.
- Do not create a broad telemetry system before privacy and redaction rules are
  explicit.
- Do not turn the preview into a takeover-first program.
- Do not create a native CCS corpus plan here beyond roadmap positioning.

## Self-Review

- No incomplete requirements remain; each plan has a concrete owner surface.
- The scope is intentionally decomposed into four plans so execution can stop
  cleanly after any slice.
- The release claim remains limited preview, not GA.
- The strongest differentiator remains adoption/unadoption on existing
  distributions.
