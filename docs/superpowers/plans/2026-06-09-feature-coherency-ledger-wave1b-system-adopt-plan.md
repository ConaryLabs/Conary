# Feature Coherency Ledger Wave 1b System Adopt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute feature coherency Wave 1b against the `conary system adopt` command family, adding durable parser proof, capturing help/dispatch/module evidence, repairing any bounded honesty gaps, and closing ledger rows under `wave_scope=1b-system-adopt`.

**Architecture:** Keep this wave inside the CLI command tree. Audit `system adopt` help, Clap parser constraints, dispatch routing, generated root manpage/root example behavior, focused adopt module tests, and active docs only when they repeat the selected CLI claim. Treat single-package `--dry-run` as an intentional deferred surface only if help and runtime refusal remain explicit and verified.

**Tech Stack:** Rust, Clap, existing Conary CLI tests, shell evidence capture, `docs/superpowers/feature-coherency-ledger.tsv`, `scripts/check-coherency-ledger.sh`, docs-audit scripts.

---

## Current Repository Facts

- Execution should start from the current `main` commit that contains this plan, with `HEAD` and `origin/main` synced. The historical Wave 1a closure tip before this plan was `272f1ae3e49710e2fc86fcff43d7d1b3705c9581`; do not reset to that historical SHA.
- Current docs-audit inventory count with this committed plan and the active single-package dry-run follow-up note is `172` tracked doc-like files.
- `LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l` should continue to return `172`.
- The coherency ledger currently has four closed `1a-root-cli` rows and no `1b-system-adopt` rows.
- `apps/conary/src/commands/adopt/system.rs` is 1236 lines. This is below the repo's 1500-line major-edit threshold, but it is still the main bulk-adoption command body; Wave 1b should avoid growing it unless evidence shows a bounded behavior repair.

## Non-Goals

- Do not audit `system unadopt`, `system native-handoff`, conaryd routes, Remi HTTP/MCP routes, or broad active docs in Wave 1b.
- Do not implement single-package `conary system adopt <pkg> --dry-run` in this wave unless evidence shows the current honest refusal is misleading or internally inconsistent.
- Do not add TSV ledger files to the documentation accuracy audit inventory; the coherency ledger remains protected by `scripts/check-coherency-ledger.sh`.
- Do not commit generated manpage output from `apps/conary/man/`.

## File Map

| Path | Role In This Wave |
| --- | --- |
| `apps/conary/src/cli/system.rs` | `SystemCommands::Adopt` Clap mode definitions and help text. Read and edit only if help or parser constraints drift. |
| `apps/conary/src/cli/mod.rs` | Existing CLI parser tests. Add `system adopt` characterization tests here. |
| `apps/conary/src/command_risk.rs` | Pre-dispatch risk classification for adoption dry-run labels and hidden `--from-sync-hook` policy. Run focused tests because dispatch intentionally ignores that flag after policy enforcement. |
| `apps/conary/src/dispatch/system.rs` | Dispatch routing from `SystemCommands::Adopt` into package, system, status, refresh, convert, and sync-hook commands. Edit only for bounded dispatch honesty repairs. |
| `apps/conary/src/commands/adopt/` | Adopt command implementations and focused unit tests. Run the full `cargo test -p conary --lib commands::adopt` suite as behavior proof, which is broader than the ownership card's narrower native-handoff and unadopt filters; avoid broad refactors in this wave. |
| `apps/conary/tests/live_host_mutation_safety.rs` | Existing `system_adopt*` end-to-end CLI safety tests. Run focused filter. |
| `apps/conary/tests/cli_daily_ux.rs` | Existing daily UX references to `conary system adopt --refresh`. Run focused adopted-package filter. |
| `apps/conary/build.rs` | Generated root manpage source. Run `cargo build -p conary` and inspect generated ignored manpage output. |
| `docs/operations/daily-driver-ux-matrix.md` | Active doc repeats the `conary system adopt --refresh` guidance. Inspect only if command evidence shows that claim is stale. |
| `docs/operations/system-adopt-single-package-dry-run-follow-up.md` | Active follow-up owner for true single-package adopt dry-run preview or removal of package-mode dry-run visibility. |
| `docs/superpowers/feature-coherency-ledger.tsv` | Add and close `1b-system-adopt` rows. |
| `docs/superpowers/documentation-accuracy-audit-*` | Register this Markdown plan and keep docs-audit checks green. |

---

### Task 0: Verify Plan Metadata And Clean Baseline

**Files:**
- Read: `docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1b-system-adopt-plan.md`
- Read: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Read: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Read: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Confirm the plan is already tracked and registered once**

Run:

