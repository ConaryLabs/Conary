---
last_updated: 2026-07-01
revision: 1
summary: Implementation plan for the first-external-tester-loop umbrella design
---

# First External Tester Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the four slices of
`docs/superpowers/specs/2026-07-01-first-external-tester-loop-design.md`:
meta-layer budget policy, preview CLI surface tiering, the first external
tester loop pre-launch gate, and the self-hosted Remi tutorial.

**Architecture:** Slices 1 and 2 are repo changes (one policy paragraph, then
clap `hide` attributes plus a `--help-advanced` root flag rendered from the
clap command tree). Slice 3 is docs/templates plus a maintainer-gated release
and launch sequence. Slice 4 is a new operator guide verified by executing it
on a fresh VM.

**Tech Stack:** Rust (clap derive, workspace at repo root), Markdown docs,
repo gate scripts (`scripts/check-doc-truth.sh`,
`scripts/check-doc-audit-ledger.sh`, `scripts/check-coherency-ledger.sh`,
`scripts/check-release-matrix.sh`).

## Global Constraints

- Debug builds only for dev work: `cargo build -p conary` (never `--release`
  locally; release binaries come from CI).
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --check` must pass before every commit that touches Rust.
- Every Rust source file starts with a path comment (`// apps/conary/src/...`).
- Conventional commit subjects (`feat(cli): ...`, `docs: ...`, `fix(docs): ...`).
- No renames or removals of CLI commands — tiering is help-visibility only.
- No schema migrations.
- Every **new** tracked doc must be registered in
  `docs/superpowers/documentation-accuracy-audit-inventory.tsv` (columns:
  `path	family	audience`) and
  `docs/superpowers/documentation-accuracy-audit-ledger.tsv` (columns:
  `origin_path	path	family	audience	claim_clusters	evidence_sources	status	disposition	notes`),
  then verified with
  `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending`.
- Steps marked **MAINTAINER GATE** need Peter (credentials, public hosts, or
  a posting decision). Agents prepare everything up to those steps, then stop
  and report.

---

### Task 1: Meta-layer budget policy (slice 1)

**Files:**
- Modify: `AGENTS.md` (the "Maintainability & Refactor Discipline" section)
- Modify: `ROADMAP.md` (the "Near-Term Priorities" section intro)

**Interfaces:**
- Produces: the policy text later tasks may cite; no code.

- [ ] **Step 1: Add the policy paragraph to AGENTS.md**

Append this paragraph to the end of the `## Maintainability & Refactor
Discipline` section of `AGENTS.md`:

```markdown
Meta-layer budget: ledger, ownership-card, gate, and agent-tooling changes are
allowed only when product work forces them — a touched path, a failing gate,
or a factual drift. Discretionary meta-layer improvement is capped at one meta
slice per four product slices. This budget holds at least until the first
external tester milestone in
`docs/superpowers/specs/2026-07-01-first-external-tester-loop-design.md` is
met.
```

- [ ] **Step 2: Add the ROADMAP pointer**

In `ROADMAP.md`, insert this line directly under the `## Near-Term Priorities`
heading, before the numbered list:

```markdown
Process note: meta-layer (ledger/card/gate/tooling) work is budget-capped per
the meta-layer budget rule in `AGENTS.md` until the first external tester
milestone is met.
```

- [ ] **Step 3: Run the doc gates**

Run:
```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```
Expected: both pass. (No new doc files were created, so no ledger rows are
needed; if `check-doc-truth.sh` flags the AGENTS/ROADMAP edits, fix the
flagged wording rather than the gate.)

- [ ] **Step 4: Commit**

```bash
git add AGENTS.md ROADMAP.md
git commit -m "docs: add meta-layer budget policy per tester-loop design"
```

---

### Task 2: Hide advanced commands from default help (slice 2, part 1)

**Files:**
- Modify: `apps/conary/src/cli/mod.rs` (the `Commands` enum, lines ~153–900,
  and the `after_help` string at line ~136)
