---
last_updated: 2026-07-23
proof_baseline: "v0.11.3 at 0fc31c33b42a84bb00c9c8d9bdfc574ebe960ae0; release proof complete"
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

The supported public package-manager preview is adoption-led and bounded to
Fedora 44, Ubuntu 26.04 LTS, and Arch Linux. It centers on the local CLI,
reversible adoption/unadoption, and an operator-run Remi service. Native
package managers remain authoritative for adopted packages until a user makes
an explicit takeover decision. Generation, conaryd, and federation claims stay
inside the narrower limits recorded below.

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
`install -> adopt -> list/search -> update --dry-run -> unadopt` on supported
systems and report friction. Alternatively, a maintainer may record a pivot
when a reproducible systemic blocker inside the supported scope is backed by
the affected attempts and a chosen remediation or explicit scope change.

Launching outreach, publishing a release, receiving partial reports, or seeing
ordinary outreach difficulty does not satisfy the milestone. The active
[milestone tracker](external-tester-milestone.md) currently records 1/10 from
one external tester who completed the full flow on two supported hosts.

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

This is the dated 2026-07-19 baseline. The label describes reliability within
the stated scope, not whether a workstream happens to be active.

| Subsystem | Maturity | Current limitation or next proof |
| --- | --- | --- |
| CLI and core package operations | solid | Scope is the limited preview; external use and fresh combined proof matter more than new surface area. |
| Adoption, unadoption, and native handoff | solid | Proof covers the current three-distro scope; the released Arch package initializes only its exact native profile and synchronizes Remi. |
| Database, CAS, native parsing, and resolution | solid | Some advanced repository-policy abstractions and integration edges remain incomplete. |
| Packaging, static repositories, trust, and self-update | solid | `v0.11.3` replaces the reopened `v0.11.2` gate with exact tag, artifact hashes, signature, native onboarding, deployment, and real installed-binary self-update proof. No SBOM/provenance sidecars are published or planned. |
| CCS conversion and scriptlet authority | limited | The reviewed authority work has green local plus public-redownload QEMU file-capability fixture proof and is published in `v0.11.3`; the deliberately narrow public scriptlet scope remains separate. |
| Generation build and export | limited | Proven paths are x86_64; non-x86 assets, signed boot authority, and persistent-effect rollback remain later work. |
| Model, source selection, and replatforming | limited | Some resolution deltas and builder inputs are not wired end to end. |
| Bootstrap and self-hosting | limited | Rootful, chroot, fixture, and QEMU dependencies need repeatable current proof. |
| Remi core and publication | solid | Operator-run limited-preview scope; distribution, cold-start readiness, and a wider stranger-operated path remain limited. |
| conaryd package and query service | limited | System routes, authorization policy, restart semantics, dry-run behavior, deployment, and PolicyKit remain incomplete. |
| Federation | experimental | Coordinator and fetch paths are not wired into serving; TLS identity documentation and enforcement do not yet agree. |
| Advanced derivation, lock, and reproducibility flows | unfinished | Several interfaces exist without complete persisted inputs or update-path integration. |
| External product readiness | unfinished | One organic external tester completed the full flow on Ubuntu 26.04 and Fedora 44 with verified release-package checksums; the unique-person milestone is 1/10, the former 2026-07-20 through 2026-07-22 outreach slots passed without posts, and rescheduling waits on current fix-now remediation plus the existing launch gates. |

## Workstreams

### W0 Neutral Planning Migration

- **Outcome:** active repository planning is concise, tool-neutral, and
  discoverable without retaining completed process history or losing current
  product, release, or branch-resume truth.
- **Current truth:** execution began from clean `main` at rewritten commit
  `b5d7f479`, three commits ahead of `origin/main` at rewritten commit
  `7202649b`. The old planning tree contained
  176 tracked files and the retired assistant archive contained one file. The
  authority worktree was clean at the pinned head. The public and detailed
  roadmap, milestone tracker, and gated outreach draft are now active.
  Comparison of the old limited-preview checkpoint with the release artifact
  matrix was `verified-no-change`: the matrix already carries newer artifact,
  checksum, signature, self-update, SBOM/provenance, source, and caveat truth.
