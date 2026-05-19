---
last_updated: 2026-05-19
revision: 1
summary: Daily-driver readiness program design with seven Codex /goal tracks
---

# Daily Driver Readiness Program: Design Spec

**Date:** 2026-05-19
**Status:** Active program design; implementation goals not yet launched
**Goal:** Turn Conary from a credible preview package-manager path into a daily-driver replacement candidate by closing the seven obvious gaps identified from the current codebase and docs.

---

## Purpose

This design ignores the narrow limited-release question and asks what must be
true before a technically careful Linux user could use Conary as the normal
package manager for a real machine.

The answer is not one giant implementation pass. The work spans package
ownership, distro corpus validation, security advisory metadata, daemon
execution, sandboxing, boot/recovery artifacts, and everyday operator polish.
Each slice needs an explicit contract, a validation loop, and a stopping
condition. That makes the work a good fit for Codex `/goal`, where the goal
objective should be bigger than one prompt, smaller than an open-ended backlog,
and tied to verifiable evidence.

Official Codex references for the goal mechanism:

- <https://developers.openai.com/codex/use-cases/follow-goals>
- <https://developers.openai.com/codex/cli/slash-commands#set-an-experimental-goal-with-goal>

## Daily Driver Definition

For this program, "daily-driver replacement" means:

1. Conary can own normal package operations without requiring the user to keep
   dnf, apt, or pacman in the critical path for Conary-owned packages.
2. Adoption and takeover boundaries are reversible and honest, including after
   a Conary generation is selected.
3. Common packages and package behaviors are validated across Fedora 44,
   Ubuntu 26.04 LTS, and Arch Linux with real integration evidence.
4. Security updates are truthful and useful, not merely fail-closed when
   advisory data is absent.
5. Local daemon execution can safely run package jobs with durable progress,
   privilege boundaries, and recovery.
6. Scriptlet and hook sandboxing protects the host in the modes that claim to
   protect it.
7. Boot, rollback, recovery, and artifact trust are strong enough that the user
   can recover when an update or generation fails.
8. The CLI and operator docs are boring, clear, and accurate enough that normal
   workflows do not require maintainer intuition.

This is not a promise to match every native package-manager flag, repository
ecosystem, or output format. It is a replacement-readiness bar for the Conary
model: packages, generations, CAS, Remi conversion, and system state.

## Program Principles

- Keep the work goal-shaped. Every track below has one objective, one stop
  condition, read-first context, and evidence commands.
- Make the first goal unlock the rest. Active-generation handoff is the
  primary authority boundary; without it, the replacement story remains
  structurally incomplete.
- Prefer integration evidence over documentation optimism. Docs should follow
  passing product behavior, not describe intended behavior as if it already
  landed.
- Preserve existing truthful preview boundaries until a goal replaces them
  with tested behavior.
- Update the audit inventory and ledger whenever tracked docs change.
- Commit and push after each completed goal so later goals start from a stable
  `main`.

## Shared Read-First Context

Every goal should begin by reading:

- `AGENTS.md`
- `README.md`
- `ROADMAP.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/source-selection.md`
- `docs/operations/post-generation-export-follow-up-roadmap.md`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Goal-specific sections below add more files.

Every goal should finish by considering whether these docs need updates:

- `README.md`
- `ROADMAP.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/*.md`
- `docs/operations/*.md`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

## Shared Verification Gate

Every implementation goal should run the narrow tests for touched code plus:

```bash
cargo fmt --check
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Goals that touch Rust behavior should also run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Long QEMU or distro matrix runs are track-specific. They should be treated as
release evidence only when result files report zero failed, skipped, and
cancelled results.

## Goal Track Order

1. Native Authority Handoff
2. Real Package Corpus Validation
3. Security Advisory Pipeline
4. Host-Mutation Sandbox Hardening
5. conaryd Package Execution
6. Recovery, Boot, And Artifact Trust
7. Daily UX And Operator Polish

Goal 7 can collect small polish found during earlier tracks, but it should not
become the dumping ground for behavior that belongs in Goals 1 through 6.

## Goal 1: Native Authority Handoff

### Intent

Close the largest replacement-readiness gap: returning from a selected Conary
generation to native package-manager authority without corrupting Conary state,
native package-manager state, or future generation builds.

### Current Gap

The current contract is safe but incomplete. `system unadopt --all` is the
escape hatch only before a Conary generation is selected. Once a generation is
selected, unadoption fails closed because deleting tracking rows can make
future generated roots omit packages still known to the native package manager.

### Scope

- Define the active-generation handoff state machine.
- Decide whether handoff means reselecting a mutable native root, generating a
  native-authority handoff generation, or a staged operation with explicit boot
  confirmation.
- Preserve native package files and native package-manager databases.
- Make adopted tracking removal safe while a generation exists.
- Record operation state and recovery instructions.
- Prove dry-run, refusal, success, interruption, and recovery behavior.
- Add or extend a dedicated selected-generation handoff integration suite; the
  existing pre-selection unadopt tests and Fedora composefs modernization suite
  are supporting evidence, not enough by themselves.

### Out Of Scope

- Silent takeover of adopted packages.
- Full native transaction-history import.
- Replacing dnf, apt, or pacman databases.

### Read First

- `apps/conary/src/commands/adopt/unadopt.rs`
- `apps/conary/src/commands/adopt/system.rs`
- `apps/conary/src/commands/generation/switch.rs`
- `apps/conary/src/commands/generation/takeover.rs`
- `crates/conary-core/src/generation/metadata.rs`
- `crates/conary-core/src/generation/mount.rs`
- `crates/conary-core/src/transaction/recovery.rs`
- `apps/conary/tests/integration/remi/manifests/phase1-advanced.toml`
- `apps/conary/tests/integration/remi/manifests/phase3-composefs-modernization.toml`

### Stop Condition

The goal is complete when a selected-generation host can run a documented
handoff flow back to native authority without deleting native package files or
corrupting package-manager databases. A focused selected-generation handoff
suite must prove dry-run, refusal, success, interruption/recovery, and native
database preservation on Fedora 44, Ubuntu 26.04 LTS, and Arch; docs no longer
list active-generation handoff as an open follow-up gap.

### Verification

```bash
cargo test -p conary --bin conary adopt::unadopt
cargo test -p conary --bin conary generation
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro ubuntu-26.04 --phase 3
cargo run -p conary-test -- run --suite phase3-active-generation-handoff --distro arch --phase 3
```

### Goal Template

```text
/goal Implement Conary native authority handoff for selected-generation hosts. Read docs/superpowers/specs/2026-05-19-daily-driver-readiness-program-design.md, README.md, ROADMAP.md, docs/INTEGRATION-TESTING.md, apps/conary/src/commands/adopt/unadopt.rs, apps/conary/src/commands/generation/switch.rs, apps/conary/src/commands/generation/takeover.rs, and crates/conary-core/src/transaction/recovery.rs first. Preserve native package files and native package-manager authority, make adopted tracking removal safe after a Conary generation is selected, add a selected-generation handoff suite that proves dry-run/refusal/success/recovery on Fedora 44, Ubuntu 26.04 LTS, and Arch authority, update docs and audit metadata, and stop only when that focused suite plus the shared verification gate pass.
```

## Goal 2: Real Package Corpus Validation

### Intent

Move beyond fixture-sized parity and prove that ordinary unpleasant packages
work across package formats.

### Current Gap

The current Phase 4 native package-manager parity suite proves the command
surface against purpose-built fixtures. That is necessary but not sufficient
for daily-driver confidence.

### Scope

- Build a curated daily-driver corpus for Fedora 44, Ubuntu 26.04 LTS, and
  Arch.
- Cover packages with systemd units, config files, alternatives, users/groups,
  dependencies, scriptlets, triggers, file conflicts, large payloads, and
  package removal hooks.
- Include at least one bootloader-adjacent or kernel-adjacent package class in
  a QEMU-safe way.
- Add result gates that reject failed, skipped, and cancelled cases.
- Record known unsupported classes explicitly.

### Out Of Scope

- Installing arbitrary internet package selections without a curated allowlist.
- Claiming full repository ecosystem parity.
- Mutating the developer's host outside container or QEMU validation.

### Read First

- `apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml`
- `apps/conary/tests/native_pm_daily_driver.rs`
- `apps/conary/tests/native_pm_live_root.rs`
- `apps/conary/tests/fixtures/native/`
- `apps/conary-test/src/engine/runner.rs`
- `scripts/check-conary-test-result-gate.sh`

### Stop Condition

The goal is complete when a named conary-test suite validates the curated
corpus on Fedora 44, Ubuntu 26.04 LTS, and Arch with zero failed, skipped, and
cancelled results, and the docs identify which package classes are daily-driver
covered versus still unsupported.

### Verification

```bash
cargo run -p conary-test -- list
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/fedora44-phase4.json
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/ubuntu-26.04-phase4.json
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/arch-phase4.json
```

### Goal Template

```text
/goal Expand Conary daily-driver package corpus validation beyond fixtures. Read docs/INTEGRATION-TESTING.md, apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml, apps/conary/tests/native_pm_daily_driver.rs, apps/conary/tests/native_pm_live_root.rs, apps/conary/tests/fixtures/native, and apps/conary-test/src/engine/runner.rs first. Add a curated corpus or extend the existing matrix to cover systemd units, config files, alternatives, users/groups, dependencies, scriptlets, removal hooks, file conflicts, large payloads, and QEMU-safe bootloader/kernel-adjacent behavior across Fedora 44, Ubuntu 26.04 LTS, and Arch. Stop when the suite has zero failed/skipped/cancelled results, result gates enforce that, docs distinguish covered and unsupported package classes, and the shared verification gate passes.
```

## Goal 3: Security Advisory Pipeline

### Intent

Make `conary update --security` useful enough for a daily driver rather than
only honest enough to refuse unverifiable sources.

### Current Gap

Conary correctly refuses security-only updates when a source cannot prove
trusted advisory metadata support. Daily-driver replacement requires trusted
advisory ingestion, display, and update selection for supported repositories.

### Scope

- Define supported advisory metadata inputs for Remi and local repository
  metadata.
- Persist advisory identity, CVEs, severity, fixed versions, affected packages,
  and source trust status.
- Make `update --security` select only trusted security fixes.
- Make output explain applied, unavailable, unsupported, and up-to-date states.
- Keep refusal before mutation for unknown or unsupported sources.
- Add a positive fixture for the supported path that proves advisory ingestion,
  persistence, and trusted candidate selection before package mutation.
- Revisit the remaining `rsa` waiver and document the exact release gate.

### Out Of Scope

- Perfect distro-vendor advisory parity on the first pass.
- Security claims for repositories without supported metadata.
- Replacing external vulnerability scanners.

### Read First

- `apps/conary/src/commands/update.rs`
- `apps/conary/src/commands/repo.rs`
- `crates/conary-core/src/repository/metadata.rs`
- `crates/conary-core/src/repository/sync/`
- `apps/remi/src/server/handlers/`
- `scripts/release-cargo-audit.sh`
- `docs/superpowers/release-security-waivers-2026-05-06.md`

### Stop Condition

The goal is complete when at least one named supported advisory metadata path
drives `update --security` from ingestion through persisted metadata and
trusted candidate selection to a positive update plan. Unknown and unsupported
paths must still fail before mutation, security output must distinguish applied,
unavailable, unsupported, and up-to-date states, the waiver state is updated,
and the behavior has unit and integration tests.

### Verification

```bash
cargo test -p conary security_update -- --nocapture
cargo run -p conary -- update --help
bash scripts/release-cargo-audit.sh
cargo run -p conary-test -- run --suite phase4-security-advisory-pipeline --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
```

### Goal Template

```text
/goal Make Conary security updates daily-driver useful. Read apps/conary/src/commands/update.rs, apps/conary/src/commands/repo.rs, crates/conary-core/src/repository/metadata.rs, crates/conary-core/src/repository/sync, apps/remi/src/server/handlers, scripts/release-cargo-audit.sh, and docs/superpowers/release-security-waivers-2026-05-06.md first. Implement or complete a trusted advisory metadata path with persisted CVE/severity/fixed-version/source-trust data, add a positive advisory fixture that proves ingestion through trusted update selection, make update --security apply trusted fixes and refuse unknown or unsupported sources before mutation, update CLI/docs/audit metadata, revisit the rsa waiver, and stop when focused security tests, the advisory pipeline integration proof, cargo audit gating, and the shared verification gate pass.
```

## Goal 4: Host-Mutation Sandbox Hardening

### Intent

Make sandbox wording match actual host protection, especially for scriptlets
and hooks on live roots.

### Current Gap

Current CLI help says scriptlet sandboxing provides PID/network isolation while
`/etc` and `/var` remain writable on live roots. The follow-up roadmap calls
out that sandboxing still has uneven host mutation boundaries.

### Scope

- Define sandbox modes in terms of filesystem, network, PID, and syscall
  boundaries.
- Add tmpfs or overlay-backed writable layers for live-root scriptlet paths
  that claim host protection.
- Keep target-root installs as the full-isolation path where appropriate.
- Fail closed when sandbox guarantees cannot be provided.
- Make help text and docs precise for each mode.
- Add regression tests that prove package hooks cannot mutate host `/etc` or
  `/var` in protected modes.

### Out Of Scope

- A complete container runtime rewrite.
- Unsupported kernel feature emulation.
- Claiming full isolation for modes that intentionally allow host mutation.

### Read First

- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/commands/install/scriptlets.rs`
- `crates/conary-core/src/scriptlet/mod.rs`
- `crates/conary-core/src/container/mod.rs`
- `crates/conary-core/src/capability/enforcement/`
- `docs/SCRIPTLET_SECURITY.md`
- `apps/conary/tests/scriptlet_harness/README.md`

