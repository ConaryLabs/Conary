# External Audit Response Umbrella Plan

> **Umbrella register:** This is not an executable mega-plan. Use it to merge
> high-context external model findings, verify them against the repository, and
> split them into bounded `/goal` implementation packets.

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans only after a track below has been promoted into a
> focused implementation plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert external Fable/DeepSeek/Gemini repository-audit findings into
verified, prioritized, bounded repair tracks without letting one expensive
review become a shapeless maintenance backlog.

**Architecture:** Treat every external finding as review input until a local
repo check proves it. Maintain a small finding register, group accepted findings
by the invariant they protect, and promote one track at a time into a detailed
task plan. The first recommended implementation track is conaryd authorization
fail-closed policy, followed by conary-test evidence trust.

**Tech Stack:** Rust, Cargo, conary-test, TOML integration manifests, shell
validators, GitHub Actions, docs-audit, docs-truth, feature coherency ledger.

---

## Current Repository Facts

- Umbrella execution packets were drafted from `main` after commit
  `a55d3b32388dc5e470a3cff1ca2585bad05e1791`.
- The Fable review dated 2026-06-10 was partially spot-checked locally before
  this plan: QEMU skip-as-pass, result-gate fail-open, release-build test-gate
  absence, duplicate manifest IDs, and the `stderr_not_contains` unknown-key
  issue all matched current repo shape.
- The Gemini review dated 2026-06-10 was partially spot-checked locally before
  this revision: hardcoded conaryd trusted GID fallbacks, PolicyKit/docs
  mismatch, route-extractor method limits, local QEMU build scope, SBOM routing
  wording, missing install routing modules, and Remi deploy double-restart shape
  all matched current repo shape at least structurally.
- The DeepSeek review dated 2026-06-10 was partially spot-checked locally before
  this revision: TOML integration suites are listed but not executed in
  workflows, Phase 4 counts differ from docs, conaryd deploy jobs are gated on
  an unreachable deploy mode, some script executable bits differ from the
  common `./scripts/...` pattern, and the integration CLI table omits commands.
- This umbrella and the linked track packets must be registered in the
  documentation accuracy audit ledger in the same commit that locks the packet.

## Execution Packet Index

Use this umbrella as the register and sequencing guide. Use the linked focused
files for actual `/goal` execution.

| Track | Design | Implementation Plan |
| --- | --- | --- |
| Track 0: conaryd Authorization Fail-Closed Policy | `docs/superpowers/specs/archive/2026-06-10-conaryd-authorization-fail-closed-policy-design.md` | `docs/superpowers/plans/archive/2026-06-10-conaryd-authorization-fail-closed-implementation-plan.md` |
| Track 1: conary-test Evidence Trust | `docs/superpowers/specs/archive/2026-06-10-conary-test-evidence-trust-design.md` | `docs/superpowers/plans/archive/2026-06-10-conary-test-evidence-trust-plan.md` |
| Track 2: Release And CI Safety | `docs/superpowers/specs/archive/2026-06-10-release-ci-safety-design.md` | `docs/superpowers/plans/archive/2026-06-10-release-ci-safety-plan.md` |
| Track 3: Deploy And Artifact Integrity | `docs/superpowers/specs/archive/2026-06-10-deploy-artifact-integrity-design.md` | `docs/superpowers/plans/archive/2026-06-10-deploy-artifact-integrity-plan.md` |
| Track 4: Public Surface And Agent Routing Truth | `docs/superpowers/specs/archive/2026-06-10-public-surface-agent-routing-truth-design.md` | `docs/superpowers/plans/archive/2026-06-10-public-surface-agent-routing-truth-plan.md` |
| Track 5: Remaining Validator Soundness | `docs/superpowers/specs/archive/2026-06-10-validator-soundness-follow-up-design.md` | `docs/superpowers/plans/archive/2026-06-10-validator-soundness-follow-up-plan.md` |

Do not execute from the register table below without first opening the focused
design and plan for the selected track.

## Intake Rules