- Test: `apps/conary/tests/cli_daily_ux.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: hidden clap subcommands that Task 3's `--help-advanced` listing
  discovers via `is_hide_set()`; the epilogue line
  `Advanced packaging and platform commands: run 'conary --help-advanced'`.

- [ ] **Step 1: Write the failing tests**

Add to `apps/conary/tests/cli_daily_ux.rs` (uses the existing `run_conary`
and `output_text` helpers at the top of that file):

```rust
#[test]
fn preview_tiering_default_help_shows_only_daily_driver_commands() {
    let output = run_conary(&["--help"]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in [
        "install",
        "remove",
        "update",
        "search",
        "list",
        "autoremove",
        "pin",
        "unpin",
        "try",
        "system",
        "repo",
        "config",
        "distro",
        "self-update",
    ] {
        assert!(
            stdout.contains(&format!("\n  {cmd} ")),
            "missing daily-driver command {cmd} in:\n{stdout}"
        );
    }
    for cmd in [
        "cook",
        "new",
        "publish",
        "convert-pkgbuild",
        "recipe-audit",
        "canonical",
        "groups",
        "registry",
        "query",
        "ccs",
        "derive",
        "derivation",
        "model",
        "collection",
        "automation",
        "bootstrap",
        "cache",
        "profile",
        "provenance",
        "capability",
        "trust",
        "verify-derivation",
        "sbom",
        "federation",
        "export",
        "mcp",
    ] {
        assert!(
            !stdout.contains(&format!("\n  {cmd} ")),
            "advanced command {cmd} leaked into default help:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("conary --help-advanced"),
        "missing advanced-help pointer in:\n{stdout}"
    );
}

#[test]
fn preview_tiering_hidden_commands_still_execute() {
    for args in [&["cook", "--help"][..], &["ccs", "--help"][..], &["bootstrap", "--help"][..]] {
        let output = run_conary(args);
        assert!(output.status.success(), "{}", output_text(&output));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p conary --test cli_daily_ux preview_tiering -- --nocapture
```
Expected: `preview_tiering_default_help_shows_only_daily_driver_commands`
FAILS (advanced commands are visible and the pointer line is missing);
`preview_tiering_hidden_commands_still_execute` PASSES (nothing hidden yet is
also fine — it must never regress).

- [ ] **Step 3: Hide the advanced variants**

In the `Commands` enum in `apps/conary/src/cli/mod.rs`, add `hide = true` to
exactly these variants (three attribute shapes exist; follow the matching
pattern):

Struct variants without an existing `#[command(...)]` attribute — `Cook`,
`New`, `Publish`, `ConvertPkgbuild`, `Sbom`, `Export` — get a new line above
the variant:

```rust
    #[command(hide = true)]
    Cook {
```

`RecipeAudit` already has `#[command(name = "recipe-audit")]` (line ~675);
extend it:

```rust
    #[command(name = "recipe-audit", hide = true)]
    RecipeAudit {
```

Tuple variants with `#[command(subcommand)]` — `Canonical`, `Groups`,
`Registry`, `Query`, `Ccs`, `Derive`, `Model`, `Collection`, `Automation`,
`Bootstrap`, `Cache`, `Derivation`, `Profile`, `Provenance`, `Capability`,
`Trust`, `VerifyDerivation`, `Federation` — extend the existing attribute:

```rust
    #[command(subcommand, hide = true)]
    Canonical(CanonicalCommands),
```

Do NOT touch `Mcp` (already hidden) or any daily-driver variant (`Install`,
`Remove`, `Update`, `Search`, `List`, `Autoremove`, `Pin`, `Unpin`, `Try`,
`System`, `Repo`, `Config`, `Distro`, `SelfUpdate`).

- [ ] **Step 4: Extend the help epilogue**

In the `after_help` string at line ~136 of `apps/conary/src/cli/mod.rs`,
append this to the end of the existing string (inside the quotes, after the
`conaryd` line):

```text
\n\nAdvanced packaging and platform commands: run 'conary --help-advanced'
```

- [ ] **Step 5: Update the existing epilogue test intentionally**

In `root_help_includes_daily_workflow_examples`
(`apps/conary/tests/cli_daily_ux.rs:64`), add one assertion at the end of the
function (this is the test rewrite the design spec requires to be named —
record it in the commit message):

```rust
    assert!(stdout.contains("conary --help-advanced"), "{stdout}");
```

- [ ] **Step 6: Run tests — default-help test now passes**

Run:
```bash
cargo test -p conary --test cli_daily_ux -- --nocapture
```
Expected: all `cli_daily_ux` tests PASS, including both new
`preview_tiering_*` tests and the updated
`root_help_includes_daily_workflow_examples`. Note: the default-help test's
pointer-line assertion passes after Step 4; the `--help-advanced` flag itself
is Task 3.

- [ ] **Step 7: Lint, format, coherency check**

Run:
```bash
cargo clippy -p conary --all-targets -- -D warnings
cargo fmt --check
grep -n "apps/conary/src/cli/mod.rs" docs/superpowers/feature-coherency-ledger.tsv
```
Expected: clippy and fmt clean. If the grep prints rows, run each row's
listed proof command and confirm it still passes before committing.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/tests/cli_daily_ux.rs
git commit -m "feat(cli): tier preview help to the daily-driver surface

Hide builder/platform subcommands from default --help per the
2026-07-01 tester-loop design; commands keep working unchanged.
Intentional test rewrite: root_help_includes_daily_workflow_examples
gains the advanced-help pointer assertion."
```

---

### Task 3: Add the `--help-advanced` listing (slice 2, part 2)

**Files:**
- Modify: `apps/conary/src/cli/mod.rs` (the `Cli` struct at line ~139)
- Modify: `apps/conary/src/app.rs` (the `run()` function, lines 10–16)
- Test: `apps/conary/tests/cli_daily_ux.rs`

**Interfaces:**
- Consumes: the hidden variants from Task 2 (`is_hide_set()` finds them).
- Produces: `pub fn render_advanced_help() -> String` in `conary::cli`, and
  the `Cli.help_advanced: bool` field checked in `app::run()`.

- [ ] **Step 1: Write the failing test**

Add to `apps/conary/tests/cli_daily_ux.rs`:

```rust
#[test]
fn preview_tiering_help_advanced_lists_hidden_surface() {
    let output = run_conary(&["--help-advanced"]);
    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in [
        "cook",
        "new",
        "publish",
        "convert-pkgbuild",
        "recipe-audit",
        "canonical",
        "groups",
        "registry",
        "query",
        "ccs",
        "derive",
        "derivation",
        "model",
        "collection",
        "automation",
        "bootstrap",
        "cache",
        "profile",
        "provenance",
        "capability",
        "trust",
        "verify-derivation",
        "sbom",
        "federation",
        "export",
        "mcp",
    ] {
        assert!(
            stdout.contains(&format!("\n  {cmd}")),
            "missing advanced command {cmd} in:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("\n  install"),
        "daily-driver command leaked into advanced help:\n{stdout}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cargo test -p conary --test cli_daily_ux preview_tiering_help_advanced -- --nocapture
```
Expected: FAIL — clap rejects the unknown `--help-advanced` flag (non-zero
exit).

- [ ] **Step 3: Add the flag and the renderer**

In the `Cli` struct in `apps/conary/src/cli/mod.rs` (next to the existing
global flags around line 139–150), add:

```rust
    /// List advanced packaging and platform commands
    #[arg(long = "help-advanced")]
    pub help_advanced: bool,
```

At the bottom of `apps/conary/src/cli/mod.rs`, add:

```rust
/// Render the advanced-command listing from the live clap command tree so it
/// cannot drift from the real surface. Everything marked `hide = true` on the
/// root command is, by definition, the advanced tier (this includes `mcp`,
/// which is intentionally part of the truthful surface).
pub fn render_advanced_help() -> String {
    use clap::CommandFactory;

    let cmd = Cli::command();
    let mut rows: Vec<(String, String)> = cmd
        .get_subcommands()
        .filter(|sub| sub.is_hide_set())
        .map(|sub| {
            (
                sub.get_name().to_string(),
                sub.get_about().map(|a| a.to_string()).unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);

    let mut out =
        String::from("Advanced packaging and platform commands (hidden from default help):\n");
    for (name, about) in rows {
        out.push_str(&format!("  {name:<width$}  {about}\n"));
    }
    out.push_str("\nRun 'conary <command> --help' for details on any command.\n");
    out
}
```

In `apps/conary/src/app.rs`, inside `run()` immediately after
`let cli = Cli::parse();` (line ~13) and before the seccomp override line,
add:

```rust
    if cli.help_advanced {
        print!("{}", crate::cli::render_advanced_help());
        return Ok(());
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p conary --test cli_daily_ux -- --nocapture
```
Expected: all PASS, including `preview_tiering_help_advanced_lists_hidden_surface`.

- [ ] **Step 5: Lint, format, commit**

Run:
```bash
cargo clippy -p conary --all-targets -- -D warnings
cargo fmt --check
git add apps/conary/src/cli/mod.rs apps/conary/src/app.rs apps/conary/tests/cli_daily_ux.rs
git commit -m "feat(cli): add --help-advanced rendered from the clap tree"
```

---

### Task 4: Align README, site copy, and doc gates with the tiered help (slice 2, part 3)

**Files:**
- Modify: `README.md` (Quick Start section, lines ~99–200)
- Possibly modify: `site/` copy if the docs-truth gate flags it
- Create: `docs/guides/advanced-commands.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`,
  `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

**Interfaces:**
- Consumes: `conary --help-advanced` output from Task 3 (source for the docs
  page).
- Produces: `docs/guides/advanced-commands.md`, linked from README.

- [ ] **Step 1: Create the advanced-commands docs page**

Create `docs/guides/advanced-commands.md`:

```markdown
---
last_updated: 2026-07-01
revision: 1
summary: The advanced packaging and platform command surface hidden from default CLI help
---

# Advanced Commands

The default `conary --help` shows the daily-driver surface: install, remove,
update, search, list, autoremove, pin/unpin, try, system, repo, config,
distro, and self-update.

The advanced packaging and platform surface is hidden from default help but
fully supported at its existing paths. List it any time with:

​```bash
conary --help-advanced
​```

The listing is rendered from the CLI's own command tree, so this page does
not duplicate it; run the command for the current surface. Broad areas:

- **Packaging and recipes:** `cook`, `new`, `publish`, `convert-pkgbuild`,
  `recipe-audit`, `ccs`
- **System modeling and composition:** `model`, `collection`, `groups`,
  `derive`, `derivation`, `profile`, `cache`
- **Provenance and trust:** `provenance`, `capability`, `trust`,
  `verify-derivation`, `sbom`, `canonical`, `registry`
- **Platform and distribution:** `bootstrap`, `federation`, `export`,
  `query`, `automation`, `mcp`

Every command keeps `conary <command> --help`.
```

(Remove the zero-width characters around the code fence when creating the
file — they only escape the fence inside this plan.)

- [ ] **Step 2: Link it from README**

In `README.md`, at the end of the `### Developer Build` code block section
(after line ~200, before `## Features`), add:

```markdown
The default `conary --help` shows the daily-driver commands. The full
packaging/platform surface is listed by `conary --help-advanced` and
described in [docs/guides/advanced-commands.md](docs/guides/advanced-commands.md).
```

- [ ] **Step 3: Register the new doc in the audit inventory and ledger**

Add to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
(alphabetical position among the `docs/guides/` rows; tab-separated):

```text
docs/guides/advanced-commands.md	guide	user
```

Append to `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
(one tab-separated line):

```text
docs/guides/advanced-commands.md	docs/guides/advanced-commands.md	guide	user	cli-tiering; advanced-help	apps/conary/src/cli/mod.rs; apps/conary/tests/cli_daily_ux.rs	verified	verified-no-change	Documents the preview CLI tiering split and the --help-advanced discovery path added by the 2026-07-01 tester-loop design.
```

- [ ] **Step 4: Run the gates**

Run:
```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
bash scripts/check-doc-truth.sh
```
Expected: both pass. If `check-doc-truth.sh` flags README or site copy that
still implies all commands are visible in root help, fix that copy and rerun.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/guides/advanced-commands.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: describe the tiered CLI surface and advanced-help path"
```

---

### Task 5: Compatibility checklist, two tiers (slice 3, pre-launch)

**Files:**
- Create: `docs/guides/compatibility-checklist.md`
- Modify: `README.md` (`### Five-Minute Preview`, line ~101)
- Modify: the two audit TSVs (as in Task 4)

**Interfaces:**
- Produces: `docs/guides/compatibility-checklist.md`, linked from the
  quickstart and later from the tester post (Task 8).

- [ ] **Step 1: Verify the tier-2 requirement claims against repo docs**

Run:
```bash
grep -rn "composefs" docs/ARCHITECTURE.md docs/conaryopedia-v2.md | head -20
grep -rn "6\.[0-9]\+\|kernel" docs/ARCHITECTURE.md | head -10
```
Expected: statements of the composefs/overlayfs and kernel expectations for
the generation model. Use what these docs actually say in Step 2 — if they
state a different kernel floor than 6.5, the repo docs win. If they state no
floor, write "a composefs-capable kernel (6.5 or newer recommended)".

- [ ] **Step 2: Create the checklist**

Create `docs/guides/compatibility-checklist.md` (adjust tier-2 specifics per
Step 1):

```markdown
---
last_updated: 2026-07-01
revision: 1
summary: Host requirements for the Conary limited preview, split by tier
---

# Compatibility Checklist

Check this before trying the preview so an unsupported host hits a doc, not
a wall.

## Tier 1 — Basic package loop

Covers: `install`, `remove`, `update`, `search`, `list`, adopt/unadopt, and
`try`. This is everything the first external tester loop asks you to run.

- Fedora 44, Ubuntu 26.04 LTS, or Arch Linux
- Stock distribution kernel — no composefs, UEFI, or special boot-stack
  requirement
- x86_64
- Root access (`sudo`)
- A VM, snapshot, or non-critical host (preview etiquette, not a technical
  requirement — adopt/unadopt is designed to be reversible)

## Tier 2 — Generation-model features

Covers: generation build/switch/rollback, `system generation export`, and
next-boot activation. NOT required for the basic package loop above.

- composefs-capable kernel with overlayfs
- systemd
- UEFI boot stack
- Sufficient disk for generation artifacts under `/conary`

If your host fails a Tier 2 item, everything in Tier 1 still works.
```

- [ ] **Step 3: Link from the quickstart**

In `README.md`, add as the first line under `### Five-Minute Preview`
(line ~101):

```markdown
Before starting, skim the [compatibility checklist](docs/guides/compatibility-checklist.md) — the basic package loop runs on stock kernels; only generation-model features need more.
```

- [ ] **Step 4: Register in the audit TSVs and run gates**

Inventory row:

```text
docs/guides/compatibility-checklist.md	guide	user
```

Ledger row:

```text
docs/guides/compatibility-checklist.md	docs/guides/compatibility-checklist.md	guide	user	compatibility-tiers; preview-requirements	docs/ARCHITECTURE.md; README.md	verified	verified-no-change	Two-tier host requirements for the limited preview per the 2026-07-01 tester-loop design; tier 1 basic loop on stock kernels, tier 2 generation-model requirements.
```

Run:
```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
bash scripts/check-doc-truth.sh
```
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add docs/guides/compatibility-checklist.md README.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: add two-tier preview compatibility checklist"
```

---

### Task 6: Narrow the beta feedback intake for the loop (slice 3, pre-launch)

**Files:**
- Modify: `.github/ISSUE_TEMPLATE/beta_feedback.md`

**Interfaces:**
- Produces: the "completed the full loop" yes/no field that Task 7's tracker
  counts.

- [ ] **Step 1: Add the tester-loop lane and completion field**

In `.github/ISSUE_TEMPLATE/beta_feedback.md`, make the first `## Preview
Lane` checkbox:

```markdown
- [ ] First external tester loop (install → adopt → list/search → update --dry-run → unadopt)
```

(keep the existing six lanes below it), and insert this new section between
`## Preview Lane` and `## Environment`:

```markdown
## First External Tester Loop

Fill this in if you ran the tester loop from the preview post.

- **Completed the full loop (install → adopt → list/search → update --dry-run → unadopt)**: yes / no / partial
- **If partial or no — where did it stop, and what did you see?**:
```

- [ ] **Step 2: Verify the template still parses as a GitHub issue template**

Run:
```bash
python3 -c "
import re, sys
text = open('.github/ISSUE_TEMPLATE/beta_feedback.md').read()
m = re.match(r'^---\n(.*?)\n---\n', text, re.S)
assert m, 'frontmatter missing'
assert 'name:' in m.group(1) and 'labels:' in m.group(1)
print('template frontmatter OK')
"
```
Expected: `template frontmatter OK`.

- [ ] **Step 3: Run doc gates and commit**

```bash
bash scripts/check-doc-truth.sh
git add .github/ISSUE_TEMPLATE/beta_feedback.md
git commit -m "docs: foreground the tester loop in the beta feedback intake"
```
(The template is already registered in the audit inventory; no TSV change.)

---

### Task 7: Tester-loop tracker (slice 3, pre-launch)

**Files:**
- Create: `docs/superpowers/first-external-tester-loop-tracker.md`
- Modify: the two audit TSVs

**Interfaces:**
- Consumes: the completion field wording from Task 6.
- Produces: the tracker later triage work appends to; the milestone counter.

- [ ] **Step 1: Create the tracker**

Create `docs/superpowers/first-external-tester-loop-tracker.md`:

```markdown
---
last_updated: 2026-07-01
revision: 1
summary: Completion and friction tracker for the first external tester loop milestone
---

# First External Tester Loop Tracker

Milestone per
`docs/superpowers/specs/2026-07-01-first-external-tester-loop-design.md`:
**10 strangers complete install → adopt → list/search → update --dry-run →
unadopt on their own machines and report via the beta feedback template.**

Stall rule: no new completions for three consecutive weeks after launch
forces a documented pivot decision recorded at the bottom of this file.

**Completions so far: 0 / 10**

## Launch record

- Release tag: (filled at launch)
- Post venues and dates: (filled at launch)

## Reports

| Date | Report | Distro | Full loop completed | Friction summary | Triage |
|------|--------|--------|---------------------|------------------|--------|
| —    | —      | —      | —                   | —                | —      |

Triage values: `fix-now`, `next-slice`, `declined` (with a reason in the
linked issue).

## Pivot record

None.
```

- [ ] **Step 2: Register in the audit TSVs and run the gate**

Inventory row:

```text
docs/superpowers/first-external-tester-loop-tracker.md	planning	maintainer
```

Ledger row:

```text
docs/superpowers/first-external-tester-loop-tracker.md	docs/superpowers/first-external-tester-loop-tracker.md	planning	maintainer	external-tester-loop; milestone-tracking	docs/superpowers/specs/2026-07-01-first-external-tester-loop-design.md; .github/ISSUE_TEMPLATE/beta_feedback.md	verified	verified-no-change	Live tracker counting stranger completions toward the 10-completion milestone with the three-week stall rule and triage log.
```

Run:
```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```
Expected: passes.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/first-external-tester-loop-tracker.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(superpowers): add tester-loop milestone tracker"
```

---

### Task 8: Refresh the tester post copy (slice 3, pre-launch)

**Files:**
- Modify: `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`

**Interfaces:**
- Consumes: checklist link (Task 5), feedback template wording (Task 6).
- Produces: launch-ready post copy with two explicit `RELEASE-TAG` fill-in
  markers that Task 9 resolves.

- [ ] **Step 1: Update the draft copy**

Edit `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`:

1. Add directly under the H1:

```markdown
> Launch state: copy refreshed 2026-07-01 for the first-external-tester-loop
> design. The two `RELEASE-TAG` markers below MUST be replaced with the real
> pinned tag and artifact URL during Task 9 of the 2026-07-01 plan before
> posting.
```

2. After the "Tested preview targets" list, add:

```markdown
Install the pinned preview release (tag `RELEASE-TAG`):
download and checksum/signature instructions at
https://github.com/conary/conary/releases/tag/RELEASE-TAG
(adjust the URL to the canonical repo slug when filling in the tag).

Before starting, check the compatibility checklist —
the whole loop below runs on stock kernels:
https://conary.io/docs/compatibility-checklist (or
`docs/guides/compatibility-checklist.md` in the repo).
```

3. After the command-loop code block, add:

```markdown
When you are done (or stuck), please file what happened — good, bad, or
confusing — using the Beta Feedback issue template, and answer its
"Completed the full loop" question so I can count it:
https://github.com/conary/conary/issues/new?template=beta_feedback.md
```

4. In the command-loop code block, confirm the sequence matches the milestone
   loop (`install`, `adopt`, `list`/`search`, `update --dry-run`, `unadopt`)
   and add `conary system unadopt --all --dry-run` before the `--yes` variant
   if not already present.

- [ ] **Step 2: Run the doc gate and commit**

```bash
bash scripts/check-doc-truth.sh
git add docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md
git commit -m "docs(superpowers): refresh tester post for the pinned-release loop"
```

---

### Task 9: Pinned release, evidence, prewarm, and launch — **MAINTAINER GATE** (slice 3)

**Files:**
- Modify: `docs/operations/release-artifact-matrix.md` (the `conary` row)
- Modify: `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`
  (resolve `RELEASE-TAG`)
- Modify: `docs/superpowers/first-external-tester-loop-tracker.md`
  (launch record)

**Interfaces:**
- Consumes: everything from Tasks 2–8 (all must be merged first).
- Produces: the launched loop; the tracker's launch record.

Every step below needs Peter's credentials or a posting decision. An agent
reaching this task should verify Tasks 1–8 are committed, then stop and
present this checklist.

- [ ] **Step 1 (MAINTAINER): Cut and push the release**

```bash
./scripts/release.sh conary
git push && git push --tags
```
Expected: `release-build.yml` runs on the tag; watch with
`gh run watch` until green, then `deploy-and-verify.yml` deploys to Remi.

- [ ] **Step 2 (MAINTAINER): Verify assets, checksums, signatures**

```bash
gh release view <tag> --json assets
# download the CCS/RPM/DEB/Arch assets plus checksum and signature files
# verify per the release-artifact-matrix "Required evidence" column
bash scripts/check-release-matrix.sh
```
Expected: assets present; checksums and signatures verify; matrix gate
passes. Record the verification commands and outputs.

- [ ] **Step 3 (MAINTAINER): Move the matrix row off source-build-only**

Edit the `conary` row of `docs/operations/release-artifact-matrix.md`:
replace `source-build-only until a preview release links artifact URLs` with
the concrete GitHub release URL(s) for the tag, and update the pending
checksum/signature evidence cells with what Step 2 actually verified. Run:

```bash
bash scripts/check-release-matrix.sh
bash scripts/check-doc-truth.sh
```
Expected: both pass. Commit:

```bash
git add docs/operations/release-artifact-matrix.md
git commit -m "docs(release): link pinned preview artifacts for the tester loop"
```

- [ ] **Step 4 (MAINTAINER): Resolve RELEASE-TAG in the post and tracker**

Replace both `RELEASE-TAG` markers in the tester post with the real tag and
URL, delete the "Launch state" fill-in note added in Task 8, and fill the
tracker's "Launch record" (tag; leave venues until Step 6). Commit both.

- [ ] **Step 5 (MAINTAINER): Prewarm the public Remi**

On the Remi host (see `docs/operations/infrastructure.md` for access):

```bash
remi prewarm --help   # confirm current flags
# then run prewarm for each supported distro against the package set named
# in the tester post's command loop (at minimum: the <small-package>
# examples the post suggests, e.g. nginx and the post's named packages)
```
Expected: prewarm completes for fedora-44, ubuntu-26.04, and arch; spot-check
one conversion via `bash scripts/remi-health.sh --smoke`.

- [ ] **Step 6 (MAINTAINER): Post, then record the launch**

Post the refreshed copy to the chosen venue(s). Record venues and dates in
the tracker's launch record, commit, and the loop is live. From this point,
triage incoming beta-feedback issues into the tracker per its `fix-now` /
`next-slice` / `declined` values, and tester-reported friction outranks the
"keep green" rotation until 10 completions or a pivot.

---

### Task 10: Self-hosted Remi tutorial (slice 4 — parallel after Task 4)

**Files:**
- Create: `docs/guides/self-hosted-remi.md`
- Modify: the two audit TSVs

**Interfaces:**
- Consumes: `deploy/remi.toml.example`, `deploy/systemd/remi.service`,
  `scripts/remi-health.sh` (existing artifacts the tutorial reuses).
- Produces: the guide Task 11 executes verbatim on a fresh VM.

- [ ] **Step 1: Extract the real config and unit facts**

Run:
```bash
sed -n '1,80p' deploy/remi.toml.example
cat deploy/systemd/remi.service
bash scripts/remi-health.sh --help 2>&1 | head -20
grep -n "admin_bind\|bind\|root =" deploy/remi.toml.example
```
Expected: the config keys (`[server] bind/admin_bind/workers`,
`[storage] root/max_cache_size`, `[upstream.*]` blocks) and the unit's
`ExecStart=/usr/local/bin/remi --config /etc/conary/remi.toml`. The tutorial
must only state what these files actually contain.

- [ ] **Step 2: Write the tutorial**

Create `docs/guides/self-hosted-remi.md` with exactly these sections, each
built from Step 1 facts (this outline is binding; the prose comes from the
extracted facts, not from memory):

```markdown
---
last_updated: 2026-07-01
revision: 1
summary: Run your own Remi conversion server in about 30 minutes
---

# Self-Hosted Remi in 30 Minutes

## What you get
(Remi's two jobs — conversion proxy and repo server — one paragraph, and
what self-hosting does NOT require: no conary.io account, no federation.)

## Requirements
(A Linux host with Rust 1.96+, disk sized to the [storage] max_cache_size
you choose, outbound HTTPS to distro mirrors. Reference
docs/guides/compatibility-checklist.md tier 1 for client hosts.)

## 1. Build the binary
cargo build --release -p remi
(plus copying target/release/remi to /usr/local/bin/remi)

## 2. Write /etc/conary/remi.toml
(A minimal config derived from deploy/remi.toml.example: [server] bind and
admin_bind, [storage] root and max_cache_size sized down for a small host,
one [upstream.<name>] block per distro the operator wants. State that
admin_bind must stay on localhost.)

## 3. Install the systemd unit
(Copy deploy/systemd/remi.service, systemctl daemon-reload, enable --now.)

## 4. Verify
(curl the health endpoint on the bind address; run
bash scripts/remi-health.sh --smoke from a checkout if available; convert
one package end-to-end from a client with `conary repo add` pointing at the
new server.)

## 5. Optional: S3/R2 chunk storage
(Only if remi.toml.example documents it — otherwise state it is out of
scope for the 30-minute path and link docs/operations/infrastructure.md.)

## Verified run
(Filled by the fresh-VM verification: date, host OS, elapsed time, and any
deviations that were folded back into the steps above.)
```

- [ ] **Step 3: Register in the audit TSVs**

Inventory row:

```text
docs/guides/self-hosted-remi.md	guide	operator
```

Ledger row:

```text
docs/guides/self-hosted-remi.md	docs/guides/self-hosted-remi.md	guide	operator	remi-self-host; operator-onboarding	deploy/remi.toml.example; deploy/systemd/remi.service; scripts/remi-health.sh; apps/remi/src/bin/remi.rs	pending	pending	Self-hosted Remi tutorial per the 2026-07-01 tester-loop design; status flips to verified after the fresh-VM run in the companion verification task.
```

Run:
```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```
Expected: passes (`--allow-pending` covers the pending row).

- [ ] **Step 4: Commit**

```bash
git add docs/guides/self-hosted-remi.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(guides): add self-hosted Remi 30-minute tutorial"
```

---

### Task 11: Fresh-VM verification of the tutorial — **MAINTAINER GATE** (slice 4)

**Files:**
- Modify: `docs/guides/self-hosted-remi.md` (the "Verified run" section)
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  (flip the Task 10 row to verified)

**Interfaces:**
- Consumes: the tutorial from Task 10, executed verbatim.

Needs a fresh VM (local QEMU per `scripts/local-qemu-validation.sh` patterns,
or any clean cloud host).

- [ ] **Step 1 (MAINTAINER): Execute the tutorial start to finish**

On a fresh VM, follow `docs/guides/self-hosted-remi.md` exactly as written,
timing the run. Every deviation needed to succeed is a tutorial bug: fix the
guide, not the VM.

- [ ] **Step 2: Record the run and flip the ledger row**

Fill the guide's "Verified run" section (date, host OS, elapsed time,
deviations folded back). In the audit ledger row for
`docs/guides/self-hosted-remi.md`, change `pending	pending` to
`verified	corrected` (or `verified	verified-no-change` if no edits were
needed) and extend the notes with the run date. Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --allow-pending
```
Expected: passes.

- [ ] **Step 3: Commit**

```bash
git add docs/guides/self-hosted-remi.md docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(guides): record fresh-VM verification of the Remi tutorial"
```