### Stop Condition

The goal is complete when protected sandbox modes prevent unintended host
`/etc` and `/var` mutations in tests, unsupported protection states fail
before running scriptlets, and CLI/docs accurately describe what each mode
does and does not protect.

### Verification

```bash
cargo test -p conary-core scriptlet
cargo test -p conary scriptlet
cargo run -p conary -- install --help
```

### Goal Template

```text
/goal Harden Conary live-root sandboxing so sandbox promises match host mutation boundaries. Read docs/SCRIPTLET_SECURITY.md, apps/conary/src/cli/mod.rs, apps/conary/src/commands/install/scriptlets.rs, crates/conary-core/src/scriptlet/mod.rs, crates/conary-core/src/container/mod.rs, crates/conary-core/src/capability/enforcement, and apps/conary/tests/scriptlet_harness/README.md first. Add protected-mode filesystem isolation or fail-closed checks for scriptlets and hooks, prove host /etc and /var are not mutated unintentionally, update help/docs/audit metadata, and stop when focused sandbox tests plus the shared verification gate pass.
```

## Goal 5: conaryd Package Execution

### Intent

Make the local daemon capable of executing package install/remove/update jobs
instead of only exposing read routes and returning `501 Not Implemented`.

### Current Gap

`conaryd` has a Unix-socket daemon, auth, persistent jobs, REST/SSE plumbing,
and read-route scaffolding. Package mutation routes intentionally return
`501 Not Implemented`.

