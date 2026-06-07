# Documentation Accuracy Audit Summary

## Scope

This refresh updates the documentation baseline after the limited public
release readiness pass and the completed composefs atomic switching
modernization, then refreshes the Adopt Without Regret docs/integration proof
slice, records the Native Package Manager Parity Matrix design, and catches the
completed Slice A/Slice B/Slice C/Slice D parity records, the Slice B preview-doc
refresh, the focused Slice C daily-driver/provider-metadata proof, the Slice D
three-distro parity evidence, the 2026-05-16 limited-preview checkpoint, and the
2026-05-19 generation-export/security refresh. It also records the 2026-05-19
daily-driver readiness program design that decomposes the seven replacement
gaps into Codex `/goal` tracks and the first result-gated daily-driver corpus
expansion, plus the Goal 3 trusted security advisory pipeline path.
It now also records the Goal 5 conaryd package execution path: package
install/remove/update routes queue daemon jobs instead of returning blanket
`501 Not Implemented`, and non-dry-run package jobs retain the CLI's explicit
live-host mutation acknowledgement boundary. The Goal 7 refresh adds the
checked-in daily-driver UX matrix, root help examples, bash/zsh completion
rendering checks, and focused CLI diagnostics proof for native PM, adoption
refresh, explicit takeover, unadopt/purge, generation, and conaryd routes.
The 2026-05-22 completion-validation refresh first recorded that Goal 1
native-authority handoff evidence was absent; the later Goal 1 refresh
supersedes that blocker with selected-generation `native-handoff` dry-run,
refusal, apply, and recovery evidence across Fedora 44, Ubuntu 26.04 LTS, and
Arch while preserving the broader rule that program-complete claims need fresh
combined gates. This clean-slate archive pass moves the completed Goal 6,
Goal 7, completion-validation, Goal 1, and daily-driver umbrella records under
archive paths and left the active `plans/` and `specs/` roots clear for the next
current plan or spec. The 2026-05-24 MCP stateless prep refresh
extends that clean-slate pass by archiving the completed LLM-native operations,
local bootstrap smoke, stateless compliance/raw HTTP proof, conary-test
stateless discovery, bootstrap-status, and suites resource records. The
2026-05-25 preview invariant hardening umbrella opens the next active spec lane
for command-risk, CAS/adoption integrity, generation publication durability, and
docs/CI truth-check planning. The matching Plan A checklist opens the active
plans lane for adoption acknowledgement gates, dry-run behavior, private CAS
capture, ghost-trove cleanup, and refresh savepoints; the 2026-05-26 review
tightening records a constrained sync-hook refresh path, touched legacy shared
CAS-object repair, changeset-envelope adoption warnings, production-path
ghost-trove tests, and dirty-worktree-safe final commit instructions. The
2026-05-26 Plan B design now splits generation publication durability into a
DB-backed publication debt/recovery slice and a durable filesystem fsync sweep;
the external review follow-up tightened it around current-DB publication sweeps,
parameterless publish retries, a smaller recovery phase model, and the
activation-order refactor required in `rebuild_and_mount`.
The matching Plan B implementation landed in `c7a549d0..46b268da` with the
schema v69 publication debt model, publication helper, CLI/status surfaces,
recovery guardrails, current-link durability, daemon history truthfulness, GC
protection, and the B2 fsync sweep.

### 2026-05-26 Plan C Docs Truth Gate

Plan C closed the preview invariant hardening umbrella by adding an always-on
docs-truth PR gate, strict docs-audit CI checks, a maintained conaryd endpoint
reference, fail-closed PolicyKit wording, retired-command checks,
schema-version truth checks, and the internal `conary-core` crate guard. The
completed Plan A, Plan B, Plan C, and umbrella docs were moved to archive paths
after landing.

### 2026-05-26 Review-Derived Release Hardening Queue

