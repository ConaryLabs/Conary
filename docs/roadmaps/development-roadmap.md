---
last_updated: 2026-09-03
revision: 36
summary: Track Conary's signed-universe launch gate, daily-driver floor, synchronized preview release, and external tester milestone
proof_baseline: "W7/#110 closed through PR #487; immutable v0.16.1 remains historical release evidence rather than tester authority; #598 owns first complete signed public universe; external tester result remains 0/10"
current_milestone: first external tester loop
active_workstream: W7.5 Signed Universe And Launch Gate
next_workstream: W8 External Tester Outreach
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

Bootable-image export is a retained generation capability, not a bootstrap
capability. Generation keeps raw/qcow2/ISO export with a UEFI QEMU boot proof,
and that proof now stands on a supported-host fixture: Groups O and P assemble
a Fedora 44 root from ordinary repository installs on a scratch disk, publish
it as a generation, export it, and boot the artifact. The bootstrap-run export
cases and their fixture are gone. The boot lane needs a host with `/dev/kvm`
and OVMF firmware; remi-dev is the current such host. The re-based suites are
green locally on the 2026-07-31 PR #151 implementation.

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
ordinary outreach difficulty does not satisfy the milestone. W8 and the
[machine-readable launch status](launch-status.json) currently record 0/10 for
the revised cross-distro flow. One external tester's two successful
adoption-led reports remain useful historical evidence but do not satisfy the
new source-format/host-format crossing requirement.

### Engineering precondition for outreach

W7's ordinary-package corpus proof passed through #110 and PR #487. Outreach
now waits on W7.5: one complete signed zero-exclusion public universe, the
pre-release daily-driver safety and usability floor, a synchronized release
whose clean-host journey uses only public artifacts, and recorded
performance/usefulness proof. Milestone `v0.17 Limited Preview — useful package
bridge` holds exactly ten launch deliverables. [Launch status](launch-status.json)
is their machine-readable current-state owner.

This ordering does not relax the 10-unique-tester exit condition. It prevents
first-impression attempts from testing a partial public catalog, a transaction
that discovers conflicts during mutation, or a client/server combination that
was never released together.

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
| Packaging, static repositories, trust, and self-update | solid | Immutable synchronized `v0.16.1` has an exact annotated tag, 15 checksum/digest-verified assets across four products, GitHub release attestation, detached CCS and bootstrap-manifest signatures, deployments, and native RPM/DEB/Arch lifecycle proof. No current artifact publishes an SBOM or additional provenance sidecar. |
| CCS conversion and native lifecycle authority | active hard switch | The exact RPM, Debian, and ALPM lifecycle contract is released and deployed; current source-backed format defects are explicit work in #98, #99, and #102 through #105 rather than manual-review authority. These share one root cause: the shared package model normalizes source facts at parse time instead of at each consumer's boundary. W5 owns the structural correction. |
| Generation build and export | limited | Bootable-image export (raw/qcow2/ISO plus a UEFI QEMU boot proof) is retained and proven from a supported-host fixture rather than from bootstrap. Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | active launch gate | The signed immutable-universe architecture is implemented through its production ceremony. #598 remains open because no complete Fedora/Ubuntu/Arch universe is active; public readiness correctly stays false. The v0.16.1 schema-40 deployment record is historical release evidence, not current production or tester authority. |
| conaryd package and query service | limited | Authorization is exact root/daemon/configured-group authority; restart semantics, resolver-backed dry-run proof, and deployment remain incomplete. |
| Federation | experimental | Only `/v1/federation/directory` is routed and no federation call exists in chunk serving, so the router is a library nothing invokes on a local miss. TLS fingerprint pinning is enforced in code; the documented disagreement is a docs defect. See the federation horizon for measured state and slice ordering. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | W7 passed. The cross-distro milestone remains 0/10, and outreach is gated on W7.5's signed universe, daily-driver floor, synchronized release, performance/usefulness proof, guided pilot, cached-history clearance, and venue eligibility. Release proof is not tester readiness. |

## Workstreams

### W6 Authority Audit Closure

- **Outcome:** the finite authority defects in #67 are closed as narrow owned
  slices with proof, and #67 becomes an audit epic rather than an
  implementation issue spanning twelve subsystems.
- **Execution status:** active. #109 closed the trigger-status slice; remaining
  ledger items proceed as narrow issues after the completed W4 aggregate gate.
