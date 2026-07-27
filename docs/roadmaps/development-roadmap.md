---
last_updated: 2026-07-27
revision: 5
summary: Track Conary's cross-distro package milestone, ordered workstreams, evidence, blockers, and post-milestone horizons
proof_baseline: "v0.13.0 and remi-v0.8.5 at 6f1429c362ac161f1ef817233e72ee9c9a031c11; post-hard-cut release, deployment, and three-distro artifact proof complete"
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
| Packaging, static repositories, trust, and self-update | solid | `v0.13.0` has exact tag, hashes, detached signature, current signed self-update, deployment, and native RPM/DEB/Arch installed-binary proof. The intentional 0.12 schema boundary requires a fresh native-package install. No SBOM/provenance sidecars are published or planned. |
| CCS conversion and native lifecycle authority | active hard switch | The exact RPM, Debian, and ALPM lifecycle contract is released and deployed; current source-backed format defects are explicit work in #98, #99, and #102 through #105 rather than manual-review authority. |
| Generation build and export | limited | Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | `remi-v0.8.5` is deployed on the current schema with five populated sources, exact signing profiles, fair prewarm, real conversions, and full public health; distribution and a wider stranger-operated path remain limited. |
| conaryd package and query service | limited | Authorization is exact root/daemon/configured-group authority; restart semantics, resolver-backed dry-run proof, and deployment remain incomplete. |
| Federation | experimental | Coordinator and fetch paths are not wired into serving; TLS identity documentation and enforcement do not yet agree. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | The `v0.13.0` released-package matrix is complete, but the revised cross-distro milestone remains 0/10. The former 2026-07-20 through 2026-07-22 outreach slots passed without posts; rescheduling still requires cached-history and current venue-eligibility clearance. |

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
- **Current truth:** the post-hard-cut release gate is complete at immutable
  `v0.13.0` with compatible production `remi-v0.8.5`. The released RPM, DEB,
  and Arch packages passed the Cartesian source-format lifecycle on Fedora 44,
  Ubuntu 26.04 LTS, and Arch. No broad external venue post has been published.
  One organic external tester's former adoption-led Ubuntu and Fedora reports
  remain valid onboarding evidence but do not count toward the revised
  cross-distro milestone, so the tracker remains 0/10. The former 2026-07-20
  through 2026-07-22 outreach window passed without a post. Issue #41's Artix
  host and Fedora-form source route remain outside the supported host proof and
  require exact source-selection, source-pin, and repository evidence before
  classification.
- **Execution status:** active; release, deployment, and published-package
  proof are complete, while broad outreach and replacement dates remain
  postponed.
- **Dependencies:** GitHub Support must dereference the cached pull-request and
  commit views that still expose pre-rewrite history, and the maintainer must
  re-check each venue's current account/rule eligibility immediately before
  submission.
- **Next gate:** after GitHub Support confirms the cached history is no longer
  reachable and venue checks pass, assign a new staggered schedule. Then submit
  the refreshed Show HN packet, record its actual URL and timestamp, and
  collect privacy-safe reports toward the first unique qualifying
  tester.
- **Proof:** immutable `v0.13.0` published at `2026-07-27T11:57:44Z`;
  annotated tag object `f8298522fd7fe95a4994184ae20c34cf64096818`
  peels to `6f1429c362ac161f1ef817233e72ee9c9a031c11`. Exact-tag
  release-build `30261256730` passed the workspace and four package builders.
  Independent downloads found exactly seven assets, matched all five
  `SHA256SUMS` entries and every GitHub digest, and verified the detached CCS
  signature with the official preceding release key. A copied released 0.13
  binary forced a signed update through the production channel and remained
  `conary 0.13.0`; the incompatible 0.12 CCS parser is an intentional hard-cut
  reinstall boundary, not a compatibility path. Deploy-and-proof run
  `30263948968` passed: the server's exact seven files match the release,
  conary.io serves the exact tag and a branded HTTP 404, and native RPM, DEB,
  and Arch packages each installed and passed the owned Cartesian lifecycle on
  their matching supported host. Current `remi-v0.8.5` tag object
  `1d9b8588fe01453bab20f1a7956e4aa9d6263702` peels to the same commit;
  release-build `30259180128` and protected deploy `30260847616` passed. Live
  inspection reports schema revision 21, five populated sources, 98,266
  repository packages, exact Arch/Fedora/Ubuntu signing profiles, 2,368
  conversions, full health 10/10, and public converted CCS proof. conaryd
  0.7.0 and conary-test 0.9.0 remain immutable build-only products from
  `a231276a900bbe8a8ccb6a0942f104cba2ab86b4`. Exact hashes and complete
  per-product evidence live in the release matrix. No SBOM or provenance
  sidecars are published. The known conversion failures are owned by #98,
  #99, and #102 through #105; the false-success unknown API fallback is owned
  by #67 rather than hidden as release success. Issues 37 and 38 remain two
  same-format onboarding attempts by one person and contribute zero qualifying
  completions. Each future completion belongs to a unique outsider and covers
  exactly `foreign artifact install -> list/query -> update --dry-run ->
  remove` on a supported host where source and host formats differ.
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
and support-bundle defects, then superseded by the post-hard-cut package
authority suite. That release gate is closed with verified immutable
`v0.13.0` and production `remi-v0.8.5`. The 2026-07-20 through 2026-07-22
manual outreach window passed without a post and is now retired. Rescheduling
waits for GitHub Support to dereference cached pre-rewrite pull-request and
commit views and for the venue-specific eligibility checks. No replacement
date is assigned yet.