- **Execution status:** complete.
- **Dependencies:** none remain inside W0.
- **Next gate:** W1 completed the integrated candidate, authority reconciliation,
  TGE05 proof, public v4 fixture publication, fresh-cache boot, and final
  integrated gates. W2 subsequently completed and W3 is active.
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
  canonical rewritten commit `72f58820`. No unique blocker, decision, or follow-up exists
  only in local scratch. Head `11583a1c5e3e94155b5459b4f998887ee87c5669`
  immediately precedes self-deletion: it removed the remaining 179 tracked
  history/registry files after rehearsal, and the ignored main/authority SDD
  state and local reviews were removed after handoff preservation. No
  replacement planning archive exists. A permanent neutral-layout check and
  its clean plus six negative fixture cases are implemented in the closeout
  tree. Final 2026-07-15 acceptance passed shell syntax; documentation truth and
  fixtures; 13-card routing validation and CCS/packaging packets; optional
  review, support-bundle, line-count, maintainability, and workflow-runtime
  fixtures; Rust formatting; the 28-suite/333-case harness inventory; tracked
  Markdown local links; neutral-layout, archive, ignore, and stale-token scans;
  workflow lint; diff hygiene; and the pinned clean authority-worktree check.
  The release audit separately reproduced only unwaived `RUSTSEC-2026-0204`
  plus the documented `RUSTSEC-2026-0173` and yanked-crate warnings. The final
  live clean-worktree proof runs immediately after the closeout commit.
- **Limitations:** at W0 closeout the release was not security-green and W1
  still owned crossbeam remediation, authority integration, and TGE05 or the
  exact risk decision. W1 has since closed those items plus authenticated v4
  fixture publication and public fresh-cache proof.
- **Non-goals:** Rust behavior changes, authority-branch integration, dependency
  remediation, a release, or tester launch.

### W1 Integrated Release-Green Baseline

- **Outcome:** one clean integrated commit is safe to treat as the release
  candidate, with authority work reconciled and current release gates green.
- **Current truth:** W1 started from clean `main` at rewritten commit `b8be9f0d` and reconciles
  the independently reviewed authority head
  `b3bb30766b5c03a1e997c70e36b19afdd5f9e870` without restoring retired process
  files. `Cargo.lock` now resolves `crossbeam-epoch 0.9.20`; the fresh release
  audit no longer reports `RUSTSEC-2026-0204`. Both nonblocking authority-review
  notes are closed: one regression crosses the raw private artifact writer and
  sanitized admin response, and balanced quoted environment assignments now
  cluster as `<env-assignment>`. The 2026-07-16 Fedora 44 local KVM Group O run
  passed all five cases against `minimal-boot-v4`, including capability-absent
  and capability-enabled TGE05 boots.
- **Execution status:** complete; the integrated merge commit is the
  release-green candidate consumed by W2.
- **Dependencies:** none remain inside W1.
- **Next gate:** W2 completed the post-integration preview release, compatible
  Remi deployment, and representative clean-host smoke; W3 owns external
  tester evidence.
- **Proof:** formatting, workspace Clippy with warnings denied, owning
  Conary/core/Remi/conaryd tests, the 28-suite/334-case `conary-test` inventory,
  release audit, release-matrix validation, neutral documentation/routing
  gates, and routed scriptlet/generation interaction proof passed on the W1
  tree. The repaired runner uses Fedora's 4 MiB OVMF path as read-only pflash,
  waits for systemd boot completion before adoption, and versions the cached
  test identity with the active source image. On 2026-07-16 the complete Group
  O run passed 5 / failed 0 / skipped 0 / cancelled 0: TGE01 36,281 ms, TGE03
  625,760 ms, TGE04 1,725,736 ms, TGE05 3,024,480 ms, and TGE02 1,914,811 ms.
  TGE05 also passed a focused run in 3,060,588 ms, and a recompiled-harness
  TGE01 rerun passed in 36,068 ms with `conaryos-test-key-v4`. Authenticated
  Remi staging and destination verification preserved the image and key hashes;
  all three v4 artifact URLs returned HTTP 200. An isolated empty cache then
  downloaded the public image and private test key with matching SHA-256 values
  and passed TGE01 under KVM in 63,320 ms. The final formatting, Clippy,
  owning-package, documentation, routing, release-matrix, and release-audit
  gates passed after that proof.