The multi-model release review opened a new planning lane with one umbrella
design and four implementation plans: preview truth/onboarding, scriptlet trust
assurance, release evidence and supportability, and generation state
resilience. These records converted the external review findings into a
sequenced queue rather than another broad readiness checklist.

The DeepSeek follow-up review tightened the active queue around Remi cold-start
latency, the live-mutation acknowledgement explanation, first-run `system init`
failure modes, generation-state resilience pre-work, support-bundle redaction
scope, and source-selection documentation before behavior changes.

The Gemini follow-up review then shifted Plan D away from a custom JSON
trove/file/publication mirror toward SQLite-native backups, added rotational
pre-mutation DB backups for adoption-only recovery, tightened support bundles
to an allowlist-only model, and added namespace preflight diagnostics for
hardened kernels and restricted containers.

The agentic plan review then tightened blocker sequencing and implementation
specificity: minimum Plan C supportability gated first public testers until it
landed, Plan D required live-mutation inventory plus pre/post DB checkpoints
and non-generation recovery, Plan B distinguishes sandbox setup failures from
script exit failures, and Plan A's first-run path is bounded by timing/disk
evidence.

The follow-up reconciliation completed preview truth/onboarding,
scriptlet trust assurance, release-evidence/supportability, and
generation-state-resilience, then moved the four plan records plus the umbrella
design to archive paths. The active `plans/` and `specs/` roots are clear
again; the release-hardening queue now survives as active product behavior,
tests, and user/operator docs rather than as open planning records.

The legacy scriptlet semantics sequence has now landed through Goal 8. The
first archive pass moved the completed Goal 0 Remi benchmark/corpus plan, the
Goal 1 through Goal 6 detailed implementation plans, and the matching Goal 1
through Goal 6 design specs to archive paths. This follow-up archive pass moves
the completed umbrella goal queue and semantics design, the Goal 7 compatibility
matrix/override audit design and plan, and the Goal 8 golden-fixture/regex-truth
plan to archive paths. The active `plans/` and `specs/` roots are clear again;
current behavior now survives in implementation, tests, and canonical module
docs rather than as open planning records.

Audited every tracked documentation-like file returned by
`bash scripts/docs-audit-inventory.sh`: tracked files spanning root docs,
assistant entrypoints, GitHub templates, canonical docs under `docs/`,
deploy/operator docs, `deploy/remi.toml.example`, app-local READMEs, active
planning records, historical/archive docs, release security waivers, and
the `site/`/`web` frontend READMEs.

Ignored local or credential-bearing files were reviewed only as scan findings
and were not edited. That includes `deploy/.credentials.toml`,
`docs/operations/LOCAL_ACCESS.md`, and local ignored plan/spec archives.

## Why This Refresh Happened

The repo moved materially after the last audit:

- four-track release hardening and releases (`conary`, `remi`, `conaryd`,
  `conary-test`)
- conaryd Forge-local staging deployment
- bootstrap self-hosting VM flow with rootful handoff and QEMU validation
- generation artifact export unification
- self-contained installed-runtime generation exports with Fedora 44 QEMU
  validation
- Remi deploy/access and conversion-burst hardening
- schema v68 and architecture-aware conversion cache migration
- schema v69 generation publication debt, recovery/status surfaces, and
  durable fsync helper coverage
- release-readiness dependency/security refresh
- Fedora 44, Ubuntu 26.04 LTS, and Arch Linux public-preview matrix alignment
- conaryd package execution, with queued install/remove/update jobs and the
  CLI live-host mutation acknowledgement boundary preserved
- live Remi metadata realignment to 26.04/`resolute` while keeping host OS
  claims separate from client distro support
- composefs atomic switching completion, with ordinary package mutation and
  transaction recovery selecting complete generation artifacts for the next
  boot instead of relying on live generation remounts
- Adopt Without Regret preview framing, with non-destructive unadoption,
  native package-manager authority, explicit takeover boundaries, and later
  selected-generation native handoff recovery documented for the limited
  preview
