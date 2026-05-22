# Goal 1 Native Authority Handoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an explicit selected-generation handoff flow that returns adopted packages to native package-manager authority without deleting native files or mutating native package-manager databases.

**Architecture:** Goal 1 uses a staged CLI operation instead of weakening `system unadopt --all`. `conary system native-handoff` records durable handoff state, clears the selected Conary generation pointer, removes only adopted Conary tracking rows, and can recover an interrupted handoff by replaying incomplete stages. Native package managers remain authoritative because the command only reads their presence and never invokes native package install/remove operations.

**Tech Stack:** Rust CLI, Clap, SQLite via `conary_core::db`, Conary runtime-root generation symlinks, operation-state JSON, `conary-test` integration manifests, Markdown docs, doc-audit scripts.

---

## Design Decision

Goal 1 chooses the staged-operation model from the readiness spec:

1. Preflight confirms a Conary generation is selected, a supported native package manager is present, and the Conary database contains adopted packages to hand back.
2. Dry-run prints the selected generation, adopted packages, Conary-owned packages left alone, and recovery-state path without mutating the database or `/conary/current`.
3. Apply mode requires `--yes` and the existing live-system mutation acknowledgement. It writes an in-progress record before mutating state.
4. The selected generation pointer is cleared by atomically moving `/conary/current` to a handoff backup path, which reselects the mutable native root for subsequent Conary generation builds.
5. Adopted tracking rows are removed in a single Conary DB transaction, a changeset and state snapshot are recorded, and Conary-owned packages remain tracked.
6. Native package files and native package-manager databases are preserved because the flow does not delete filesystem package payloads and does not call dnf, rpm, apt, dpkg, pacman, or pacman-key mutation commands.
7. If interruption happens after the record is written, `conary system native-handoff --recover --yes` resumes missing stages from the record. It is idempotent if the selected-generation pointer was already cleared or adopted rows were already removed.

This does not implement live bootloader rollback, native transaction-history import, silent package takeover, or native package-manager database replacement.

## File Structure

- Create `apps/conary/src/commands/adopt/native_handoff.rs`: state machine, operation record, dry-run/apply/recover summaries, unit tests.
- Modify `apps/conary/src/commands/adopt/mod.rs`: expose the command and option type.
- Modify `apps/conary/src/commands/mod.rs`: re-export the command for dispatch.
- Modify `apps/conary/src/cli/system.rs`: add `system native-handoff` CLI options.
- Modify `apps/conary/src/dispatch.rs`: wire the command through live-mutation safety.
- Create `apps/conary/tests/integration/remi/manifests/phase3-active-generation-handoff.toml`: selected-generation handoff suite for Fedora 44, Ubuntu 26.04 LTS, and Arch.
- Modify `README.md`, `ROADMAP.md`, `docs/INTEGRATION-TESTING.md`, and `docs/conaryopedia-v2.md`: update only after fresh evidence exists.
- Modify doc audit ledger, inventory, and summary when docs are touched.

## State Machine

| State | Durable marker | Meaning | Recovery action |
|---|---|---|---|
| `planned` | record exists, no stage complete | Preflight passed and package list was captured | clear current pointer, then remove adopted tracking |
| `current-cleared` | `current_link_cleared=true` | `/conary/current` was moved aside or was already absent during replay | remove adopted tracking |
| `tracking-removed` | `tracking_removed=true` | adopted rows were removed and changeset/state snapshot exists or no rows remained | remove hooks unless `--keep-hooks`, then complete |
| `completed` | `completed_at` set | native authority handoff is complete | print completed summary |

## Task 1: Unit Tests For Handoff Planning

**Files:**
- Create: `apps/conary/src/commands/adopt/native_handoff.rs`

- [ ] **Step 1: Add tests that describe dry-run planning**

Add tests that create a temp Conary DB, seed one `AdoptedFull` package and one Conary-owned package, create a fake `/current` generation link using `ConaryRuntimeRoot::from_db_path`, and call the internal handoff helper with a supported package-manager probe and `dry_run=true`.

Expected assertions:
- returned summary stage is dry-run
- selected generation is reported
- adopted package appears in the handoff plan
- Conary-owned package appears in the skipped list
- database rows remain unchanged
- current generation link remains selected

- [ ] **Step 2: Run the test and watch it fail**

Run:

```bash
cargo test -p conary --bin conary adopt::native_handoff::tests::native_handoff_dry_run_reports_selected_generation_without_mutation
```

Expected: fail to compile because the new module/helper does not exist yet.

## Task 2: Implement Dry-Run And Refusal Behavior