### Scope

- Define the shared daemon package-operation executor around the same contracts
  used by the CLI.
- Preserve privilege and peer-auth boundaries.
- Persist job state, progress, logs, final outcome, and recovery markers.
- Stream useful SSE progress.
- Keep CLI and daemon behavior compatible for dry-run, apply, failure, and
  partial follow-up states.
- Add tests for install, remove, update, refusal, job recovery, and route
  output.

### Out Of Scope

- Remote multi-host orchestration.
- UI clients beyond API/CLI verification.
- Weakening live-host mutation acknowledgement.

### Read First

- `apps/conaryd/src/daemon/routes.rs`
- `apps/conaryd/src/daemon/routes/transactions.rs`
- `apps/conaryd/src/daemon/jobs.rs`
- `apps/conaryd/src/daemon/client.rs`
- `apps/conaryd/src/daemon/auth.rs`
- `crates/conary-core/src/operations.rs`
- `apps/conary/src/commands/install/mod.rs`
- `apps/conary/src/commands/remove.rs`
- `apps/conary/src/commands/update.rs`

### Stop Condition

The goal is complete when conaryd package install/remove/update routes execute
real jobs, return durable progress and results, recover or report incomplete
jobs after restart, and no docs describe package routes as blanket `501`
responses.

### Verification