- Native Package Manager Parity Matrix framing for Tier 0 and Tier 1
  Conary-owned package-manager flows against dnf, apt, and pacman expectations
- Slice A, Slice B, Slice C, and Slice D native package-manager parity implementation records,
  including no-generation install/remove/update proof, adopted-package native
  authority, explicit takeover, security-advisory support refusal, and the
  focused Tier 1 daily-driver command parity, declared-provider matching work,
  and the three-distro conary-test matrix plan
- 2026-05-16 limited-preview checkpoint evidence, including refreshed
  adoption/unadoption proof and refreshed Group N QEMU proof
- 2026-05-19 refresh evidence, including the restored Group O `TGE04`
  installed-runtime export boot proof, the package-manager-only tester-post
  recommendation, and the removal of the `tough` trust-root dependency path
- the first result-gated daily-driver corpus expansion for the Phase 4 native
  package-manager parity suite, including covered package classes and explicit
  unsupported native alternatives/bootloader mutation boundaries
- the Goal 3 security advisory pipeline, including trusted JSON repository
  advisory ingestion, Remi metadata advisory parsing, fail-closed unknown-source
  behavior, and advisory/CVE/fixed-version/source-trust output
- the Goal 7 daily-driver UX matrix and CLI diagnostics proof, including
  rendered root help examples, bash/zsh completion output, live-host mutation
  refusal guidance, adopted-package native-PM/update-refresh guidance, explicit
  takeover routes, unadopt/purge routes, and conaryd operator wording
- the 2026-05-22 daily-driver readiness completion-validation audit, including
  fresh Phase 1 and Phase 4 distro matrices, local QEMU Group N/O/P evidence,
  the booted ISO export proof, and the superseded Goal 1 handoff blocker
- the 2026-05-22 Goal 1 native authority handoff proof, including the
  `native-handoff` CLI, durable recovery record, and three-distro
  `phase3-active-generation-handoff` matrix

## Major Corrections

- Refreshed user-facing status in `README.md` and `ROADMAP.md` around the
  Fedora 44, Ubuntu 26.04 LTS, and Arch Linux adoption-led limited preview;
  native package-manager authority for adopted packages; the non-destructive
  `system unadopt --all` escape hatch; explicit takeover; Conary-owned updates
  on mutable live roots; security-update advisory support honesty;
  selected-generation native handoff; local QEMU/security gates; raw/qcow2
  generation export; OCI artifact-source alignment; and ISO/bundle follow-ups.
- Updated `SECURITY.md` to 0.8.x support and replaced stale journal language
  with the current database/EROFS generation model and preview distro scope.
- Updated deploy/operator docs and `deploy/remi.toml.example` for Fedora 44,
  Ubuntu 26.04 LTS, Remi admin-origin access, current host assumptions, and
  Forge/conaryd paths.
- Updated `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md` for schema v69,
  runtime generation input validation, LFS 13.0 bootstrap phases, and
  self-contained generation export; refreshed them again after composefs atomic
  switching landed so transaction and recovery wording reflects next-boot
  selection through `/conary/current`; then refreshed the package-update,
  adoption, unadoption, repository advisory-support, and explicit-takeover
  command guidance after Slice B landed; refreshed security advisory metadata
  guidance after Goal 3 added trusted JSON and Remi advisory ingestion; then
  refreshed schema wording again after Plan B added generation publication debt.
- Updated `README.md` with the operator path for pending generation publication
  debt: inspect with `system generation pending` and retry current-DB
  publication with live-mutation acknowledgement.
- Updated `CHANGELOG.md`, `CONTRIBUTING.md`, and `SECURITY.md` so public and
  developer-facing docs describe rebuild/reselect semantics rather than older
  live remount recovery language.
- Updated site install/features/compare copy to remove Debian as a public
  support claim and to clarify that non-x86_64 generation boot assets remain
  follow-up work while OCI export source loading has moved onto the generation
  artifact contract.
