---
last_updated: 2026-07-28
revision: 8
summary: Track Conary's cross-distro package milestone, ordered workstreams, evidence, blockers, and post-milestone horizons
proof_baseline: "v0.13.0 and remi-v0.8.5 at 6f1429c362ac161f1ef817233e72ee9c9a031c11; post-hard-cut release, deployment, and three-distro artifact proof complete"
current_milestone: first external tester loop
active_workstream: W4 Source Fidelity Hard Cut
next_workstream: W5 Source Authority Model
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

Bootable-image export is a retained generation capability, not a bootstrap
capability. Generation keeps raw/qcow2/ISO export with a UEFI QEMU boot proof,
and that proof now stands on a supported-host fixture: Groups O and P assemble
a Fedora 44 root from ordinary repository installs on a scratch disk, publish
it as a generation, export it, and boot the artifact. The bootstrap-run export
cases and their fixture are gone. The boot lane needs a host with `/dev/kvm`
and OVMF firmware; remi-dev is the current such host. The re-based suites have
not yet had a green live run.

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
| Packaging, static repositories, trust, and self-update | solid | `v0.13.0` has exact tag, hashes, detached signature, current signed self-update, deployment, and native RPM/DEB/Arch installed-binary proof. The intentional 0.12 schema boundary requires a fresh native-package install. No SBOM/provenance sidecars are published or planned. |
| CCS conversion and native lifecycle authority | active hard switch | The exact RPM, Debian, and ALPM lifecycle contract is released and deployed; current source-backed format defects are explicit work in #98, #99, and #102 through #105 rather than manual-review authority. These share one root cause: the shared package model normalizes source facts at parse time instead of at each consumer's boundary. W5 owns the structural correction. |
| Generation build and export | limited | Bootable-image export (raw/qcow2/ISO plus a UEFI QEMU boot proof) is retained and proven from a supported-host fixture rather than from bootstrap; the re-based Group O/P suites await their first green live run. Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | `remi-v0.8.5` is deployed on the current schema with five populated sources, exact signing profiles, fair prewarm, real conversions, and full public health; distribution and a wider stranger-operated path remain limited. `/health/ready` became evidence bearing in #107 and now fails closed on an absent database or an unrunnable probe; the deployed build predates that change until the next release. |
| conaryd package and query service | limited | Authorization is exact root/daemon/configured-group authority; restart semantics, resolver-backed dry-run proof, and deployment remain incomplete. |
| Federation | experimental | Only `/v1/federation/directory` is routed and no federation call exists in chunk serving, so the router is a library nothing invokes on a local miss. TLS fingerprint pinning is enforced in code; the documented disagreement is a docs defect. See the federation horizon for measured state and slice ordering. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | The `v0.13.0` released-package matrix is complete, but the revised cross-distro milestone remains 0/10. Outreach is now gated on the W7 corpus proof rather than on venue scheduling alone; cached-history and venue-eligibility clearance remain separate prerequisites. |

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
- **Current truth:** the post-hard-cut release gate is complete at immutable
  `v0.13.0` with compatible production `remi-v0.8.5`. The released RPM, DEB,
  and Arch packages passed the Cartesian source-format lifecycle on Fedora 44,
  Ubuntu 26.04 LTS, and Arch. That lifecycle is a curated proof, not a
  repository-wide one; W7 owns the ordinary-package corpus.
- **Execution status:** complete for the release, deployment, and
  published-package proof it owns.
- **Dependencies:** none remaining.
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
  by #67 rather than hidden as release success.
- **Limitations:** the release proof does not establish that ordinary
  repository packages convert. W4 through W7 own that claim.

### W4 Source Fidelity Hard Cut

- **Outcome:** ordinary Fedora and Arch repository packages stop failing
  conversion on Conary-invented invariants, and hosted Remi health becomes
  evidence bearing.
- **Execution status:** active. This is the current workstream.
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
- **Execution status:** blocked on W4.
- **Issues:** #108 (design specification under `docs/specs/`), then #104 and
  #105 as sibling implementation slices.
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
- **Execution status:** blocked on W4; overlaps W5.
- **Issues:** #67 as epic, #109 for the trigger-status gap, plus one narrow issue per remaining ledger item.
- **Ledger gaps found on audit that #67 does not currently own:**
  - `TriggerStatus::parse` in `db/models/trigger.rs` resolves unknown or
    corrupt persisted status to `Pending` through `unwrap_or`. #67 covers
    trigger *patterns*, not trigger *status*. A corrupt persisted status
    therefore becomes executable pending work, which is a mutation-authority
    defect rather than a display fallback. Add a ledger entry or a narrow issue.
  - The `sanitize_path` ASCII heuristic is a #67-class invented authority but is
    owned only by #99, and only for ALPM. Cross-reference both.
- **Priority split within the ledger:** the publish-destination parser, Remi
  typed transport failures and retry disposition, trigger status, and required
  source-pin strength are P1 and belong here. The `ccs/policy.rs`
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
- **Execution status:** blocked on W4 through W6.
- **Issues:** #110 as the corpus umbrella, plus #39 for bootstrap.
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
  source artifact identity, target capability snapshot, per-stage results, and
  a typed outcome. Failures aggregate by stage and failure enum. Aggregating by
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
| Closed | #41 closed 2026-07-27 as fixed in v0.13.0; Artix route tracked as W8 evidence | not engineering work |

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
- **#67 and #66 already coordinate.** The conary-test MCP dual mutation
  authority item names #66 as its coordination point.
- **#62, #68, and #71 form one ecosystem cluster.** #68 depends on stable
  source and repository authority from #62; #71 adds a fourth source ABI and
  waits for both.
- **#65 and #66 both target fleet-scale control.** #65 is the replatforming
  operation, #66 the typed gateway that would expose it. #66's local typed APIs
  stabilize first.
- **#41 overlaps #99's path authority but is not open work.** The reported
  defect shipped fixed in v0.12.0; only reporter confirmation is outstanding.
  It belongs in a support state, not the engineering queue. Its Artix host
  remains outside supported scope and is tracked as W8 evidence.

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
- **Execution status:** gated on W9. #62 remains an epic and is not implemented
  as one slice.
- **Issues:** #62 as epic, decomposed into the slices below; #68 follows.
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
- **Schema prerequisite:** `db/current_schema/sql/remi.sql` currently constrains
  `source_profile` to an inline three-value `CHECK` list. That is acceptable for
  the current release epoch, but W10 requires replacing hard-coded schema
  membership with an exact repository and source identity table, foreign keys,
  immutable authenticated snapshot identity, a typed ecosystem and version
  scheme, and separately recorded target compatibility. Do not grow the `CHECK`
  list. Pre-alpha rules apply: replace the schema, rebuild disposable state, and
  delete the old definition in the same slice. #67's final ledger item already
  delegates the fixed feed catalog here.

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
- Close CCS v2 dependency-authoring, lock, and reproducibility gaps.
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
- Make CCS v2 authority explicit across kernel, initramfs, bootloader, PAM, and
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
  it. #65 additionally depends on #63. #67's conary-test MCP ledger item is
  explicitly coordinated with #66.

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
then superseded by the post-hard-cut package authority suite; it closed with
verified immutable `v0.13.0` and production `remi-v0.8.5`.

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
