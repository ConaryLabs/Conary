---
last_updated: 2026-08-15
revision: 21
summary: Track Conary's active W7 just-works corpus gate, typed semantic coverage, external tester milestone, and later workstreams
proof_baseline: "immutable synchronized v0.15.0 suite at 642750878d5a59a9aa27976347cafc6f9dd86cfd; exact tagged Remi deployed; external tester result remains 0/10 behind #110"
current_milestone: first external tester loop
active_workstream: W7 Just-Works Corpus Gate
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

Remote Forge validation and conary-test deployment are decommissioned. The dated
2026-05-21 Group O local QEMU run established the earlier export baseline. The
dated 2026-07-16 Group O local KVM run superseded it by passing all five
installed-runtime, file-capability, and bootstrap-run raw/qcow2 cases against
`minimal-boot-v4`. That run predates the #61 schema epoch cut, and the
`minimal-boot-*` lineage cannot run the current build at all: those images ship
a bootstrap-built Conary database at a retired schema. The suites now target
`fedora44-guest-v2`, an official Fedora Cloud Base 44 image built by
`scripts/build-qemu-guest-image.sh` per #153's re-base decision. Its immutable
public artifact is pinned at
`f688ac2a02b0b0558e28de1c97bbcb2e45b6772a4f019b037f72ec584a420174`
and includes Fedora's packaged systemd-boot EFI binary.
The dated 2026-07-31 Group O implementation proof passed all five qcow2 cases
against that fixture, including ordinary signed Remi CCS assembly and
selected-generation publication. The dated 2026-07-31 Group P implementation
proof passed its ISO case, including provenance, copy-back, read-only carrier
boot, and writable `/etc` overlay proof. This is local x86_64 evidence, not a
broad remote-validation claim.

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
ordinary outreach difficulty does not satisfy the milestone. The active
[milestone tracker](external-tester-milestone.md) currently records 0/10 for
the revised cross-distro flow. One external tester's two successful
adoption-led reports remain useful historical evidence but do not satisfy the
new source-format/host-format crossing requirement.

### Engineering precondition for outreach

Outreach is gated on the corpus proof in W7, not merely on venue scheduling.
Ordinary Fedora and Arch repository packages currently fail conversion in
classes owned by #98, #99, and #102 through #105. A tester asking for
`ansible`, `btop`, `aisleriot`, `NetworkManager`, or `bash-completion` reaches
a known typed failure, so outreach conducted before W7 spends scarce
first-impression attempts reproducing defects the tracker already owns.

W4 through W7 therefore precede the outreach half of the milestone. This does
not relax the 10-unique-tester exit condition; it orders the work so each
attempt tests onboarding rather than parser fidelity.

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
| Packaging, static repositories, trust, and self-update | solid | Immutable synchronized `v0.15.0` has an exact annotated tag, 13 checksum/digest-verified assets across four products, GitHub release attestation, detached CCS signature, self-update endpoint, deployments, and native RPM/DEB/Arch lifecycle proof. No current artifact publishes an SBOM or additional provenance sidecar. |
| CCS conversion and native lifecycle authority | active hard switch | The exact RPM, Debian, and ALPM lifecycle contract is released and deployed; current source-backed format defects are explicit work in #98, #99, and #102 through #105 rather than manual-review authority. These share one root cause: the shared package model normalizes source facts at parse time instead of at each consumer's boundary. W5 owns the structural correction. |
| Generation build and export | limited | Bootable-image export (raw/qcow2/ISO plus a UEFI QEMU boot proof) is retained and proven from a supported-host fixture rather than from bootstrap; the re-based Group O/P suites passed locally on 2026-07-31. Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | Exact tagged `remi 0.15.0` is deployed at schema revision 37 with 6/6 populated sources, four exact signing profiles, 110,220 repository packages, 1,798 conversions, and full health 10/10. Its binary hash matches the immutable release asset. Distribution and a wider stranger-operated path remain limited. |
| conaryd package and query service | limited | Authorization is exact root/daemon/configured-group authority; restart semantics, resolver-backed dry-run proof, and deployment remain incomplete. |
| Federation | experimental | Only `/v1/federation/directory` is routed and no federation call exists in chunk serving, so the router is a library nothing invokes on a local miss. TLS fingerprint pinning is enforced in code; the documented disagreement is a docs defect. See the federation horizon for measured state and slice ordering. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | The synchronized `v0.15.0` released-package matrix is complete. The revised cross-distro milestone remains 0/10, and outreach is separately gated on W7/#110 corpus proof, cached-history clearance, and venue eligibility. Release proof is not tester readiness. |

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