- Updated conaryd CLI help and public docs so package install/remove/update
  routes are described as daemon-executed package jobs rather than blanket
  `501 Not Implemented` responses.
- Updated integration docs and `apps/conary-test/README.md` to include the
  Phase 1 `T21a`-`T21c` non-destructive unadoption proof, focused live-root
  update/security-refusal proof, Phase 3 Group O generation-export QEMU suite,
  temporary local QEMU release gate, refreshed Group N proof, the current
  2026-05-21 Group O installed-runtime/bootstrap-run boot proof, and the Phase 4
  native package-manager parity manifest with its 18-test three-distro corpus
  evidence.
- Added the 2026-05-16 limited-preview release checkpoint and refreshed
  README/ROADMAP/bootstrap/generation-export docs so a package-manager-only
  preview ask treats generation export as supporting evidence instead of the
  headline public promise.
- Refreshed assistant-facing docs to route broad doc work through the inventory
  and ledger checker, and added the post-generation export roadmap to the map.
- Refreshed assistant-facing docs again on 2026-05-22 for GPT-5.5/Codex-first
  work, removed tracked Claude active guidance/harness files, and kept
  Claude-era context only as archived history.
- Reframed completed dated plans/specs as historical implementation records
  and archived completed implementation plans/specs after their validation
  evidence landed on `main`.
- Added the Native Package Manager Parity Matrix design and Slice A/B/C/D
  implementation records for Conary-owned install/remove/update, adjacent daily
  package-manager commands, and the three-distro parity matrix; then moved them
  to archive paths once the current integration docs carried the release
  evidence.
- Recorded and archived the Daily Driver Readiness Program design with seven Codex `/goal`
  tracks for active-generation authority handoff, real package corpus
  validation, security advisory support, live-root sandbox hardening, conaryd
  package execution, recovery/artifact trust, and daily UX polish.
- Refreshed the Goal 4 sandbox docs after protected live-root scriptlet modes
  gained private `/etc` and `/var` writable layers plus fail-closed setup
  behavior.
- Archived the completed Goal 6 recovery/artifact trust implementation plan and refreshed
  README, ROADMAP, integration, bootstrap, and post-generation-export docs for
  x86_64 ISO generation-carrier export, raw/qcow2/ISO provenance sidecars, the
  Group P QEMU manifest, passing Group P ISO evidence, and self-host workspace
  freshness checks.
- Archived the completed Goal 7 daily-driver UX matrix implementation plan, refreshed
  README, ROADMAP, assistant, integration, and program-design docs for CLI
  diagnostics/completion checks, and aligned package-command output with
  native package-manager, adoption refresh, explicit takeover, unadopt/purge,
  generation, and conaryd guidance.
- Archived the completed 2026-05-22 daily-driver readiness completion
  validation plan. The audit records fresh green Phase 1/Phase 4 matrices and the local
  QEMU composefs, Group N, Group O, and Group P ISO boot gates; the later Goal 1
  branch supersedes the audit's missing-suite blocker with selected-generation
  native handoff implementation and matrix proof.
- Archived the completed Goal 1 native authority handoff implementation plan, refreshed
  README, ROADMAP, integration, conaryopedia, limited-preview, and audit docs,
  and recorded the Fedora 44, Ubuntu 26.04 LTS, and Arch
  `phase3-active-generation-handoff` evidence.
- Archived the completed MCP/LLM-native operations plans and design specs after
  the contract, bootstrap, stateless adapter proof, conary-test discovery, and
  read-only resource slices landed.

## Archive Decisions

- Existing archive docs and recipe READMEs were retained as historical
  reference material.
- Completed top-level superpowers plans, including Goal 6, Goal 7, completion
  validation, Goal 1, the MCP/LLM-native operations prep slices, and the
  completed legacy scriptlet Goal 0 through Goal 6 detailed records, were moved
  to `docs/superpowers/plans/archive/`.