```bash
git ls-files docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1b-system-adopt-plan.md
rg -n '^docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1b-system-adopt-plan\.md\t' docs/superpowers/documentation-accuracy-audit-inventory.tsv
rg -n '^docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1b-system-adopt-plan\.md\t' docs/superpowers/documentation-accuracy-audit-ledger.tsv
```

Expected:

```text
docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1b-system-adopt-plan.md
```

The two `rg` commands should each print exactly one row. If either prints zero rows, add the missing docs-audit metadata before executing Wave 1b. If either prints more than one row, remove duplicates before executing Wave 1b.

- [ ] **Step 2: Verify docs-audit and coherency baseline**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
bash scripts/check-doc-truth.sh
git diff --check
git status --short --branch
```

Expected:

```text
Documentation audit ledger check passed (--require-complete).
Coherency ledger check passed.
Documentation truth checks passed.
## main...origin/main
```

The inventory diff and `git diff --check` should produce no output.

---

### Task 1: Add System Adopt Parser Characterization Tests

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Test: `cargo test -p conary --lib cli::tests`

- [ ] **Step 1: Add parser tests for the selected command family**

In `apps/conary/src/cli/mod.rs`, inside the existing `#[cfg(test)] mod tests`, insert these tests near the existing `system unadopt` parser tests:

```rust
    #[test]
    fn parses_system_adopt_system_dry_run_filters() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--system",
            "--dry-run",
            "--pattern",
            "lib*",
            "--exclude",
            "kernel*",
            "--explicit-only",
        ])
        .expect("system adopt --system dry-run filters should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                pattern,
                exclude,
                explicit_only,
                refresh,
                convert,
                sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(!full);
                assert!(system);
                assert!(!status);
                assert!(dry_run);
                assert_eq!(pattern.as_deref(), Some("lib*"));
                assert_eq!(exclude.as_deref(), Some("kernel*"));
                assert!(explicit_only);
                assert!(!refresh);
                assert!(!convert);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_refresh_quiet_from_sync_hook() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
        ])
        .expect("installed sync hook refresh path should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                refresh,
                convert,
                sync_hook,
                quiet,
                from_sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(!full);
                assert!(!system);
                assert!(!status);
                assert!(!dry_run);
                assert!(refresh);
                assert!(!convert);
                assert!(!sync_hook);
                assert!(quiet);
                assert!(from_sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn rejects_system_adopt_from_sync_hook_with_full() {
        let err = match Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--refresh",
            "--quiet",
            "--from-sync-hook",
            "--full",
        ]) {
            Ok(_) => panic!("--from-sync-hook must conflict with --full"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_system_adopt_convert_dry_run_jobs() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--convert",
            "--dry-run",
            "--jobs",
            "4",
            "--no-chunking",
        ])
        .expect("system adopt --convert dry-run jobs should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                convert,
                dry_run,
                jobs,
                no_chunking,
                system,
                status,
                refresh,
                sync_hook,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(convert);
                assert!(dry_run);
                assert_eq!(jobs, Some(4));
                assert!(no_chunking);
                assert!(!system);
                assert!(!status);
                assert!(!refresh);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_sync_hook_remove_hook() {
        let cli = Cli::try_parse_from([
            "conary",
            "system",
            "adopt",
            "--sync-hook",
            "--remove-hook",
        ])
        .expect("system adopt --sync-hook --remove-hook should parse");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                sync_hook,
                remove_hook,
                system,
                status,
                refresh,
                convert,
                ..
            })) => {
                assert!(packages.is_empty());
                assert!(sync_hook);
                assert!(remove_hook);
                assert!(!system);
                assert!(!status);
                assert!(!refresh);
                assert!(!convert);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn parses_system_adopt_package_dry_run_refusal_surface() {
        let cli = Cli::try_parse_from(["conary", "system", "adopt", "curl", "--dry-run"])
            .expect("single-package dry-run should parse before runtime refuses it");

        match cli.command {
            Some(Commands::System(SystemCommands::Adopt {
                packages,
                full,
                system,
                status,
                dry_run,
                refresh,
                convert,
                sync_hook,
                ..
            })) => {
                assert_eq!(packages, vec!["curl".to_string()]);
                assert!(!full);
                assert!(!system);
                assert!(!status);
                assert!(dry_run);
                assert!(!refresh);
                assert!(!convert);
                assert!(!sync_hook);
            }
            _ => panic!("expected system adopt command"),
        }
    }

    #[test]
    fn rejects_system_adopt_package_with_refresh_mode() {
        let err = match Cli::try_parse_from(["conary", "system", "adopt", "curl", "--refresh"]) {
            Ok(_) => panic!("package adopt must conflict with --refresh mode"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_system_adopt_quiet_without_refresh() {
        let err = match Cli::try_parse_from(["conary", "system", "adopt", "--quiet"]) {
            Ok(_) => panic!("--quiet must remain scoped to --refresh"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(
            rendered.contains("--refresh"),
            "quiet error should point users back to --refresh: {rendered}"
        );
    }
```

