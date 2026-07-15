---
last_updated: 2026-07-15
proof_baseline: a610dcf8b76f0c555086e9bab09e07644ac23b5d
current_milestone: first external tester loop
active_workstream: W0 Neutral Planning Migration
---

# Codebase Development Roadmap

## Purpose and Supported Limited-Preview Scope

This document is the detailed source of truth for Conary's current maturity,
ordered delivery work, evidence, blockers, and longer horizons. It separates
subsystem maturity from workstream execution state and evidence freshness.

The supported public package-manager preview is adoption-led and bounded to
Fedora 44, Ubuntu 26.04 LTS, and Arch Linux. It centers on the local CLI,
reversible adoption/unadoption, and an operator-run Remi service. Native
package managers remain authoritative for adopted packages until a user makes
an explicit takeover decision. Generation, conaryd, and federation claims stay
inside the narrower limits recorded below.

Remote Forge validation is paused pending a KVM-capable runner. The dated
2026-05-21 Group O QEMU run passed installed-runtime and bootstrap-run
raw/qcow2 boot proof, and the dated 2026-05-21 Group P QEMU run passed ISO
generation-carrier, provenance-sidecar, copy-back, read-only carrier boot, and
writable `/etc` overlay proof. These are local x86_64 evidence, not a broad or
current remote-validation claim.

## Milestone 1 Exit Condition

The first external tester milestone closes when ten unique people outside the
existing project circle complete
`install -> adopt -> list/search -> update --dry-run -> unadopt` on supported
systems and report friction. Alternatively, a maintainer may record a pivot
when a reproducible systemic blocker inside the supported scope is backed by
the affected attempts and a chosen remediation or explicit scope change.

Launching outreach, publishing a release, receiving partial reports, or seeing
ordinary outreach difficulty does not satisfy the milestone. The active
[milestone tracker](external-tester-milestone.md) currently records 0/10.

## Principles and Safety Boundaries

- Prefer reversible adoption and explicit authority transfer over silent
  takeover.
- Fail closed at public, security, persistence, and package-mutation
  boundaries; do not turn missing proof into optimistic behavior or copy.
- Keep supported scope, execution status, and proof freshness as independent
  facts. Do not collapse them into percentages or a single readiness color.
- Treat package-manager, scriptlet, trust, and release claims as evidence
  bearing. Focused tests do not substitute for an explicitly required QEMU,
  clean-host, installed-binary, or external-user proof.
- Preserve privacy in tester and operator evidence. Do not request secrets,
  private keys, credential files, broad machine dumps, or live databases by
  default.
- Keep active planning proportional to risk and remove it after canonical
  truth, verification, roadmap state, and resume facts are durable.

## Subsystem Maturity

This is the dated 2026-07-15 baseline. The label describes reliability within
the stated scope, not whether a workstream happens to be active.

| Subsystem | Maturity | Current limitation or next proof |
| --- | --- | --- |
| CLI and core package operations | solid | Scope is the limited preview; external use and fresh combined proof matter more than new surface area. |
| Adoption, unadoption, and native handoff | solid | Proof covers the current three-distro scope and is dated; revalidate on the integrated release candidate. |
| Database, CAS, native parsing, and resolution | solid | Some advanced repository-policy abstractions and integration edges remain incomplete. |
| Packaging, static repositories, trust, and self-update | solid | Preview scope only; refresh release evidence and keep supply-chain caveats explicit. |
| CCS conversion and scriptlet authority | limited | The isolated authority branch is materially stronger than `main`; integrate it and close the QEMU shipping-proof gap. |
| Generation build and export | limited | Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | Operator-run limited-preview scope; distribution, cold-start readiness, and a wider stranger-operated path remain limited. |
| conaryd package and query service | limited | System routes, authorization policy, restart semantics, dry-run behavior, deployment, and PolicyKit remain incomplete. |
| Federation | experimental | Coordinator and fetch paths are not wired into serving; TLS identity documentation and enforcement do not yet agree. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | The prepared tester loop has not launched or completed. |

## Workstreams

### W0 Neutral Planning Migration

- **Outcome:** active repository planning is concise, tool-neutral, and
  discoverable without retaining completed process history or losing current
  product, release, or branch-resume truth.
- **Current truth:** execution began from clean `main` at `a610dcf8`, three
  commits ahead of `origin/main` at `ce6841ec`. The old planning tree contained
  176 tracked files and `docs/llms/archive/` contained one file. The authority
  worktree was clean at the pinned head. The public and detailed roadmap,
  milestone tracker, and gated outreach draft are being established first.
  Comparison of the old limited-preview checkpoint with the release artifact
  matrix was `verified-no-change`: the matrix already carries newer artifact,
  checksum, signature, self-update, SBOM/provenance, source, and caveat truth.
