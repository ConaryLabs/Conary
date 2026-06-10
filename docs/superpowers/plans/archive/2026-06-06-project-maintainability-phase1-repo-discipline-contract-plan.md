# Project Maintainability Phase 1 Repo Discipline Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary's maintainability expectations explicit enough that future refactor, pruning, and contributor-UX slices start from shared repo discipline instead of maintainer lore.

**Architecture:** Keep durable policy in the existing top-level guidance files rather than adding a broad new manual. Add one lightweight reporting script so file-size pressure is easy to inspect, but keep thresholds as review signals rather than hard CI failures.

**Tech Stack:** Bash, Markdown, existing docs-audit tooling, Git, Cargo verification commands only where later implementation touches Rust.

---

## Status

Draft child implementation plan for review.

This is the Phase 1 child plan for
`docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.
It does not refactor hotspot code. It creates the small discipline contract, a
line-count report artifact, and a focused script-test companion that later
Phase 2 and Phase 4 child plans will rely on.

## Read First

- `AGENTS.md`
- `CONTRIBUTING.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `scripts/docs-audit-inventory.sh`
- `scripts/check-doc-audit-ledger.sh`

## Design Summary

Phase 1 should make maintenance work easier to start without making the root
docs heavier than they already are. The implementation should update the
existing guidance layers:

- `AGENTS.md`: compact assistant-facing contract for refactor and maintenance
  slices.
- `docs/llms/README.md`: routing note for assistants so maintenance packets
  include ownership, state, and verification evidence.
- `CONTRIBUTING.md`: human-facing contribution guidance for maintainability
  slices.
- `.github/PULL_REQUEST_TEMPLATE.md`: prompt authors to name plan, ownership,
  and verification evidence.
- `scripts/line-count-report.sh`: cheap local visibility into Rust file-size
  pressure.
- `scripts/test-line-count-report.sh`: focused script test for the new report.

The plan intentionally avoids a standalone discipline manual. If later phases
prove that the compact contract is too large for top-level docs, a later child
plan can extract a narrow canonical doc and replace the duplicated prose with a
link.

## Discipline Contract

The implementation should encode these rules consistently:

- Persisted state is sacred: SQLite migrations and tables, package archives,
  manifest formats, trust metadata, generated artifacts, Remi state, conaryd
  jobs, integration-test manifests, `data/distros.toml`, and recipe version or
  checksum inputs require explicit compatibility decisions.
- Internal Rust APIs are negotiable: workspace-only modules, helper types,
  command internals, service internals, and test helpers may be broken when the
  new boundary is better and the plan names the owning module.
- Large files are review signals: crossing a line threshold should trigger
  ownership and verification questions, not an automatic failure.
- Refactor slices must name the current responsibility, the new owner, the
  behavior-preserving tests, and any docs or subsystem-map updates.
- Command files should stay thin; service routes should stay thin; core modules
  should keep contracts, planning, execution, persistence, and rendering
  separable when those concerns can change independently.
- Nested `AGENTS.md` files are rare and should appear only for subtrees with
  durable local rules that differ from the repo root.
- No compatibility shim should be added only to preserve stale internal module
  paths or misleading user-facing surfaces.

## File Structure

- Create `scripts/line-count-report.sh`
  - Reports the largest Rust files under `apps/` and `crates/`.
  - Uses `find`, `wc`, `awk`, and `sort` so it does not require `rg`.
  - Accepts an optional numeric row limit, defaulting to 60.
- Create `scripts/test-line-count-report.sh`
  - Verifies executable bit, header shape, row count, Rust-file paths, numeric
    line counts, descending sort order, and invalid-argument handling.
- Modify `AGENTS.md`
  - Add a compact maintainability/refactor discipline section.
- Modify `docs/llms/README.md`
  - Bump frontmatter and add assistant routing rules for maintenance packets.
- Modify `CONTRIBUTING.md`
  - Add human-facing maintainability slice guidance without replacing existing
    build/test sections.
- Modify `.github/PULL_REQUEST_TEMPLATE.md`
  - Add fields for plan, ownership boundary, and verification commands.
- Modify docs-audit files when this branch adds, moves, archives, or
  materially changes tracked documentation claims that the ledger summarizes.

## Pre-Implementation Lock-In For This Plan

Before executing Task 1, commit this reviewed plan with docs-audit metadata.
This avoids starting implementation from an untracked plan file.