### W3 Post-Hard-Cut Release Gate

- **Outcome:** complete. Conary publishes an immutable, signed, deployed
  release whose RPM, DEB, and Arch packages each pass the Cartesian
  source-format lifecycle on their matching supported host.
- **Scope change:** W3 originally bundled this release gate with external
  tester outreach. The release half is closed; the outreach half moved to W8
  behind the W4 through W7 engineering gate, because the release proof covers
  a curated lifecycle rather than ordinary repository packages. Outreach state,
  venue eligibility, and the 0/10 tracker are now W8's.
- **Current truth:** the latest immutable release is synchronized suite
  `v0.15.0`. Its released RPM, DEB, and Arch packages passed the Cartesian
  source-format lifecycle on Fedora 44, Ubuntu 26.04 LTS, and Arch. Remi runs
  the exact tagged suite binary; conaryd and conary-test are checksum-verified
  build-only artifacts. This is current release authority, not pinned tester
  authority. W7/#110 owns the ordinary-package corpus gate.
- **Execution status:** complete for the release, deployment, and
  published-package proof it owns.
- **Dependencies:** none remaining.
- **Proof:** annotated tag object
  `83ef2d8a264cb49c5deb9e79e2a84a20e6883dab` peels to reviewed merge commit
  `642750878d5a59a9aa27976347cafc6f9dd86cfd`. Exact-tag release-build
  `31766900566` published the immutable 13-asset release on 2026-08-13 PDT;
  every asset passed checksum, GitHub digest, and immutable-attestation
  verification. Protected deployment `31769739765` passed both deployment
  routes, both build-only routes, and all three native-package lifecycle lanes.
  Exact hashes and complete per-product evidence live in the release matrix.
  The CCS has a detached signature; no product publishes an SBOM or additional
  provenance sidecar.
- **Current deployment evidence:** independent proof reports installed
  `remi 0.15.0` with the exact release hash, schema revision 37, 6/6 populated
  sources, four signing profiles, 110,220 repository packages, 1,798
  conversions, and full health 10/10. The self-update endpoint serves the exact
  released `conary-0.15.0.ccs`.
- **Limitations:** the release proof does not establish that ordinary
  repository packages convert. W4 through W7 own that claim.

### W4 Source Fidelity Hard Cut

- **Outcome:** ordinary Fedora and Arch repository packages stop failing
  conversion on Conary-invented invariants, and hosted Remi health becomes
  evidence bearing.
- **Execution status:** complete. The owned #98, #99, #102, #103, and #107
  slices are closed. W7's landed typed corpus vocabulary reports their
  aggregate evidence without message-text authority.
- **Issues:** #102 (P0), #103 (P0), #98 (P0), #99 (P0, split into three
  slices), plus #107 for the Remi readiness probe.
- **Ordering:** #102, #103, and #107 run in parallel with #98
  and the #99 slices. They touch disjoint owners: Arch trust, CCS container
  authority, Remi serving, RPM payload projection, and ALPM payload
  projection.
- **Cross-workstream dependency:** #110's typed corpus result schema lands
  before the W4 slices close, so each slice reports against it. This is a
  deliberate exception to the workstream ordering. Retrofitting typed results
  onto six finished fixes costs more than defining the schema once up front,
  and without it the W4 gate has no vocabulary to report in other than message
  text. A small piece of W7 therefore starts during W4.