- Do not implement directly from external model text.
- For each finding, record whether it is `verified`, `partially-verified`,
  `inferred`, `unverified`, or `rejected`.
- Preserve the external finding source (`Fable`, `DeepSeek`, `Gemini`, or
  `Codex-local`) so conflicting claims can be resolved explicitly.
- Promote a finding only when it is tied to a concrete repo invariant: evidence
  validity, release safety, artifact integrity, public-surface truth, agent
  routing, or validator soundness.
- Keep each `/goal` small enough to finish with tests, docs, and a clean push.

## Finding Register

| ID | Source | External Severity | Local Status | Track | Summary | Next Action |
| --- | --- | --- | --- | --- | --- | --- |
| AUDIT-AUTH-001 | Gemini | Critical | verified | Track 0 | conaryd resolves missing `wheel` by hardcoding GID 10, which is `uucp` on Debian/Ubuntu-like systems and receives `Permission::Full`. | Promote before evidence-harness work unless local policy says trusted GIDs are being removed separately. |
| AUDIT-AUTH-002 | Gemini | Important | verified | Track 0 | conaryd docs say non-root PolicyKit write authorization is fail-closed, but code grants full access to trusted GIDs before PolicyKit. | Resolve by fail-closing trusted GID fallback and updating docs/tests to match the intended policy. |
| AUDIT-QEMU-001 | Fable | Critical | verified | Track 1 | QEMU missing-tool/download skips return `exit_code=0`, so skipped QEMU boot steps can be reported as passed. | Promote to Track 1. |
| AUDIT-QEMU-002 | Gemini | Important | verified-by-structure | Track 1 | `scripts/local-qemu-validation.sh` builds only `conary` and `conary-test`, so Remi/conaryd changes can be validated against stale binaries. | Promote with Track 1 local validation changes. |
| AUDIT-RESULT-001 | Fable | Critical | verified | Track 1 | `scripts/check-conary-test-result-gate.sh` accepts `{}` and trusts missing or self-inconsistent summary counts. | Promote to Track 1. |
| AUDIT-RESULT-002 | Gemini | Minor | design-decision | Track 1 | Gemini suggests skipped tests may need warning-only mode, which conflicts with release-evidence strictness. | Decide in Track 1: default release gate should fail skipped; optional non-release mode may warn. |
| AUDIT-INTEGRATION-CI-001 | DeepSeek | Important | verified | Track 1 | Workflows run `cargo test -p conary-test` unit tests and `conary-test -- list`, but no workflow executes the TOML integration suites with `conary-test run`. | Promote to Track 1 docs and CI decision. |
| AUDIT-RELEASE-001 | Fable | Critical | verified-by-structure | Track 2 | Tag-triggered release builds and deploy follow-up do not depend on a workspace test gate. | Promote after Track 1 or pair with CI hardening. |
| AUDIT-MANIFEST-001 | Fable | Important | verified | Track 1 | Duplicate integration manifest IDs include `T138` and `T230` through `T251`; uniqueness is not validated. | Promote to Track 1. |
| AUDIT-MANIFEST-002 | Fable | Important | verified | Track 1 | `stderr_not_contains` appears in `phase4-group-d.toml` but is not an `Assertion` field, so the intended assertion is ignored. | Promote to Track 1. |
| AUDIT-REMI-DEPLOY-001 | Fable | Important | partially-verified | Track 3 | Remi deploy helper regenerates `SHA256SUMS` after transfer and treats CCS signatures as optional. | Re-check against release artifact flow before planning. |
| AUDIT-REMI-DEPLOY-002 | Gemini | Minor | partially-verified | Track 3 | `deploy-remi` and `configure-concurrency` both restart Remi when run in sequence. | Verify workflow call order and decide whether to batch config and binary replacement. |
| AUDIT-CONARYD-DEPLOY-001 | DeepSeek | Important | verified | Track 3 | conaryd deploy/verify jobs exist in `deploy-and-verify.yml`, but release matrix fixes `conaryd` at `deploy_mode=none`, so those jobs are unreachable. | Decide delete, explicitly disabled flag, or consistency check. |
| AUDIT-RELEASE-MATRIX-001 | DeepSeek | Important | verified | Track 3 | `scripts/check-release-matrix.sh` does not detect deploy jobs that are unreachable because product deploy mode is `none`. | Promote with conaryd deploy cleanup. |
| AUDIT-ACTIONS-001 | Fable | Important | verified-by-structure | Track 2 | GitHub action pin checker looks for known pinned actions rather than rejecting every unpinned `uses:` line, and does not cover all workflows equally. | Promote with CI hardening. |
| AUDIT-SCRIPT-MODE-001 | DeepSeek | Important | verified | Track 2 | `scripts/forge-smoke.sh` and `scripts/check-github-action-runtimes.sh` have shebangs but lack executable bits unlike most script helpers. | Include in CI/tooling polish or fix opportunistically. |
| AUDIT-MANPAGE-001 | Fable | Important | verified-corrected | Track 4 | External review treated `man/conary.1` as tracked, but local check shows root and app manpage outputs are ignored/generated. | Verify generated-manpage policy and prevent docs from treating ignored output as tracked truth. |
| AUDIT-REMI-OPENAPI-001 | Fable | Important | unverified | Track 4 | Remi admin OpenAPI advertises test-harness operations but omits live admin routes. | Verify router/spec delta. |
| AUDIT-CONARYD-001 | Fable | Important | unverified | Track 4 | conaryd depends on the CLI crate and calls CLI command functions, but docs do not name that inverted dependency. | Verify and decide document vs extract. |
| AUDIT-SBOM-001 | Gemini | Important | verified | Track 4 | `docs/modules/query.md` maps top-level `conary sbom` to `commands/query/sbom.rs`, but dispatch calls `commands/derivation_sbom.rs`; `query/sbom.rs` owns nested system SBOM behavior. | Promote to public-surface routing cleanup. |
| AUDIT-ROUTING-001 | Fable/Gemini | Important | partially-verified | Track 4 | Agent routing docs may omit load-bearing files in install/system/provenance/state areas; Gemini specifically identified omitted install submodules. | Verify with file map sweep. |
| AUDIT-CONARY-TEST-MCP-001 | DeepSeek | Important | unverified | Track 4 | conary-test CLI and MCP surfaces may be intentionally asymmetric, but that asymmetry is not documented where agents expect mirrored tools. | Verify tool and command catalogs before documenting. |
| AUDIT-OPS-001 | Fable | Important | partially-verified | Track 2 / Track 3 | Some ops scripts may contradict the supported Remi deploy path, and release/deploy helper tests are not wired into CI. | Split deploy path cleanup from CI wiring. |
| AUDIT-INTEGRATION-DOC-001 | Fable | Minor | unverified | Track 1 | `docs/INTEGRATION-TESTING.md` may drift from actual result JSON shape, `--json` behavior, phase help, env vars, and test ranges. | Verify during Track 1 docs pass. |
| AUDIT-INTEGRATION-DOC-002 | Gemini/DeepSeek | Minor | partially-verified | Track 1 | `docs/INTEGRATION-TESTING.md` command reference omits `conary-test images build`, `images list`, and `deploy rebuild`. | Verify CLI and update during Track 1 docs pass. |
| AUDIT-INTEGRATION-DOC-003 | DeepSeek | Important | verified | Track 1 | Phase 4 integration docs undercount actual manifests: current local count is 159 tests across 7 Phase 4 manifest files. | Promote to Track 1 docs pass. |
| AUDIT-INTEGRATION-DOC-004 | DeepSeek | Important | partially-verified | Track 1 | Integration docs still contain stale group/range wording such as `Phase 3: Adversarial (Groups G-O)` despite Group P existing. | Promote to Track 1 docs pass. |
| AUDIT-INTEGRATION-DOC-005 | DeepSeek | Minor | verified | Track 1 | Phase 1 integration docs do not map IDs cleanly across `phase1-core.toml` and `phase1-advanced.toml`; current local counts are 10 and 31. | Promote to Track 1 docs pass. |
| AUDIT-VALIDATOR-001 | Fable | Minor | verified-resolved | Track 5 | Some validators still had fail-open edges such as header-only ledgers or shrinking scan scopes. | Header-only coherency ledgers now fail; docs-truth required scan roots are fail-closed and covered by self-tests. |
| AUDIT-ROUTE-DOC-001 | Gemini | Important | verified-resolved | Track 5 | `scripts/check-doc-truth.sh` only extracted conaryd routes for separate `GET`, `POST`, and `DELETE` `.route(...)` calls and doc rows. | Route extraction now covers `PUT`, `PATCH`, and chained Axum route handlers, with unsupported shapes failing clearly. |
| AUDIT-CLI-DOC-001 | DeepSeek | Important | verified-resolved | Track 5 | `check-doc-truth.sh` had a conaryd route truth check but no analogous documented CLI command existence check for `conary` docs. | Active backticked `conary <root>` docs references are now checked against the root `Commands` enum. |
| AUDIT-LEGACY-REPLAY-001 | Fable | Minor | verified-follow-up | Track 4 | Legacy replay refusal helpers are similar across install/remove/system, but batch/restore helpers carry package and phase context. | Do not hoist as a drive-by Track 4 cleanup; create a focused refactor plan only if the duplication becomes active maintenance pain. |
| AUDIT-FLAG-001 | Fable | Minor | verified-resolved | Track 4 | `experimental` gated no production code and only hid a negative test for the removed automation AI subcommand family. | Removed the dead feature flag and made the negative parser test part of the normal Conary library suite. |
| AUDIT-DEPLOY-EXCEPTION-001 | Fable | Minor | unverified | Track 3 | A standing deploy exception may remain in production workflow policy. | Verify and retire if no longer needed. |
| AUDIT-CREDS-DOC-001 | DeepSeek | Minor | verified | Track 3 | `deploy/.credentials.toml` still mentions building `/usr/local/bin/conary` with retired `--features server` wording. | Fix with deploy docs cleanup. |
| AUDIT-DISPATCH-NAME-001 | DeepSeek | Minor | verified-resolved | Track 4 | `VerifyCommands` live in `cli/verify.rs` while dispatch lives in `dispatch/verify_derivation.rs`, which can confuse file-name based routing. | Added a source comment explaining that the dispatch file follows the user-visible `verify-derivation` root command name. |
| AUDIT-ADOPT-ROUTING-001 | DeepSeek | Minor | partially-verified | Track 4 | Subsystem map lists adoption implementation files but does not foreground the CLI route through `SystemCommands::Adopt` and `dispatch/system.rs`. | Clarify routing path in subsystem map. |

