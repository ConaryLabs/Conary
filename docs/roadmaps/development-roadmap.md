---
last_updated: 2026-07-25
revision: 4
summary: Track Conary's cross-distro package milestone, ordered workstreams, evidence, blockers, and post-milestone horizons
proof_baseline: "v0.12.0 at eb256b19b4f04ca1d03b6af39a2819d746d3a22a; release and issue-remediation proof complete"
current_milestone: first external tester loop
active_workstream: W3 First External Tester Loop
next_workstream: post-milestone work selected from tester evidence
---

# Codebase Development Roadmap

## Purpose and Supported Limited-Preview Scope

This document is the detailed source of truth for Conary's current maturity,
ordered delivery work, evidence, blockers, and longer horizons. It separates
subsystem maturity from workstream execution state and evidence freshness.

A coordinated credential-history cleanup on 2026-07-18 changed commit object
IDs at and after the removed history without changing the corresponding
shipping trees. Current-history commit references below use the rewritten
identities. Explicit obsolete pre-rebase and disposable-rehearsal identifiers
remain labeled as historical, and workflow runs created before the rewrite
still display their original head IDs.

The next supported package-manager preview is cross-distro and bounded to
Fedora 44, Ubuntu 26.04 LTS, and Arch Linux. It centers on installing RPM, DEB,
and Arch artifacts through Conary regardless of the host's native package
format. Source format owns lifecycle, dependency, version, payload, and
configuration semantics; the target supplies typed host capabilities. Adoption
and unadoption remain a migration bridge for already-installed native packages,
not the product's primary package path. Generation, conaryd, and federation
claims stay inside the narrower limits recorded below.

Remote Forge validation is paused pending a KVM-capable runner. The dated
2026-05-21 Group O local QEMU run established the earlier export baseline. The
dated 2026-07-16 Group O local KVM run superseded it by passing all five
installed-runtime, file-capability, and bootstrap-run raw/qcow2 cases against
`minimal-boot-v4`.
The dated 2026-05-21 Group P QEMU run passed ISO generation-carrier,
provenance-sidecar, copy-back, read-only carrier boot, and writable `/etc`
overlay proof. These are local x86_64 evidence, not a broad or current
remote-validation claim.

## Milestone 1 Exit Condition

The first external tester milestone closes when ten unique people outside the
existing project circle complete
`foreign artifact install -> list/query -> update --dry-run -> remove` on
supported systems and report friction. The installed artifact's source format
must differ from the host's native package format. Alternatively, a maintainer
may record a pivot when a reproducible systemic blocker inside the supported
scope is backed by the affected attempts and a chosen remediation or explicit
scope change.

Launching outreach, publishing a release, receiving partial reports, or seeing
ordinary outreach difficulty does not satisfy the milestone. The active
[milestone tracker](external-tester-milestone.md) currently records 0/10 for
the revised cross-distro flow. One external tester's two successful
adoption-led reports remain useful historical evidence but do not satisfy the
new source-format/host-format crossing requirement.

## Principles and Safety Boundaries

- Make source-independent package installation the primary path. Preserve
  reversible adoption and explicit authority transfer as migration tools.
- Fail closed at public, security, persistence, and package-mutation
  boundaries; do not turn missing proof into optimistic behavior or copy.
- Keep supported scope, execution status, and proof freshness as independent
  facts. Do not collapse them into percentages or a single readiness color.
- Treat package-manager, scriptlet, trust, and release claims as evidence
  bearing. Focused tests do not substitute for an explicitly required QEMU,
  clean-host, installed-binary, or external-user proof.
- Use heuristics only for diagnostics, redaction, discovery, and prioritization.
  Compatibility, publication, mutation, and security authority require parsed
  typed contracts plus payload or persisted-state validation.
- Preserve privacy in tester and operator evidence. Do not request secrets,
  private keys, credential files, broad machine dumps, or live databases by
  default.
- Keep active planning proportional to risk and remove it after canonical
  truth, verification, roadmap state, and resume facts are durable.

## Subsystem Maturity

This is the dated 2026-07-19 baseline. The label describes reliability within
the stated scope, not whether a workstream happens to be active.

