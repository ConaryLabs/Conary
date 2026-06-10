# Project Maintainability Phase 2 Dead Surface Pruning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the first small set of tracked stale public surfaces and create a repo-grounded pruning inventory so later refactors do not carry misleading names, examples, or compatibility assumptions forward.

**Architecture:** First remove the docs-audit inventory script's unnecessary `rg` dependency so plan lock-in works in more developer environments. Then start with tests around user-visible CLI help and generated output, make the smallest string changes that align the public surface with current supported targets and command names, and record broader stale-surface candidates in a docs-audited inventory while deferring risky behavior deletions until Phase 3 test/fixture discipline provides stronger coverage.

**Tech Stack:** Rust integration tests, Clap help output, Markdown, existing docs-audit tooling, Git, Cargo.

---

## Status

Draft child implementation plan for review.

This is the Phase 2 child plan for
`docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.
It does not decompose hotspot files. It performs the first small pruning slice
and creates the inventory that later pruning and refactor plans can use.

## Read First

- `AGENTS.md`
- `CONTRIBUTING.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `data/distros.toml`
- `apps/conary/src/cli/repo.rs`
- `apps/conary/src/commands/ccs/init.rs`
- `apps/remi/src/bin/remi.rs`
- `scripts/docs-audit-inventory.sh`
- `scripts/check-doc-audit-ledger.sh`

## Design Summary

Phase 2 should prune obvious stale surfaces before larger refactors start. The
first implementation slice intentionally avoids deleting persisted formats,
legacy parser internals, package-format support, or scriptlet replay code.

The first cleanup targets are tracked, user-visible, and trivially verifiable:

- `apps/conary/src/commands/ccs/init.rs` prints `conary ccs-build`, but the
  command is `conary ccs build`.
- `apps/conary/src/cli/repo.rs` tells users Remi distro examples include
  `debian`, even though public support is Fedora 44, Ubuntu 26.04, and Arch.
- `apps/remi/src/bin/remi.rs` help strings list `debian` as an index,
  prewarm, and benchmark target even though Remi rejects it as unsupported.
- `scripts/docs-audit-inventory.sh` uses `rg` for a simple tracked-doc filter.
  That dependency is avoidable with `grep -E` and otherwise blocks docs-audit
  verification on developer hosts without ripgrep.
- Active public docs and help must not reintroduce unsupported distro claims
  for Debian, Linux Mint, or other non-supported targets. Negative tests and
  internal parser/version-scheme references should be classified instead of
  deleted blindly.

The first inventory should distinguish:

- **Fix now:** tracked public help or output strings covered by focused tests.
- **Inventory only:** internal parser/version-scheme behavior, packaging format
  support, archived historical docs, ignored host-local files, and broad code
  comments that need later judgment.
- **Defer:** behavior-changing deletions without focused tests.

## Current Evidence

The following commands were used while drafting this plan:

```bash
scripts/line-count-report.sh 30
grep -R -n -E "ccs-build|conary ccs build|Distribution to generate index|Distribution to pre-warm|Distribution to benchmark|Examples: fedora, arch, debian, ubuntu|debian, or arch|arch, fedora, ubuntu, debian" apps docs README.md CONTRIBUTING.md .github
grep -R -n -E "Linux Mint|linux-mint|linux mint" README.md ROADMAP.md CONTRIBUTING.md AGENTS.md docs apps crates data recipes deploy packaging .github
git ls-files .claude docs/operations/LOCAL_ACCESS.md docs/operations/LOCAL_ACCESS.example.md
git check-ignore -v .claude/settings.local.json
command -v rg
```

Findings:

- `.claude/settings.local.json` exists locally but is ignored by `.gitignore`
  and untracked. This slice should not delete local personal state.
- `data/distros.toml` currently lists only `fedora-44`, `ubuntu-26.04`, and
  `arch`.
- `scripts/docs-audit-inventory.sh` currently pipes `git ls-files` through
  `rg`; this can be replaced with `grep -E` without changing the filter.
- The Remi and Conary CLI strings above are tracked public surfaces.
- Many `debian` references are internal parser, version-scheme, package-format,
  or archived-history references. This slice should not remove those.