## Track 0: conaryd Authorization Fail-Closed Policy

**Purpose:** Remove or tightly document daemon write-authorization paths that
grant root-equivalent access before the PolicyKit fail-closed stub runs.

**Accepted inputs:**

- `AUDIT-AUTH-001`
- `AUDIT-AUTH-002`

**Target files:**

- `apps/conaryd/src/daemon/auth.rs`
- `docs/modules/conaryd.md`
- `docs/ARCHITECTURE.md`
- `docs/llms/subsystem-map.md`

**Proposed `/goal` objective:**

Fix conaryd authorization so missing admin group lookups never fall back to
distribution-specific numeric GIDs, tests prove Debian/Ubuntu GID 10 users do
not receive daemon admin access, and docs accurately describe the remaining
root and daemon-identity policy.

**Candidate task boundaries:**

- [ ] Add a failing unit test proving a non-root peer with primary GID 10 does
  not receive `Permission::Full` merely because `wheel` lookup is unavailable.
- [ ] Replace hardcoded `wheel=10` and `sudo=27` fallback behavior with
  fail-closed name resolution. Root remains trusted through UID 0, not through a
  hardcoded group fallback.
- [ ] Remove default `sudo`/`wheel` trusted-group write authorization while
  PolicyKit is stubbed. Retain explicit trusted-GID helper APIs only as
  test/future-policy hooks if needed.