- Completed design specs, including the MCP/LLM-native operations design
  records, the Plan A/B/C invariant-hardening records, and the completed legacy
  scriptlet Goal 1 through Goal 6 design specs, were moved to
  `docs/superpowers/specs/archive/`; completed implementation plans were moved
  to `docs/superpowers/plans/archive/`. The current top-level `plans/` and
  `specs/` directories intentionally retain the legacy scriptlet goal queue and
  umbrella semantics design for the remaining future goals.
- The completed May 14 limited-preview honesty review TSV was moved beside its
  archived plan in `docs/superpowers/plans/archive/`; it is outside the doc
  inventory script because that script tracks documentation-like files.
- The current package-manager tester decision is represented by
  `docs/superpowers/limited-preview-release-checkpoint-2026-05-16.md` and
  `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`, not
  by a broad top-level release plan.
- The retired `CLAUDE.md` compatibility shim was deleted after Claude stopped
  being an active toolchain for this repo. The historical Claude-era notes were
  retained under `docs/llms/archive/`.

### 2026-06-06 Maintainability Planning

The maintainability reset reopened the active planning lane after the prior
clean-slate archive pass. Phase 1 added the repo discipline contract and
line-count report. Phase 2 now opens a focused dead-surface pruning plan for
portable docs-audit inventory filtering, tested public CLI help cleanup, and a
pruning inventory. Active plan roots are therefore not expected to be empty;
current active roadmap and child-plan files are tracked through the ledger.

The first Phase 2 pruning inventory records the fixed public help strings,
intentional internal/parser/archive surfaces, deferred runtime target-ID
normalization, and refresh commands for extending the queue.

The Phase 3 test and fixture discipline plan now opens the fixture-map lane for
Remi and CCS conversion/publication evidence. It keeps the first slice
documentation-first so fixture ownership and verification recipes are explicit
before helper moves or hotspot decompositions begin.

The first fixture-map artifact records Remi and CCS conversion/publication
fixture families as contributor-facing proof surfaces, including owner files,
consuming tests, fast and medium proof commands, slow proof boundaries, and
safety notes.

Assistant routing and the CCS/Remi module guides now point contributors to the
fixture map before changing conversion golden cases, support-matrix evidence,
publication gates, static fixture uploads, or `conary-test` manifest behavior.

The Phase 4 install hotspot decomposition plan now opens the first code-moving
maintenance slice. It chooses `apps/conary/src/commands/install/mod.rs` over the
CCS install hotspot so CCS native contract work can proceed separately, and it
narrows the first refactor to the install-side legacy replay adapter plus docs
routing.

The first Phase 4 implementation slice extracted the install legacy replay
adapter into `apps/conary/src/commands/install/legacy_replay.rs` and refreshed
assistant plus fixture routing so replay refactors no longer start in the full
install orchestrator.

The Phase 5/6 feature ownership and workflow UX plan now combines subsystem-map
consolidation with contributor workflow routing. It scopes the first slice to a
canonical feature ownership map, human/assistant entrypoint links, focused proof
recipes, and cross-system interaction gates while leaving MIT-only licensing and
live-system mutation UX as separate follow-up designs.

The first Phase 5/6 implementation slice adds
`docs/modules/feature-ownership.md` as the canonical feature-card map for major
Conary capabilities. The map records start-here files, neighboring systems,
focused proof commands, broader interaction gates, docs to update, and safety
notes so feature-focused work can stay local without hiding cross-system
coupling.

Assistant and contributor entrypoints now route feature-scoped work through the
feature ownership map. `docs/llms/README.md`, `docs/llms/subsystem-map.md`,
`CONTRIBUTING.md`, and the pull request template point contributors toward
start-here files, focused proof commands, and broader interaction gates without
turning any one entrypoint into a duplicate manual.