**Files:**
- Add: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Stage the plan so docs-audit inventory can see it**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md
```

- [ ] **Step 2: Refresh docs-audit inventory**

The existing docs-audit tooling requires Bash 4+ and `rg`.

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 3: Add the plan ledger row**

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with
literal tab separators:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md	planning	maintainer	maintainability; phase1; discipline-contract; line-count-report	AGENTS.md; docs/llms/README.md; CONTRIBUTING.md; .github/PULL_REQUEST_TEMPLATE.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; scripts/docs-audit-inventory.sh; scripts/check-doc-audit-ledger.sh	verified	corrected	Added the reviewed Phase 1 implementation plan for compact maintainability discipline guidance, umbrella hotspot-refresh routing, and a lightweight Rust line-count report script.
```

- [ ] **Step 4: Verify and commit the locked plan**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
git diff --check
```

Expected: docs-audit passes and both diff hygiene commands exit 0.

Commit:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md
git commit -m "docs: add maintainability phase 1 plan"
```

## Task 1: Add Line-Count Report Script

**Files:**
- Create: `scripts/line-count-report.sh`
- Create: `scripts/test-line-count-report.sh`

- [ ] **Step 1: Write the failing script test**

Create `scripts/test-line-count-report.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

[[ -x scripts/line-count-report.sh ]] || fail "scripts/line-count-report.sh is not executable"

output="$(scripts/line-count-report.sh 5)"
header="$(printf '%s\n' "$output" | sed -n '1p')"
[[ "$header" == $'lines\tpath' ]] || fail "unexpected header: $header"

row_count="$(printf '%s\n' "$output" | sed '1d' | wc -l | tr -d ' ')"
[[ "$row_count" -eq 5 ]] || fail "expected 5 report rows, got $row_count"

printf '%s\n' "$output" | awk -F '\t' '
    NR == 1 { next }
    $1 !~ /^[0-9]+$/ { exit 1 }
    $2 !~ /^(apps|crates)\// { exit 1 }
    $2 !~ /\.rs$/ { exit 1 }
' || fail "report rows must contain numeric line counts and apps/ or crates/ Rust paths"

printf '%s\n' "$output" | awk -F '\t' '
    NR == 1 { next }
    previous != "" && $1 > previous { exit 1 }
    { previous = $1 }
' || fail "report rows must be sorted by descending line count"

tmp_out="$(mktemp)"
trap 'rm -f "$tmp_out"' EXIT

if scripts/line-count-report.sh not-a-number >"$tmp_out" 2>&1; then
    fail "invalid limit unexpectedly succeeded"
fi

if ! grep -q "Usage: scripts/line-count-report.sh" "$tmp_out"; then
    fail "invalid limit did not print usage"
fi
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
bash scripts/test-line-count-report.sh
```

Expected: FAIL with `scripts/line-count-report.sh is not executable`.

- [ ] **Step 3: Add the line-count report script**

Create `scripts/line-count-report.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/line-count-report.sh [limit]

Print the largest Rust files under apps/ and crates/.

Arguments:
  limit    Positive integer row limit. Defaults to 60.
EOF
}

limit="${1:-60}"

if [[ $# -gt 1 || "$limit" == "-h" || "$limit" == "--help" ]]; then
    usage
    exit 2
fi

case "$limit" in
    ''|*[!0-9]*)
        usage
        exit 2
        ;;
esac

if (( 10#$limit == 0 )); then
    usage
    exit 2
fi

printf 'lines\tpath\n'

find apps crates -type f -name '*.rs' -exec wc -l {} + \
    | awk '
        $NF == "total" { next }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            count = line
            sub(/[[:space:]].*$/, "", count)
            path = line
            sub(/^[0-9]+[[:space:]]+/, "", path)
            printf "%s\t%s\n", count, path
        }
    ' \
    | sort -rn -k1,1 \
    | awk -v limit="$limit" 'NR <= limit { print }'
```

- [ ] **Step 4: Make both scripts executable**

Run:

```bash
chmod +x scripts/line-count-report.sh scripts/test-line-count-report.sh
```

- [ ] **Step 5: Run the focused script test**

Run:

```bash
scripts/test-line-count-report.sh
```

Expected: PASS with no output.

- [ ] **Step 6: Inspect a short report**

Run:

```bash
scripts/line-count-report.sh 10
```

Expected: output starts with `lines	path` and lists the ten largest Rust files
under `apps/` and `crates/`.

- [ ] **Step 7: Commit Task 1**

```bash
git add scripts/line-count-report.sh scripts/test-line-count-report.sh
git commit -m "chore: add line count report"
```

## Task 2: Add Compact Discipline Guidance