- **Execution status:** active.
- **Dependencies:** the approved roadmap design and committed W0 execution
  plan; both are temporary and close with this workstream.
- **Next gate:** replace live routing and planning conventions, preserve release
  security policy, then pass the disposable authority-branch reconciliation
  rehearsal before deleting retired paths or local resume state.
- **Proof:** on 2026-07-15, doc truth, its fixtures, agent-context fixtures and
  validation, and optional review-helper fixtures passed. The maintainability
  fixture reproduced its known federation `pipefail` false negative. Release
  audit failed on unwaived `RUSTSEC-2026-0204` through
  `crossbeam-epoch 0.9.18`; W0 must preserve that blocker for W1. A NUL-safe
  inventory covered all 182 ignored local files (6,485,292 bytes): 2 progress
  files, 42 task briefs/reports, 75 review diffs, 6 final-verification reports,
  55 local plan reviews, and 2 ignore-scaffolding files. Full progress review
  found no unfinished main task and reduced the authority resume state to the
  W1 handoff below. Every referenced implementation commit exists; four missing
  full base IDs are obsolete pre-rebase markers, and one stale report's
  `f519bc42` sysctl-versioning reference maps by subject and task range to
  canonical commit `e56d71ac`. No unique blocker, decision, or follow-up exists
  only in local scratch.
- **Limitations:** the old tree and checks remain until rehearsal; the release
  is not security-green; ignored local execution evidence remains until the
  rehearsal proves its deletion safe.
- **Non-goals:** Rust behavior changes, authority-branch integration, dependency
  remediation, a release, or tester launch.

### W1 Integrated Release-Green Baseline

- **Outcome:** one clean integrated commit is safe to treat as the release
  candidate, with authority work reconciled and current release gates green.
- **Current truth:** the independently reviewed authority implementation is
  isolated, tracked-clean, and ready to integrate at
  `e6eeb4da9c560c34317b57b2a422717d4d556b37`, with merge base `ce6841ec`.
  `RUSTSEC-2026-0204` remains unwaived against `crossbeam-epoch 0.9.18` and is
  fixed upstream in 0.9.20. Generation file-capability shipping still requires
  a TGE05 Group O QEMU success or an explicit maintainer decision naming the
  missing proof, affected behavior, rationale, and expiration condition.
- **Execution status:** queued behind W0.
- **Dependencies:** W0 closeout; a non-vulnerable dependency resolution; the
  authority reconciliation contract below; TGE05 or the precise risk decision.
- **Next gate:** an exact integrated candidate with the crossbeam advisory
  absent from the release audit and the generation file-capability shipping
  decision recorded.
- **Proof:** run formatting, workspace Clippy with warnings denied, owning
  Conary/core/Remi/conaryd tests, `conary-test` inventory parsing, release
  audit, release-matrix validation, neutral documentation/routing gates, and
  routed scriptlet/generation interaction proof from the integrated head.
- **Limitations:** focused tests are not equivalent to the outstanding real-VM
  gate. The later production merge may not resurrect retired process paths.
- **Non-goals:** expanding public scriptlet authority, enabling federation, or
  adding unrelated package-manager features.

Branch canonical documentation to reconcile deliberately:

1. `docs/ARCHITECTURE.md`
2. `docs/INTEGRATION-TESTING.md`
3. `docs/SCRIPTLET_SECURITY.md`
4. `docs/conaryopedia-v2.md`
5. `docs/modules/ccs.md`
6. `docs/modules/remi.md`
7. `docs/modules/test-fixtures.md`
8. `docs/operations/post-generation-export-follow-up-roadmap.md`

Branch process changes to discard are the modified documentation inventory,
documentation audit ledger, and feature-coherency ledger, plus the completed
branch-only plans for file-capability public policy, generation file-capability
xattrs, LSM policy semantics, network/package recursion authority, PAM
authority, publication-summary schema/docs truth, Remi local-only test serving,
and sysctl target-profile public policy.

The two nonblocking review notes for W1 or an explicitly owned follow-up are
recorded exactly in the reconciliation map below.

### W2 Preview Release and Remi Readiness

- **Outcome:** external testers have one current pinned post-integration
  release and a prepared compatible service path matching the integrated code
  and documentation.