- Linux Mint references currently appear in archived docs and negative
  unsupported-distro tests. This slice should keep the negative tests but gate
  active public docs/help against reintroducing Linux Mint as a support claim.

## File Structure

- Modify `apps/conary/tests/cli_daily_ux.rs`
  - Add focused regression tests for the Conary public help/output strings.
- Modify `apps/conary/src/cli/repo.rs`
  - Replace stale path comment with the workspace-relative path.
  - Replace stale Remi distro examples with `fedora-44, ubuntu-26.04, arch`.
- Modify `apps/conary/src/commands/ccs/init.rs`
  - Replace stale path comment with the workspace-relative path.
  - Replace `conary ccs-build` with `conary ccs build`.
- Create `apps/remi/tests/cli_help.rs`
  - Add focused Remi binary help-output regression tests.
- Modify `apps/remi/src/bin/remi.rs`
  - Replace stale Remi distro help text with supported public targets.
- Modify `scripts/docs-audit-inventory.sh`
  - Replace its `rg` filter with `grep -E`.
- Create `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`
  - Record fix-now, inventory-only, and deferred pruning candidates.
- Modify `docs/superpowers/documentation-accuracy-audit-summary.md`
  - Refresh the docs-audit narrative and counts for the active maintainability
    Phase 2 plan and inventory.
- Modify docs-audit files when this branch adds the plan and inventory doc or
  materially changes tracked documentation claims.

## Non-Goals

- Do not remove Debian/DEB parser support, version comparison, package-format
  support, or tests that prove Ubuntu/DEB-family behavior.
- Do not remove negative unsupported-distro tests solely because they mention
  Debian, Linux Mint, or another rejected distro name.
- Do not remove archived historical docs solely because they mention retired
  tools or unsupported distros.
- Do not delete ignored host-local files.
- Do not decompose `apps/conary/src/commands/ccs/install.rs`,
  `apps/remi/src/server/conversion.rs`, or other hotspots.
- Do not change `data/distros.toml`.
- Do not alter Remi conversion behavior, native publication behavior, or CCS
  package contract semantics.

## Task 0: Lock The Reviewed Plan And Docs-Audit Row

**Files:**
- Modify: `scripts/docs-audit-inventory.sh`
- Add: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Make docs-audit inventory independent of `rg`**

In `scripts/docs-audit-inventory.sh`, replace:

```bash
        | rg '(^|/)(README\.md|AGENTS\.md|CONTRIBUTING\.md|ROADMAP\.md|CHANGELOG\.md|SECURITY\.md|.*\.md|.*\.mdx|.*\.rst|.*\.adoc|.*\.toml\.example)$' \
```

with:

```bash
        | grep -E '(^|/)(README\.md|AGENTS\.md|CONTRIBUTING\.md|ROADMAP\.md|CHANGELOG\.md|SECURITY\.md|.*\.md|.*\.mdx|.*\.rst|.*\.adoc|.*\.toml\.example)$' \
```

- [ ] **Step 2: Verify docs-audit inventory syntax**

```bash
bash -n scripts/docs-audit-inventory.sh
```

Expected: syntax check exits 0.

- [ ] **Step 3: Stage the plan so docs-audit inventory can see it**

```bash
git add scripts/docs-audit-inventory.sh docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md
```

- [ ] **Step 4: Refresh docs-audit inventory**

The existing docs-audit tooling requires Bash 4+.

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 5: Add the plan ledger row**

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with
literal tab separators:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md	planning	maintainer	maintainability; phase2; dead-surface-pruning; stale-surface-inventory	AGENTS.md; CONTRIBUTING.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md; data/distros.toml; apps/conary/src/cli/repo.rs; apps/conary/src/commands/ccs/init.rs; apps/remi/src/bin/remi.rs; scripts/docs-audit-inventory.sh; scripts/check-doc-audit-ledger.sh	verified	corrected	Added the reviewed Phase 2 implementation plan for the first dead-surface pruning slice: portable docs-audit inventory filtering, tested public CLI help cleanup, current supported-target wording, CCS init command-name correction, and a pruning inventory artifact.
```

- [ ] **Step 6: Update the docs-audit summary for the active plan**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, insert this
section before `## Verification Commands`:

```markdown
### 2026-06-06 Maintainability Phase 2 Planning

The maintainability reset reopened the active planning lane after the prior
clean-slate archive pass. Phase 1 added the repo discipline contract and
line-count report. Phase 2 now opens a focused dead-surface pruning plan for
portable docs-audit inventory filtering, tested public CLI help cleanup, and a
pruning inventory. Active plan roots are therefore not expected to be empty;
current active roadmap and child-plan files are tracked through the ledger.
```

Then update the `## Final Counts` block to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 142
- `verified-no-change`: 14
- `corrected`: 41
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

- [ ] **Step 7: Update the docs-audit summary ledger row**

Replace the existing `docs/superpowers/documentation-accuracy-audit-summary.md`
row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with this
literal-tab row:

```text
docs/superpowers/documentation-accuracy-audit-summary.md	docs/superpowers/documentation-accuracy-audit-summary.md	planning	maintainer	audit-summary; verification; release-hardening; active-planning; maintainability	docs/superpowers/documentation-accuracy-audit-ledger.tsv; docs/superpowers/documentation-accuracy-audit-inventory.tsv; scripts/check-doc-audit-ledger.sh; ROADMAP.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md	verified	corrected	Refreshed the audit summary for the active maintainability planning lane, current docs-audit counts, Phase 1 discipline contract, and Phase 2 dead-surface pruning plan.
```

- [ ] **Step 8: Verify docs-audit and diff hygiene**

```bash
bash -n scripts/docs-audit-inventory.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
git diff --check
```

Expected: syntax check, docs-audit, and both diff checks exit 0.

- [ ] **Step 9: Commit the reviewed plan**

```bash
git add scripts/docs-audit-inventory.sh docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md
git commit -m "docs: plan maintainability pruning slice"
```

## Task 1: Add Conary CLI Stale-Surface Regression Tests

**Files:**
- Modify: `apps/conary/tests/cli_daily_ux.rs`

- [ ] **Step 1: Add tests for public Conary CLI output**

Add these tests after `root_help_includes_daily_workflow_examples()` so the
help/output regression tests stay grouped together:

```rust
#[test]
fn phase2_pruning_repo_add_help_lists_only_supported_remi_distro_examples() {
    let output = run_conary(&["repo", "add", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fedora-44, ubuntu-26.04, arch"), "{stdout}");
    assert!(!stdout.to_lowercase().contains("debian"), "{stdout}");
}

#[test]
fn phase2_pruning_ccs_init_next_steps_use_current_build_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_conary(&[
        "ccs",
        "init",
        dir.path().to_str().unwrap(),
        "--name",
        "phase2-pruning",
        "--version",
        "1.0.0",
    ]);

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("conary ccs build"), "{stdout}");
    assert!(!stdout.contains("conary ccs-build"), "{stdout}");
    assert!(dir.path().join("ccs.toml").exists());
}
```

- [ ] **Step 2: Run the tests and verify they fail before implementation**

```bash
cargo test -p conary --test cli_daily_ux phase2_pruning
```

Expected before implementation:

- `phase2_pruning_repo_add_help_lists_only_supported_remi_distro_examples`
  fails because help text contains `debian` and does not contain the full
  supported target IDs.
- `phase2_pruning_ccs_init_next_steps_use_current_build_subcommand` fails
  because output contains `conary ccs-build`.

## Task 2: Fix Conary CLI Public Stale Strings

**Files:**
- Modify: `apps/conary/src/cli/repo.rs`
- Modify: `apps/conary/src/commands/ccs/init.rs`
- Test: `apps/conary/tests/cli_daily_ux.rs`

- [ ] **Step 1: Update the repo CLI path comment**

In `apps/conary/src/cli/repo.rs`, replace the opening path comment:

```rust
// src/cli/repo.rs
```

with:

```rust
// apps/conary/src/cli/repo.rs
```

- [ ] **Step 2: Update Remi distro examples in repo add help**

In `apps/conary/src/cli/repo.rs`, replace:

```rust
        /// Examples: fedora, arch, debian, ubuntu
```

with:

```rust
        /// Examples: fedora-44, ubuntu-26.04, arch
```

Note: `--remi-distro` still accepts any string at Clap parse time. This step
only corrects the public examples; runtime validation of target IDs is deferred
to the inventory.