```bash
cargo test -p conaryd package_routes
cargo test -p conaryd daemon::routes
cargo run -p conaryd -- --help
cargo test -p conary install
cargo test -p conary remove
cargo test -p conary update
```

### Goal Template

```text
/goal Implement conaryd package install/remove/update execution. Read README.md, docs/ARCHITECTURE.md, apps/conaryd/src/daemon/routes.rs, apps/conaryd/src/daemon/routes/transactions.rs, apps/conaryd/src/daemon/jobs.rs, apps/conaryd/src/daemon/client.rs, apps/conaryd/src/daemon/auth.rs, crates/conary-core/src/operations.rs, and the CLI install/remove/update command implementations first. Build a daemon executor that reuses CLI package-operation contracts, preserves auth and live-mutation boundaries, persists job progress and recovery state, streams useful SSE events, updates docs/audit metadata, and stop when conaryd package route tests, CLI compatibility tests, and the shared verification gate pass without 501 package-route claims remaining in active docs.
```

## Goal 6: Recovery, Boot, And Artifact Trust

### Intent

Make Conary's strongest system feature safe enough for daily-driver recovery:
selected-generation activation, rollback, boot artifacts, and artifact trust.

### Current Gap

x86_64 raw/qcow2 generation export is green, but ISO export, portable bundles,
boot-artifact provenance, non-x86_64 boot assets, and pristine self-host
validation remain follow-up work. Selected next-boot activation and rollback
need broader end-to-end failure coverage.

### Scope

- Expand selected-generation activation and rollback failure tests.
- Keep Group N and Group O QEMU gates green.
- Implement the next recovery artifact as bootable ISO export on the existing
  generation artifact contract; do not choose among artifact types inside the
  `/goal` run.
- Add or extend an ISO export integration suite that validates the generated
  artifact and boots it under QEMU.
- Emit digest/provenance data for generated raw, qcow2, and ISO artifacts that
  lets an operator answer which generation produced a bootable artifact.
- Keep non-x86_64 support reserved unless real boot assets and validation land.
- Make self-host validation pristine by default and fail before QEMU when the
  staged workspace or source image is stale.

### Out Of Scope

- Claiming broad hardware support from QEMU-only evidence.
- Adding non-x86 boot claims without validation.
- Reintroducing live-host scraping into runtime export.
- Signed portable generation bundles; keep that as the later bundle-specific
  follow-up from the post-generation-export roadmap.

### Read First

- `docs/operations/post-generation-export-follow-up-roadmap.md`
- `docs/modules/bootstrap.md`
- `apps/conary/src/commands/generation/export.rs`
- `apps/conary/src/commands/generation/switch.rs`
- `crates/conary-core/src/generation/artifact.rs`
- `crates/conary-core/src/generation/export.rs`
- `apps/conary/tests/integration/remi/manifests/phase3-group-n-qemu.toml`
- `apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`
- `scripts/local-qemu-validation.sh`

### Stop Condition

The goal is complete when named activation/rollback failure cases cover invalid
artifacts before pointer updates, rollback without an active generation, and
failed boot-selection recovery; Group N and Group O remain green; bootable ISO
export lands on the generation artifact contract with QEMU validation; raw,
qcow2, and ISO outputs include a digest/provenance manifest naming the source
generation and output digest; and self-host validation refuses stale staged
inputs before QEMU starts.

### Verification

```bash
scripts/local-qemu-validation.sh
cargo test -p conary --bin conary generation::export
cargo test -p conary-core generation::artifact
cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-group-n-qemu --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3
```

### Goal Template