| Subsystem | Maturity | Current limitation or next proof |
| --- | --- | --- |
| CLI and core package operations | solid | Scope is the limited preview; external use and fresh combined proof matter more than new surface area. |
| Adoption, unadoption, and native handoff | solid | Proof covers the current three-distro scope; the released Arch package initializes only its exact native profile and synchronizes Remi. |
| Database, CAS, native parsing, and resolution | solid | Some advanced repository-policy abstractions and integration edges remain incomplete. |
| Packaging, static repositories, trust, and self-update | solid | `v0.12.0` has exact tag, artifact hashes, signature, supported Arch-path, deployment, and real installed-binary self-update proof. No SBOM/provenance sidecars are published or planned. |
| CCS conversion and native lifecycle authority | active hard switch | PR #61 replaces review/refusal-era scriptlet policy with exact RPM, Debian, and ALPM lifecycle transactions; release, deployment, and cross-distro matrix proof remain. |
| Generation build and export | limited | Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | Operator-run limited-preview scope; distribution, cold-start readiness, and a wider stranger-operated path remain limited. |
| conaryd package and query service | limited | Authorization is exact root/daemon/configured-group authority; restart semantics, resolver-backed dry-run proof, and deployment remain incomplete. |
| Federation | experimental | Coordinator and fetch paths are not wired into serving; TLS identity documentation and enforcement do not yet agree. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | One organic tester completed the former adoption flow on Ubuntu 26.04 and Fedora 44 with verified release-package checksums; the revised cross-distro milestone is 0/10, the former 2026-07-20 through 2026-07-22 outreach slots passed without posts, and new outreach requires a released cross-distro matrix. |

## Workstreams

### W0 Neutral Planning Migration

- **Outcome:** complete. Repository guidance is tool-neutral, current roadmap
  truth stays under `docs/roadmaps/`, durable decisions live with their
  canonical architecture, module, or specification owner, and bounded
  execution state lives in its issue and pull request.
- **Durable guard:** documentation truth rejects retired planning trees and
  replacement archives; feature cards and `scripts/agent-context.sh` own
  assistant routing.
- **Dependencies:** none. Git history owns the retired migration inventory and
  closeout evidence.

### W1 Integrated Release-Green Baseline

- **Outcome:** complete. The authority work, security-dependency remediation,
  KVM generation proof, public test fixture, and release gates were integrated
  into the baseline consumed by W2.
- **Durable truth:** current behavior and proof commands live in their owning
  canonical docs, feature cards, and tests rather than in this roadmap.
- **Dependencies:** none. W3 owns current external-use evidence.

### W2 Preview Release and Remi Readiness

- **Outcome:** complete. A pinned preview, compatible Remi deployment, release
  artifacts, update proof, and representative install/remove smoke established
  the service path later superseded by W3's current release evidence.
- **Durable truth:** current release identity and artifact proof live in
  `docs/operations/release-artifact-matrix.md`; supported fixture and service
  behavior lives in the owning module docs and tests.
- **Dependencies:** none. W3 owns current release, outreach, and tester status.

### W3 First External Tester Loop

- **Outcome:** Conary has external evidence that strangers can install and
  remove a package whose source format differs from the supported host's native
  format.
- **Current truth:** the W3a publication and issue-remediation gate is complete
  at immutable `v0.12.0`. It preserves the preceding exact SONAME, ELF-class,
  and constraint-aware Arch fixes and adds safe in-root system-symlink
  traversal plus usable installed-host support diagnostics. No broad external
  venue post has been published. One organic external tester completed the
  former adoption-led flow on Ubuntu 26.04 LTS and Fedora 44. Those reports
  remain valid release/onboarding evidence but do not count toward the revised
  cross-distro milestone, so the tracker is 0/10.
  The former 2026-07-20 through 2026-07-22 outreach window passed without a
  post. Issue #41's reported Artix host and Fedora-form source route remain
  outside supported-scope proof and require the repaired bundle's source-selection,
  source-pin, and repository evidence before classification.
- **Execution status:** active; release remediation is complete, but broad
  outreach and its replacement dates remain postponed.
- **Dependencies:** GitHub Support must dereference the cached pull-request and
  commit views that still expose pre-rewrite history, and the maintainer must
  re-check each venue's current account/rule eligibility immediately before
  submission.
- **Next gate:** after GitHub Support confirms the cached history is no longer
  reachable and venue checks pass, assign a new staggered schedule. Then submit
  the refreshed Show HN packet, record its actual URL and timestamp, and
  continue collecting privacy-safe reports toward the second unique qualifying
  tester.