The Phase 7 enforcement and drift-control plan now opens the final
maintainability-roadmap lane. It scopes the first slice to a warn-only local
drift report that maps changed paths to feature ownership hints, focused proof
commands, docs-audit health, and current Rust hotspot context without adding a
hard CI gate or line-count threshold.

The first Phase 7 implementation slice adds
`scripts/maintainability-drift-report.sh` as a warn-only local report for
changed-path owner hints, docs-audit health, focused proof suggestions, and
current Rust hotspot context. Contributor and assistant guidance now point to
the report as review support for broad maintenance work, not as a hard gate.

The live-system mutation UX redesign spec now opens the first post-Phase 7
follow-up design from the maintainability roadmap. It inventories the current
global acknowledgement flag, command-risk taxonomy, conaryd request surface,
tests, docs, and integration manifests, then scopes a risk-tiered replacement
that reduces daily CLI friction without weakening active-host, generation,
recovery, or daemon safety gates.

The live-system mutation UX implementation plan now turns that reviewed design
into a task-by-task behavior packet. It keeps the old global flag parseable as
a hidden compatibility alias, removes the live-system gate from DB/CAS-only
adoption paths, adds command-scoped apply intent for active-host and
always-live flows, preserves conaryd request compatibility, expands manifest
validation coverage, and inventories the active source, script, site, docs,
manpage, and test surfaces that must migrate together.

The Phase 8 install CCS transaction decomposition plan now continues the
install-hotspot lane after the legacy replay extraction. It scopes the next
behavior-preserving slice to `apps/conary/src/commands/install/ccs_transaction.rs`,
keeping shared live-root transaction machinery and manifest-provides
persistence in `install/mod.rs` while moving the direct CCS transaction entry
point, option/result pair, manifest selection helper, hook-status helpers, and
scriptlet capability gate into a focused install submodule.

The Phase 9 CCS payload paths decomposition plan continues the CCS
maintainability work by targeting the current largest source file,
`apps/conary/src/commands/ccs/install.rs`. It extracts CCS payload path
normalization and symlink safety helpers into
`apps/conary/src/commands/ccs/payload_paths.rs`, while keeping dependency
validation, component selection, capability policy, trust verification, and
command orchestration in `ccs/install.rs`, and preserving existing
cross-module re-exports.

The Phase 10 update selection decomposition targeted the former flat update
hotspot, `apps/conary/src/commands/update.rs`. It converted the update command
into a directory module and extracted repository candidate
selection, latest-mode source-switch previewing, and security-update metadata
eligibility into `apps/conary/src/commands/update/selection.rs`, while keeping
update execution, delta handling, collection orchestration, replatform
previewing, and adopted-package authority policy in `update/mod.rs`.

The Phase 11 update adopted authority decomposition plan continues reducing
`apps/conary/src/commands/update/mod.rs` after the selection split. It extracts
adopted-package update takeover decisions, native package-manager fallback
selection, adopted skip records, and native-authority summary text into
`apps/conary/src/commands/update/adopted_authority.rs`, while keeping update
selection, collection orchestration, delta/full update execution, and legacy
replay preflight in their existing owners.

The Phase 12 update collection decomposition plan continues reducing
`apps/conary/src/commands/update/mod.rs` after the selection and adopted
authority splits. It extracts collection update target modeling and
`update @collection` orchestration into
`apps/conary/src/commands/update/collection.rs`, while keeping single-package
update execution, candidate selection, adopted-package authority policy,
collection management commands, delta/full update preparation, and legacy
replay preflight in their existing owners.

The Phase 13 update module completion decomposition plan finishes the update
hotspot lane by targeting the remaining responsibilities in
`apps/conary/src/commands/update/mod.rs`. It plans to turn `update/mod.rs` into
a routing hub while extracting single-package update execution into
`apps/conary/src/commands/update/package.rs`, source-policy/replatform update
previewing into `apps/conary/src/commands/update/source_policy.rs`, pinning
commands into `apps/conary/src/commands/update/pinning.rs`, and delta statistics
into `apps/conary/src/commands/update/delta_stats.rs`.