- **Scope corrections applied to the existing issues:**
  - #99's path slice widens beyond ALPM. `crates/conary-core/src/filesystem/path.rs`
    rejects non-ASCII paths, and `packages/archive_utils.rs` routes RPM, Debian,
    and ALPM parsing through it, so valid non-ASCII UTF-8 paths in RPM and DEB
    packages are also rejected as traversal. The shared authority is fixed once
    and proved with fixtures from all three source formats.
  - #99 splits into three independently owned slices under one parent: lossless
    source and deployment paths, the exact libarchive xattr grammar, and
    declared archive/spool resource budgeting. They share a parent epic, not a
    single implementation pull request.
  - #99's resource-budget slice and #103 must share one declared-budget owner
    type. #103 already delineates ownership; the constraint here is that the two
    slices do not each invent a budget model.
- **Gate:** a bounded hosted Remi prewarm sample advances past the RPM
  hardlink, ALPM path, libarchive xattr, payload-size, Arch trust, and CCS
  authority-size failure classes with no package-specific handling, and
  `/health/ready` fails closed on an absent database or a failed disk probe.

### W5 Source Authority Model

- **Outcome:** source package facts are preserved in their native ontology and
  normalized at each consumer's boundary, so the class of defect behind #98,
  #99, #104, and #105 cannot recur.
- **Execution status:** complete. #108 owns the specification; #104 and #105
  shipped the identity/provision and configuration halves as one current-only
  CCS v3/schema-24 hard cut.
- **Issues:** #108, #104, and #105.
- **Rationale:** `PackageMetadata` and the flat `ProvidedCapability` and
  `ConfigFileInfo` shapes assume that similarly named facts across ecosystems
  share one ontology. They do not. RPM hardlink sets, ALPM backup declarations,
  and source-declared same-name provisions each carry source-specific semantics
  that the shared model erases at parse time.
- **Design rule:** normalization belongs at the boundary of a specific
  consumer, not at initial parse time. Each native parser produces a lossless
  source-specific authority model; explicit fallible projections serve
  resolution, CCS authoring, and native transaction planning.
- **Ordering:** the specification lands before #104 and #105 so both slices
  delete the ambiguous representation against one agreed target rather than
  two. #98 informs the specification but is not blocked by it.
- **Gate:** the pinned ASP.NET and `bash-completion` fixtures convert, sign,
  verify, and install with no special cases, and the old ambiguous projections
  are deleted rather than aliased.

### W6 Authority Audit Closure

- **Outcome:** the finite authority defects in #67 are closed as narrow owned
  slices with proof, and #67 becomes an audit epic rather than an
  implementation issue spanning twelve subsystems.
- **Execution status:** active. #109 closes the trigger-status slice; remaining
  ledger items proceed as narrow issues while the W4 aggregate gate remains
  active.
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

### W7 Just-Works Corpus Gate

- **Outcome:** a clean machine on each supported host completes the ordinary
  user journey end to end, and failures are reported as typed stages rather
  than message text.
- **Execution status:** active after W4 through W6 completion. Typed per-case
  reporting is landed; #456 owns the next bounded semantic-coverage authority
  before the remaining fixture and hosted-execution slices.
- **Issues:** #110 as the corpus umbrella, #456 for attributable semantic
  coverage, plus #39 for bootstrap.
- **User journey:** install the signed release through one documented bootstrap
  entry point; `conary system init` with no hand-edited state; sync and
  authenticate repositories; request a package by ordinary name; resolve the
  complete transaction; obtain or produce a signed CCS artifact through Remi;
  reopen and verify it; preview the exact transaction; install with no manual
  script approval or package-specific exception; query installed identity,
  provenance, files, and reason; update; remove; roll back; and leave package
  database, filesystem, generation state, and activation debt mutually
  consistent after forced failures.
- **Matrix:** the existing Cartesian principle holds. RPM, DEB, and ALPM source
  artifacts each against Fedora 44, Ubuntu 26.04 LTS, and Arch targets.