- **Limitations:** W1 establishes an integrated release candidate, not a new
  tagged preview or stranger-operated service claim. W2 owns those outcomes.
  The production merge did not resurrect retired process paths.
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

Branch process changes to discard are the modified generated-document
inventory, per-file documentation-audit registry, and feature-claim registry,
plus the completed branch-only plans for file-capability public policy, generation file-capability
xattrs, LSM policy semantics, network/package recursion authority, PAM
authority, publication-summary schema/docs truth, Remi local-only test serving,
and sysctl target-profile public policy.

The two nonblocking review notes for W1 or an explicitly owned follow-up are
recorded exactly in the reconciliation map below.

### W2 Preview Release and Remi Readiness

- **Outcome:** external testers have one current pinned post-integration
  release and a prepared compatible service path matching the integrated code
  and documentation.
- **Current truth:** `v0.11.1` is the pinned post-integration preview. Its exact
  rewritten source/tag commit is `cb16f4876fdaa1ca422c3e9cce331788cacadfb1`;
  the GitHub Release publishes Fedora 44, Ubuntu 26.04 LTS, Arch, and CCS
  artifacts with `SHA256SUMS` and a detached CCS signature. Remi remains the
  deliberately chosen maintainer-operated service and source-build preview,
  not a public operator-artifact claim.
- **Execution status:** complete.
- **Dependencies:** none remain inside W2.
- **Next gate:** W3 must close its current fix-now remediation and refresh the
  release evidence before assigning a replacement Show HN, r/codex, and
  r/ClaudeAI schedule.
- **Proof:** release-build run `29540722051` and deploy-and-verify run
  `29542934278` ran at pre-rewrite head
  `4d4b422b45b055fa07a3885a68a4ab8e8d16b526`. The current rewritten tag
  commit is `cb16f4876fdaa1ca422c3e9cce331788cacadfb1`; both commits have tree
  `17c857bb27daa69b54a5c0688dc6892848da52b6`. Independent downloads
  matched every entry in `SHA256SUMS`; the detached CCS signature verified
  against the published CCS digest. The official preceding-preview Fedora RPM
  binary, using an isolated schema-75 database, detected `v0.11.1`, printed
  `Signature verified`, replaced itself, reported `conary 0.11.1`, and then
  reported itself up to date. The release records the explicit decision not to
  publish SBOM/provenance sidecars for this limited preview. Current compatible
  Remi commit `27ec2eccb6befdf06d9a826b84cc5a6948eff5fb` and the deployed pre-rewrite
  source commit `c001f8d69b9e8ef34fba39139576a9809800a9a6` both have tree
  `4307eea1f795056ce66d588d599082eb09690b78`. The deployed binary SHA-256 is
  `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`;
  its rollback/redeploy rehearsal and 10/10 public health passed, its
  conversion-version-6 prewarm state contains 11 public rows and one
  fail-closed private-review row, and a clean Fedora 44 host installed, ran,
  and removed public `htop 3.4.1` after target normalization was corrected.
- **Limitations:** the Remi operator path remains maintainer-led with a
  source-build fallback, and `v0.11.1` has no SBOM/provenance sidecars. W2
  completion prepares outreach but does not authorize or automate posting it.
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
- **Current truth:** the W3a publication and public-readiness gate is complete.
  Immutable `v0.11.3` replaces the reopened `v0.11.2` gate after the real
  supported `htop` path exposed unreachable generic SONAME evidence, an
  inexact or ABI-unchecked critical-library fallback, and discarded Arch
  capability constraints. The release now requires exact cache entries,
  compatible ELF class, and constraint-aware `pacman` proof. Its exact tag,
  workflows, artifact hashes, signature, released package paths, Remi
  synchronization, installed-binary self-update, service, and checked-site
  evidence are complete. Conary remains MIT-only and the GitHub safety
  settings remain in place. No broad external venue post has been published.
  One organic external tester completed the full flow on Ubuntu 26.04 LTS and
  Fedora 44 with verified `v0.11.3` package checksums. The two host reports
  count once toward the unique-person milestone, so the tracker is 1/10. The
  former 2026-07-20 through 2026-07-22 outreach window passed without a post.
  Issue #41 then exposed a valid safe-system-symlink false-positive on an
  Arch-style root, while its support bundle exposed unusable unprivileged
  diagnostics against the installed root-owned database. The reported Artix
  host and Fedora-form source route remain outside supported-scope proof and
  require the repaired bundle's host-profile, source-pin, and repository
  evidence before classification.