- **Proof:** immutable `v0.12.0` was published at
  `2026-07-23T21:39:20Z`; annotated tag object
  `8411169b40d8523ee716518cb3dc3e51acddb019` peels to commit
  `eb256b19b4f04ca1d03b6af39a2819d746d3a22a`. Remediation merge CI
  `30041401268` and release-commit CI `30042990554` passed 11/11 jobs,
  release-build `30043930486` passed, and exact-tag deploy-and-verify
  `30047027525` passed. Seven assets are published; independent downloads
  matched all five `SHA256SUMS` payloads and all seven REST digests, and the
  official preceding binary verified the detached CCS signature. An isolated
  schema-77 database completed a signed update from the official preceding
  preview to `v0.12.0` and then reported itself current. The exact Arch package
  binary initialized profile `arch`, synchronized 15,462 Arch-only Remi rows,
  installed and
  executed `htop 3.5.2-1-arch`, and removed all five files and the trove. In
  the same isolated writable root it installed and removed a one-file probe
  through `/usr/lib64 -> lib`, while a paired out-of-root symlink was rejected
  without an outside write or trove. Host pacman inventory, linker cache,
  installed Conary binary, and `/usr/lib64` stayed unchanged. This was exact
  released-binary proof with live-host pacman evidence read-only, not native
  `pacman -U` or a pristine VM. Support-bundle self-tests passed, and an
  isolated bundle captured integrity, table, repository, and profile summaries
  without including its database; this host had no installed root-owned
  Conary database for a live cached-sudo success. Full Remi health passed
  10/10, six public routes returned HTTP 200, and the deployed CCS matched
  release hash
  `c973fb654b67da0619d6837b34e2f5f78bbea90dfd9fb8de19b6edf9cbe9582a`,
  size `16183371`, and signature. The preceding Fedora-form KVM result remains
  a conaryOS regression baseline rather than literal stock Fedora native-PM
  proof. No SBOM or provenance sidecars are published or planned. The
  repository Welcome Discussion remains live at discussion 36, and issue 35
  remains closed with its released-path proof; neither repository action
  launches broad outreach. Issues 37 and 38 remain two supported-host attempts
  by one unique tester. They are historical onboarding evidence, not
  completions under the revised milestone. Each qualifying completion belongs
  to a unique outsider and covers exactly `foreign artifact install ->
  list/query -> update --dry-run -> remove` on a supported host with the pinned
  release, where source and host native package formats differ. Record failed
  attempts and triage every report as `fix-now`, `next-slice`,
  `validated-no-action`, or `declined-with-reason` with an owner.
- **Limitations:** no qualifying completion for three weeks after launch
  triggers a maintainer review of venue reach, onboarding friction, and
  observed failures. A pivot still requires a reproducible systemic blocker;
  it cannot be inferred from low interest alone.
- **Non-goals:** automated posting, redefining partial attempts as completion,
  or broadening the supported scope to make the count easier.

## Post-Milestone Horizons

### Feedback-Driven Compatibility and Authority

- Repair the highest-impact adoption, resolution, scriptlet, or service
  friction shown by real testers.
- Expand public scriptlet authority only through formal package/helper
  contracts, positive target policy, and end-to-end preservation proof.
- Preserve fail-closed serving and explicit native-package-manager authority.
- Refresh supported-distro proof before adding breadth.

### Native Packaging and System-Building Completeness

- Deliver general package-building and static publishing workflows usable by
  third parties, then make bootstrap a consumer of that tooling.
- Close CCS v2 dependency-authoring, lock, and reproducibility gaps.
- Connect model, source selection, builder, and derivation inputs end to end.
- Improve self-hosting, recipes, groups, migration runbooks, and key/trust UX
  based on demonstrated users rather than speculative surface area.

### Kernel, Boot, and Security Outcomes

- Require fail-closed target-profile facts for public behavior.
- Add proof-backed native adapters rather than unverified policy exceptions.
- Make CCS v2 authority explicit across kernel, initramfs, bootloader, PAM, and
  LSM effects.
- Derive release validation from the owning routes and fixtures.
- Promote only target-profile rows backed by the proof corpus.

### Service and Operator Maturity