- [ ] **Step 3: Update the CCS init path comment**

In `apps/conary/src/commands/ccs/init.rs`, replace the opening path comment:

```rust
// src/commands/ccs/init.rs
```

with:

```rust
// apps/conary/src/commands/ccs/init.rs
```

- [ ] **Step 4: Update the CCS init next-step command**

In `apps/conary/src/commands/ccs/init.rs`, replace:

```rust
    println!("  2. Run 'conary ccs-build' to create the package");
```

with:

```rust
    println!("  2. Run 'conary ccs build' to create the package");
```

- [ ] **Step 5: Verify Conary CLI tests now pass**

```bash
cargo test -p conary --test cli_daily_ux phase2_pruning
```

Expected: both `phase2_pruning_*` tests pass.

- [ ] **Step 6: Run focused package check**

```bash
cargo check -p conary
```

Expected: package check exits 0.

- [ ] **Step 7: Commit Conary CLI cleanup**

```bash
git add apps/conary/tests/cli_daily_ux.rs apps/conary/src/cli/repo.rs apps/conary/src/commands/ccs/init.rs
git commit -m "fix(cli): prune stale public help strings"
```

## Task 3: Add Remi CLI Help Regression Tests

**Files:**
- Create: `apps/remi/tests/cli_help.rs`

- [ ] **Step 1: Create the Remi integration test directory**

```bash
mkdir -p apps/remi/tests
```

- [ ] **Step 2: Create the Remi help test file**

Create `apps/remi/tests/cli_help.rs`:

```rust
// apps/remi/tests/cli_help.rs

use std::process::{Command, Output};

fn run_remi(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_remi"))
        .args(args)
        .output()
        .expect("failed to run remi")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_supported_public_targets_only(text: &str) {
    assert!(text.contains("fedora-44, ubuntu-26.04, arch"), "{text}");
    assert!(!text.to_lowercase().contains("debian"), "{text}");
}

#[test]
fn phase2_pruning_index_gen_help_lists_only_supported_public_targets() {
    let output = run_remi(&["index-gen", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
}

#[test]
fn phase2_pruning_prewarm_help_lists_only_supported_public_targets() {
    let output = run_remi(&["prewarm", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
}

#[test]
fn phase2_pruning_conversion_benchmark_help_lists_only_supported_public_targets() {
    let output = run_remi(&["conversion-benchmark", "--help"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_supported_public_targets_only(&String::from_utf8_lossy(&output.stdout));
}
```

- [ ] **Step 3: Run the tests and verify they fail before implementation**

```bash
cargo test -p remi --test cli_help phase2_pruning
```

Expected before implementation: all three tests fail because Remi help text
contains `debian` and does not contain the full supported target IDs.

## Task 4: Fix Remi CLI Public Stale Strings

**Files:**
- Modify: `apps/remi/src/bin/remi.rs`
- Test: `apps/remi/tests/cli_help.rs`

- [ ] **Step 1: Update `index-gen` distro help**

In `apps/remi/src/bin/remi.rs`, replace:

```rust
    /// Distribution to generate index for (arch, fedora, ubuntu, debian)
```

with:

```rust
    /// Distribution to generate index for (fedora-44, ubuntu-26.04, arch)
```

- [ ] **Step 2: Update `prewarm` distro help**

In `apps/remi/src/bin/remi.rs`, replace:

```rust
    /// Distribution to pre-warm (arch, fedora, ubuntu, debian)
```

with:

```rust
    /// Distribution to pre-warm (fedora-44, ubuntu-26.04, arch)
```

- [ ] **Step 3: Update conversion benchmark distro help**

In `apps/remi/src/bin/remi.rs`, replace:

```rust
    /// Distribution to benchmark, such as fedora, ubuntu, debian, or arch
```

with:

```rust
    /// Distribution to benchmark (fedora-44, ubuntu-26.04, arch)
```

- [ ] **Step 4: Verify Remi CLI tests now pass**

```bash
cargo test -p remi --test cli_help phase2_pruning
```

Expected: all three `phase2_pruning_*` tests pass.

- [ ] **Step 5: Run focused package check**

```bash
cargo check -p remi --bin remi
```

