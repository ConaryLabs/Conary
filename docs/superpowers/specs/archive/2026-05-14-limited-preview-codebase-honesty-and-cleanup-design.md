---
last_updated: 2026-05-14
revision: 2
summary: Fresh design for a limited-preview codebase honesty, cleanup, deduplication, documentation alignment, and agentic-review reconciliation pass
---

# Limited Preview Codebase Honesty And Cleanup: Design Spec

**Date:** 2026-05-14
**Status:** Draft design for user review; DeepSeek and agentic review feedback
reconciled
**Goal:** Prepare Conary for a limited public preview by reviewing the
release-facing codebase with fresh eyes, removing misleading surfaces,
reducing targeted duplication, simplifying large modules where it helps
reviewability, and making docs match actual behavior.

---

## Purpose

The previous active release plan remains useful as a validation gate. It
tracks CI, security, local QEMU evidence, distro matrix, and public release
readiness. This design covers a different pass: a line-by-line codebase
honesty and cleanup review before the limited public preview.

The central rule is simple:

> If Conary exposes a command, API route, test surface, or public doc claim, it
> should either work, fail explicitly with clear preview guidance, or be
> hidden/reworded until it is real.

This pass should prefer small, high-signal corrections over broad redesign. It
should make the current preview slice more trustworthy without widening the
release scope.

## Current Preview Contract

The supported limited public preview surface is:

- Fedora 44, Ubuntu 26.04 LTS, and Arch Linux
- user-facing CLI install, remove, and update workflows
- immutable composefs/EROFS generations
- raw/qcow2 generation artifact export
- Remi conversion and serving
- local KVM-backed QEMU validation through `scripts/local-qemu-validation.sh`

The following remain deferred and must stay honest:

- conaryd package install/remove/update execution
- ISO generation export
- portable signed generation bundles
- non-x86_64 generation boot assets
- remote Forge QEMU/deep validation until a KVM-capable runner exists
- broad conaryd package executor implementation

## Non-Goals

- Do not implement every forward-looking feature.
- Do not convert historical archive docs into current promises.
- Do not rewrite the whole architecture for neatness.
- Do not add schema migrations unless a focused cleanup truly requires one.
- Do not weaken trust, signature, federation, or sandbox defaults.
- Do not remove explicit `NotImplemented` responses merely to make scans look
  clean.

## Review Principles

### Honesty Before Polish

A command that fails with a direct, tested "not implemented for preview"
message is acceptable. A command that appears to succeed while doing nothing,
or claims a bootable artifact that is not bootable, is not acceptable.

### Reviewable Boundaries

Large files are not automatically wrong, but each large file should have a
clear review boundary. If a file combines unrelated responsibilities, split it
only where doing so makes line-by-line review safer.

### Deduplicate Proven Patterns

Deduplicate when there is already a stable helper or obvious single owner.
Avoid inventing abstractions for structural similarity alone.

### Current Docs Win

Living docs should describe the current product. Historical specs and archived
plans may retain old wording if they are clearly historical and do not appear
as active instructions.

## Finding Categories

Each finding in the implementation plan should be classified as one of:

- **Release blocker:** likely to make the preview misleading, unsafe, or
  unverifiable.
- **Misleading public surface:** exposed command/API/doc behavior suggests a
  feature works when it does not.
- **Cleanup/simplification:** code is harder than needed, oversized, or has
  stale forward-looking scaffolding.
- **Duplication/consolidation:** repeated logic has a safe shared owner.
- **Documentation drift:** living docs disagree with current code, tests, or
  preview scope.
- **Deferred but honest:** intentionally not implemented and already clearly
  represented.

## Review Coverage Ledger

Because this pass is intended to support a thorough line-by-line review, each
execution plan should create or update a review coverage ledger for its slice.
The ledger may live beside the active implementation plan and should include at
least:

- file path
- owning slice
- reviewer or agent
- review status (`pending`, `reviewed`, `changed`, `deferred`)
- finding category
- decision
- verification command or reason verification is not applicable

A file is not considered reviewed until the ledger records either a concrete
finding with a decision or an explicit "reviewed, no change" disposition.