- **Execution status:** active; broad outreach is postponed while W3 repairs
  and publishes the path-safety and support-diagnostic defects.
- **Dependencies:** the current fixes must pass supported Arch-path and
  installed-host-style proof and ship in a refreshed preview release. GitHub
  Support must dereference the cached pull-request and commit views that still
  expose pre-rewrite history, and the maintainer must re-check each venue's
  current account/rule eligibility immediately before submission.
- **Next gate:** after the remediation release and its public claims are
  verified, GitHub Support confirms the cached history is no longer reachable,
  and venue checks pass, assign a new staggered schedule. Then submit the
  refreshed Show HN packet, record its actual URL and timestamp, and continue
  collecting privacy-safe reports toward the second unique qualifying tester.
- **Proof:** annotated `v0.11.3` tag object
  `a2a12791e695379e9313a210d2fd5eea2a39b352` peels to commit
  `0fc31c33b42a84bb00c9c8d9bdfc574ebe960ae0`; the immutable release was
  published at `2026-07-18T04:31:28Z`. Final merge CI run `29628990277` passed
  11/11 jobs, release-build run `29629361456` passed, and exact-tag
  deploy-and-verify run `29630694438` passed for both sites. Seven assets are
  published; independent downloads matched all five `SHA256SUMS` payloads and
  every REST asset digest, and the official `v0.11.2` binary verified the
  detached CCS signature. That binary also updated through the signed CCS in
  an isolated schema-77 database, reported `conary 0.11.3`, and then reported
  itself current. The released Arch package initialized profile `arch`,
  synchronized 15,429 Remi rows with zero foreign rows, planned zero installs
  and five adoptions with nothing blocked or unresolved, and installed,
  executed, and removed `htop 3.5.1-1-arch`; this was an isolated `bwrap`
  target backed by read-only live-host package evidence, not a pristine Arch
  VM. Fedora-form KVM proof synchronized 76,685 rows and installed, executed,
  and removed `htop 3.4.1` using exact ELF64 `libcap.so.2` and
  `libncursesw.so.6` evidence, but `minimal-boot-v4` lacks `rpm` and `dnf`, so
  it is not literal stock Fedora native-PM proof. Full Remi health passed
  10/10, six public routes returned HTTP 200 with exact-release site chunks,
  and the deployed API CCS matched the release hash, size, and signature. No
  SBOM or provenance sidecars are published or planned for this limited
  preview. The repository Welcome Discussion is live at discussion 36, and
  issue 35 was closed after its released-path proof was recorded; neither
  repository action launches broad outreach or counts as a qualifying external
  completion. Issues 37 and 38 subsequently recorded full `v0.11.3` loops on
  x86_64 Ubuntu 26.04 LTS and Fedora 44, with both package checksums confirmed.
  Because both reports came from the same external tester, they count as one
  unique completion. Each later milestone completion belongs to a unique
  outsider and covers exactly `install -> adopt -> list/search -> update
  --dry-run -> unadopt` on a supported host with the pinned release. Record
  failed attempts and triage every report as `fix-now`, `next-slice`,
  `validated-no-action`, or `declined-with-reason` with an owner.
- **Limitations:** no qualifying completion for three weeks after launch
  triggers a maintainer review of venue reach, onboarding friction, and
  observed failures. A pivot still requires a reproducible systemic blocker;
  it cannot be inferred from low interest alone.
