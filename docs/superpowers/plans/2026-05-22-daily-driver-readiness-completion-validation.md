# Daily Driver Readiness Completion Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate whether the seven-track daily-driver readiness program satisfies the spec completion bar on `main`, and record the result honestly.

**Architecture:** This is an evidence and release-honesty pass, not a new feature implementation. The audit compares the program completion bar against current `main`, runs the supported final validation commands, records blockers where infrastructure or suites are missing, and updates active docs only when fresh evidence changes the truth.

**Tech Stack:** Git worktrees, Rust/Cargo, `conary-test`, local QEMU wrapper, Markdown docs, Conary doc-audit scripts.

---

## File Structure

- Create `docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md`: validation report with goal-track status, command evidence, blockers, and next actions.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`: include the new validation report if tracked.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`: add a ledger row for the validation report and refresh touched active docs.
- Modify `docs/superpowers/documentation-accuracy-audit-summary.md`: add the validation pass only if docs are touched.
- Modify `README.md`, `ROADMAP.md`, or `docs/INTEGRATION-TESTING.md` only when fresh evidence or discovered stale wording requires it.

## Task 1: Baseline And Completion Criteria

**Files:**
- Create: `docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md`

- [x] **Step 1: Confirm integration branch state**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
git worktree list
```

Expected: the validation worktree is on `readiness-completion-validation`, and `/home/peter/Conary` `main` matches `origin/main`.

- [x] **Step 2: Record the spec completion bar**

Read:

```bash
sed -n '657,682p' docs/superpowers/specs/2026-05-19-daily-driver-readiness-program-design.md
```

Expected: capture the exact required final validation set in the audit report.

- [x] **Step 3: Check manifest inventory for required suites**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: `phase1-advanced`, `phase4-native-pm-parity`, and the local QEMU suites are listed. If a suite required by a goal stop condition is absent, record it as missing evidence.

## Task 2: Goal Track Truth Map

**Files:**
- Modify: `docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md`

- [x] **Step 1: Search active docs for the seven goal gaps**

Run:

```bash
rg -n "active-generation handoff|Real Package Corpus|Security Advisory Pipeline|Host-Mutation Sandbox|conaryd Package Execution|Recovery, Boot|Daily UX|Goal [1-7]|follow-up work|remaining gap|not implemented|reserved follow-up" README.md ROADMAP.md docs apps crates
```

Expected: active docs may still mention real follow-up gaps. Every active open-gap reference must be classified as either truthful preview caveat, stale completion blocker, or historical/archive-only.

- [x] **Step 2: Search for required final evidence suites**

Run:

```bash
rg -n "phase3-active-generation-handoff|phase4-native-pm-parity|phase1-advanced|local-qemu-validation|cli_daily_ux|security-advisory-pipeline" docs apps scripts
```

Expected: evidence paths for implemented tracks are discoverable; absent required handoff suite evidence is recorded as a blocker rather than inferred.

- [x] **Step 3: Fill the goal-track table**

Add this table to the audit report and fill each `Evidence` and `Status` cell from the searches:

```markdown
| Track | Completion Requirement | Evidence | Status |
|---|---|---|---|
| Goal 1 Native Authority Handoff | Selected-generation handoff suite and no active-doc open-gap wording |  |  |
| Goal 2 Real Package Corpus Validation | Phase 1/Phase 4 distro evidence and corpus coverage |  |  |
| Goal 3 Security Advisory Pipeline | Trusted advisory ingestion and update proof |  |  |
| Goal 4 Host-Mutation Sandbox Hardening | Protected live-root sandbox proof |  |  |
| Goal 5 conaryd Package Execution | Package job execution proof and no blanket 501 package-route claims |  |  |
| Goal 6 Recovery, Boot, And Artifact Trust | Group N/O/P and artifact provenance evidence |  |  |
| Goal 7 Daily UX And Operator Polish | UX matrix, completion rendering, CLI diagnostics proof |  |  |
```

Expected: the table must distinguish landed evidence from remaining follow-up work.

## Task 3: Final Validation Commands

**Files:**
- Modify: `docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md`

- [x] **Step 1: Run the shared non-QEMU gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands pass before any completion claim.

- [x] **Step 2: Run the phase1 advanced distro matrix**

Run each command and record pass/fail/blocker:

```bash
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
```

Expected: release evidence only if each result reports zero failed, skipped, and cancelled tests. Missing Podman, network, image, or runner infrastructure is a blocker.

- [x] **Step 3: Run the phase4 native-PM parity distro matrix**

Run each command and record pass/fail/blocker:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
```

Expected: release evidence only if each result reports zero failed, skipped, and cancelled tests.

- [x] **Step 4: Run local QEMU gate when host support exists**

Run:

```bash
test -e /dev/kvm
scripts/local-qemu-validation.sh
```

Expected: if `/dev/kvm` exists, the wrapper must pass with zero failed, skipped, and cancelled results. If `/dev/kvm` is missing, record the QEMU gate as locally blocked instead of complete.

## Task 4: Docs And Audit Metadata

**Files:**
- Modify: `README.md` if stale completion or next-step wording is discovered.
- Modify: `ROADMAP.md` if stale completion or next-step wording is discovered.
- Modify: `docs/INTEGRATION-TESTING.md` if fresh validation evidence changes current evidence claims.
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [x] **Step 1: Apply only evidence-backed docs changes**

If validation proves a program item complete, update docs to say so. If validation finds blockers, keep or add clear caveats. Do not remove active-generation handoff caveats unless the selected-generation handoff evidence exists.

- [x] **Step 2: Refresh doc audit inventory**

Run:

```bash
git add -N docs/superpowers/daily-driver-readiness-completion-audit-2026-05-22.md
bash scripts/docs-audit-inventory.sh > /tmp/conary-doc-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-doc-inventory.tsv
```

Expected: the only required inventory delta is the new validation report unless other docs were added or removed.

- [x] **Step 3: Update ledger and summary**

Add a `verified` ledger row for the validation report. Update summary scope and verification commands to include the completion audit and any fresh evidence actually run.

## Task 5: Final Hygiene And Cleanup

**Files:**
- No additional files beyond validation docs and audit metadata.

- [x] **Step 1: Re-run required hygiene checks**

Run:

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all pass after docs edits.

- [x] **Step 2: Run stale-reference sweeps**

Run:

```bash
rg -n "Fedora 4[3]|fedora4[3]|f4[3]" docs README.md ROADMAP.md
rg -n "<unverified daily-driver completion claim phrases>" README.md ROADMAP.md docs
```

Expected: no stale active-doc references to older Fedora preview baselines; no unverified completion claims.

- [x] **Step 3: Check runtime cleanup**

Run:

```bash
ps -eo pid,cmd | rg "([q]emu-system)|([c]onary-test serve)|([t]arget/debug/conary-test)" || true
find /tmp -maxdepth 1 \( -iname '*conary*' -o -iname '*qemu*' \) -print
find target -maxdepth 4 \( -name '*.qcow2' -o -name '*.raw' -o -name '*.iso' \) -print
```

Expected: no stale QEMU/conary-test processes and no temporary artifacts left from this validation pass.

## Self-Review Notes

- Spec coverage: the plan maps the program completion bar, seven goal tracks, final evidence commands, docs audit, and cleanup checks.
- Release honesty: missing suites, missing `/dev/kvm`, failed matrix runs, or active open-gap docs are blockers, not partial completion.
- Scope guard: this pass does not implement Goal 1 or any new product behavior; it records whether the current program can honestly be called complete.