- Prove conaryd's exact root/daemon/configured-group authorization contract
  through packaged deployment and restart scenarios.
- Replace echo-style dry runs with real resolver plans and prove restart safety
  before adding destructive routes.
- Decide distribution and deployment support for Remi, conaryd, and the test
  harness; improve observability, migration, and recovery guidance.

### System Artifacts and Platform Breadth

- Keep x86_64 generation boot proof repeatable on maintained fixtures.
- Move boot activation to strict verity only when the kernel and image
  pipeline can prove it, and remove developer live-switch machinery once
  next-boot activation covers the maintained workflow.
- Add signed boot-artifact authority and portable generation-bundle trust.
- Make build and validation share one deterministic input-staging path, and
  use disposable overlays or snapshot mode so reruns are pristine by default.
- Derive raw, qcow2, ISO, OCI, and any future VMDK output from the canonical
  generation artifact instead of adding provider-specific package state.
- Add non-x86 architectures only with owned boot assets and proof.
- Add distros only after the existing three-distro path is repeatable.
- Model any remaining non-filesystem lifecycle effects as typed generation or
  external-transaction intents with exact retry and rollback semantics.

### Federation Runtime and Trust Closure

Federation remains experimental until one peer identity model is documented
and enforced in the TLS client, coordinator and fetch paths are wired into
Remi serving, admin changes reach runtime state, public chunk reachability is
defined, and a two-node failure matrix passes. Topology tuning and alternative
transports come later.

## Explicit Deferrals and Non-Goals

- W0 does not change Rust product behavior, dependency resolution, database
  schema, persisted state, CLI/API surface, release artifacts, branch history,
  or tester outreach state.
- PAM authority and network/package-manager recursion remain deliberately
  non-public until positive policy and proof exist. W0 does not turn them into
  public work by moving documentation.
- The generic evidence-free request to prune helper APIs is dropped. Concrete
  product slices own concrete removals with focused tests.
- conaryd fleet readiness, federation enablement, broad distro support,
  non-x86 generation support, and full production claims remain outside
  Milestone 1.
- rBuilder integration, `cvc` revival, original-lineage appliance groups, and
  specialized desktop package templates are not planned.

## Roadmap Maintenance and Closeout Rules

Update this roadmap when implementation truth changes, a workstream changes
state, proof is refreshed or becomes stale, a blocker is identified, or a
milestone transitions. Do not use it as a daily activity log.

Represent each active implementation slice with one primary GitHub issue and
land repository changes through an issue-linked pull request. The issue owns
bounded scope, acceptance criteria, current status, and follow-up work; this
roadmap owns ordering, cross-issue blockers, and milestone truth. A broad
workstream may span several issues and PRs, but the roadmap is not a substitute
for the actionable issue queue.

Record durable design decisions in the architecture, module, or `docs/specs/`
document that owns the affected surface. The primary issue and draft pull
request own bounded multi-step execution state; stable public or persisted
contracts live under `docs/specs/`. Planning must be proportional to risk and
tool-neutral.

Close an implementation slice only after it meets its acceptance condition,
enduring truth is canonical, roadmap and release state are current,
verification lives with its owner, and no active branch or handoff depends on
temporary planning material. Delete completed, superseded, or abandoned
planning from the current tree and use Git history when historical context is
needed. Do not create a replacement planning archive.

New work enters the ordered sequence only when it advances the current
milestone, resolves a safety or release blocker, or is explicitly accepted as
a post-milestone horizon. W0 closed after the authority rehearsal and
neutral-layout acceptance pass; W1 closed after authenticated v4 fixture
publication, public fresh-cache KVM proof, and final integrated release gates.
W2 closed after exact release identity, multi-distro artifact publication,
checksum and CCS-signature verification, production deployment, official
installed-binary self-update, compatible prewarmed Remi, rollback, and
clean-host proof. W3 is now active; its earlier release proof gate was reopened
for the supported `htop` SONAME repair and again for issue #41's path-safety
and support-bundle defects. That remediation gate is closed with verified
immutable `v0.12.0`. The 2026-07-20 through 2026-07-22 manual outreach window
passed without a post and is now retired. Rescheduling waits for GitHub Support
to dereference cached pre-rewrite pull-request and commit views and for the
venue-specific eligibility checks. No replacement date is assigned yet.