**Files:**
- Create: `apps/conary/src/commands/adopt/native_handoff.rs`
- Modify: `apps/conary/src/commands/adopt/mod.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/cli/system.rs`
- Modify: `apps/conary/src/dispatch.rs`

- [ ] **Step 1: Add minimal command types**

Implement:

```rust
pub struct NativeHandoffOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub recover: bool,
    pub keep_hooks: bool,
}
```

and:

```rust
pub async fn cmd_native_handoff(options: NativeHandoffOptions, db_path: &str) -> Result<NativeHandoffSummary>
```

- [ ] **Step 2: Add preflight refusals**

Refuse apply mode when:
- no selected generation exists and no incomplete handoff record exists
- no supported native package manager is detected
- adopted packages are absent
- `--yes` is missing outside dry-run
- `--recover` is requested without an incomplete handoff record

Messages must point users to `system unadopt --all` when no generation is selected, and to `--dry-run` for preview.

- [ ] **Step 3: Wire the CLI**

Add:

```text
conary system native-handoff [--dry-run] [--yes] [--recover] [--keep-hooks] [--db-path PATH]
```

Dispatch must call `require_live_mutation` with label `conary system native-handoff` and class `CurrentlyLiveEvenWithRootArguments`, honoring `dry_run`.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary --bin conary adopt::native_handoff
```

Expected: all current native handoff tests pass.

## Task 3: Apply And Recovery State Machine

**Files:**
- Modify: `apps/conary/src/commands/adopt/native_handoff.rs`

- [ ] **Step 1: Add failing apply/recovery tests**

Add tests for:
- apply mode clears selected generation and removes adopted rows
- apply mode leaves Conary-owned rows intact
- apply mode creates a changeset/state snapshot
- interruption after current-link clearing can be recovered with `--recover --yes`

Use a test-only failpoint:

```rust
CONARY_TEST_NATIVE_HANDOFF_FAIL_AFTER=current-cleared
```

Expected: the first run errors after clearing `/current`, and recovery completes the tracking removal.

- [ ] **Step 2: Implement durable operation records**

Write JSON under the runtime root:

```text
<runtime-root>/native-authority-handoff.json
```

The record includes selected generation, adopted package names, skipped Conary-owned package names, backup path, booleans for completed stages, optional changeset id, timestamps, and recovery instructions.

- [ ] **Step 3: Implement idempotent stage replay**

Stage replay must:
- keep going if `/current` is already absent after the record says it was cleared
- remove adopted rows only for packages recorded in the plan and still present as adopted
- leave native files and native PM databases untouched
- mark completion only after the DB transaction and hook-removal stage finish

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary --bin conary adopt::native_handoff
cargo test -p conary --bin conary adopt::unadopt
```

Expected: handoff tests pass and pre-selection unadopt behavior remains unchanged.

## Task 4: Integration Manifest

**Files:**
- Create: `apps/conary/tests/integration/remi/manifests/phase3-active-generation-handoff.toml`

- [ ] **Step 1: Add selected-generation handoff tests**

The manifest must run on Fedora 44, Ubuntu 26.04 LTS, and Arch and prove:
- dry-run leaves `/conary/current` and adopted rows in place
- apply refusal happens without `--yes`
- success clears `/conary/current`, removes adopted rows, preserves a native package-manager query for the adopted package, and preserves package files
- interruption after current-link clearing can be recovered with `--recover --yes`

- [ ] **Step 2: Check suite inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: `phase3-active-generation-handoff` appears in the phase 3 suites.

## Task 5: Docs And Audit Metadata

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update user-facing docs only after evidence**

Replace open-gap wording with the implemented `system native-handoff` flow only after the focused tests and suite are present. Keep caveats for bootloader rollback, native transaction-history import, and non-validated behavior.

- [ ] **Step 2: Refresh audit metadata**

Run:

```bash
bash scripts/docs-audit-inventory.sh > /tmp/conary-doc-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-doc-inventory.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: inventory and ledger include the new plan/manifest/doc changes.

## Task 6: Verification

**Files:**
- All touched files.

- [ ] **Step 1: Run focused Goal 1 tests**

Run:

```bash
cargo test -p conary --bin conary adopt::native_handoff
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

Expected: every focused command passes. Infrastructure blockers must be documented instead of being treated as success.

- [ ] **Step 2: Run shared gates**

Run:

```bash
cargo fmt --check
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
rg -n "Fedora 4[3]" README.md ROADMAP.md docs apps crates
pgrep -af "qemu|conary-test" || true
find /tmp -maxdepth 1 -name 'conary-*' -o -name 'tmp.*'
```

Expected: formatting, suite inventory, Clippy, diff whitespace, doc audit, older-Fedora-baseline sweep, and cleanup checks all pass or are honestly reported.