- [ ] Update `docs/modules/conaryd.md` and architecture/routing docs so the
  authorization policy and code agree.
- [ ] Run focused conaryd auth tests and docs-truth checks.

**Done criteria:**

- Hardcoded distribution-specific admin GID fallbacks are gone.
- A Debian/Ubuntu-style `uucp` GID 10 peer is denied write access unless it also
  satisfies an explicitly trusted identity path.
- The docs state that non-root write authorization is fail-closed until a real
  PolicyKit path or another reviewed policy replaces it.
- `cargo test -p conaryd daemon::auth` and `bash scripts/check-doc-truth.sh`
  pass.

## Track 1: conary-test Evidence Trust

**Purpose:** Make conary-test output reliable enough that future release and
coherency waves can cite it as evidence.

**Accepted inputs:**

- `AUDIT-QEMU-001`
- `AUDIT-QEMU-002`
- `AUDIT-RESULT-001`
- `AUDIT-RESULT-002`
- `AUDIT-INTEGRATION-CI-001`
- `AUDIT-MANIFEST-001`
- `AUDIT-MANIFEST-002`
- `AUDIT-INTEGRATION-DOC-001`
- `AUDIT-INTEGRATION-DOC-002`
- `AUDIT-INTEGRATION-DOC-003`
- `AUDIT-INTEGRATION-DOC-004`
- `AUDIT-INTEGRATION-DOC-005`