Expected: package check exits 0.

- [ ] **Step 6: Commit Remi CLI cleanup**

```bash
git add apps/remi/src/bin/remi.rs apps/remi/tests/cli_help.rs
git commit -m "fix(remi): prune stale distro help"
```

## Task 5: Add Dead Surface Inventory

**Files:**
- Create: `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Create the inventory document**

Create `docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md`:

````markdown
# Project Maintainability Dead Surface Inventory 2026-06-06

## Purpose

This inventory records stale or potentially stale surfaces found during Phase 2
of the project maintainability roadmap. It is a pruning queue, not permission
to delete every listed surface. Each item records whether it was fixed now,
kept as intentional behavior, or deferred until stronger tests exist.

## Current Supported Public Targets

Conary public distro support is limited to:

- Fedora 44: `fedora-44`
- Ubuntu 26.04 LTS: `ubuntu-26.04`
- Arch Linux: `arch`

Internal parsers, package-format support, version-scheme helpers, tests, and
historical archives may mention broader ecosystem families when they are not
claiming public distro support.

## Fixed In Phase 2 First Slice

| Surface | Path | Reason | Proof |
|---------|------|--------|-------|
| CCS init next step printed `conary ccs-build` | `apps/conary/src/commands/ccs/init.rs` | The current command is `conary ccs build` | `cargo test -p conary --test cli_daily_ux phase2_pruning` |
| `conary repo add --remi-distro` examples listed `debian` | `apps/conary/src/cli/repo.rs` | Public Remi targets are Fedora 44, Ubuntu 26.04, and Arch | `cargo test -p conary --test cli_daily_ux phase2_pruning` |
| Remi `index-gen`, `prewarm`, and `conversion-benchmark` help listed `debian` | `apps/remi/src/bin/remi.rs` | Remi rejects Debian as an unsupported public target | `cargo test -p remi --test cli_help phase2_pruning` |

## Intentional Or Inventory-Only Surfaces

| Surface | Paths | Current Decision |
|---------|-------|------------------|
| DEB package-format handling | `packaging/deb/`, `crates/conary-core/src/ccs/legacy/deb.rs`, `crates/conary-core/src/packages/`, repository parser/versioning modules | Keep. Ubuntu 26.04 support needs DEB-family parsing and package-format logic. |
| Debian version-scheme tests | resolver, repository, selector, provider, and update tests that use `VersionScheme::Debian` or `"debian"` as a scheme string | Keep unless a child plan replaces the scheme vocabulary. These tests are about version ordering and parser behavior, not public distro support. |
| Linux Mint unsupported-distro tests | `apps/conary/src/commands/distro.rs`, `crates/conary-core/src/repository/distro.rs` | Keep. These tests prove parser-recognized or adjacent distro names do not become supported public targets. |
| Archived historical docs mentioning retired tools or broader distros | `docs/**/archive/`, `docs/plans/archive/`, `docs/superpowers/reviews/archive/` | Keep as historical evidence unless a separate archive cleanup plan decides to redact or summarize them. |
| Local `.claude/settings.local.json` | ignored by `.gitignore`, untracked by `git ls-files` | Do not delete tracked repo content because there is none. Treat as host-local state outside this pruning slice. |
| Future distro-expansion note | `ROADMAP.md` | Keep for now. The CCS native ecosystem roadmap explicitly says future distro expansion is out of current scope. |
| Broad code comments with maintenance notes | Rust comments containing future-work notes | Inventory only. Remove or rewrite only when the owning subsystem confirms they are stale. |

## Deferred Candidates

| Candidate | Why Deferred | Required Next Proof |
|-----------|--------------|---------------------|
| Narrow Remi command validation to reject unsupported target IDs at Clap parse time | This changes behavior, not just help text | Focused Remi command tests plus confirmation that config-driven Remi prewarm/index paths still accept current target IDs |
| Normalize Remi runtime target IDs | Remi currently has unversioned runtime defaults and handler validation for `arch`, `fedora`, and `ubuntu`, while public support is tracked as `arch`, `fedora-44`, and `ubuntu-26.04` | Focused Remi tests for `generate_indices`, handler validation, prewarm, and conversion benchmark using supported public IDs plus any required compatibility decision for existing internal family names |
| Review `apps/conary/src/cli/repo.rs` Remi distro validation | The current CLI stores a string and validation may happen later in repository or Remi flow | Focused CLI tests for `repo add --default-strategy remi --remi-distro <target>` and source-selection behavior |
| Review internal `debian` distro identifiers in Remi conversion service tests | Some tests assert Debian is rejected, while others use Debian-family conversion internals | Phase 3 fixture ownership map for Remi conversion and supported-target test data |
| Prune dead helper APIs found by usage search | Usage search alone is insufficient for public or persisted behavior | Existing focused tests or new tests proving the helper is unreachable and undesired |
| Normalize old archive references to retired assistant tooling | Historical docs intentionally preserve context | Archive policy decision and docs-audit update |

## Refresh Commands

Use these commands when extending this inventory:

```bash
grep -R -n -E "ccs-build|conary ccs-build|Distribution to generate index|Distribution to pre-warm|Distribution to benchmark|Examples: fedora, arch, debian, ubuntu" apps docs README.md CONTRIBUTING.md .github
grep -R -n -E "CentOS|RHEL|Debian stable|Linux Mint|linux-mint|linux mint|openSUSE|Alpine|centos|rhel|opensuse|alpine" README.md ROADMAP.md CONTRIBUTING.md AGENTS.md docs apps crates data recipes deploy packaging .github
git ls-files .claude
git check-ignore -v .claude/settings.local.json
```
````

- [ ] **Step 2: Refresh docs-audit inventory**

```bash
git add docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 3: Add the inventory ledger row**

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with
literal tab separators:

```text
docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	planning	maintainer	maintainability; phase2; dead-surface-inventory; pruning-queue	AGENTS.md; docs/llms/README.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md; data/distros.toml; apps/conary/src/cli/repo.rs; apps/conary/src/commands/ccs/init.rs; apps/remi/src/bin/remi.rs	verified	corrected	Added the Phase 2 dead-surface inventory with fixed public CLI strings, intentional internal/parser/archive surfaces, deferred behavior-changing candidates, and refresh commands.
```

- [ ] **Step 4: Update the docs-audit summary for the inventory**

In the `### 2026-06-06 Maintainability Phase 2 Planning` section of
`docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph:

```markdown
The first Phase 2 pruning inventory records the fixed public help strings,
intentional internal/parser/archive surfaces, deferred runtime target-ID
normalization, and refresh commands for extending the queue.
```

Then update the `## Final Counts` block to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 143
- `verified-no-change`: 14
- `corrected`: 42
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

- [ ] **Step 5: Update the docs-audit summary ledger row**

Replace the existing `docs/superpowers/documentation-accuracy-audit-summary.md`
row in `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with this
literal-tab row:

```text
docs/superpowers/documentation-accuracy-audit-summary.md	docs/superpowers/documentation-accuracy-audit-summary.md	planning	maintainer	audit-summary; verification; release-hardening; active-planning; maintainability	docs/superpowers/documentation-accuracy-audit-ledger.tsv; docs/superpowers/documentation-accuracy-audit-inventory.tsv; scripts/check-doc-audit-ledger.sh; ROADMAP.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md; docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	verified	corrected	Refreshed the audit summary for the active maintainability planning lane, current docs-audit counts, Phase 1 discipline contract, Phase 2 dead-surface pruning plan, and the Phase 2 pruning inventory.
```

- [ ] **Step 6: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
git diff --check
```

Expected: docs-audit passes and both diff checks exit 0.

- [ ] **Step 7: Commit the inventory**

```bash
git add docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: add dead surface inventory"
```

## Task 6: Final Verification And Push

**Files:**
- Verify all files changed by Tasks 0 through 5.

- [ ] **Step 1: Run focused tests**

```bash
cargo test -p conary --test cli_daily_ux phase2_pruning
cargo test -p remi --test cli_help phase2_pruning
cargo test -p remi
```

Expected: all focused pruning tests pass, and the Remi package test suite
passes because this slice touches the Remi package.

- [ ] **Step 2: Run focused package checks**

```bash
cargo check -p conary
cargo check -p remi --bin remi
```

Expected: both package checks exit 0.