- [ ] **Step 2: Run the parser tests**

Run:

```bash
cargo test -p conary --lib cli::tests
```

Expected:

```text
test result: ok
```

If these characterization tests fail, inspect whether the failure is a real parser/help mismatch. Fix the smallest affected Clap constraint in `apps/conary/src/cli/system.rs` or adjust the test only if the current parser behavior is more honest and more specific than the test.

- [ ] **Step 3: Format the Rust edit**

Run:

```bash
cargo fmt --check
```

Expected:

```text
```

If formatting fails, run `cargo fmt`, then rerun `cargo fmt --check`.
The pasted test snippet is illustrative; `rustfmt` may rewrite array layout before the commit. Commit the formatted result, not the byte-for-byte snippet.

- [ ] **Step 4: Commit parser proof**

Run:

```bash
git status --short
git diff --name-only
git add apps/conary/src/cli/mod.rs
if git diff --name-only -- apps/conary/src/cli/system.rs | rg -q '^apps/conary/src/cli/system.rs$'; then
  git add apps/conary/src/cli/system.rs
fi
git diff --cached --name-only
git commit -m "test: cover system adopt CLI mode parsing"
```

Expected:

```text
[main <sha>] test: cover system adopt CLI mode parsing
```

---

### Task 2: Capture System Adopt Evidence

**Files:**
- Read: `apps/conary/src/cli/system.rs`
- Read: `apps/conary/src/dispatch/system.rs`
- Read: `apps/conary/src/commands/adopt/`
- Read: `docs/operations/daily-driver-ux-matrix.md`
- Modify later: `docs/superpowers/feature-coherency-ledger.tsv`

- [ ] **Step 1: Create scratch directory**

Run:

```bash
scratch="$(mktemp -d /tmp/conary-coherency-wave1b-system-adopt.XXXXXX)"
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
printf '%s\n' "$scratch" > "$scratch_file"
printf '%s\n' "$scratch"
```

Expected:

```text
/tmp/conary-coherency-wave1b-system-adopt.<suffix>
```

For every later command block that uses `$scratch`, start by reloading and validating it:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
```

Using `git rev-parse --git-common-dir` keeps the scratch pointer valid in the primary checkout and linked worktrees. If `test -d "$scratch"` fails because `/tmp` was cleaned, rerun Task 2 Step 1 and recapture the Task 2 evidence before continuing.

- [ ] **Step 2: Capture help and version surfaces**

Run:

```bash
set -euo pipefail
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
cargo run -p conary -- system --help > "$scratch/system-help.txt"
cargo run -p conary -- system adopt --help > "$scratch/system-adopt-help.txt"
cargo run -p conary -- --help > "$scratch/root-help.txt"
cargo run -p conary -- --version > "$scratch/root-version.txt"
sed -n '1,180p' "$scratch/system-adopt-help.txt"
cat "$scratch/root-version.txt"
```

Expected `system adopt --help` must include:

```text
Adopt system packages into Conary tracking
Use --system to adopt all packages
Use --refresh to detect version drift
Single-package dry-run is rejected until it has a true non-mutating preview path
--sync-hook
--quiet
```

Treat these as required substrings, not exact line renderings; Clap may wrap or join doc-comment lines.

Expected version output must include:

```text
conary
```

- [ ] **Step 3: Capture selected runtime behavior that is safe on a temp DB**

Run:

```bash
set -euo pipefail
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
cargo run -p conary -- system init --db-path "$scratch/conary.db" > "$scratch/system-init.txt"
cargo run -p conary -- system adopt --status --db-path "$scratch/conary.db" > "$scratch/adopt-status.txt"
set +e
cargo run -p conary -- system adopt curl --dry-run --db-path "$scratch/conary.db" > "$scratch/adopt-package-dry-run.stdout" 2> "$scratch/adopt-package-dry-run.stderr"
dry_run_status=$?
set -e
printf '%s\n' "$dry_run_status" > "$scratch/adopt-package-dry-run.exit"
cat "$scratch/adopt-package-dry-run.exit"
sed -n '1,80p' "$scratch/adopt-package-dry-run.stderr"
```

Expected:

```text
1
```

The stderr must include:

```text
single-package adoption dry-run is not implemented yet
conary system adopt --system --dry-run
```

This is an honest deferred surface, not a Wave 1b repair, as long as `system adopt --help` also states that single-package dry-run is rejected.

- [ ] **Step 4: Run focused proof commands**

Run:

```bash
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests::
cargo test -p conary --lib commands::adopt
cargo test -p conary --test live_host_mutation_safety system_adopt
cargo test -p conary --test cli_daily_ux adopted
bash scripts/check-doc-truth.sh
```

Expected:

```text
test result: ok
Documentation truth checks passed.
```

- [ ] **Step 5: Regenerate and inspect ignored local manpage output**

Run:

```bash
set -euo pipefail
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
cargo build -p conary
test -f apps/conary/man/conary.1
cp apps/conary/man/conary.1 "$scratch/conary.1"
sed 's/\\-/-/g' "$scratch/conary.1" > "$scratch/conary.1.normalized"
for pattern in \
  "conary system adopt --refresh" \
  "Daily workflow examples"