- **Issues:** #67 as epic; #109 for persisted status; #257 and #259 for Remi
  acquisition integrity; #261 for exact publish-destination routing; #263 for
  required source-pin strength; #265 for typed try-session divergence; #267 for
  exact GRUB executable/output identity; and #269 for captured-root version
  authority.
- **Closed ledger items:**
  - PR #61 rejects missing package architecture, separates typed
    `InstallReason` from diagnostic selection prose, and validates trigger
    patterns before persistence and again on read.
  - #109 replaces trigger and derived-package status defaults with typed row
    corruption, validates every candidate row before mutation planning, and
    constrains the current schema. The current revision retains those
    revision-25 constraints.
  - #257 carries Remi retry disposition through typed acquisition failures;
    #259 stages, validates, and atomically publishes downloaded CCS files.
  - #261 parses CLI and packaging-MCP publish destinations through one exact
    route contract.
  - #263 requires an explicit typed dependency-mixing strength in every source
    pin and advances the current-only schema to revision 26.
  - #265 carries try-session divergence as a typed error through watch-mode
    context instead of classifying diagnostic prose.
- **Ledger gaps found on audit that #67 does not currently own:**
  - The `sanitize_path` ASCII heuristic is a #67-class invented authority but is
    owned only by #99, and only for ALPM. Cross-reference both.
- **Priority split within the ledger:** the publish-destination parser, Remi
  typed transport failures and retry disposition, and required source-pin
  strength are P1 and belong here. The `ccs/policy.rs`
  transformations are P2 and move to W9, because every `BuildPolicyConfig`
  toggle defaults to `false` and `ccs/convert/converter.rs` constructs the
  default, so stripping, shebang rewriting, and man-page compression never run
  on the foreign-package conversion path. They are opt-in build-recipe policy,
  not conversion authority.
- **Gate:** each closed ledger item records landing commit, typed owner,
  deleted authority, proof, and contract revision.

### W7.5 Signed Universe And Launch Gate

- **Outcome:** one complete signed Fedora/Ubuntu/Arch public universe is active;
  ordinary mutations meet the public-preview safety and usability floor; and
  one exact synchronized release completes the clean-host newcomer journey
  using only public artifacts.
- **Execution status:** active. #517 owns the immutable-universe architecture;
  #598 owns its first zero-exclusion production ceremony. #637 restored public
  ingress and prevents deploy-helper ownership drift. #638 must prove typed
  unavailability before activation and one-revision search/detail/index/stats
  agreement after activation. #639 owns the release cut and released journey.
- **Phase 0 -- ingress and truth:** #637's production remediation, protected
  helper deployment, and closeout are complete. Land #640 so W7 closure and
  current blockers are truthful; pursue the cached-history dereference and
  dated venue checks in parallel.
- **Phase 1 -- complete public universe:** close #606 and #619, record #605 and
  #614 production closeout, export exact native-oracle inputs through #601,
  prove #638's no-active-universe failure posture, then execute #598. Promotion
  requires every exact candidate variant to succeed with zero exclusions. A
  proof-derived partial preview universe would be a separate reviewed authority
  contract; it is not an alternate #598 readout or a post-failure goal change.
- **Phase 2 -- daily-driver floor:** #534 proves that a supported normal apply
  publishes its exact changeset before success or returns a typed recoverable
  partial outcome; #122 rejects complete materialization conflicts before any
  mutation; #718 keeps the basic package loop independent of composefs by
  selecting a verified materialized generation lower when the direct carrier
  is unavailable; and #132's first three ranked slices establish correct TTY
  progress, one warning/error voice, and one grouped transaction summary. #642 gives
  removal the same non-mutating plan/apply control as install and update. #643
  provides one read-only typed status result for release, repository,
  generation/publication, carrier, and database health.
- **Phase 3 -- synchronized release:** #639 cuts one exact tagged suite, deploys
  the matching Remi binary and schema, completes #39's endpoint/release proof,
  and re-proves the Fedora/Ubuntu/Arch journey. The installer invokes the native
  package transaction; package post-install owns initialization, and the journey
  verifies that result read-only rather than asking the user to rerun it.