- [ ] **Step 3: Run formatting and docs-audit checks**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 4: Run targeted stale-surface sweeps**

Run the exact sweeps below:

```bash
stale_matches="$(
    grep -R -n -E "ccs-build|conary ccs-build|Examples: fedora, arch, debian, ubuntu|arch, fedora, ubuntu, debian|fedora, ubuntu, debian|Distribution to generate index for \\(arch, fedora, ubuntu, debian\\)|Distribution to pre-warm \\(arch, fedora, ubuntu, debian\\)|Distribution to benchmark, such as fedora, ubuntu, debian, or arch" apps docs README.md CONTRIBUTING.md .github \
        | grep -v -E "^(docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md|docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md|[^:]+/archive/)" || true
)"
test -z "$stale_matches" || { printf '%s\n' "$stale_matches"; exit 1; }

production_debian_matches="$(
    grep -n -i "debian" apps/remi/src/bin/remi.rs apps/conary/src/cli/repo.rs apps/conary/src/commands/ccs/init.rs || true
)"
test -z "$production_debian_matches" || { printf '%s\n' "$production_debian_matches"; exit 1; }

test_debian_matches="$(
    grep -n -i "debian" apps/conary/tests/cli_daily_ux.rs apps/remi/tests/cli_help.rs || true
)"
printf '%s\n' "$test_debian_matches"
unexpected_test_debian_matches="$(
    printf '%s\n' "$test_debian_matches" | grep -v 'assert!(!.*contains("debian")' || true
)"
test -z "$unexpected_test_debian_matches" || { printf '%s\n' "$unexpected_test_debian_matches"; exit 1; }

linux_mint_public_matches="$(
    grep -R -n -E "Linux Mint|linux-mint|linux mint" README.md ROADMAP.md CONTRIBUTING.md AGENTS.md docs deploy .github \
        | grep -v -E "^(docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md|docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md|[^:]+/archive/)" || true
)"
test -z "$linux_mint_public_matches" || { printf '%s\n' "$linux_mint_public_matches"; exit 1; }
```

Expected:

- The first sweep exits 0 and finds no stale public strings outside this plan,
  the inventory, and archived historical docs.
- The second sweep exits 0 and finds no `debian` references in production
  help/output sources touched by this plan.
- The third sweep prints only negative assertions that protect against
  reintroducing `debian` into public help.
- The Linux Mint sweep exits 0 and finds no active public docs/help support
  claims outside this plan, the inventory, and archived historical docs.

- [ ] **Step 5: Verify repo state before push**

```bash
git status --short --branch
git rev-list --left-right --count HEAD...origin/main
git log --oneline -5
```

Expected: local `main` is ahead only by this Phase 2 plan's commits.

- [ ] **Step 6: Push**

```bash
git push
```

- [ ] **Step 7: Verify clean local/remote parity**

```bash
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git worktree list --porcelain
```

Expected:

- `git status --short --branch` shows `## main...origin/main`.
- `git rev-parse HEAD origin/main` prints the same SHA twice.
- `git rev-list --left-right --count HEAD...origin/main` prints `0	0`.
- `git worktree list --porcelain` shows only the expected worktree unless the
  user intentionally created another one.

## Implementation Order

Run the tasks serially:

1. Task 0: lock the reviewed plan.
2. Task 1 and Task 2: Conary CLI tests and cleanup.
3. Task 3 and Task 4: Remi CLI tests and cleanup.
4. Task 5: inventory doc and docs-audit metadata.
5. Task 6: final verification and push.

Do not combine this with Phase 3 fixture discipline or Phase 4 hotspot
decomposition. The point of this slice is to remove a few obvious public
misdirections and leave a better queue for the next pruning pass.

## Review Checklist

Before locking this plan in, verify:

- Public target wording remains limited to Fedora 44, Ubuntu 26.04, and Arch.
- Internal Debian/DEB-family parser and version-scheme behavior is preserved.
- Active public docs/help are gated against Linux Mint support claims while
  negative unsupported-distro tests remain allowed.
- The tests prove the stale strings before changing them.
- The inventory distinguishes fixed, intentional, and deferred surfaces.
- Docs-audit ordering stages new docs before regenerating inventory.
- Verification commands are focused and runnable without network access.