- **Non-goals:** automated posting, redefining partial attempts as completion,
  or broadening the supported scope to make the count easier.

## Authority-Branch Reconciliation Map

The authority worktree is read-only during W0. Its pinned head is
`b3bb30766b5c03a1e997c70e36b19afdd5f9e870`, its merge base with the W0
baseline is `7202649b6ea6df2d20a681601f9e763bc16147c7`, and it was tracked-clean
on 2026-07-15. The branch contains 78 commits on its side of that merge base.
Its final independent review found no Critical or Important issue and returned
`Ready to merge? Yes`.

| Reconciliation class | Count or exact scope | W0 rehearsal and later W1 disposition |
| --- | --- | --- |
| Branch product source and tests | 58 paths outside the process set and canonical-doc set | Preserve the authority-head blob or deletion state byte-for-byte. |
| Canonical documentation | The eight files listed under W1 | Integrate branch product truth, then retain only W0's neutral path and proof wording where the concerns overlap. |
| Modified branch registries | Generated-document inventory, per-file documentation-audit coverage, and feature-claim implementation truth | Delete as completed process bookkeeping. |
| Completed branch-only plans | File-capability public policy, generation file-capability xattrs, LSM policy semantics, network/package recursion authority, PAM authority, publication-summary schema/docs truth, Remi local-only test serving, and sysctl target-profile public policy | Delete as completed process history; do not recreate under neutral planning paths. |
| Remaining shipping gate | v4 fixture publication | Closed on 2026-07-16: the versioned v4 image/key set is public, an isolated download matched the source hashes, and TGE01 booted it under KVM. |
| Production integration | The later real merge | Retired planning, audit, coherency, and local execution paths may not return. |

### Authority A-I Disposition