**Files:**
- Modify: `AGENTS.md`
- Modify: `docs/llms/README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`

- [ ] **Step 1: Update `AGENTS.md` with refactor discipline**

Add this section after the commit-conventions paragraph in `## Coding Style,
Safety, and Commits` (the paragraph that begins `Recent history uses
conventional-style prefixes`) and before `## Testing and Documentation
Guidance`:

```markdown
## Maintainability & Refactor Discipline
Treat large files as review signals, not automatic failures. When a change adds
substantial behavior to a Rust file over 1000 lines, or adds or changes
behavior in a Rust file over 1500 lines, name the ownership boundary you are
preserving or improving before editing. Files over 2500 lines should get a
reviewed decomposition path before major feature work unless the task is an
urgent fix.

Refactor and pruning slices must say which behavior moves, which module owns it
afterward, which persisted state or public surface is affected, and which
focused test proves behavior stayed the same or changed intentionally. Do not
split files mechanically, keep command and route handlers thin, and update
`docs/llms/subsystem-map.md` or the relevant `docs/modules/*.md` file when the
"look here first" path changes.
```

- [ ] **Step 2: Update `docs/llms/README.md` frontmatter**

Change:

```markdown
last_updated: 2026-05-22
revision: 8
summary: GPT-5.5/Codex-first map with local bootstrap smoke routing
```

to:

```markdown
last_updated: 2026-06-06
revision: 9
summary: GPT-5.5/Codex-first map with maintainability packet routing and local bootstrap smoke routing
```

- [ ] **Step 3: Add maintenance packet routing to `docs/llms/README.md`**

Add this bullet group under `## Working Rules` after the existing rule that
mentions version-specific library behavior:

```markdown
- For maintainability, pruning, or refactor work, require the task packet to
  name the owning subsystem, the current large-file or stale-surface pressure,
  the intended new boundary, persisted-state impact, focused verification, and
  docs or subsystem-map updates.
- Use `scripts/line-count-report.sh` when a planning or review pass needs a
  fresh Rust hotspot snapshot. Treat the report as a prioritization aid, not a
  CI failure condition.
```

- [ ] **Step 4: Point the umbrella roadmap at the line-count script**

In
`docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`,
replace the paragraph that starts `This is an initial snapshot, not a canonical
inventory` with:

````markdown
This is an initial snapshot, not a canonical inventory. Child plans should
refresh these numbers before implementation with:

```bash
scripts/line-count-report.sh 30
```

If the script is unavailable in an older checkout, or a one-off shell refresh
is easier, use:

```bash
find apps crates -type f -name '*.rs' -exec wc -l {} + \
    | awk '$NF != "total" { print $1 "\t" $2 }' \
    | sort -rn -k1,1 \
    | awk 'NR <= 30'
```
````

- [ ] **Step 5: Add human-facing maintainability guidance to `CONTRIBUTING.md`**

Add this section after `### Rust Specifics` and before `### Commit Messages`:

```markdown
### Maintainability Slices

Refactor and cleanup PRs are welcome when they make ownership clearer. Keep
them focused: name the current responsibility, the module or helper that should
own it, and the focused verification command that proves behavior is preserved
or intentionally changed.

Large files are review signals. Use `scripts/line-count-report.sh` to refresh
the current hotspot list when planning broad maintenance work. Do not split a
file only to reduce line count; split when a responsibility has a clearer home.
Persisted state, package formats, trust metadata, and integration-test
manifests need explicit compatibility or migration decisions before they change.
```

- [ ] **Step 6: Update the PR template**

Replace `.github/PULL_REQUEST_TEMPLATE.md` with:

Note: the outer fence below uses 4 backticks because the template contains a
3-backtick `text` code block.

````markdown
## Summary

Brief description of what this PR does.

## Changes

-

## Ownership / Boundary

- Owning subsystem:
- Boundary changed or preserved:
- Persisted state or public surface impact:

## Verification

- [ ] Listed the exact verification commands run below
- [ ] Added or updated tests when behavior changed
- [ ] Ran affected-package verification directly when touching service or daemon code
- [ ] Updated subsystem docs or maps when the "look here first" path changed

```text
- cargo fmt --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test -p conary
```

## Related Issues / Plans

Closes #
Plan / Roadmap:
````

- [ ] **Step 7: Review the guidance for duplication**

Run:

```bash
grep -n -E "Maintainability|line-count|large files|Persisted state|look here first" AGENTS.md docs/llms/README.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md
```

Expected: matches are limited to the compact guidance added in this task and
existing related wording. If a paragraph repeats the same operational detail in
three files, keep the most durable form in `CONTRIBUTING.md` and shorten the
assistant-facing copy.

- [ ] **Step 8: Commit Task 2**

```bash
git add AGENTS.md docs/llms/README.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md
git commit -m "docs: define maintainability discipline"
```

## Task 3: Verify And Lock Phase 1 Implementation

**Files:**
- Modify if tracked docs inventory changed: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify if tracked docs claims changed: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Confirm the reviewed plan was locked before implementation**

If this plan is not already committed with docs-audit metadata, stop and run
`Pre-Implementation Lock-In For This Plan` before continuing. Implementation
should not start from an untracked plan file.

```bash
git ls-files --error-unmatch docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md >/dev/null
```

- [ ] **Step 2: Refresh docs-audit metadata for changed guidance docs**

The existing docs-audit tooling requires Bash 4+ and `rg`. This Phase 1 plan
keeps the new line-count report script independent of `rg`, but it does not
change the existing docs-audit tool requirements.

Refresh inventory if tracked doc paths changed:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Update `docs/superpowers/documentation-accuracy-audit-ledger.tsv` rows for any
tracked docs whose audited claim clusters, evidence sources, disposition, or
notes changed in Task 2. At minimum, review the rows for `AGENTS.md`,
`docs/llms/README.md`, `CONTRIBUTING.md`, `.github/PULL_REQUEST_TEMPLATE.md`,
and `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.
Do not rely on `check-doc-audit-ledger.sh` alone for claim freshness; it checks
path coverage and TSV shape, not semantic accuracy of notes.

- [ ] **Step 3: Run focused verification**

Run:

```bash
scripts/test-line-count-report.sh
scripts/line-count-report.sh 10
bash -n scripts/line-count-report.sh scripts/test-line-count-report.sh scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
git diff --check
```

Expected:

- `scripts/test-line-count-report.sh` exits 0 with no output.
- `scripts/line-count-report.sh 10` prints a header and ten Rust file rows.
- `bash -n ...` exits 0.
- Docs-audit ledger check passes.
- Both diff hygiene commands exit 0.

- [ ] **Step 4: Run stale guidance sweeps**

Run:

```bash
phase1_plan="docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md"
git ls-files --error-unmatch "$phase1_plan" >/dev/null

matches=0
for term in T''BD TO''DO FI''XME Cent''OS RH''EL "Debian sta""ble" open''SUSE Al''pine CLAU''DE Cla''ude "Open Review"" Questions"; do
    if {
        git diff -U0 -- AGENTS.md docs/llms/README.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md "$phase1_plan"
        git diff -U0 -- docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md
        git diff --cached -U0 -- AGENTS.md docs/llms/README.md CONTRIBUTING.md .github/PULL_REQUEST_TEMPLATE.md "$phase1_plan"
        git diff --cached -U0 -- docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md
    } | grep '^[+][^+]' | grep -F -n -- "$term"; then
        matches=1
    fi
done
test "$matches" -eq 0
```

Expected: no matches in changed hunks.

- [ ] **Step 5: Commit docs-audit metadata if needed**

If Step 2 changed docs-audit files, commit them:

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: update maintainability audit metadata"
```

If docs-audit files did not change, skip this commit.

- [ ] **Step 6: Report final evidence**

Report:

- Commit SHA or SHAs created.
- Verification commands run.
- `git status --short --branch`.
- `git rev-list --left-right --count HEAD...origin/main` if the branch is
  expected to be synchronized with `origin/main`.
- Any unexpected changes.

## Final Verification Gate

Before the Phase 1 implementation is considered complete:

```bash
scripts/test-line-count-report.sh
scripts/line-count-report.sh 10
bash -n scripts/line-count-report.sh scripts/test-line-count-report.sh scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
git diff --check
git status --short --branch
```

Also rerun the stale guidance sweep from Task 3 Step 4 before the final commit.

If Task 2 is docs-only and Task 1 is the only script behavior change, do not run
full Cargo workspace tests for this phase. Use the focused script test and docs
gates above.

## Review Checklist

- Does the plan keep the root guidance map-like?
- Does it avoid creating a new manual before the repo proves one is needed?
- Does it distinguish persisted-state compatibility from internal Rust API
  flexibility?
- Does the script avoid requiring `rg`?
- Does every implementation task have a focused verification command?
- Does the plan avoid hotspot refactors and CCS-native work?
- Does it make later Phase 2 pruning and Phase 4 decomposition easier to start?