**Target files:**

- `apps/conary-test/src/engine/qemu.rs`
- `apps/conary-test/src/engine/runner.rs`
- `apps/conary-test/src/config/manifest.rs`
- `apps/conary-test/src/engine/executor.rs`
- `apps/conary/tests/integration/remi/manifests/*.toml`
- `scripts/check-conary-test-result-gate.sh`
- `scripts/local-qemu-validation.sh`
- `docs/INTEGRATION-TESTING.md`
- `apps/conary-test/README.md`

**Proposed `/goal` objective:**

Fix conary-test evidence hardening so QEMU skips are represented as skipped, the
release-evidence gate rejects empty or inconsistent result JSON, integration
manifest IDs are unique, unknown assertion keys fail validation, and integration
docs match the actual harness behavior.

**Candidate task boundaries:**

- [ ] Add failing tests for QEMU early-return skip semantics and runner status
  mapping.
- [ ] Change QEMU skip early returns so the runner records `TestStatus::Skipped`
  rather than a pass-shaped `exit_code=0`.
- [ ] Harden `scripts/check-conary-test-result-gate.sh` with required summary,
  minimum passed count, recomputed result counts, and self-inconsistency
  failures.
- [ ] Decide whether non-release result-gate runs may allow skipped tests with a
  warning flag; keep the release-evidence default fail-closed on skipped tests.
- [ ] Wire the result gate into `scripts/local-qemu-validation.sh`.
- [ ] Expand `scripts/local-qemu-validation.sh` build coverage to include every
  binary that the selected suites can exercise, or document why the selected
  suites cannot touch Remi/conaryd.
- [ ] Decide the honest CI posture for TOML integration suites: add a runnable
  container-only subset to CI, add a KVM-capable lane, or document that CI
  currently parses manifests and runs conary-test unit tests but does not execute
  the TOML suites.
- [ ] Add manifest validation for duplicate IDs across loaded manifests and for
  unknown assertion keys.
- [ ] Renumber duplicate manifest IDs or introduce an intentional namespacing
  scheme if the duplicates are confirmed intentional.
- [ ] Implement `stderr_not_contains` or remove the stale manifest key and
  replace it with an equivalent supported assertion.
- [ ] Refresh integration testing docs and help text for actual JSON shape,
  phases, env vars, suite counts, ID ranges, command table entries, and QEMU
  skip semantics.

**Done criteria:**

- QEMU missing-tool/download paths cannot be counted as passed tests.
- `{}` and summary/result-count mismatches fail the result gate.
- `cargo run -p conary-test -- list` fails on duplicate IDs or unknown manifest
  keys, or an explicit namespacing scheme is documented and enforced.