do
  if ! rg -n -- "$pattern" "$scratch/conary.1.normalized"; then
    echo "ERROR: root manpage missing required Wave 1b text: $pattern" >&2
    exit 1
  fi
done
rg -n -- "Adopt system packages" "$scratch/conary.1.normalized" || true
```

Expected:

The root manpage must include the root daily example `conary system adopt --refresh`. It may not include full nested `system adopt` help text. If `Adopt system packages` is absent from the root manpage, record that as non-public/out-of-scope for Wave 1b rather than a defect, because the generated artifact is the root manpage.
The root manpage only reflects root-level help, so the nested subcommand text `Adopt system packages` is expected to be absent unless the generator starts expanding nested commands.

- [ ] **Step 6: Sweep the selected Wave 1b scope**

Run:

```bash
set -euo pipefail
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
rg -n --glob '!target/**' --glob '!docs/superpowers/plans/archive/**' --glob '!docs/superpowers/specs/archive/**' 'TODO|not implemented|stub|future|unsupported|broken' \
  apps/conary/src/cli/system.rs \
  apps/conary/src/dispatch/system.rs \
  apps/conary/src/commands/adopt/mod.rs \
  apps/conary/src/commands/adopt/packages.rs \
  apps/conary/src/commands/adopt/system.rs \
  apps/conary/src/commands/adopt/status.rs \
  apps/conary/src/commands/adopt/refresh.rs \
  apps/conary/src/commands/adopt/convert.rs \
  apps/conary/src/commands/adopt/hooks.rs \
  apps/conary/src/commands/adopt/cas_capture.rs \
  docs/operations/daily-driver-ux-matrix.md \
  "$scratch/system-help.txt" \
  "$scratch/system-adopt-help.txt" \
  "$scratch/root-help.txt" \
  "$scratch/conary.1.normalized" \
  > "$scratch/wave1b-system-adopt-sweep.txt" || true
sed -n '1,240p' "$scratch/wave1b-system-adopt-sweep.txt"
```

Expected:

Known public in-scope hits may include:

- the single-package dry-run runtime refusal in `apps/conary/src/dispatch/system.rs`, which must be recorded as `honest-deferred` with `disposition=deferred-owned` or repaired if help/runtime behavior disagree;
- the unsupported package-manager sync-hook refusal in `apps/conary/src/commands/adopt/hooks.rs`, which is an honest runtime refusal and needs no repair unless help claims broader support;
- the unsupported special-file refusal in `apps/conary/src/commands/adopt/cas_capture.rs`, which is an honest full-adoption helper refusal and needs no repair unless help claims broader support.

Known non-public or contextual hits may include source comments, helper tests, and daily-driver matrix metadata. Classify them in `$scratch/classification.txt`; do not turn them into Wave 1b repairs unless they directly make the selected `conary system adopt` surface misleading.

Do not broaden the sweep to unrelated docs, conaryd routes, Remi, MCP, or codebase-wide comments.
Do not sweep `apps/conary/src/commands/adopt/checkpoint.rs`, `conflicts.rs`, `outcome.rs`, `unadopt.rs`, or `native_handoff.rs` in Wave 1b unless evidence shows a direct effect on the selected `conary system adopt` surface.

- [ ] **Step 7: Capture source excerpts**

Run:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
sed -n '80,170p' apps/conary/src/cli/system.rs > "$scratch/cli-system-adopt.rs.txt"
sed -n '84,92p' apps/conary/src/commands/adopt/refresh.rs > "$scratch/adopt-refresh-signature.rs.txt"
rg -n "Used by: default \\(package adopt\\), --system(, --refresh)?$" apps/conary/src/cli/system.rs > "$scratch/adopt-full-used-by.txt" || true
rg -n "_full: bool" apps/conary/src/commands/adopt/refresh.rs > "$scratch/adopt-refresh-full-consumer.txt" || true
sed -n '70,116p' apps/conary/src/dispatch/system.rs > "$scratch/dispatch-system-adopt.rs.txt"
sed -n '1,80p' apps/conary/src/commands/adopt/mod.rs > "$scratch/commands-adopt-mod.rs.txt"
sed -n '14,28p' docs/operations/daily-driver-ux-matrix.md > "$scratch/daily-driver-adopt-claim.md.txt"
```

Expected:

The commands exit 0. If `adopt-full-used-by.txt` shows `--refresh` while `adopt-refresh-full-consumer.txt` shows `_full: bool`, classify that as a bounded help/runtime disagreement: the parser passes `--full` to refresh, but the refresh implementation intentionally ignores it.

- [ ] **Step 8: Write a scratch classification note**

Create `$scratch/classification.txt` with exactly these filled sections:

Before writing it, reload the persisted scratch path:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
```

```text
Wave scope: 1b-system-adopt

System adopt help:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Parser and mode constraints:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Dispatch and command modules:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Single-package dry-run:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Active docs tied to selected CLI claim:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Sweep findings:
- Public in scope:
- Non-public or out of scope:
- Requires repair before scope completion:
```

No field may be left blank. If evidence shows a help/runtime mismatch, mark the affected row as `fix-now` or `misleading` in the note and repair it in Task 3 before closing the scope.

- [ ] **Step 9: Confirm generated output is ignored**

Run:

```bash
git status --short --ignored apps/conary/man man
```

Expected:

```text
!! apps/conary/man/
```

`!! man/` may also appear if another local step created the ignored root manpage directory. If either generated directory appears as staged or tracked changes, stop and remove the accidental staged generated output before continuing.

---

### Task 3: Repair Any Bounded Wave 1b Gaps

**Files:**
- Modify only if evidence requires repair: `apps/conary/src/cli/system.rs`
- Modify only if evidence requires repair: `apps/conary/src/dispatch/system.rs`
- Modify only if evidence requires repair: selected files under `apps/conary/src/commands/adopt/`
- Modify only if evidence requires repair: `docs/operations/daily-driver-ux-matrix.md`
- Test: focused commands from Task 2

At current HEAD, the single-package dry-run wording and dispatch structures below are already aligned. Treat Step 2 and Step 3 as check-only confirmations unless Task 2 evidence shows drift that must be repaired. If Task 2 exposes the bounded `--full`/`--refresh` annotation mismatch, repair that help wording before closing the `works` rows.

- [ ] **Step 1: Check whether repair is required**

Run:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
test -f "$scratch/classification.txt"
if rg -n '^- (Claim|Actual|Status|Decision|Verification|Public in scope|Non-public or out of scope|Requires repair before scope completion):[[:space:]]*$' "$scratch/classification.txt"; then
  echo "ERROR: classification note has empty required fields" >&2
  exit 1
fi
rg -n 'Status: (fix-now|misleading|duplicate-stale)' "$scratch/classification.txt" || true
rg -n 'Requires repair before scope completion:' "$scratch/classification.txt" | rg -v 'Requires repair before scope completion: None\.' || true
```

Expected if no repair is needed:

```text
```

If the status scan prints a `fix-now`, `misleading`, or `duplicate-stale` row, or the repair-required pipeline prints a row, do the smallest repair before Task 4.

- [ ] **Step 2: Repair help/runtime disagreement if present**

If help says `--full` is used by `--refresh` while `cmd_adopt_refresh` still accepts `_full: bool`, make the help honest before closing `CLI-ADOPT-001` or `CLI-ADOPT-002` as `works`. The preferred Wave 1b repair is to remove `--refresh` from the `full` option's `Used by:` doc line:

```rust
        /// Copy files to CAS for full management (enables rollback)
        /// Used by: default (package adopt), --system
        #[arg(long, conflicts_with_all = ["status", "convert", "sync_hook", "from_sync_hook"])]
        full: bool,
```

Do not make `--refresh --full` a behavioral feature in this wave unless you also add focused tests that prove the refresh path consumes it.

If help says single-package dry-run works but runtime refuses it, either make the help honest or implement the preview. The preferred Wave 1b repair is honest wording only:

Before editing, verify whether the honest wording is already present:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
rg -n "rejected until it has a true non-mutating preview path" apps/conary/src/cli/system.rs
cargo run -p conary -- system adopt --help > "$scratch/system-adopt-help-before-repair.txt"
rg -n "Single-package dry-run is rejected" "$scratch/system-adopt-help-before-repair.txt"
```

If both commands print a row, this repair is already satisfied; do not edit `apps/conary/src/cli/system.rs` solely to match the snippet below.

```rust
        /// Show what would be adopted without making changes
        /// Used by: --system, --convert, --refresh. Single-package dry-run is
        /// rejected until it has a true non-mutating preview path.
        #[arg(long, conflicts_with_all = ["status", "sync_hook"])]
        dry_run: bool,
