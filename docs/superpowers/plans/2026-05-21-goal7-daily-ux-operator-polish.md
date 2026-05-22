# Goal 7 Daily UX And Operator Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish Conary daily-driver CLI diagnostics, shell integration, quick-start docs, and operator routes without reopening Goal 6 boot/artifact trust work.

**Status:** Implemented and verified locally in the `goal7-daily-ux` worktree on 2026-05-21.

**Architecture:** Keep behavior changes small and test-first. Add a checked-in operator UX matrix under operations docs, centralize only the guidance text that multiple CLI paths need, and assert the matrix through focused CLI integration tests plus completion rendering checks.

**Tech Stack:** Rust CLI (`apps/conary`), clap/clap_complete, integration tests under `apps/conary/tests`, Markdown docs, Conary doc audit scripts.

---

## File Structure

- Create `docs/operations/daily-driver-ux-matrix.md`: checked-in matrix for daily-driver command outcomes, unsupported-case routes, exact guidance phrases, and verification targets.
- Create `apps/conary/tests/cli_daily_ux.rs`: focused binary-level tests for help examples, bash/zsh completion rendering, live-mutation refusal guidance, and adopted-package routes.
- Modify `apps/conary/src/cli/mod.rs`: add compact root help examples for dry-run/apply, adoption refresh, completions, generation export, and conaryd/operator routes.
- Modify `apps/conary/src/live_host_safety.rs`: make live-host refusal output route users to dry-run, explicit acknowledgement, and conaryd package jobs.
- Modify `apps/conary/src/commands/install/mod.rs`, `apps/conary/src/commands/remove.rs`, and `apps/conary/src/commands/update.rs`: align adopted-package diagnostics with the UX matrix.
- Modify `README.md`, `ROADMAP.md`, `docs/INTEGRATION-TESTING.md`, `docs/llms/README.md`, and doc audit metadata if those active docs need Goal 7 status or verification references.

## Task 1: Checked-In UX Matrix

**Files:**
- Create: `docs/operations/daily-driver-ux-matrix.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [x] **Step 1: Write the matrix**

Add a table with rows for `install`, `remove`, `update`, `search`, `list`, `autoremove`, `pin`, and `unpin`. Each row must name the success route, refusal route, unsupported-case route, operator guidance phrase, and focused test target.

- [x] **Step 2: Register the doc**

Run:

```bash
bash scripts/docs-audit-inventory.sh
```

Expected: the generated inventory includes `docs/operations/daily-driver-ux-matrix.md`.

- [x] **Step 3: Update ledger metadata**

Add a ledger row for the matrix and update any touched active docs with source pointers to the matrix and focused test file.

## Task 2: Failing CLI UX Tests

**Files:**
- Create: `apps/conary/tests/cli_daily_ux.rs`

- [x] **Step 1: Add help and completion tests**

Add tests that run:

```bash
conary --help
conary system completions bash
conary system completions zsh
```

Expected assertions: root help includes `Daily workflow examples`, `conary system adopt --refresh --dry-run`, `conary system completions bash`, and `conaryd`; bash output includes `_conary`; zsh output includes `#compdef conary`.

- [x] **Step 2: Add diagnostic route tests**

Add tests that assert:

- missing live mutation acknowledgement mentions `--dry-run`, `--allow-live-system-mutation`, and `conaryd`;
- adopted install refusal mentions `conary system adopt --refresh`, `conary install <pkg> --dep-mode takeover`, and `conary system takeover`;
- adopted remove refusal mentions native package-manager authority, `conary system unadopt`, and `--purge-files`;
- adopted update dry-run mentions native package-manager authority and `conary system adopt --refresh`.

- [x] **Step 3: Run focused tests and confirm RED**

Run:

```bash
cargo test -p conary --test cli_daily_ux
```

Expected: tests fail because the matrix doc and new guidance phrases are not fully implemented yet.

## Task 3: Implement Guidance And Help Polish

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/live_host_safety.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/update.rs`

- [x] **Step 1: Update root help examples**

Add a compact `after_help` block to `Cli` with daily dry-run/apply examples, adoption refresh, shell completions, generation artifact export, and conaryd operator wording.

- [x] **Step 2: Update live-host refusal guidance**

Append explicit next routes to `require_live_system_mutation_ack`: use `--dry-run` for preview, rerun with `--allow-live-system-mutation` to mutate, or use conaryd package jobs for background/durable operator execution.

- [x] **Step 3: Align adopted-package diagnostics**

Update install/remove/update messages so adopted packages consistently route to native package-manager authority, `conary system adopt --refresh`, explicit package takeover, and system takeover where relevant.

- [x] **Step 4: Run focused tests and confirm GREEN**

Run:

```bash
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test native_pm_daily_driver
cargo test -p conary live_host_safety
```

Expected: all focused UX tests pass.

## Task 4: Docs And Audit Refresh

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/superpowers/specs/2026-05-19-daily-driver-readiness-program-design.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [x] **Step 1: Refresh active docs**

Record the Goal 7 UX matrix, completion rendering checks, and focused `cli_daily_ux` test target without claiming unvalidated QEMU or full program completion.

- [x] **Step 2: Run stale wording sweeps**

Run:

```bash
rg -n "Fedora 4[3]|fedora4[3]|f4[3]" docs README.md ROADMAP.md
rg -n "Goal 7|Daily UX|Operator Polish|daily-driver UX|completion rendering" docs README.md ROADMAP.md
```

Expected: no active references to the retired Fedora baseline; Goal 7 references point to the current matrix and tests.

- [x] **Step 3: Run doc audit checks**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected: both pass.

## Task 5: Final Verification And Cleanup

**Files:**
- No new files beyond the implementation/doc changes.

- [x] **Step 1: Run focused help/completion checks**

Run:

```bash
cargo run -p conary -- --help
cargo run -p conary -- install --help
cargo run -p conary -- update --help
cargo run -p conary -- system --help
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
cargo run -p conary -- system completions zsh >/tmp/conary-completion.zsh
```

Expected: commands exit successfully and completion files are non-empty.

- [x] **Step 2: Run shared verification gate**

Run:

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all pass.

- [x] **Step 3: Check cleanup state**

Run:

```bash
git status --short --branch
git worktree list
ps -eo pid,cmd | rg "qemu-system|conary-test serve|target/debug/conary-test" || true
find /tmp -maxdepth 1 -iname "*conary*" -print
find target/local-validation -maxdepth 2 -type d -print 2>/dev/null
```

Expected: no stale QEMU or conary-test service processes from this work; only intentional retained validation directories appear.

## Self-Review Notes

- Spec coverage: UX matrix, diagnostic routes, shell completion rendering, quick-start docs, audit metadata, and shared gate are represented.
- Scope guard: no new UI client, no package-manager behavior changes without tests, and no Goal 6 boot/artifact work.
- Ambiguity resolved: conaryd guidance is operator routing text and tests, not a new daemon feature.