```text
/goal Harden Conary recovery, boot, and artifact trust for daily-driver use. Read docs/operations/post-generation-export-follow-up-roadmap.md, docs/modules/bootstrap.md, apps/conary/src/commands/generation/export.rs, apps/conary/src/commands/generation/switch.rs, crates/conary-core/src/generation/artifact.rs, crates/conary-core/src/generation/export.rs, the Phase 3 Group N/O manifests, and scripts/local-qemu-validation.sh first. Expand the named selected-generation activation/rollback failure cases, keep Group N and Group O green, implement bootable ISO export as the next recovery artifact path, emit digest/provenance manifests for raw/qcow2/ISO outputs, make self-host validation refuse stale staged inputs before QEMU, update docs/audit metadata, and stop when the ISO suite, QEMU evidence, stale-input checks, and the shared verification gate pass.
```

## Goal 7: Daily UX And Operator Polish

### Intent

Make Conary feel boring and understandable in daily workflows after the
structural behavior has landed.

### Current Gap

The CLI surface is broad, but the roadmap still calls out shell integration,
troubleshooting output, unsupported-case guidance, and fewer special-knowledge
testing paths.

### Scope

- Add a checked-in UX matrix that lists the install/remove/update/search/list/
  autoremove/pin/unpin diagnostics and the expected user route for each
  unsupported case.
- Make those diagnostics match the matrix and add tests for native PM, adoption
  refresh, explicit takeover, generation command, and daemon route guidance.
- Add or refresh shell completions and command examples, with a verification
  command that renders completion output instead of relying on visual review.
- Add smoke workflows for the commands a daily user runs repeatedly and name
  the test targets in this spec.
- Refresh docs and quick-start language after Goals 1 through 6.
- Keep troubleshooting output compact enough for humans and structured enough
  for tests.

### Out Of Scope

- Cosmetic rewrites that do not reduce confusion.
- New UI clients.
- Changing behavior without tests.

### Read First

- `apps/conary/src/cli/`
- `apps/conary/src/dispatch.rs`
- `apps/conary/src/live_host_safety.rs`
- `apps/conary/src/commands/progress.rs`
- `apps/conary/tests/native_pm_daily_driver.rs`
- `docs/llms/README.md`
- `README.md`
- `ROADMAP.md`

### Stop Condition

The goal is complete when the checked-in UX matrix maps each daily-driver
command to tested success, refusal, and unsupported-case wording; tests assert
the native PM, adoption refresh, explicit takeover, generation command, and
daemon route guidance; shell completion rendering is verified for at least bash
and zsh; quick-start docs reflect the final daily-driver contract; and
`native_pm_daily_driver` smoke tests run through the integration-test target.

### Verification

```bash
cargo run -p conary -- --help
cargo run -p conary -- install --help
cargo run -p conary -- update --help
cargo run -p conary -- system --help
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
cargo run -p conary -- system completions zsh >/tmp/conary-completion.zsh
cargo test -p conary --bin conary cli::
cargo test -p conary --test native_pm_daily_driver
cargo run -p conary-test -- list
```

### Goal Template

```text
/goal Polish Conary daily-driver UX after the structural readiness goals. Read apps/conary/src/cli, apps/conary/src/dispatch.rs, apps/conary/src/live_host_safety.rs, apps/conary/src/commands/progress.rs, apps/conary/tests/native_pm_daily_driver.rs, docs/llms/README.md, README.md, and ROADMAP.md first. Add a checked-in UX matrix for common package command diagnostics and unsupported-case routes, make the CLI output match that matrix, refresh shell completions and command examples, ensure unsupported cases route users to native PM/adoption refresh/takeover/generation/daemon guidance as appropriate, update docs/audit metadata, and stop when focused CLI tests, completion rendering, help-output checks, daily workflow smoke coverage, and the shared verification gate pass.
```

## Program Completion Bar

The daily-driver readiness program is complete only when all seven goal tracks
have landed on `main`, active docs no longer describe any of the seven gaps as
open follow-up work, the audit ledger is complete, and the final validation set
has fresh evidence:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
scripts/local-qemu-validation.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

At that point, the replacement-readiness question can be re-asked from
evidence rather than caveats.