## Proposed Review Slices

### Slice 1: CLI And Public Surface Honesty

**Primary files:**

- `apps/conary/src/cli/**`
- `apps/conary/src/dispatch.rs`
- `apps/conary/src/commands/**`
- `apps/conary/tests/integration/remi/manifests/phase4-*.toml`
- `README.md`
- `docs/conaryopedia-v2.md`

**Review questions:**

- Does each visible command have an implementation path?
- If a command is preview-only, does help text say so?
- Does the command return a useful non-success error when it cannot do the
  requested work?
- Do Phase 4 manifests assert current behavior rather than stale stubs?

**Initial findings to resolve or explicitly classify:**

- README quick-start and feature examples show copy/paste install, generation,
  takeover, and state-revert commands without the required
  `--allow-live-system-mutation` acknowledgement. These examples should either
  include the flag, use `--dry-run`, or clearly explain why the release-preview
  safety guard will stop direct mutation.
- `conary system generation export --format iso` is advertised in CLI help, but
  `crates/conary-core/src/generation/export.rs` returns
  `Error::NotImplemented` for ISO.
- `conary bootstrap image --format iso` can produce an ISO-shaped artifact, but
  the implementation warns that boot artifact population is not implemented and
  reports it is not EFI/BIOS bootable.
- The README provenance/SBOM example currently shows
  `conary system sbom nginx --format spdx`, but `system sbom` only supports
  CycloneDX. SPDX output belongs to `conary provenance export`.
- The top-level `conary export` command produces an OCI image layout, while
  `conary system generation export` produces raw/qcow2 disk images. The names
  are technically distinct but easy to confuse.
- `conary system state revert` and `conary system state rollback` both expose
  rollback-like wording with different identifiers. The help text should make
  the state-number versus changeset distinction obvious.
- `conary system generation recover` should be described as a manual/initramfs
  recovery helper if it remains user-facing.
- `docs/conaryopedia-v2.md` says automation history, daemon mode, and config
  persistence return explicit "not yet implemented" guidance, but
  `apps/conary/src/commands/automation.rs` now contains real history/config
  paths and tests around them.
- Phase 4 Group D still expects the old automation history guidance string.

**Acceptance criteria:**

- CLI help and command behavior agree for every release-facing command touched.
- Preview-only features fail explicitly and are tested as such.
- Copy/paste README examples run against the documented format support.
- No command succeeds silently when it did not perform or clearly preview the
  requested operation.

### Slice 2: conaryd Honesty Boundary

**Primary files:**

- `apps/conaryd/src/bin/conaryd.rs`
- `apps/conaryd/src/daemon/mod.rs`
- `apps/conaryd/src/daemon/routes.rs`
- `apps/conaryd/src/daemon/routes/system.rs`
- `apps/conaryd/src/daemon/routes/transactions.rs`
- `apps/conaryd/src/daemon/client.rs`
- `README.md`
- `ROADMAP.md`
- `docs/ARCHITECTURE.md`

**Review questions:**

- Which routes are real read/control-plane routes?
- Which routes are intentionally package-executor-deferred?
- Do all deferred routes return 501 instead of empty successful stubs?
- Does the daemon client describe the same contract as the server?

**Initial findings to resolve or classify:**

- Package install/remove/update routes already return explicit 501 responses.
- Job execution has a defense-in-depth `not implemented` fallback for job kinds
  rejected at the API boundary.
- `POST /v1/transactions/dry-run` currently returns `200 OK` with a synthetic
  count of requested package names. Unless this becomes a real resolver/planner
  route, it is a successful package-operation stub and should return explicit
  preview/deferred guidance.
- Generic `POST /v1/transactions` rejects install/remove/update operations as
  `400 Bad Request`, while the direct package routes use 501. Deferred package
  executor surfaces should use one consistent not-implemented response shape.
- `/v1/system/states` currently returns an empty `200 OK` list, which may be
  less honest than a typed response or explicit 501 unless it is genuinely a
  supported empty read route.
