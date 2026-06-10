# conary-test Evidence Trust Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `conary-test` evidence fail closed for QEMU skips, malformed result JSON, duplicate IDs, and ignored manifest keys.

**Architecture:** Harden the harness from the inside out: runner semantics first, result gate second, manifest validation third, docs last. Keep KVM-required execution optional and clearly labeled.

**Tech Stack:** Rust, Serde/TOML manifests, shell validators, GitHub Actions docs.

---

## Design Source

- `docs/superpowers/specs/archive/2026-06-10-conary-test-evidence-trust-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `apps/conary-test/src/engine/qemu.rs` | Return skip-shaped results for QEMU preflight skips. |
| `apps/conary-test/src/engine/executor.rs` | Preserve explicit skip outcomes through generic execution plumbing if QEMU results pass through it. |
| `apps/conary-test/src/engine/runner.rs` | Map skip-shaped QEMU results to `TestStatus::Skipped`. |
| `apps/conary-test/src/config/manifest.rs` | Reject unknown keys and duplicate loaded test IDs. |
| `apps/conary-test/src/engine/assertions.rs` | Add assertion evaluation if `stderr_not_contains` is retained. |
| `apps/conary-test/src/engine/variables.rs` | Expand variables in `stderr_not_contains` if retained. |
| `apps/conary/tests/integration/remi/manifests/*.toml` | Renumber duplicate IDs and fix unsupported assertion keys. |
| `scripts/check-conary-test-result-gate.sh` | Reject empty or inconsistent result JSON. |
| `scripts/test-conary-test-result-gate.sh` | Automated positive and negative fixtures for the result gate. |
| `scripts/local-qemu-validation.sh` | Build all exercised binaries and invoke the hardened result gate. |
| `docs/INTEGRATION-TESTING.md` | Record actual harness behavior and suite counts. |
| `apps/conary-test/README.md` | Refresh command and evidence claims if repeated there. |

## Task 0: Baseline And Inventory

- [ ] Run `cargo test -p conary-test engine::runner`.
- [ ] Run `cargo test -p conary-test config::manifest`.
- [ ] Run `cargo run -p conary-test -- list`.
- [ ] Record current manifest counts:

```bash
for f in apps/conary/tests/integration/remi/manifests/phase*.toml; do
  printf '%s %s\n' "$(rg -c '^\[\[test\]\]' "$f")" "$f"
done | LC_ALL=C sort -k2
```

Expected current total before repairs: 333 manifest tests.

## Task 1: Fix QEMU Skip Semantics

- [ ] Add a failing runner test by changing the current QEMU missing-tool test expectation from pass to skip:

```rust
        assert_eq!(suite.passed(), 0);
        assert_eq!(suite.skipped(), 1);
        assert_eq!(suite.failed(), 0);
        assert_eq!(suite.results[0].status, TestStatus::Skipped);
```

- [ ] Introduce an explicit skip marker in `apps/conary-test/src/engine/qemu.rs`. A minimal shape is acceptable as long as `runner.rs` can distinguish skip from pass:

```rust
const QEMU_SKIP_EXIT_CODE: i32 = 77;
```

- [ ] Change `skipped_result()` to use that marker and keep the existing human-readable skip message.
- [ ] Preserve the skip outcome across `qemu.rs`, any `engine/executor.rs` conversion path it uses, and `engine/runner.rs`. Do not rely on a generic success-shaped `ExecResult`; setup-step and assertion handling must not convert an intentional skip into pass.
- [ ] Update runner QEMU step handling so the explicit skip outcome becomes `TestStatus::Skipped`.
- [ ] Run `cargo test -p conary-test engine::runner::tests::test_runner_qemu_boot_step_skips_when_tooling_missing -- --exact`.
  Expected: the command reports `running 1 test` and passes, with skipped count equal to 1.

## Task 2: Harden The Result Gate

- [ ] Create `scripts/test-conary-test-result-gate.sh` with temporary JSON fixtures for the result gate.
- [ ] Cover these negative cases:

```json
{}
```

```json
{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[]}
```

```json
{"summary":{"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"passed"}]}
```

```json
{"summary":{"total":2,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"passed"}]}
```

```json
{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"failed"}]}
```

- [ ] Update `scripts/check-conary-test-result-gate.sh` so it requires `summary.total`, requires at least one result, requires `summary.total == results.length`, recomputes status counts from `results[]`, and fails when summary counts disagree.
- [ ] Preserve the release default: any failed, skipped, or cancelled result exits non-zero.
- [ ] Run the negative fixtures and verify they fail.
- [ ] Run the gate against a temporary good result:

```json
{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","name":"smoke","status":"passed"}]}
```

Expected: `/tmp/conary-test-good-result.json: ok`.
- [ ] Run `bash scripts/test-conary-test-result-gate.sh`.
  Expected: all fixture cases pass, including negative cases that expect the gate to fail.

## Task 3: Wire Local QEMU Validation To The Gate

- [ ] In `scripts/local-qemu-validation.sh`, build every binary that selected suites can exercise:

```bash
cargo build -p conary -p conary-test -p remi -p conaryd --verbose
```

- [ ] Capture each `conary-test run` JSON result path or stdout JSON in a stable file under `${LOG_DIR}`.
- [ ] Invoke:

```bash
bash scripts/check-conary-test-result-gate.sh "${result_json}"
```

for every suite result.
- [ ] Keep the boot-marker grep as an extra proof, not as the only result gate.
- [ ] Run `bash -n scripts/local-qemu-validation.sh`.

## Task 4: Reject Unknown Manifest Keys

- [ ] Add `#[serde(deny_unknown_fields)]` to manifest structs that are deserialized from TOML, including `Assertion`.
- [ ] Decide the existing `stderr_not_contains = "panic"` line in `phase4-group-d.toml`:
  - implement `stderr_not_contains` in `Assertion` and `engine/assertions.rs`; or
  - remove the unsupported key and replace it with a supported assertion.
- [ ] If retained, add this field:

```rust
    #[serde(default)]
    pub stderr_not_contains: Option<String>,
```

and assert failure when stderr contains the forbidden text.
- [ ] If retained, update `apps/conary-test/src/engine/assertions.rs` to fail when stderr contains the forbidden text, update `apps/conary-test/src/engine/variables.rs` to expand variables in the field, and add unit coverage for both paths.
- [ ] Run `cargo test -p conary-test config::manifest`.

## Task 5: Enforce Unique Manifest IDs

- [ ] Add a validation path that checks duplicate IDs across all manifests loaded by `conary-test -- list`.
- [ ] Add a negative unit test with two manifest snippets containing the same `id = "T01"`.
- [ ] Before renumbering, compare duplicate test names and grep for references:

```bash
awk 'FNR==1{file=FILENAME} /^id = /{gsub(/"/,"",$3); print $3 "\t" file ":" FNR}' apps/conary/tests/integration/remi/manifests/*.toml | sort | awk -F '\t' 'seen[$1]{print seen[$1] "\n" $0} !seen[$1]{seen[$1]=$0}'
rg -n 'T138|T230|T231|T232|T233|T234|T235|T236|T237|T238|T239|T240|T241|T242|T243|T244|T245|T246|T247|T248|T249|T250|T251' docs apps scripts
```

- [ ] Use this default renumbering unless the pre-check finds documented references that require a different mapping:
  - Keep Phase 3 Group M as `T138` through `T149`; rename the out-of-sequence duplicate in `phase3-group-l.toml` from `T138` to `T130a`.
  - Keep Phase 4 Group D as `T221` through `T255`; rename Phase 4 Group E numeric IDs from `T230` through `T251` to `T256` through `T277`.
  - Preserve Group E suffix shape by renaming `T232a` to `T258a` and `T232b` to `T258b`.
- [ ] The duplicate-ID validation failure contract is: print each duplicate ID with every source file to stderr and exit non-zero.
- [ ] Run:

```bash
cargo test -p conary-test config::manifest
cargo run -p conary-test -- list
```

Expected: no duplicate-ID diagnostics.

## Task 6: Refresh Integration Docs

- [ ] Update `docs/INTEGRATION-TESTING.md` with actual Phase 1 through Phase 4 suite counts and ID ranges after renumbering.
- [ ] Add `conary-test images build`, `conary-test images list`, and `conary-test deploy rebuild` to the CLI table if they remain live commands. Keep `conary-test deploy rollout` if the CLI still exposes it; do not replace a live command while adding the missing one.
- [ ] State clearly that ordinary CI parses manifest inventory and runs conary-test unit tests, while full TOML suite execution depends on local or hosted runner capability.
- [ ] Update result JSON examples to match `apps/conary-test/src/report/json.rs`.
- [ ] Update `apps/conary-test/README.md` if it repeats stale command, MCP, or result-gate claims.

## Task 7: Final Verification And Commit

- [ ] Run:

```bash
cargo test -p conary-test engine::runner
cargo test -p conary-test config::manifest
cargo test -p conary-test
cargo run -p conary-test -- list
bash scripts/test-conary-test-result-gate.sh
printf '%s\n' '{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","name":"smoke","status":"passed"}]}' > /tmp/conary-test-good-result.json
bash scripts/check-conary-test-result-gate.sh /tmp/conary-test-good-result.json
bash -n scripts/local-qemu-validation.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

- [ ] Review `git diff --name-only`, then stage only the exact files changed by
  this track. Do not stage directories.
- [ ] Commit:

```bash
git commit -m "fix(conary-test): harden evidence gates"
```

Do not use a broad directory add in a dirty worktree. The expected file set is
under `apps/conary-test/src/`, `apps/conary/tests/integration/remi/manifests/`,
the result-gate/local-QEMU scripts, and integration-test docs, but the final
stage list must come from the actual diff.