| Area | Durable disposition and owner |
| --- | --- |
| A. File-capability policy precision | Implemented at the pinned head. Canonical owners are `docs/SCRIPTLET_SECURITY.md` and `docs/modules/ccs.md`. |
| B. Generation xattr propagation | Implemented at the pinned head and proven by the 2026-07-16 TGE05 local KVM pass plus public fixture redownload and KVM boot. |
| C. Sysctl target-profile policy | Implemented at the pinned head. Canonical owner is `docs/modules/ccs.md`, backed by target-profile policy and conversion tests. |
| D. LSM policy semantics | Implemented at the pinned head. Canonical owners are `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and `docs/modules/remi.md`. |
| E. PAM, kernel, initramfs, and bootloader | PAM remains deliberately non-public. Later boot and security outcomes remain in the post-M1 horizon rather than becoming implied authority. |
| F. Network and package-manager recursion | Deliberately blocked and non-public; exact owners and proof are transferred below. |
| G. Remi non-public test serving | Implemented at the pinned head and canonical in `docs/modules/remi.md`. |
| H. Publication schema and docs truth | Implemented at the pinned head. W1 closed both nonblocking review minors with the cross-boundary route regression and quoted-assignment normalization decision below. |
| I. Enabling refactors | No standalone work item. Apply the existing maintainability boundary only when a product slice touches the hotspot. |

### Branch-Only Claim Transfer

| Claim | Canonical owners | Required focused proof |
| --- | --- | --- |
| Network fetch and package-manager recursion remain non-public and blocked. | `docs/SCRIPTLET_SECURITY.md`, `docs/modules/ccs.md`, and the CCS/Remi feature cards | `blocked_classes_block_live_fetch_and_package_manager_recursion`; `live_fetch_and_package_manager_classes_remain_blocked_without_native_adapters` and its fake-Known-row guard; `corpus_summary_marks_live_fetch_and_package_manager_recursion`; and `blocked_live_fetch_and_package_manager_reports_stay_private_and_non_public_only`. |
| Stale publication summaries fail closed; public responses are sanitized while raw review artifacts remain private. | `docs/modules/remi.md` and `docs/SCRIPTLET_SECURITY.md` | `converted_ccs_path_for_download_rejects_stale_conversion_records`; `stale_converted_rows_are_not_scriptlet_public_ready`; `publication_report_sanitizes_boot_and_security_policy_intents`; `publication_report_sanitizes_unknown_commands_and_reasons_while_raw_report_retains_them`; `raw_publication_report_retains_private_intents_for_review_artifacts`; and the non-public/admin sanitization tests `non_public_test_serving_manifest_returns_sanitized_blocked_metadata` and `non_public_test_serving_manifest_sanitizes_private_intent_values`. |

W1 closed the two nonblocking review notes as follows:

1. `raw_review_artifact_and_admin_response_keep_separate_visibility` combines
   the real raw private artifact writer with the authenticated admin response
   route and proves the raw path and values remain absent from the response.
2. Balanced quoted and unquoted environment assignments normalize to
   `<env-assignment>` for clustering invariance, covered by
   `balanced_quoted_env_assignments_cluster_with_unquoted_forms`.

### Disposable Rehearsal Proof

On 2026-07-15, the migration head now represented in rewritten history by
`0cd45b180b0ce3cb9f934ba849a6926105a842f4` was cloned into a disposable
repository. The rehearsal first committed deletion of the former planning
tree, retired assistant archive, and five structural-registry scripts, then
merged the authority head now represented by
`b3bb30766b5c03a1e997c70e36b19afdd5f9e870`
with `--no-commit --no-ff`. The merge stopped on five expected conflicts:
`docs/SCRIPTLET_SECURITY.md`, `docs/modules/test-fixtures.md`, and the three
retired branch registries for generated-document inventory, per-file audit
coverage, and feature-claim truth.

The three registries and eight completed branch plans stayed deleted.
`docs/SCRIPTLET_SECURITY.md` took the authority branch's complete product truth.
`docs/modules/test-fixtures.md` took the authority fixture additions plus W0's
neutral frontmatter and docs-only proof command. The other canonical-doc merges
were clean: six of eight canonical docs match the authority head byte-for-byte;
the intentional differences are the fixture reconciliation and W0's dead-link
cleanup in `docs/conaryopedia-v2.md`. All 58 branch-owned product source/test
paths matched the authority-head blob or deletion state. No retired path or
caller survived the staged scan. Naming the discarded files in the handoff had
initially collided with that final token scan, so their durable dispositions
are expressed here by purpose while the temporary W0 design and execution plan
retain the exact historical path inventory.

The following rehearsal gates passed:

- documentation truth, its fixtures, agent-context fixtures, and validation of
  13 feature cards;
- `cargo fmt --all -- --check` plus worktree and staged diff checks;
- `cargo test -p conary-core public_policy` (15 matched tests) and
  `cargo test -p conary-core file_capability` (11);
- `cargo test -p conary generation_file_capabilities` (7);
- `cargo test -p remi publication` (18),
  `cargo test -p remi non_public_test_serving` (11), and
  `cargo test -p remi scriptlet_corpus` (5); and
- `cargo run -p conary-test -- list` (28 suites, 334 cases).

No selector correction was required because every named focused filter matched
tests. Unreachable historical disposable-only rehearsal commit
`5fb111b5f4b95758310dd8e44e6714d863afc924` had the then-current authority head
as its second parent, and the authority-head ancestry check passed. Before this
proof record was edited, both live worktrees were
confirmed clean and the authority worktree remained at the pinned head.

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
a post-milestone horizon. W0 closed after the authority rehearsal and
neutral-layout acceptance pass; W1 closed after authenticated v4 fixture
publication, public fresh-cache KVM proof, and final integrated release gates.
W2 closed after exact release identity, multi-distro artifact publication,
checksum and CCS-signature verification, production deployment, official
installed-binary self-update, compatible prewarmed Remi, rollback, and
clean-host proof. W3 is now active; its W3a `v0.11.2` proof gate closed, was
reopened after the supported `htop` SONAME-probe flaw was found, and is now
closed again with the verified `v0.11.3` replacement. The 2026-07-20 through
2026-07-22 manual outreach window passed without a post and is now retired.
Rescheduling waits for the current path-safety and support-bundle remediation
to ship with refreshed release evidence, GitHub Support to dereference cached
pre-rewrite pull-request and commit views, and the venue-specific eligibility
checks. No replacement date is assigned yet.