- **Current truth:** `v0.10.1` is the current documented preview and includes
  multi-distro artifacts, checksums, a CCS signature, and installed-binary
  self-update evidence. It predates the integrated W1 candidate. Remi remains
  a maintainer/operator-run path with source-build fallbacks and no public
  operator artifact decision.
- **Execution status:** queued behind W1.
- **Dependencies:** a clean W1 candidate and release-green audit.
- **Next gate:** publish a fresh post-integration release and prove a prewarmed,
  compatible Remi path from a clean supported host.
- **Proof:** require identity equality among the source commit, tag, published
  metadata, and artifacts; distro artifact checksums and signatures; an
  explicit SBOM/provenance publication or caveat decision; installed-binary
  self-update from the actual preceding preview; canonical installation
  guidance; rollback evidence; exact compatible Remi commit; stale-row
  handling for the integrated `CONVERSION_VERSION`; prewarmed package set; and
  representative clean-host conversion/install smoke.
- **Limitations:** release copy and the outreach draft cannot be published
  until compatibility, tester, feedback, and launch material are repinned.
  W2 must explicitly choose between a proven Remi operator artifact and honest
  service-operator/source-build-preview status.
- **Non-goals:** distributing every workspace binary or claiming broad
  production support.

W2 also owns one compatibility/proof decision spanning Clap-time Remi target
rejection, runtime target-ID normalization, and
`repo add --remi-distro` validation. W0 changes none of that behavior.
Debian-family identifiers in conversion tests remain internal package-family
fixture coverage owned by `docs/modules/test-fixtures.md`; they are not public
distro claims. The old generic dead-helper row had no evidence-backed surface
and creates no roadmap item.

### W3 First External Tester Loop

- **Outcome:** Conary has external evidence about whether its adoption-led
  preview works for strangers on supported systems.
- **Current truth:** preparation exists, but the launch venue and timestamp are
  unset and the tracker remains 0/10.
- **Execution status:** queued behind W2.
- **Dependencies:** W2's pinned release, compatible prewarmed Remi path,
  clean-host smoke, and maintainer posting decision.
- **Next gate:** manually launch the verified copy, record venue and timestamp,
  and obtain the first privacy-safe qualifying report.
- **Proof:** each completion belongs to a unique outsider and covers exactly
  `install -> adopt -> list/search -> update --dry-run -> unadopt` on a
  supported host with the pinned release. Record failed attempts and triage
  every report as `fix-now`, `next-slice`, or `declined-with-reason` with an
  owner.
- **Limitations:** no qualifying completion for three weeks after launch
  triggers a maintainer review of venue reach, onboarding friction, and
  observed failures. A pivot still requires a reproducible systemic blocker;
  it cannot be inferred from low interest alone.
- **Non-goals:** automated posting, redefining partial attempts as completion,
  or broadening the supported scope to make the count easier.

## Authority-Branch Reconciliation Map

The authority worktree is read-only during W0. Its pinned head is
`e6eeb4da9c560c34317b57b2a422717d4d556b37`, its merge base with the W0
baseline is `ce6841ec1dcfdddf12c68feefac648507fd1538d`, and it was tracked-clean
on 2026-07-15. The branch contains 78 commits on its side of that merge base.
Its final independent review found no Critical or Important issue and returned
`Ready to merge? Yes`.

| Reconciliation class | Count or exact scope | W0 rehearsal and later W1 disposition |
| --- | --- | --- |
| Branch product source and tests | 58 paths outside the process set and canonical-doc set | Preserve the authority-head blob or deletion state byte-for-byte. |
| Canonical documentation | The eight files listed under W1 | Integrate branch product truth, then retain only W0's neutral path and proof wording where the concerns overlap. |
| Modified branch ledgers | `documentation-accuracy-audit-inventory.tsv`, `documentation-accuracy-audit-ledger.tsv`, and `feature-coherency-ledger.tsv` under `docs/superpowers/` | Delete as completed process bookkeeping. |
| Completed branch-only plans | File-capability public policy, generation file-capability xattrs, LSM policy semantics, network/package recursion authority, PAM authority, publication-summary schema/docs truth, Remi local-only test serving, and sysctl target-profile public policy | Delete as completed process history; do not recreate under neutral planning paths. |
| Remaining shipping gate | TGE05 Group O | W1 must pass the QEMU proof or record the exact affected behavior, rationale, missing proof, and expiration condition of a maintainer risk decision. |
| Production integration | The later real merge | Retired planning, audit, coherency, and local execution paths may not return. |

### Authority A-I Disposition