- **Corpus selection:** by typed semantic dimension, not by popular package
  name. Identity, payload, metadata, relations, configuration, lifecycle,
  trust, runtime, and failure each require declared coverage, and every fixture
  is selected for its recorded properties.
- **Typed result:** the runner emits a per-case record carrying source profile,
  role-tagged source artifact identities for install and update, the typed
  authority that produced each digest, target capability snapshot, per-stage
  results, and a typed outcome. Declared and emitted case counts must agree.
  Failures aggregate by stage and failure enum. Aggregating by
  error-message text is prohibited. Remi's current prewarm result keeps failed
  identity plus a string error, which is adequate for logs and inadequate as
  roadmap authority.
- **Bootstrap:** #39 completes as a release-owned protocol, not a wrapper
  around internal steps. Discover supported host facts, fetch signed release
  metadata, select the exact platform artifact, verify digest and signature,
  install atomically, initialize current-profile repository authority, and run
  a non-mutating health proof.
- **Gate:** the full three-by-three corpus passes through hosted Remi with zero
  package-specific exceptions, and remote KVM validation is restored.

### W8 External Tester Outreach

- **Outcome:** Conary has external evidence that strangers can install and
  remove a package whose source format differs from the supported host's native
  format.
- **Execution status:** gated on W7. Scope inherited from the former W3.
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
- **Dependencies:** W7's corpus gate; GitHub Support must dereference the
  cached pull-request and commit views that still expose pre-rewrite history;
  and the maintainer must re-check each venue's current account and rule
  eligibility immediately before submission.
- **Next gate:** after W7 passes, GitHub Support confirms the cached history is
  no longer reachable, and venue checks pass, assign a new staggered schedule.
  Then submit the refreshed Show HN packet, record its actual URL and
  timestamp, and collect privacy-safe reports toward the first unique
  qualifying tester.
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

## Issue Map

Every open issue has exactly one owning workstream. The roadmap owns ordering,
cross-issue blockers, and milestone truth; each issue owns bounded scope,
acceptance criteria, and proof.

| Workstream | Issues | Priority |
| --- | --- | --- |
| W4 Source Fidelity Hard Cut | #102, #103, #98, #99 (slices 99a-99c), #107 | P0 |
| W5 Source Authority Model | #108 specification, #104, #105 | P0 |
| W6 Authority Audit Closure | #67 epic, #109, plus narrow ledger slices | P1 |
| W7 Just-Works Corpus Gate | #110 umbrella, #39 | P0 |
| W8 External Tester Outreach | #48 | gated |
| W9 Common Package Capability Classes | #74, #50, #46, #67 P2 remainder | P1 |
| W10 Distro-Agnostic Takeover | #62 epic decomposed, #68 | P2 |
| Native packaging horizon | #70, #51, #72 | P2 |
| Service and operator horizon | #69, #65, #66, #73 | P2 |
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
- **Execution status:** gated on W7. Ordered after outreach begins, not before.
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
immutable release evidence is synchronized `v0.15.0` at
`642750878d5a59a9aa27976347cafc6f9dd86cfd`; current Remi deployment authority
is the exact tagged `remi 0.15.0` binary. This release authority remains
separate from the unassigned tester pin and 0/10 external milestone.

W3 was subsequently split. Its release gate is complete, and its external
tester outreach moved to W8 behind an engineering gate, because a bounded
hosted prewarm sample showed that ordinary Fedora and Arch repository packages
still fail conversion in classes owned by #98, #99, and #102 through #105.
Outreach against those defects would spend first-impression attempts
reproducing known typed failures. W4 is now active.

The 2026-07-20 through 2026-07-22 manual outreach window passed without a post
and is retired. Rescheduling now waits on the W7 corpus gate in addition to
GitHub Support dereferencing cached pre-rewrite pull-request and commit views
and the venue-specific eligibility checks. No replacement date is assigned.