```

Run:

```bash
scratch_file="$(git rev-parse --git-common-dir)/info/wave1b-system-adopt-scratch"
scratch="$(cat "$scratch_file")"
: "${scratch:?run Task 2 Step 1 first}"
test -d "$scratch"
cargo fmt --check
cargo run -p conary -- system adopt --help > "$scratch/system-adopt-help-after-repair.txt"
rg -n "Single-package dry-run is rejected" "$scratch/system-adopt-help-after-repair.txt"
```

Expected:

```text
Single-package dry-run is rejected
```

- [ ] **Step 3: Repair dispatch mismatch if present**

If a mode parses but dispatch routes it to the wrong implementation, edit only the relevant branch in `apps/conary/src/dispatch/system.rs`. The expected dispatch order is:

The `from_sync_hook: _` field in the surrounding dispatch destructure is intentional. The hidden parser flag is retained in the Clap model for pre-dispatch `command_risk` classification, then ignored by dispatch after policy enforcement. Do not remove it from the Clap definition or dispatch destructure as a cleanup while auditing this wave.

```rust
            if sync_hook {
                commands::cmd_sync_hook_install(remove_hook).await
            } else if convert {
                commands::cmd_adopt_convert(&db.db_path, jobs, no_chunking, dry_run).await
            } else if status {
                commands::cmd_adopt_status(&db.db_path).await
            } else if refresh {
                commands::cmd_adopt_refresh(&db.db_path, full, dry_run, quiet).await
            } else if system {
                commands::cmd_adopt_system(
                    &db.db_path,
                    full,
                    dry_run,
                    pattern.as_deref(),
                    exclude.as_deref(),
                    explicit_only,
                )
                .await
            } else {
                if dry_run {
                    anyhow::bail!(
                        "single-package adoption dry-run is not implemented yet; use `conary system adopt --system --dry-run` for a system-wide preview or rerun without --dry-run when ready to adopt package(s)"
                    );
                }
                commands::cmd_adopt(&packages, &db.db_path, full).await
            }
```

Run:

```bash
cargo check -p conary
cargo test -p conary --lib command_risk::tests::
cargo test -p conary --test live_host_mutation_safety system_adopt
```

Expected:

```text
test result: ok
```

- [ ] **Step 4: Commit repair if any code or doc was changed**

If Task 3 changed files, run:

```bash
git diff --check
git status --short
git diff --name-only
# Add only the exact files intentionally edited by Task 3. Do not stage whole
# directories such as apps/conary/src/commands/adopt/.
git add apps/conary/src/cli/system.rs apps/conary/src/dispatch/system.rs
# If a tracked Markdown doc changed, update docs-audit ledger/summary metadata
# in the same commit and include those exact files here.
git diff --cached --name-only
git commit -m "fix: align system adopt coherency"
```

Expected:

```text
[main <sha>] fix: align system adopt coherency
```

If Task 3 changed no files, do not create an empty commit.
If Task 3 changed a file under `apps/conary/src/commands/adopt/`, stage that exact file path after reviewing `git diff --name-only`. If Task 3 changed `docs/operations/daily-driver-ux-matrix.md`, also refresh `docs/superpowers/documentation-accuracy-audit-ledger.tsv` and `docs/superpowers/documentation-accuracy-audit-summary.md` as needed, rerun docs-audit checks, and stage only the exact touched docs-audit files.

---

### Task 4: Record And Close Wave 1b Ledger Rows

**Files:**
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`

- [ ] **Step 1: Append closed Wave 1b rows**

Run:

```bash
verified_date="$(date +%F)"
if rg -n '^(CLI-ADOPT-00[1-4]|DOC-ADOPT-001)\t|^[^\t]*\t[^\t]*\t[^\t]*\t[^\t]*\t1b-system-adopt\t' docs/superpowers/feature-coherency-ledger.tsv; then
  echo "ERROR: Wave 1b rows already exist; edit the existing rows instead of appending duplicates" >&2
  exit 1
fi
cat >> docs/superpowers/feature-coherency-ledger.tsv <<EOF
CLI-ADOPT-001	conary system adopt help	cmd:cargo run -p conary -- system adopt --help		1b-system-adopt	Adoption, Unadoption, And Native-Authority Handoff	Parent system help and system adopt help advertise the selected adopt command family without overstating unsupported dry-run behavior	System and system-adopt help render successfully and system-adopt help states the selected modes and the single-package dry-run rejection	works	verified-no-change	${verified_date}	path:apps/conary/src/cli/system.rs;cmd:cargo run -p conary -- system --help;cmd:cargo run -p conary -- system adopt --help	none	cmd:cargo run -p conary -- system --help;cmd:cargo run -p conary -- system adopt --help	verify	Re-run system and system-adopt help capture before changing adopt flags or mode constraints	Wave 1b help evidence captured in scratch output
CLI-ADOPT-002	system adopt parser and mode constraints	test:cargo test -p conary --lib cli::tests		1b-system-adopt	Adoption, Unadoption, And Native-Authority Handoff	System adopt Clap and pre-dispatch command-risk constraints keep mutually exclusive package, system, status, refresh, convert, sync-hook, and hidden hook-refresh modes coherent	Parser characterization covers system dry-run filters, refresh quiet hook path, from-sync-hook/full conflict, convert dry-run jobs, sync-hook removal, and selected rejection cases; command-risk tests cover hook-refresh classification	works	verified-no-change	${verified_date}	path:apps/conary/src/cli/system.rs;path:apps/conary/src/cli/mod.rs;path:apps/conary/src/command_risk.rs;test:cargo test -p conary --lib cli::tests;test:cargo test -p conary --lib command_risk::tests::	none	test:cargo test -p conary --lib cli::tests;test:cargo test -p conary --lib command_risk::tests::	verify	Re-run parser and command-risk tests before changing system adopt Clap constraints	Wave 1b added durable parser and pre-dispatch policy proof for selected system adopt modes
CLI-ADOPT-003	system adopt dispatch and command modules	test:cargo test -p conary --lib commands::adopt		1b-system-adopt	Adoption, Unadoption, And Native-Authority Handoff	System adopt dispatch source routes parsed modes to the matching adopt command module, with focused module tests and selected binary safety tests covering behavior	Dispatch source maps sync hooks, convert, status, refresh, system adoption, package adoption, and package dry-run refusal to expected command paths; focused adopt module, command-risk, and safety tests pass	works	verified-no-change	${verified_date}	path:apps/conary/src/dispatch/system.rs;path:apps/conary/src/commands/adopt/mod.rs;test:cargo test -p conary --lib commands::adopt;test:cargo test -p conary --lib command_risk::tests::;test:cargo test -p conary --test live_host_mutation_safety system_adopt	none	test:cargo test -p conary --lib commands::adopt;test:cargo test -p conary --lib command_risk::tests::;test:cargo test -p conary --test live_host_mutation_safety system_adopt	verify	Re-run focused adopt module, command-risk, and live-mutation safety tests before changing adopt dispatch	System adopt command-module proof captured and closed in Wave 1b
CLI-ADOPT-004	single-package system adopt dry-run	cmd:cargo run -p conary -- system adopt curl --dry-run		1b-system-adopt	Adoption, Unadoption, And Native-Authority Handoff	Single-package adoption dry-run is visible as an option but must not pretend to be implemented until it has a true non-mutating preview path	Runtime rejects single-package dry-run with a specific error and points to system-wide dry-run or non-dry-run package adoption; help states the same limitation	honest-deferred	deferred-owned	${verified_date}	path:apps/conary/src/cli/system.rs;path:apps/conary/src/dispatch/system.rs;doc:docs/operations/system-adopt-single-package-dry-run-follow-up.md;path:docs/superpowers/specs/archive/2026-05-26-limited-preview-release-hardening-design.md;cmd:cargo run -p conary -- system adopt --help;cmd:cargo run -p conary -- system adopt curl --dry-run;test:cargo test -p conary --test live_host_mutation_safety system_adopt_package_dry_run_is_rejected_without_ack_prompt	cmd:cargo run -p conary -- system adopt curl --dry-run	cmd:cargo run -p conary -- system adopt --help;cmd:cargo run -p conary -- system adopt curl --dry-run;test:cargo test -p conary --test live_host_mutation_safety system_adopt_package_dry_run_is_rejected_without_ack_prompt	defer	Tracked by docs/operations/system-adopt-single-package-dry-run-follow-up.md; create a reviewed implementation plan before implementing true single-package adopt dry-run preview or removing package-mode dry-run visibility	Honest deferred row: active help and runtime refusal both describe the current limitation and supported alternatives; historical release-hardening design required either true preview or a deliberate unsupported route
DOC-ADOPT-001	daily driver adopt refresh claim	doc:docs/operations/daily-driver-ux-matrix.md	CLI-ADOPT-001;CLI-ADOPT-003	1b-system-adopt	Adoption, Unadoption, And Native-Authority Handoff	Daily-driver UX docs route adopted install/update workflows to conary system adopt --refresh	The selected docs claim remains consistent with root help, system adopt help, parser, dispatch, and daily UX tests	works	verified-no-change	${verified_date}	doc:docs/operations/daily-driver-ux-matrix.md;cmd:cargo run -p conary -- system adopt --help;test:cargo test -p conary --test cli_daily_ux adopted	none	test:cargo test -p conary --test cli_daily_ux adopted	verify	Re-run daily UX adopted-package tests before changing adopt-refresh guidance	Scoped active-doc claim checked because it directly repeats the selected CLI guidance
EOF
```

If Task 2 or Task 3 found a different reality, change only the affected rows before validation:

- use `status=misleading` only while the active help/docs overstate current behavior;
- use `status=fix-now` only for a bounded repair required before scope completion;
- use `status=honest-deferred` and `disposition=deferred-owned` only after the active surface gives a specific honest refusal or preview limitation;
- do not close `CLI-ADOPT-001` or `CLI-ADOPT-002` as `works` while the `--full` help line still says `--refresh` uses it and the refresh implementation still ignores `_full`;
- do not leave any `1b-system-adopt` row with `disposition=open` before Task 5.

- [ ] **Step 2: Validate ledger and scope completion**

Run:

```bash
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1b-system-adopt
```

Expected:

```text
Coherency ledger check passed.
Coherency ledger check passed.
```

- [ ] **Step 3: Commit ledger closure**

Run:

```bash
git add docs/superpowers/feature-coherency-ledger.tsv
git commit -m "docs: close feature coherency wave 1b system adopt rows"
```

Expected:

```text
[main <sha>] docs: close feature coherency wave 1b system adopt rows
```

---

### Task 5: Final Verification And Push

**Files:**
- Read: repository state

- [ ] **Step 1: Run final verification**

Run:

```bash
set -euo pipefail
bash scripts/test-coherency-ledger.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1b-system-adopt
cargo fmt --check
cargo check -p conary
manpage_scratch="$(mktemp -d /tmp/conary-wave1b-final-manpage.XXXXXX)"
cargo build -p conary
test -f apps/conary/man/conary.1
cp apps/conary/man/conary.1 "$manpage_scratch/conary.1"
sed 's/\\-/-/g' "$manpage_scratch/conary.1" > "$manpage_scratch/conary.1.normalized"
for pattern in \
  "conary system adopt --refresh" \
  "Daily workflow examples"
do
  if ! rg -n -- "$pattern" "$manpage_scratch/conary.1.normalized"; then
    echo "ERROR: final root manpage missing required Wave 1b text: $pattern" >&2
    exit 1
  fi
done
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests::
cargo test -p conary --lib commands::adopt
cargo test -p conary --test live_host_mutation_safety system_adopt
cargo test -p conary --test cli_daily_ux adopted
dry_run_scratch="$(mktemp -d /tmp/conary-wave1b-final-dry-run.XXXXXX)"
cargo run -p conary -- system init --db-path "$dry_run_scratch/conary.db" > "$dry_run_scratch/system-init.txt"
set +e
cargo run -p conary -- system adopt curl --dry-run --db-path "$dry_run_scratch/conary.db" > "$dry_run_scratch/adopt-package-dry-run.stdout" 2> "$dry_run_scratch/adopt-package-dry-run.stderr"
dry_run_status=$?
set -e
test "$dry_run_status" -eq 1
rg -n "single-package adoption dry-run is not implemented yet" "$dry_run_scratch/adopt-package-dry-run.stderr"
rg -n "conary system adopt --system --dry-run" "$dry_run_scratch/adopt-package-dry-run.stderr"
rg -n "rerun without --dry-run" "$dry_run_scratch/adopt-package-dry-run.stderr"
completion="$(mktemp /tmp/conary-wave1b-completion.XXXXXX.bash)"
cargo run -p conary -- system completions bash > "$completion"
test -s "$completion"
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
git diff --check
git status --short --ignored apps/conary/man man
if git diff --cached --name-only | rg '(^apps/conary/man/|^man/)'; then
  echo 'ERROR: generated manpage output is staged' >&2
  exit 1
fi
```

Expected:

```text
Coherency ledger validator tests passed.
Coherency ledger check passed.
test result: ok
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The docs-audit inventory diff and `git diff --check` should produce no output. Generated manpage directories may appear only as ignored output.
Because this plan pushes directly to `main`, the Task 5 commands are the enforcement gate for this slice. The PR gate only revalidates ledger syntax on future PRs; it does not replace the local `--scope-complete 1b-system-adopt` gate.

- [ ] **Step 2: Push and prove sync**

Run:

```bash
git status --short --branch
git push
git rev-list --left-right --count HEAD...origin/main
git rev-parse HEAD origin/main
```

Expected:

```text
0	0
```

The two `git rev-parse` lines must be identical.

---

## Self-Review

- **Spec coverage:** This plan implements the design's Wave 1b rule by selecting one high-visibility command family from the feature ownership map and keeping route/MCP/broad-doc sweeps deferred.
- **No open selected-scope rows:** Task 4 requires `--scope-complete 1b-system-adopt` and forbids open `1b-system-adopt` rows before final verification.
- **Honest deferral:** The single-package dry-run limitation is recorded as `honest-deferred/deferred-owned` only if help and runtime refusal both remain explicit.
- **Docs-audit boundary:** The Markdown plan is tracked by docs-audit; the TSV coherency ledger remains under its own validator.
- **Large-file discipline:** The plan avoids growing `apps/conary/src/commands/adopt/system.rs` unless a bounded behavior repair is required.