| Area | Durable disposition and owner |
| --- | --- |
| A. File-capability policy precision | Implemented at the pinned head. Canonical owners are `docs/SCRIPTLET_SECURITY.md` and `docs/modules/ccs.md`. |
| B. Generation xattr propagation | Implemented at the pinned head. W1 still requires TGE05 Group O or the named shipping-risk decision above. |
| C. Sysctl target-profile policy | Implemented at the pinned head. Canonical owner is `docs/modules/ccs.md`, backed by target-profile policy and conversion tests. |
| D. LSM policy semantics | Implemented at the pinned head. Canonical owners are `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and `docs/modules/remi.md`. |
| E. PAM, kernel, initramfs, and bootloader | PAM remains deliberately non-public. Later boot and security outcomes remain in the post-M1 horizon rather than becoming implied authority. |
| F. Network and package-manager recursion | Deliberately blocked and non-public; exact owners and proof are transferred below. |
| G. Remi non-public test serving | Implemented at the pinned head and canonical in `docs/modules/remi.md`. |
| H. Publication schema and docs truth | Implemented at the pinned head. W1 retains the two nonblocking review minors below until implementation or explicit follow-up ownership. |
| I. Enabling refactors | No standalone work item. Apply the existing maintainability boundary only when a product slice touches the hotspot. |

### Branch-Only Claim Transfer

| Claim | Canonical owners | Required focused proof |
| --- | --- | --- |
| Network fetch and package-manager recursion remain non-public and blocked. | `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and the CCS/Remi feature cards | `blocked_classes_block_live_fetch_and_package_manager_recursion`; `live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters` and its fake-Known-row guard; `corpus_summary_marks_live_fetch_and_package_manager_recursion`; and `blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only`. |
| Stale publication summaries fail closed; public responses are sanitized while raw review artifacts remain private. | `docs/modules/remi.md` and `docs/SCRIPTLET_SECURITY.md` | `converted_ccs_path_for_download_rejects_stale_conversion_records`; `stale_converted_rows_are_not_scriptlet_public_ready`; `publication_report_sanitizes_boot_and_security_policy_intents`; `publication_report_sanitizes_unknown_commands_and_reasons_while_raw_report_retains_them`; `raw_publication_report_retains_private_intents_for_review_artifacts`; and the non-public/admin sanitization tests `non_public_test_serving_manifest_returns_sanitized_blocked_metadata` and `non_public_test_serving_manifest_sanitizes_private_intent_values`. |

Retain these two nonblocking review notes exactly until W1 implements them or
assigns an explicit later owner:

1. Add one workflow regression that combines a raw private artifact writer
   with a sanitized public/admin response route.
2. Decide whether balanced quoted environment assignments normalize to
   `<path>` or `<env-assignment>` for clustering invariance.

The disposable rehearsal's migration head, merge result, conflict paths,
canonical-doc resolutions, command results, selector counts, and clean-live-tree
proof will be appended here before destructive cleanup.

## Post-Milestone Horizons

### Feedback-Driven Compatibility and Authority

- Repair the highest-impact adoption, resolution, scriptlet, or service
  friction shown by real testers.
- Expand public scriptlet authority only through positive target policy and
  end-to-end preservation proof.
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

- Resolve conaryd's root-only versus read-only authorization contract and
  implement or remove unreachable PolicyKit behavior.
- Replace echo-style dry runs with real resolver plans and prove restart safety
  before adding destructive routes.
- Decide distribution and deployment support for Remi, conaryd, and the test
  harness; improve observability, migration, and recovery guidance.

### System Artifacts and Platform Breadth

- Keep x86_64 generation boot proof repeatable on maintained fixtures.
- Add signed boot-artifact authority and portable generation-bundle trust.
- Add non-x86 architectures only with owned boot assets and proof.
- Add distros only after the existing three-distro path is repeatable.
- Address persistent scriptlet-effect rollback limitations explicitly.

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

Active designs live under `docs/designs/`; active multi-step implementation
plans live under `docs/plans/`; stable public or persisted contracts live under
`docs/specs/`. Planning must be proportional to risk and tool-neutral.

Close a design or plan only after implementation meets its acceptance
condition, enduring truth is canonical, roadmap and release state are current,
verification lives with its owner, and no active branch or handoff depends on
the file. Then delete it from the current tree and use Git history when the
decision record is needed. Do not create a replacement planning archive.

New work enters the ordered sequence only when it advances the current
milestone, resolves a safety or release blocker, or is explicitly accepted as
a post-milestone horizon. W0 closes only after the authority rehearsal and
neutral-layout acceptance pass; W1 remains the next queued workstream until
then.