## Verification Commands

- `bash scripts/docs-audit-inventory.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh`
- `cargo test -p conary security_update -- --nocapture`
- `cargo run -p conary -- update --help`
- `cargo run -p conary-test -- list`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1`
- `cargo run -p conary-test -- run --suite phase3-group-n-qemu --distro fedora44 --phase 3`
- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`
- `cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`
- `cargo test -p conary --lib adopt::native_handoff`
- `cargo test -p conary --lib adopt::unadopt`
- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3`
- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro ubuntu-26.04 --phase 3`
- `cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro arch --phase 3`
- `CONARY_LOCAL_VALIDATION_RUN_ID=readiness-completion-20260522 scripts/local-qemu-validation.sh`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4`
- `cargo test -p conary-core generation::export`
- `cargo test -p conary-test qemu_image`
- `cargo test -p conary-test qemu_boot_`
- `cargo test -p conary --test cli_daily_ux`
- `cargo test -p conary --test native_pm_daily_driver`
- `cargo test -p conary live_host_safety`
- `cargo test -p conary --test live_host_mutation_safety`
- `cargo run -p conary -- system completions bash >/tmp/conary-completion.bash`
- `cargo run -p conary -- system completions zsh >/tmp/conary-completion.zsh`
- `bash scripts/bootstrap-vm/test-validate-selfhost-vm.sh`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `git diff --check`
- `rg -n "replace dnf|replace apt|replace pacman|risk-free|unadopt|takeover" README.md ROADMAP.md docs`
- `rg -n "rebuild or remoun[t]|MOUNTE[D]|refresh the Fedora 44 QEMU source imag[e]|active execution pla[n]|active umbrella desig[n]|remain active at the top leve[l]" README.md ROADMAP.md CHANGELOG.md CONTRIBUTING.md SECURITY.md AGENTS.md GEMINI.md docs apps/conary-test deploy site web .github`
- `cargo test -p conaryd test_package_routes_return_not_implemented`
- `cargo test -p conaryd package_routes`
- `cargo run -p conaryd -- --help`
- `npm run check` in `site/`
- `npm run build` in `site/`
- `npm audit --audit-level=moderate` in `site/`

Current limited-preview evidence is tracked by
`docs/superpowers/limited-preview-release-checkpoint-2026-05-16.md`; the narrow
public tester copy is tracked by
`docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`.

## Residual Risks

- The live Remi repository rows and public metadata have been moved to
  26.04/`resolute`, but the root-owned production config file still needs an
  ops follow-up if we want the file contents to mirror the DB state.
- Historical release notes and archived design/spec files may still mention
  older distro names or broad parser support as historical context.
- `system unadopt --all` remains the pre-generation escape hatch. After a
  Conary generation is selected, users must use the explicit
  `system native-handoff` flow; it preserves native package files and native
  package-manager databases but does not import native transaction history or
  silently take over adopted packages.
- Group O `TGE04` failed in the 2026-05-16 checkpoint, but the 2026-05-19
  refresh fixed the installed-runtime initramfs path and restored the Group O
  gate to 4 passed / 0 failed / 0 skipped / 0 cancelled.
- Group P ISO export is implemented, listed by `conary-test`, and green in the
  focused 2026-05-21 local KVM run: `TISO01` passed ISO export, provenance
  sidecar, host copy-back, readonly-carrier boot, and writable `/etc` overlay
  proof with 1 passed / 0 failed / 0 skipped / 0 cancelled.
- The previous `tough` advisory path has been removed from `Cargo.lock`; the
  remaining RustSec waiver is `RUSTSEC-2023-0071` for `rsa 0.9.10`.

## Final Counts

- Total tracked doc-like files audited: 157
- `verified-no-change`: 13
- `corrected`: 57
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