- `scripts/local-qemu-validation.sh` invokes the hardened result gate.
- Local QEMU validation cannot accidentally run stale Remi or conaryd binaries
  for suites that depend on those services.
- The docs state clearly whether TOML integration suites execute in CI or only
  through local/release validation.
- `docs/INTEGRATION-TESTING.md` describes the behavior that the code actually
  enforces.
- Focused conary-test unit tests and manifest inventory commands pass.

## Track 2: Release And CI Safety

**Purpose:** Ensure a tag cannot produce deployable artifacts without the
expected tests and workflow-policy checks.

**Accepted inputs:**

- `AUDIT-RELEASE-001`
- `AUDIT-ACTIONS-001`
- `AUDIT-SCRIPT-MODE-001`
- CI portion of `AUDIT-OPS-001`

**Target files:**

- `.github/workflows/release-build.yml`
- `.github/workflows/merge-validation.yml`
- `.github/workflows/pr-gate.yml`
- `.github/workflows/deploy-and-verify.yml`
- `scripts/check-github-action-runtimes.sh`
- `scripts/check-release-matrix.sh`
- `scripts/test-release-matrix.sh`
- `scripts/test-remi-deploy-helper.sh`

**Proposed `/goal` objective:**

Put a meaningful test and policy gate in front of tag-triggered production
release/deploy flow, and make workflow policy checks fail closed for unpinned
actions and release/deploy helper regressions.

**Candidate task boundaries:**

- [ ] Add a release-build test job or reusable validation dependency that build
  jobs must need before artifacts publish.
- [ ] Mirror the PR fmt, dependency consistency, clippy, and workspace test gates
  into `merge-validation.yml`, or explicitly document and enforce PR-only flow.
- [ ] Invert action pin checking so every non-local `uses:` line must be pinned
  to a full SHA unless allowlisted.
- [ ] Include all workflows in the action pin scan.
- [ ] Wire `scripts/test-release-matrix.sh` and
  `scripts/test-remi-deploy-helper.sh` into CI where their dependencies are
  available.
- [ ] Normalize executable bits for scripts that are intended to support
  `./scripts/...` invocation, or document `bash scripts/...` as the only
  supported form.

**Done criteria:**

- A release tag cannot build publishable artifacts without an upstream test gate.
- Main-push validation no longer omits the same basic test classes that PRs run,
  unless branch protection is verified and documented as the enforcement layer.
- New unpinned actions fail a script test.
- Release matrix and deploy-helper behavior tests run in automation.

## Track 3: Deploy And Artifact Integrity

**Purpose:** Make deployment scripts prove artifact integrity instead of
re-checksumming whatever arrived, and retire stale deploy paths.

**Accepted inputs:**

- `AUDIT-REMI-DEPLOY-001`
- `AUDIT-REMI-DEPLOY-002`
- `AUDIT-CONARYD-DEPLOY-001`
- `AUDIT-RELEASE-MATRIX-001`
- deploy-path portion of `AUDIT-OPS-001`
- `AUDIT-DEPLOY-EXCEPTION-001`
- `AUDIT-CREDS-DOC-001`

**Target files:**

- `deploy/remi-deploy-helper.sh`
- `scripts/install-remi-deploy-access.sh`
- `scripts/rebuild-remi.sh`
- `scripts/test-remi-deploy-helper.sh`
- `.github/workflows/deploy-and-verify.yml`
- `docs/operations/infrastructure.md`
- `docs/operations/release-artifact-matrix.md`

**Proposed `/goal` objective:**

Harden Remi deployment artifact verification, require expected signatures when
release policy says they are mandatory, and remove or reframe stale deploy
scripts and one-off workflow exceptions.

**Candidate task boundaries:**

- [ ] Reconstruct the intended Remi artifact flow from `release-build.yml`,
  `deploy-and-verify.yml`, and `deploy/remi-deploy-helper.sh`.
- [ ] Add deploy-helper tests that prove CI-produced checksum files are verified
  before install.