- **Phase 4 -- proof and pilot preparation:** #121 records cold/warm baselines
  and work-amplification counters before #535 is considered; #149 records useful
  foreign-only workloads from the exact release; and contributor-facing #497
  follows the known non-root/umask hermeticity repairs rather than advertising
  a setup whose owning tests fail locally.
- **Parked before the preview:** #539, #538, #537, #536, #644, #69, #66, #65,
  #63, #64, #72, #70, #74, #50, #46, federation completion, and #272's
  full-system replacement proof do not enter this launch lane without a new
  issue-backed dependency justified by launch or tester evidence.
- **Gate:** #598 zero-exclusion evidence, #638 read-surface agreement, the
  #122/#534/#718/#132/#642/#643 floor, #639 released public-artifact proof, and
  #121/#149 launch proof are complete.
  The announcement claim remains the individual-package cross-distribution
  bridge recorded in `launch-status.json`; #272 owns any later full-system
  replacement claim.

### W8 External Tester Outreach

- **Outcome:** Conary has external evidence that strangers can install and
  remove a package whose source format differs from the supported host's native
  format.
- **Execution status:** gated on W7.5. Scope inherited from the former W3.
- **Issue:** #48.
- **Current truth:** no broad external venue post has been published, and the
  tracker remains 0/10. One organic external tester's former adoption-led
  Ubuntu and Fedora reports remain valid onboarding evidence but do not count
  toward the revised cross-distro milestone. The former 2026-07-20 through
  2026-07-22 outreach window passed without a post. Issues 37 and 38 remain two
  same-format onboarding attempts by one person and contribute zero qualifying
  completions. Issue #41's Artix host and Fedora-form source route remain
  outside the supported host proof and require exact source-selection,
  source-pin, and repository evidence before classification.
- **Dependencies:** W7.5's product gate assigns the exact tester release and
  opens the guided pilot. GitHub Support must dereference cached pre-rewrite
  pull-request/commit views, and the maintainer must re-check each venue's
  current account/rule eligibility, before broad submission rather than before
  direct guided participants.
- **Next gate:** after W7.5 passes, recruit the first five unaffiliated
  participants as a staggered guided pilot;
  each qualifying completion counts toward the same 10-person milestone. Track
  time to first transaction, intervention count, requested workload, rollback
  understanding, and seven-day reuse. By the fifth participant, the target is
  zero live maintainer intervention. Only after that pilot and the separate
  cached-history/venue checks, submit the refreshed broad-venue packet, record
  each actual URL and timestamp, and continue toward ten.
- **Evidence rule:** each completion belongs to a unique outsider and covers
  exactly `foreign artifact install -> list/query -> update --dry-run ->
  remove` on a supported host where source and host formats differ. Every
  reported failure is recorded as a user-journey stage failure using W7's typed
  vocabulary, not as a free-form feedback category.
- **Limitations:** no qualifying completion for three weeks after launch
  triggers a maintainer review of venue reach, onboarding friction, and
  observed failures. A pivot still requires a reproducible systemic blocker; it
  cannot be inferred from low interest alone.
- **Non-goals:** automated posting, redefining partial attempts as completion,
  or broadening the supported scope to make the count easier.

## Launch Funnel And Issue Map

This is an ordering index, not a perpetual enumeration of every open issue.
GitHub milestones and issue links own per-issue disposition; the roadmap owns
workstream order and cross-issue gates. The `v0.17 Limited Preview — useful
package bridge` milestone is deliberately capped at ten user-visible
deliverables. Its dependency issues remain attached to their owning epic or
ceremony rather than inflating the launch milestone. Anything outside the
launch lane is post-preview, blocked by user evidence, or parked in a named
horizon unless an issue-backed roadmap change says otherwise.