- TCP listener configuration fails explicitly because only Unix socket accept
  is implemented; this is acceptable preview honesty.

**Acceptance criteria:**

- Every conaryd route is categorized as supported, deferred with 501, or
  internal/test-only.
- Empty successful stubs are removed or justified in tests/docs.
- Public daemon docs match route behavior.

### Slice 3: Generation, Export, And Bootstrap Projection

**Primary files:**

- `crates/conary-core/src/generation/**`
- `apps/conary/src/commands/generation/**`
- `crates/conary-core/src/bootstrap/**`
- `apps/conary/src/commands/bootstrap/**`
- `docs/operations/post-generation-export-follow-up-roadmap.md`
- `docs/operations/bootstrap-selfhosting-vm.md`
- `docs/INTEGRATION-TESTING.md`

**Review questions:**

- Are raw/qcow2 export and OCI export clearly separated from reserved ISO work?
- Are architecture limits enforced and documented in one place?
- Does bootstrap image help distinguish bootable qcow2/raw paths from incomplete
  ISO projection?
- Are Tier 2 and self-hosting paths truthful now that `Tier2Builder::build_all`
  has real package iteration rather than an old stub?

**Initial findings to resolve or classify:**

- Generation disk export correctly rejects ISO, but CLI help still presents ISO
  as an ordinary format.
- Bootstrap ISO image creation warns that boot artifact population is missing;
  public help should not imply this is a normal bootable release artifact.
- Current Tier 2 implementation appears more real than stale docs/comments that
  previously called it unused or stubbed, so active docs and code comments
  should be checked. External audit feedback repeated the stale-comment claim;
  local inspection shows `Tier2Builder::build_all()` now sets up the chroot and
  iterates `TIER2_ORDER`.
- Non-x86_64 boot assets return explicit `NotImplemented`, which matches the
  preview contract.

**Acceptance criteria:**

- Raw/qcow2 are the only release-ready disk-image export formats presented as
  such.
- ISO is consistently reserved/follow-up unless proven bootable.
- Bootstrap docs and CLI help do not overpromise bootability.
- Group N/O and composefs modernization validation docs remain current.

### Slice 4: Remi Chunk Store, Auth, And Handler Consistency

**Primary files:**

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/cache.rs`
- `apps/remi/src/server/chunk_gc.rs`
- `apps/remi/src/server/handlers/chunks.rs`
- `apps/remi/src/server/handlers/derivations.rs`
- `apps/remi/src/server/handlers/profiles.rs`
- `apps/remi/src/server/handlers/seeds.rs`
- `apps/remi/src/server/handlers/mod.rs`
- `apps/remi/src/server/routes/public.rs`
- `apps/remi/src/server/routes/admin.rs`
- `apps/remi/src/server/routes/mcp.rs`
- `apps/remi/src/server/auth.rs`
- `apps/remi/src/server/rate_limit.rs`
- `apps/remi/src/server/admin_service.rs`
- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/mod.rs`

**Review questions:**

- Is CAS/chunk path construction owned by one helper?
- Are local disk, R2, Bloom, GC, and conversion code using the same chunk
  layout contract?
- Are write endpoints consistently protected by admin auth and rate limiting?
- Is error response shape intentionally split between public and admin routers?

**Initial findings to resolve or classify:**

- CAS path construction is still hand-rolled in conversion/cache/GC paths even
  though `conary_core::filesystem::object_path` exists.
- `scan_chunk_hashes` and `extract_hash_from_path` are duplicated between
  `chunk_gc.rs` and `handlers/chunks.rs`.
- seeds/profiles/derivations PUT endpoints are mounted on the public router,
  authenticate inline, and explicitly bypass admin-router rate limiting. Treat
  this as a release/security decision: either move them under admin protections
  or document and test equivalent public-path controls.
- `handlers/mod.rs` documents split public/admin error formats and includes a
  code note about later unification; this is a valid later cleanup if kept
  deliberate.
- Remi MCP operations such as `chunk_gc`, `canonical_rebuild`, and
  `canonical_fetch` appear to access lower-level modules/state directly where
  adjacent MCP tools delegate through `admin_service.rs`; verify whether
  service-layer ownership should be restored for consistency.