- [ ] Require CCS signature presence when installing live release artifacts that
  are supposed to be signed.
- [ ] Decide whether `scripts/rebuild-remi.sh` is deleted, archived, or rewritten
  as a thin pointer to the supported deploy helper.
- [ ] Verify whether deploy workflow runs `deploy-remi` and
  `configure-concurrency` together; if so, batch restart so rollout performs one
  service restart when possible.
- [ ] Remove stale one-off deploy exceptions when no current release path needs
  them.
- [ ] Decide whether unreachable conaryd deploy jobs are kept as explicitly
  paused future wiring, removed until deploy resumes, or guarded by a matrix
  consistency check that forces deploy mode and jobs to change together.
- [ ] Update deploy credential templates and local setup comments that mention
  retired `--features server` or old binary names.
- [ ] Update operations docs to name the supported path only.

**Done criteria:**

- Transit corruption or artifact substitution cannot be laundered by regenerated
  checksums.
- Signed-release policy and deploy-helper behavior agree.
- Unsupported legacy deploy scripts no longer contradict the operator docs.
- Deploy-helper tests cover checksum and signature behavior.

## Track 4: Public Surface And Agent Routing Truth

**Purpose:** Reduce agent confusion by aligning generated/manual public surfaces,
API specs, subsystem maps, and ownership cards with real code.

**Accepted inputs:**

- `AUDIT-MANPAGE-001`
- `AUDIT-REMI-OPENAPI-001`
- `AUDIT-CONARYD-001`
- `AUDIT-SBOM-001`
- `AUDIT-ROUTING-001`
- `AUDIT-CONARY-TEST-MCP-001`
- `AUDIT-LEGACY-REPLAY-001`
- `AUDIT-FLAG-001`
- `AUDIT-DISPATCH-NAME-001`
- `AUDIT-ADOPT-ROUTING-001`

**Target files:**