| Workstream | Issues | Priority |
| --- | --- | --- |
| W0 Neutral Planning Migration | Completed; durable routing lives in feature cards and `scripts/agent-context.sh` | complete |
| W1 Integrated Release-Green Baseline | Completed integrated baseline consumed by later workstreams | complete |
| W2 Preview Release and Remi Readiness | Completed; release evidence lives in `docs/operations/release-artifact-matrix.md` | complete |
| W3 Post-Hard-Cut Release Gate | Completed synchronized `v0.16.1` release proof; outreach moved to W8 | complete |
| W4 Source Fidelity Hard Cut | #102, #103, #98, #99 (slices 99a-99c), #107 | complete |
| W5 Source Authority Model | #108 specification, #104, #105 | complete |
| W6 Authority Audit Closure | #67 epic, #109, plus narrow ledger slices | P1 |
| W7 Just-Works Corpus Gate | #110 closed in PR #487; #39 protocol in PR #486 | complete |
| W7.5 Signed Universe And Launch Gate | milestone deliverables #598, #638, #122, #534, #132, #642, #643, #639, #121, #149; dependencies #517, #601, #605, #606, #614, #619; completed ingress #637; truth/disposition #640; contributor follow-up #497 | P0/P1 |
| W8 External Tester Outreach | #48 | gated |
| W9 Common Package Capability Classes | #74, #50, #46, #67 P2 remainder | P1 |
| W10 Distro-Agnostic Takeover | #62 epic decomposed, #68 | P2 |
| Native packaging horizon | #70, #51, #72 | P2 |
| Service and operator horizon | #69, #65, #66, #73, #644 (remaining #132 presentation slices) | P2 |
| System artifacts horizon | #63, #64, #71 | P2 |
| Closed | #41 closed 2026-07-31 after the selected-root alias regression repair and current Fedora export/boot proof; original Artix route tracked as W8 evidence | not engineering work |

### Known overlaps

These issues intersect. Each pair below is delineated rather than merged,
because merging would produce an implementation pull request spanning several
authority owners.

- **#98, #104, and #105 share one root cause.** All three exist because the
  shared package model normalizes source facts at parse time. They keep
  separate pinned fixtures and acceptance criteria, but W5's specification
  governs all three so the corrections converge on one target model instead of
  three local patches.
- **#99 and #103 both need declared budgets.** #103 already records the
  boundary: #99 owns ALPM archive resource bounds, #103 owns the generated CCS
  authority format. The remaining constraint is that they share one
  declared-budget owner type rather than each inventing a budget model.
- **#99 and #67 both touch invented authority.** The `sanitize_path` ASCII
  heuristic is a #67-class defect, but only #99 owns it, and only for ALPM.
  #99's path slice is widened to cover RPM and Debian, and #67 cross-references
  it rather than duplicating the work.
- **#67 and #62 already delegate.** #67's final ledger item hands the fixed
  three-entry repository feed catalog to #62. The `source_profile` schema
  `CHECK` constraint belongs with that handoff.
- **#67 and #66 already coordinate.** The former conary-test MCP dual mutation
  authority item was retired with the Forge server cut in #351; current live
  MCP adapter ownership is Remi's.
- **#62, #68, and #71 form one ecosystem cluster.** #68 depends on stable
  source and repository authority from #62; #71 adds a fourth source ABI and
  waits for both.
- **#65 and #66 both target fleet-scale control.** #65 is the replatforming
  operation, #66 the typed gateway that would expose it. #66's local typed APIs
  stabilize first.
- **#41 overlaps #99's path authority but is closed engineering work.** The
  original archive-path repair shipped in v0.12.0, but current Fedora
  supported-host proof exposed the same property in the live-root
  materializer. The 2026-07-31 repair reuses the bounded selected-root resolver
  for install, remove, rollback, and hardlink paths while retaining escape and
  loop rejection. The original Artix host remains outside supported scope and
  is tracked as W8 evidence.

## Post-Milestone Horizons

The first two horizons below are ordered workstreams with owning issues. The
remainder are thematic horizons that admit work only when it advances the
current milestone, resolves a safety or release blocker, or is explicitly
accepted here.

### W9 Common Package Capability Classes

- **Outcome:** package classes that require a substantive compatibility service
  rather than a parser correction complete the same user journey.
- **Execution status:** gated on W8 launch. Ordered after outreach begins, not before.
- **Issues:** #74, #50, #46, and the P2 remainder of #67.
- **Ordering and rationale:**
  - #74 `rpmlib(ConcurrentAccess)` is an ecosystem-level runtime capability, not
    a package exception. The design is a transaction-local read-only query
    service populated from Conary's typed transaction state, exposed whenever
    the package declares the typed capability. Scripts are never inspected to
    guess what they might query, and nested mutation remains a separate rejected
    capability.
  - #50 package-backed SELinux lifecycle stays a target-provider capability with
    typed install, remove, and reconcile operations. Provider executable,
    grammar, digest, and operating mode come from the target capability
    inventory, never from a package or distro list.
  - #46 narrow initramfs regeneration is boot critical and stays behind explicit
    target and VM proof. The persisted operation is a typed regeneration intent
    that an Ubuntu-capable provider lowers to its exact implementation; the
    observed literal command is evidence for deriving the source semantic, not
    the universal operation.
  - The `ccs/policy.rs` transformations deferred from W6 are hard cut here. ELF
    stripping requires a declared digest-pinned build-tool capability or is
    disabled; shared-object identity comes from parsed ELF type and dynamic
    metadata rather than filename; shebang rewriting parses the exact grammar
    with exact interpreter mappings; man-page compression requires a declared
    package role; a transformation failure fails the build unless the recipe
    marked it optional; and the output attestation records the transformation
    implementation with input and output digests.