**Acceptance criteria:**

- Chunk path and hash-scan helpers have one owner per crate or use the core
  helper when that preserves the chunk-dir versus objects-dir contract.
- Write endpoints are either routed through the admin protections or clearly
  documented and tested as intentionally public-path writes with equivalent
  controls.
- Any preview decision to keep public-path write endpoints must include the
  exact auth/rate-limit/audit contract in docs or tests.
- No broad Remi architecture refactor is introduced without a narrow need.

### Slice 5: Package Mutation And Source Selection Core

**Primary files:**

- `apps/conary/src/commands/install/**`
- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/remove.rs`
- `crates/conary-core/src/resolver/**`
- `crates/conary-core/src/repository/**`
- `crates/conary-core/src/transaction/**`
- `crates/conary-core/src/model/**`

**Review questions:**

- Do install/update/remove all flow through the current SAT/provider and
  composefs generation contract?
- Are legacy compatibility helpers still needed?
- Are Local CAS, recipe strategy, patches, and delta messages honest when not
  wired?
- Are duplicated dependency/source-selection paths still justified?

**Initial findings to resolve or classify:**

- Large command files and provider modules are review magnets rather than
  automatic refactor targets.
- `LocalCas resolution not yet implemented` and recipe strategy warnings appear
  explicit, but should be checked against any CLI claim that makes them sound
  supported.
- `allow(dead_code)` annotations in install/update helpers should be reviewed
  for stale "future unification" scaffolding.
- install/remove/update appear to repeat transaction lifecycle setup and finish
  boilerplate. A shared helper may reduce drift, but only after verifying that
  recovery and snapshot behavior is truly identical across the three flows.

**Acceptance criteria:**

- Public install/update/remove behavior remains green and preview-scoped.
- Any deferred source strategy is surfaced as explicit behavior, not silent
  downgrade.
- Splits are made only where they reduce real review risk.

### Slice 6: Test Harness And Phase 4 Truth

**Primary files:**

- `apps/conary-test/src/**`
- `apps/conary/tests/integration/remi/manifests/**`
- `apps/conary-test/README.md`
- `docs/INTEGRATION-TESTING.md`

**Review questions:**

- Are test manifests validating current behavior rather than old scaffolding?
- Are preview-only assertions consistent across docs and command behavior?
- Are QEMU release gates still represented as local evidence, not remote Forge
  evidence?
- Do HTTP and MCP service paths share business logic where expected?

**Initial findings to resolve or classify:**

- Phase 4 docs correctly say preview-only features should fail cleanly, but
  specific automation assertions appear stale.
- conary-test repeats Remi-proxy fallback logic across HTTP handlers and MCP
  tools; the shared service layer is the likely owner if verification confirms
  the paths are equivalent.
- Current conary-test HTTP and MCP fallback paths are not yet equivalent: HTTP
  list fallback preserves ascending compatibility ordering, while service/MCP
  paths use the newer service ordering. Do not deduplicate this mechanically
  until the canonical response shape is chosen.
- The Fedora 44 distro key appears throughout conary-test and Remi test
  fixtures. A shared default-test-distro constant or fixture helper would
  reduce future distro-matrix churn, but manifest data should remain explicit
  where the distro value is part of the scenario.
- The integration `config.toml` and embedded/default test config share a schema
  and should be reviewed for drift-prone duplication.
- `conary-test` has multiple 1000+ line files that deserve focused review:
  CLI, config, runner, QEMU, service, MCP, and handlers.

**Acceptance criteria:**

- `cargo run -p conary-test -- list` stays green.
- Any changed manifest has focused evidence from the owning suite or a
  documented reason not to run it.
- Local QEMU gate wording stays concrete and date-stamped.

### Slice 7: Documentation And Assistant Guidance

**Primary files:**

- `README.md`
- `ROADMAP.md`
- `CHANGELOG.md`
- `SECURITY.md`
- `CONTRIBUTING.md`
- `AGENTS.md`
- `CLAUDE.md`
- `GEMINI.md`
- `.github/copilot-instructions.md`
- `docs/ARCHITECTURE.md`
- `docs/conaryopedia-v2.md`
- `docs/llms/**`
- `docs/modules/**`
- `docs/operations/**`
- `docs/superpowers/documentation-accuracy-audit-*.tsv`

**Review questions:**

- Do public docs match the preview contract?
- Do assistant docs point to canonical docs instead of duplicating volatile
  facts?
- Are active plans/specs clearly active, while historical docs are clearly
  historical?
- Does the documentation audit ledger include the new spec and any changed
  docs?

**Initial findings to resolve or classify:**

- `docs/ARCHITECTURE.md` still lists resolver files that no longer exist
  (`graph.rs`, `engine.rs`) while current resolver shape is SAT/provider based.
- Documentation audit tooling passes, but semantic drift still exists.
- The active `2026-05-06` release-readiness plan is gate-oriented and should
  remain separate from this code cleanup design.

**Acceptance criteria:**

- `bash scripts/docs-audit-inventory.sh` reflects the active doc set.
- `bash scripts/check-doc-audit-ledger.sh ... --require-complete` passes after
  ledger updates.
- Stale active-doc phrases are removed or reframed as historical.
- Each documentation slice records the targeted stale-claim sweep used for the
  changed surface, not just the inventory/ledger result.

## Large File Review Queue

These files should be reviewed deliberately because they are large enough to
hide stale behavior or duplicated responsibilities:

| File | Lines | Review Focus |
| --- | ---: | --- |
| `apps/conary/src/commands/ccs/install.rs` | 3277 | CCS install path, direct mutation, hooks, composition contract |
| `apps/conary/src/commands/install/mod.rs` | 2328 | install orchestration, helper drift, dead-code allowances |
| `apps/remi/src/server/conversion.rs` | 2239 | conversion pipeline, CAS writes, distro/repo assumptions |
| `apps/conary/src/commands/model.rs` | 2029 | model apply/check/publish promises |
| `apps/conaryd/src/daemon/routes.rs` | 2018 | route ownership, response envelopes, tests |
| `crates/conary-core/src/generation/builder.rs` | 1955 | runtime input contract and boot asset publication |
| `apps/conary/src/dispatch.rs` | 1948 | command routing and live-host mutation gates |
| `apps/conary/src/commands/bootstrap/mod.rs` | 1946 | bootstrap command promises and image/seed flows |
| `crates/conary-core/src/model/replatform.rs` | 1925 | cross-distro forward-looking behavior |
| `crates/conary-core/src/resolver/provider/mod.rs` | 1901 | candidate loading/matching contract |
| `crates/conary-core/src/model/parser.rs` | 1872 | public model schema and preview defaults |
| `crates/conary-core/src/container/mod.rs` | 1819 | sandbox/runtime guarantees |

The implementation plan should not start by splitting all of these. It should
use them as review queues and only create split tasks where a file mixes
independent responsibilities that are actively being touched.

## DeepSeek Feedback Integration

DeepSeek produced an independent findings pass on 2026-05-14. Treat it as
review input, not an authority. Verified additions from that pass are folded
into the slices above:

- README SBOM example mismatch.
- SBOM command-surface fragmentation across system, provenance, and derivation
  data sources.
- `conary export` versus `conary system generation export` naming ambiguity.
- conary-test Remi-proxy fallback duplication.
- install/remove/update transaction-lifecycle boilerplate.
- Fedora 44 fixture/default duplication.
- Remi MCP operations that should be checked for admin-service delegation.

Corrections and cautions:

- The Tier 2 bootstrap finding in the external report is stale. Current code no
  longer has `build_all()` returning `NotImplemented`; the stale comment is the
  issue.
- The external report says all tests pass. Do not reuse that as current
  evidence unless this session reruns the commands.
- The suggested `conary export` rename should be treated as an API design
  question. A compatibility alias or help-text clarification may be safer than
  a hard rename before preview.

When more external findings arrive:

1. Compare each DeepSeek finding against this design's categories.
2. Add new concrete findings to the relevant slice.
3. Mark duplicates as corroborated rather than rewriting them.
4. Treat claims without file/line evidence as prompts for local verification.
5. Do not widen the preview contract unless the user explicitly approves.

If DeepSeek finds a release blocker, prioritize it ahead of cleanup-only work.

## Agentic Review Integration

An agentic review pass on 2026-05-14 validated the design and added several
important amendments:

- README examples must account for the live-system mutation guard.
- conaryd transaction dry-run and generic transaction creation need the same
  honesty treatment as direct package routes.
- Remi public-path write endpoints are a preview security decision, not merely
  a cleanup note.
- conary-test HTTP/MCP fallback deduplication cannot be mechanical until the
  canonical ordering and response shape are chosen.
- A line-by-line cleanup pass needs a review coverage ledger with explicit
  per-file dispositions.
- Documentation verification needs targeted stale-claim sweeps in addition to
  audit inventory and ledger checks.

The same review found that several existing design findings are already well
grounded: README SBOM mismatch, bootstrap/generation ISO overpromise, stale
automation docs and Phase 4 assertion, and architecture resolver-map drift.

## Proposed First Implementation Plan

After this design is approved, the first implementation plan should cover:

1. Create the first review coverage ledger and seed it with the files touched
   by this implementation slice.
2. CLI/public-surface honesty fixes for README live-mutation examples, README
   SBOM, generation/bootstrap ISO wording, automation drift, Phase 4 automation
   expectations, and command-help clarity.
3. conaryd route honesty cleanup for empty successful stubs, package-operation
   dry-run, and generic transaction creation response shape.
4. Remi public-path write endpoint decision and service-boundary classification
   before any broad handler movement.
5. conary-test Phase 4 truth fixes and Remi fallback response-shape decision;
   defer fallback deduplication until equivalence is proven.
6. Documentation alignment for the changed behavior, targeted stale-claim
   sweeps, and audit ledger updates.

Defer broader large-file decomposition, Remi CAS/chunk helper consolidation,
and install/remove/update transaction-lifecycle helper work until the owning
slice is actively under review and the public-surface honesty fixes are done.

## Verification Strategy

Use focused checks for each slice, then run the normal workspace gates before
claiming completion.

Baseline fast checks:

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Docs checks:

```bash
bash scripts/docs-audit-inventory.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
rg -n "not yet implemented|not yet recorded|still to run|ISO|SPDX|Forge|QEMU|remaining" README.md ROADMAP.md docs apps/conary/tests/integration/remi/manifests
```

Owning package checks:

```bash
cargo test -p conary
cargo test -p conary-core
cargo test -p remi
cargo test -p conaryd
```

Manifest checks:

```bash
cargo run -p conary-test -- run --suite phase4-group-d --distro fedora44 --phase 4
```

Run the owning Phase 4 group whenever a Phase 4 manifest expectation changes.
If the group is not run, record the reason in the implementation plan and the
review coverage ledger.

QEMU checks stay release-gate work, not default cleanup verification, unless a
change touches generation export, boot activation, QEMU fixtures, or
`scripts/local-qemu-validation.sh`.

## Open Questions

- Should ISO options remain visible with explicit "reserved/not preview
  supported" wording, or should they be hidden from public help until bootable?
- Should conaryd `/v1/system/states` become a real read route now, or return
  explicit 501 with the other deferred system mutation routes?
- Should automation daemon mode be treated as supported foreground preview
  behavior, or reworded as experimental until service integration exists?
- Should Remi's public-path write endpoints move under admin routes now, or get
  equivalent inline rate limiting first?
- Should SBOM output be unified under one command group, or should the current
  system/provenance/derivation split stay but get clearer names and examples?
- Should `conary export` remain as the OCI command with clearer help, gain a
  more explicit alias, or move under generation commands in a compatibility
  preserving way?
- Should `system state revert` and `system state rollback` remain separate
  commands with sharper help text, or converge around one public verb?
- Should this design replace the old active release-readiness plan as the next
  execution target, or live beside it as a cleanup track feeding that final
  gate?