- `apps/conary/build.rs`
- generated ignored manpage output under `apps/conary/man/`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/conaryd.md`
- `docs/ARCHITECTURE.md`
- `apps/remi/src/server/routes/admin.rs`
- `apps/remi/src/server/handlers/openapi.rs`
- `apps/conary/src/commands/install/`
- `apps/conary/src/commands/remove/`
- `apps/conary/src/commands/system.rs`
- `apps/conary/Cargo.toml`

**Proposed track objective:**

Verify and repair public-surface and routing drift that misleads humans or LLM
agents about where behavior lives, which APIs exist, and which command surfaces
are current.

Track 4 is larger than a single safe `/goal`. Execute it as sequenced slices,
starting with generated-manpage policy plus SBOM routing, then separate OpenAPI,
agent-routing/conaryd-layering, and cleanup/asymmetry slices.

**Candidate task boundaries:**

- [ ] Verify manpages are ignored/generated inspection output and correct any docs
  that treat `man/conary.1` as tracked source truth.
- [ ] Add a doc-truth or build check only if the repo intentionally starts
  tracking a generated manpage.
- [ ] Compare Remi admin routes to OpenAPI paths and either add missing paths or
  narrow the OpenAPI description.
- [ ] Correct SBOM routing docs so top-level `conary sbom` points to
  `apps/conary/src/commands/derivation_sbom.rs` and nested system SBOM behavior
  points to `apps/conary/src/commands/query/sbom.rs`.
- [ ] Document conaryd's current dependency on CLI command functions, or create
  a later extraction plan if the dependency is intentionally temporary.
- [ ] Sweep subsystem-map and feature-ownership for load-bearing files omitted
  from agent routing.
- [ ] Document intentional CLI/MCP asymmetry for conary-test if the MCP server
  owns operations that no CLI command exposes.
- [ ] Clarify dispatch naming/routing oddities such as
  `cli/verify.rs` -> `dispatch/verify_derivation.rs` and adoption routing
  through `SystemCommands::Adopt` -> `dispatch/system.rs` -> `commands/adopt/`.
- [ ] Verify legacy replay duplication and either hoist shared helpers or record
  a focused follow-up.
- [ ] Verify whether the `experimental` feature flag gates production code and
  remove or document it accordingly.

**Done criteria:**

- Agents no longer route from active docs to stale command trees or missing API
  paths.
- Public route/API specs are generated or checked against registered routes.
- Ownership docs mention all high-load files a future agent would reasonably
  need first.
- Any retained duplication or inverted dependency has an explicit owner and
  follow-up.

## Track 5: Remaining Validator Soundness

**Purpose:** Continue hardening local validators after the highest-stakes
evidence and release gates are fixed.

**Accepted inputs:**

- `AUDIT-VALIDATOR-001`
- `AUDIT-ROUTE-DOC-001`
- `AUDIT-CLI-DOC-001`

**Target files:**

- `scripts/check-coherency-ledger.sh`
- `scripts/check-coherency-wave-scopes.sh`
- `scripts/check-doc-truth.sh`
- `scripts/check-release-matrix.sh`
- Existing validator test scripts.

**Proposed `/goal` objective:**

Close remaining fail-open validator edges without expanding the validator suite
into a general static-analysis framework.

**Candidate task boundaries:**

- [ ] Add minimum-row and exact-header checks where header-only or garbage-header
  input still passes.
- [ ] Make missing expected scan paths fail clearly instead of silently shrinking
  scope.
- [ ] Teach conaryd route extraction to cover `PUT`, `PATCH`, and chained Axum
  route handlers, or fail with a clear unsupported-pattern message.
- [ ] Evaluate whether documented `conary` CLI command references can be checked
  automatically through help output, a machine-readable command listing, or a
  constrained docs-reference extractor.
- [ ] Strengthen release-matrix checks for status semantics, not only keyword
  presence.
- [ ] Add focused fixtures for each new failure mode.

**Done criteria:**

- Validator tests demonstrate every new failure mode failing before the checker
  is changed and passing afterward.
- The checks remain narrow enough to explain in the docs they protect.

## Import Steps For Additional Review

- [ ] Paste any additional external findings into the session.
- [ ] Add each new finding to the register with a stable `AUDIT-*` ID.
- [ ] Mark direct duplicates as corroboration in the `Source` field rather than
  adding separate rows.
- [ ] Promote severity when two independent reviewers identify the same risk and
  local verification agrees.
- [ ] Demote or reject findings that conflict with current repo truth.
- [ ] Re-rank the first `/goal` only after the register has been merged.

## Recommended First Goal After Merge

Start with Track 0 unless a later external review uncovers a higher-severity
release or security blocker. The conaryd GID fallback is narrower than Track 1
and has a clear security consequence. After Track 0, run Track 1 because if
conary-test can record "never ran" as "passed," every later release-evidence
and coherency claim built on that output is weaker.

Paste-ready Track 0 `/goal` objective after the register is merged:

```text
/goal Fix conaryd authorization fail-closed behavior so missing admin group
lookups never fall back to distribution-specific numeric GIDs, Debian/Ubuntu
GID 10 users do not receive daemon admin access, and conaryd docs match the
remaining root and daemon-identity policy.
```

Paste-ready Track 1 `/goal` objective after Track 0:

```text
/goal Fix conary-test evidence hardening so QEMU skips cannot count as passes,
scripts/check-conary-test-result-gate.sh rejects empty or inconsistent result
JSON, integration manifest IDs are unique, unknown assertion keys fail
validation, and integration testing docs match the actual harness behavior.
```

## Lock-In Checklist

Before executing any track:

- [ ] Verify `git status --short --branch` is clean and synced with
  `origin/main`.
- [ ] Import and deduplicate all external findings received before lock-in.
- [ ] Locally verify every finding in the selected track.
- [ ] Convert the selected track into a task-by-task implementation plan with
  exact tests, file edits, and commit boundaries.
- [ ] Register this umbrella plan and the selected implementation plan in the
  documentation accuracy audit inventory and ledger.
- [ ] Run the exact planning-doc verification gate before committing:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
git diff --check
```