- **Also in scope:** a fuzz and property-testing lane, which the workspace does
  not currently have. Targets are RPM dependency expressions, ALPM relation
  grammar, PAX paths and xattrs, CCS section budgets and canonical encoding,
  repository declaration parsers, and URL and route parsing. Parser correctness
  is too central to rest on curated fixtures alone.

### W10 Distro-Agnostic Takeover

- **Outcome:** Conary enrolls native repositories transactionally and supports
  package ecosystems without a distro-name routing matrix.
- **Execution status:** #62 is tracked as bounded child slices. #377 and #379
  through #383 are merged. #68 provides exact adopted-artifact conversion;
  #384 completed the synthetic derivative and rolling-distro model boundary.
  #396 owns authentic Linux Mint and Pop!_OS release-root execution and evidence.
- **Issues:** #62 as epic; #377 and #379-#384 as completed children; #396 as
  authentic derivative execution; #68 follows.
- **Decomposition:**
  - Lossless native repository declarations, with separate parsers and models
    for APT, RPM/DNF, Zypper, and ALPM. No generic repository-config model until
    source evidence is preserved.
  - Trust-import planning that yields a typed importable, ambiguous, or
    unsupported disposition, with exact unresolved authority shown before
    mutation. Key roles are never guessed from filenames or URLs.
  - Follow-and-pin policy modeled per repository or repository group rather than
    at a global distro level.
  - Transactional repository enrollment, where package-installed repository
    files and keys produce typed enrollment operations participating in install,
    update, remove, and rollback. A completed filesystem is never scraped for
    files that resemble repositories.
  - Native repository files become projections of Conary-owned state, with drift
    detection instead of two mutation authorities.
  - Conformance targets added one at a time: a Debian derivative, an Arch
    derivative, openSUSE Tumbleweed as an independent RPM ecosystem, and Artix
    as a non-systemd target-capability test. Named distributions are conformance
    proof, not runtime selectors.
- **Schema prerequisite:** #380 replaced fixed `source_profile` membership with
  normalized source policy, distinct repository identity, typed ecosystem and
  version ordering, immutable stream binding, and authenticated follow-or-pin
  snapshot state in binding revision 31. #381 advances the current database
  schema to revision 32 for takeover and projection ownership. #382 advances
  the current database schema to revision 33 for package and retained
  enrollment ownership. #383 advances it to revision 34, removes global
  distro-pin and allowlist state, and keys diagnostic affinity by source
  identity. #68 advances it to revision 35 for exact installed-artifact
  architecture authority, and #384 advances it to revision 36 for the distinct
  captured-OpenRC activation source kind. #380 removed the
  inline three-profile checks from repository and repository-package identity
  without growing a catalog. Target capability compatibility remains owned by #383.
  Pre-alpha rebuild and authoritative-input re-enrollment are explicit; no
  compatibility migration is retained.

### Feedback-Driven Compatibility and Authority

- Repair the highest-impact adoption, resolution, scriptlet, or service
  friction shown by real testers.
- Expand public scriptlet authority only through formal package/helper
  contracts, positive target policy, and end-to-end preservation proof.
- Preserve fail-closed serving and explicit native-package-manager authority.
- Refresh supported-distro proof before adding breadth.

### Native Packaging and System-Building Completeness

Owning issues: #70 maintainer adoption kit, #51 pristine self-host validation
reruns, #72 composable system models.

- Deliver general package-building and static publishing workflows usable by
  third parties, then make bootstrap a consumer of that tooling.
- Close CCS v3 dependency-authoring, lock, and reproducibility gaps.
- Connect model, source selection, builder, and derivation inputs end to end.
- Improve self-hosting, recipes, groups, migration runbooks, and key/trust UX
  based on demonstrated users rather than speculative surface area.
- #70 depends on the W10 contracts being settled; publishing a maintainer kit
  against contracts that are about to change spends third-party goodwill twice.

### Kernel, Boot, and Security Outcomes

The two package-authority slices in this area, #46 initramfs regeneration and
#50 SELinux module lifecycle, are promoted into W9 because they block ordinary
package classes. What remains here is the broader boot and security surface.

- Require fail-closed target-profile facts for public behavior.
- Add proof-backed native adapters rather than unverified policy exceptions.
- Make CCS v3 authority explicit across kernel, initramfs, bootloader, PAM, and
  LSM effects.
- Derive release validation from the owning routes and fixtures.
- Promote only target-profile rows backed by the proof corpus.

### Service and Operator Maturity

Owning issues: #69 desktop package backend, #65 fleet replatforming, #66 agent
control-plane gateway, #73 agent-assisted contribution.

- Prove conaryd's exact root/daemon/configured-group authorization contract
  through packaged deployment and restart scenarios.
- Replace echo-style dry runs with real resolver plans and prove restart safety
  before adding destructive routes.
- Decide distribution and deployment support for Remi, conaryd, and the test
  harness; improve observability, migration, and recovery guidance.
- #69, #65, and #66 all consume the daemon and operation contracts, so they
  follow conaryd's authorization and dry-run proof rather than running beside
  it. #65 additionally depends on #63. The former #67 conary-test MCP ledger
  item was retired with the Forge server cut in #351.

### System Artifacts and Platform Breadth

Owning issues: #63 declarative replatforming, #64 bootable installer, #51
deterministic input staging, #71 eopkg and Solus support.

- #71 adds a fourth source ABI and does not enter before W10's conformance
  slices prove the existing three are distro-agnostic.
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

Measured state on 2026-07-27, so the first slice is scoped against facts rather
than against the "experimental" label. `apps/remi/src/federation/` carries
circuit breaking, request coalescing, config, manifest, peer identity, and a
router across 72 tests, and TLS fingerprint pinning is genuinely enforced:
HTTPS peers require a pinned fingerprint and HTTP peers derive identity from a
URL hash. The documentation-versus-enforcement disagreement above is therefore
a docs defect, not absent code. What is missing is the serving wiring. Only
`/v1/federation/directory` is routed, and no federation call exists anywhere in
chunk serving, so the router is a library nothing invokes on a local miss.

Two facts constrain the first slice. Most modules were last substantively
touched on 2026-04-01, and the commits since were repo-wide sweeps rather than
federation review, so the code has not been examined against the lifecycle hard
cut, current CCS authority, or schema revision 21. Passing tests are not
evidence that those assumptions still hold. Separately,
`apps/remi/src/federation/mod.rs` is already over the 1,000-line planning gate,
so any slice adding behavior there must carry an ownership refactor in the same
change; that cost belongs in the plan rather than being discovered mid-slice.

The resulting order is review and decompose, then wire fetch into serving, then
prove the two-node failure matrix. Only the last step needs a second host, so
acquiring one does not advance the first two.

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
clean-host proof. W3's release proof gate was reopened for the supported `htop`
SONAME repair and again for issue #41's path-safety and support-bundle defects,
then superseded by the post-hard-cut package authority suite. The latest exact
immutable release evidence is synchronized `v0.16.1` at
`0fb961bacc6360107506371b16b7f0345ba6f927`. It remains historical release
authority, not current production or tester authority: the signed-universe
architecture advanced to schema 55 after that tag. #639 will establish the
next exact release/deployment proof. The tester pin remains unassigned and the
external milestone remains 0/10.

W3 was subsequently split. Its release gate is complete, and its external
tester outreach moved to W8 behind an engineering gate. W4 through W7 are now
complete. W7.5 is active because the immutable-universe architecture, current
daily-driver floor, and post-universe synchronized release must become one
publicly reproducible product path before outreach.

The 2026-07-20 through 2026-07-22 manual outreach window passed without a post
and is retired. Rescheduling now waits on W7.5, GitHub Support dereferencing
cached pre-rewrite pull-request and commit views, and the venue-specific
eligibility checks. No replacement date is assigned.
