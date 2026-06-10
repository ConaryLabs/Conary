# Project Maintainability Phase 7 Enforcement And Drift Control Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` or `superpowers:executing-plans`
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for
> tracking. This is the Phase 7 child packet under
> `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Add a cheap, warn-only maintainability drift report that maps changed
paths to owner hints, focused proof commands, docs-audit checks, and current
Rust hotspot data without turning maintainability guidance into a noisy gate.

**Architecture:** Add one local Bash report script that composes existing
repo-native signals: changed paths from Git, Rust hotspot data from
`scripts/line-count-report.sh`, docs-audit health from existing audit scripts,
and feature ownership hints from the Phase 5/6 feature map. Add a focused test
script for report shape and usage behavior, then route contributor and assistant
docs to the new report as a planning/review aid.

**Tech Stack:** Bash, Git, existing docs-audit scripts, existing line-count
report script, Markdown docs, existing docs-audit ledger.

---

## Status

Reviewed and locked in.

Phase 7 should keep the maintainability reset from decaying, but the first
slice must stay intentionally light. The repo is still in active development,
and a warning that explains the next command is more useful than a hard gate
that blocks delivery for a line-count or map-routing preference.

## Read First

- `AGENTS.md`
- `CONTRIBUTING.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/test-fixtures.md`
- `docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `scripts/line-count-report.sh`
- `scripts/test-line-count-report.sh`
- `scripts/docs-audit-inventory.sh`
- `scripts/check-doc-audit-ledger.sh`

## Design Summary

The first Phase 7 artifact should be a local report, not an enforcement wall.
It should be easy to paste into a planning session or run before a refactor PR:

```bash
scripts/maintainability-drift-report.sh
```

The report should answer four questions:

- Which changed paths look feature-owned, and which focused proof command should
  the worker consider?
- Which changed paths imply a broader interaction gate?
- Are docs-audit metadata and the docs inventory still aligned?
- What are the current largest Rust files, so hotspot pressure is visible while
  planning without creating a failure threshold?

The script should exit 0 when it finds maintainability warnings. It should exit
non-zero only for usage errors or a broken invocation. That keeps it useful in
local development and review prompts while avoiding noisy CI semantics.

## Current Repo-Grounded Inputs

| Signal | Current owner | Phase 7 use |
|--------|---------------|-------------|
| Rust hotspots | `scripts/line-count-report.sh` | Embed top rows as planning context |
| Docs-audit inventory | `scripts/docs-audit-inventory.sh` | Warn when inventory output differs |
| Docs-audit completeness | `scripts/check-doc-audit-ledger.sh` | Warn when the ledger is stale or incomplete |
| Feature ownership | `docs/modules/feature-ownership.md` | Keep script hints aligned with current cards |
| Contributor workflow | `CONTRIBUTING.md` | Route humans to the report for broad maintenance slices |
| Assistant routing | `docs/llms/README.md` | Route agents to the report when a path-scoped task spans owners |

## Non-Goals

- Do not add a CI job in the first slice.
- Do not fail builds because a file crosses a line-count threshold.
- Do not fail builds because changed paths have feature-map warnings.
- Do not require network access.
- Do not parse Markdown as a policy language.
- Do not replace focused tests, docs-audit checks, or code review.
- Do not add license-policy changes, live-mutation UX changes, or CCS-native
  package contract work to this packet.
- Do not claim complete changed-path coverage for every `crates/conary-core`
  subsystem in the first slice. Recipe, derivation, transaction, trust, and
  other areas without first-class feature cards may receive the default
  no-hint message until follow-up cards and patterns are added.

## Review Focus

Reviewers should check:

- whether the script remains warn-only for drift findings;
- whether changed-path patterns match real repo directories;
- whether the suggested proof commands exist and are not misleadingly narrow;
- whether docs-audit warnings reuse the existing audit scripts rather than
  inventing a second metadata model;
- whether docs routing stays concise and avoids turning `CONTRIBUTING.md` or
  `docs/llms/README.md` into duplicate manuals;
- whether the plan avoids new unsupported platform or distro claims.

## Implementation Plan

### Task 0: Lock The Reviewed Phase 7 Plan And Docs-Audit Row

**Files:**
- Add: `docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the reviewed plan before regenerating docs inventory**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md
```

- [ ] **Step 2: Regenerate docs-audit inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected on the current baseline: tracked doc-like files grow from 148 to 149,
with this plan file added as `planning` / `maintainer`. If another docs file
lands first, use the regenerated inventory as source of truth and update counts
accordingly.

- [ ] **Step 3: Add the plan ledger row**

Add this literal-tab row to
`docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other active
maintainability plan rows:

```text
docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md	docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md	planning	maintainer	maintainability; phase7; drift-control; verification-hints; warn-only	scripts/line-count-report.sh; scripts/test-line-count-report.sh; scripts/docs-audit-inventory.sh; scripts/check-doc-audit-ledger.sh; docs/modules/feature-ownership.md; docs/llms/README.md; CONTRIBUTING.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md	verified	corrected	Added the reviewed Phase 7 plan for a warn-only maintainability drift report that maps changed paths to owner hints, focused proof commands, docs-audit health, and Rust hotspot context without adding a hard CI gate.
```

- [ ] **Step 4: Update the audit summary for the active Phase 7 plan**

Append this paragraph to the existing
`### 2026-06-06 Maintainability Planning` section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The Phase 7 enforcement and drift-control plan now opens the final
maintainability-roadmap lane. It scopes the first slice to a warn-only local
drift report that maps changed paths to feature ownership hints, focused proof
commands, docs-audit health, and current Rust hotspot context without adding a
hard CI gate or line-count threshold.
```

Then update the final counts to:

```markdown
## Final Counts

- Total tracked doc-like files audited: 149
- `verified-no-change`: 13
- `corrected`: 49
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so its evidence and
notes mention the Phase 7 planning update.

- [ ] **Step 5: Verify docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --cached --check
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 6: Commit the reviewed plan lock-in**

```bash
git add docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan maintainability drift control"
```

### Task 1: Add The Warn-Only Drift Report Script

**Files:**
- Add: `scripts/maintainability-drift-report.sh`
- Add: `scripts/test-maintainability-drift-report.sh`

- [ ] **Step 1: Create the report script**

Create `scripts/maintainability-drift-report.sh` with this content:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/maintainability-drift-report.sh [--base <ref>] [--limit <n>] [--all]

Print a warn-only maintainability report.

Options:
  --base <ref>   Compare changed paths against this ref. Defaults to HEAD.
  --limit <n>    Number of Rust hotspot rows to show. Defaults to 15.
  --all          Report over all tracked files instead of changed paths.
  -h, --help     Show this help.
EOF
}

base_ref="HEAD"
limit="15"
scan_all=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            base_ref="$2"
            shift 2
            ;;
        --limit)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            limit="$2"
            shift 2
            ;;
        --all)
            scan_all=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

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

if [[ "$scan_all" -eq 0 ]] && ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
    echo "ERROR: base ref not found: $base_ref" >&2
    exit 2
fi

section() {
    printf '\n## %s\n' "$1"
}

warn() {
    printf '[warn] %s\n' "$1"
}

feature_hint_for_path() {
    local path="$1"

    case "$path" in
        crates/conary-agent-contract/*|crates/conary-mcp/*|apps/remi/src/server/mcp.rs|apps/conary-test/src/server/mcp.rs)
            printf 'Agent/MCP operation surfaces | focused: cargo test -p conary-agent-contract; cargo test -p conary-mcp | gate: owning service tests when adapter calls service behavior'
            ;;
        apps/conary/src/commands/install/*|apps/conary/src/commands/update.rs|apps/conary/src/commands/remove.rs)
            printf 'Native package install/update/remove | focused: cargo test -p conary --test live_host_mutation_safety; cargo test -p conary --test bundle_replay | gate: cargo test -p conaryd daemon::routes when daemon jobs change'
            ;;
        apps/conary/src/commands/adopt/*)
            printf 'Adoption and native-authority handoff | focused: cargo test -p conary --lib adopt::native_handoff; cargo test -p conary --lib adopt::unadopt | gate: cargo run -p conary-test -- list; selected handoff suite when behavior changes'
            ;;
        crates/conary-core/src/generation/*)
            printf 'Generation build/switch/export | focused: cargo test -p conary-core generation::export; cargo test -p conary-core generation::builder | gate: generation export conary-test suites when boot-carrier behavior changes'
            ;;
        crates/conary-core/src/ccs/*|apps/conary/src/commands/ccs/*)
            printf 'CCS authoring/conversion/install/replay | focused: cargo test -p conary-core golden_fixtures; cargo test -p conary-core support_matrix | gate: cargo test -p conary --test conversion_integration golden_conversion; cargo test -p remi publication when serving changes'
            ;;
        apps/remi/src/server/*|apps/remi/src/bin/remi.rs|apps/remi/src/trust.rs)
            printf 'Remi publication/serving/admin | focused: cargo test -p remi publication; cargo test -p remi test_upload_fixture | gate: cargo test -p remi'
            ;;
        apps/remi/src/federation/*)
            printf 'Remi federation | focused: cargo test -p remi | gate: docs/modules/federation.md and feature-card update if federation becomes a first-class ownership card'
            ;;
        apps/conaryd/*)
            printf 'conaryd jobs/routes | focused: cargo test -p conaryd daemon::routes | gate: cargo test -p conaryd'
            ;;
        apps/conary/src/commands/bootstrap/*|apps/conary-test/src/bootstrap.rs|crates/conary-bootstrap/*|docs/modules/bootstrap.md|docs/operations/bootstrap-selfhosting-vm.md|docs/operations/bootstrap-follow-up-investigations.md)
            printf 'Bootstrap and self-hosting | focused: cargo run -p conary-test -- bootstrap check --json; cargo run -p conary-test -- bootstrap smoke --dry-run --json | gate: cargo run -p conary-test -- bootstrap smoke --json when building local images or changing host requirements'
            ;;
        apps/conary-test/*|apps/conary/tests/integration/remi/manifests/*)
            printf 'conary-test integration execution | focused: cargo run -p conary-test -- list; cargo test -p conary-test suite_inventory | gate: touched suite command from docs/modules/feature-ownership.md'
            ;;
        AGENTS.md|CONTRIBUTING.md|.github/PULL_REQUEST_TEMPLATE.md|docs/llms/*|docs/modules/feature-ownership.md)
            printf 'Assistant/contributor guidance | focused: bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete | gate: docs-audit inventory diff and stale-term added-line sweep'
            ;;
        docs/modules/*|docs/operations/*|docs/INTEGRATION-TESTING.md|docs/ARCHITECTURE.md)
            printf 'Canonical docs | focused: docs-audit ledger and inventory checks | gate: affected feature card proof if behavior claims changed'
            ;;
        docs/superpowers/plans/*)
            printf 'Planning docs | focused: docs-audit ledger and inventory checks | gate: agentic review before lock-in'
            ;;
        *)
            return 1
            ;;
    esac
}

collect_paths() {
    if [[ "$scan_all" -eq 1 ]]; then
        git ls-files
        return
    fi

    {
        git diff --name-only "$base_ref" --
        git diff --cached --name-only --
        git ls-files --others --exclude-standard
    } | awk 'NF' | sort -u
}

printf '# Maintainability Drift Report\n'
printf 'base_ref: %s\n' "$base_ref"
printf 'mode: %s\n' "$([[ "$scan_all" -eq 1 ]] && printf all || printf changed)"

section "Docs Audit Health"
ledger_out="$(mktemp)"
inventory_out="$(mktemp)"
trap 'rm -f "$ledger_out" "$inventory_out"' EXIT

if bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete >"$ledger_out" 2>&1; then
    printf '[ok] docs-audit ledger complete\n'
else
    warn "docs-audit ledger check reported an issue"
    sed 's/^/  /' "$ledger_out"
fi

if bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv - >"$inventory_out" 2>&1; then
    printf '[ok] docs-audit inventory matches regenerated output\n'
else
    warn "docs-audit inventory differs from regenerated output"
    sed 's/^/  /' "$inventory_out"
fi

section "Changed Path Hints"
mapfile -t changed_paths < <(collect_paths)

if [[ "${#changed_paths[@]}" -eq 0 ]]; then
    printf '[ok] no changed paths detected\n'
else
    printf 'changed_paths: %s\n' "${#changed_paths[@]}"
    for path in "${changed_paths[@]}"; do
        if hint="$(feature_hint_for_path "$path")"; then
            printf -- '- %s\n  %s\n' "$path" "$hint"
        else
            printf -- '- %s\n  No feature-card hint matched. Use the owning package tests and update docs/modules/feature-ownership.md if this should be routed.\n' "$path"
        fi
    done
fi

section "Rust Hotspots"
if [[ -x scripts/line-count-report.sh ]]; then
    scripts/line-count-report.sh "$limit"
else
    warn "scripts/line-count-report.sh is missing or not executable"
fi

section "Reminder"
printf 'This report is warn-only. Follow the focused proof and interaction gate from docs/modules/feature-ownership.md when the touched behavior crosses a neighbor system.\n'
```

- [ ] **Step 2: Make the report script executable**

```bash
chmod +x scripts/maintainability-drift-report.sh
```

- [ ] **Step 3: Create the test script**

Create `scripts/test-maintainability-drift-report.sh` with this content:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

[[ -x scripts/maintainability-drift-report.sh ]] || fail "scripts/maintainability-drift-report.sh is not executable"

tmp_out="$(mktemp)"
tmp_path=""
trap 'rm -f "$tmp_out" "$tmp_path"' EXIT

help_output="$(scripts/maintainability-drift-report.sh --help 2>&1)"
grep -q "Usage: scripts/maintainability-drift-report.sh" <<<"$help_output" \
    || fail "help output did not include usage"

if scripts/maintainability-drift-report.sh --limit not-a-number >"$tmp_out" 2>&1; then
    fail "invalid limit unexpectedly succeeded"
fi
grep -q "Usage: scripts/maintainability-drift-report.sh" "$tmp_out" \
    || fail "invalid limit did not print usage"

if scripts/maintainability-drift-report.sh --base definitely-not-a-real-ref >"$tmp_out" 2>&1; then
    fail "invalid base ref unexpectedly succeeded"
fi
grep -q "base ref not found" "$tmp_out" \
    || fail "invalid base ref did not print a clear error"

all_output="$(scripts/maintainability-drift-report.sh --all --limit 3)"
grep -q "# Maintainability Drift Report" <<<"$all_output" \
    || fail "report header missing"
grep -q "## Docs Audit Health" <<<"$all_output" \
    || fail "docs audit section missing"
grep -q "## Changed Path Hints" <<<"$all_output" \
    || fail "changed path section missing"
grep -q "## Rust Hotspots" <<<"$all_output" \
    || fail "hotspot section missing"
grep -q $'lines\tpath' <<<"$all_output" \
    || fail "hotspot table missing"
grep -q "Agent/MCP operation surfaces" <<<"$all_output" \
    || fail "all-path report did not include Agent/MCP hint"
grep -q "Bootstrap and self-hosting" <<<"$all_output" \
    || fail "all-path report did not include Bootstrap hint"
grep -q "conaryd jobs/routes" <<<"$all_output" \
    || fail "all-path report did not include conaryd hint"
grep -q "Remi federation" <<<"$all_output" \
    || fail "all-path report did not include Remi federation hint"

tmp_path="$(mktemp docs/modules/drift-report-test.XXXXXX.tmp)"
printf 'temporary drift report test fixture\n' > "$tmp_path"

changed_output="$(scripts/maintainability-drift-report.sh --limit 3)"
grep -q "$tmp_path" <<<"$changed_output" \
    || fail "changed report did not include untracked test path"
grep -q "Canonical docs" <<<"$changed_output" \
    || fail "docs path did not receive canonical docs hint"
```

- [ ] **Step 4: Make the test script executable**

```bash
chmod +x scripts/test-maintainability-drift-report.sh
```

- [ ] **Step 5: Run the focused script tests**

```bash
bash scripts/test-maintainability-drift-report.sh
scripts/maintainability-drift-report.sh --all --limit 5
```

Expected: both commands exit 0. The report command prints docs-audit status,
changed path hints for all tracked paths, and five Rust hotspot rows.

### Task 2: Route The Drift Report Through Contributor And Assistant Docs

**Files:**
- Modify: `CONTRIBUTING.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Update contributor maintainability guidance**

In `CONTRIBUTING.md`, add this paragraph after the existing
`scripts/line-count-report.sh` paragraph in `### Maintainability Slices`:

```markdown
Before a broad refactor or cleanup PR, run
`scripts/maintainability-drift-report.sh` for a warn-only view of changed-path
owner hints, focused proof commands, docs-audit health, and current Rust
hotspots. Treat its output as review guidance, not as a substitute for the
feature card or the tests you actually ran.
```

- [ ] **Step 2: Update assistant routing**

In `docs/llms/README.md`, add this bullet after the existing
`scripts/line-count-report.sh` working rule:

```markdown
- Use `scripts/maintainability-drift-report.sh` before broad feature,
  refactor, or docs-routing changes to get warn-only changed-path owner hints,
  docs-audit status, and current hotspot context.
```

If the frontmatter summary no longer names drift-control routing after this
edit, update it to:

```yaml
summary: GPT-5.5/Codex-first map with feature ownership, bootstrap smoke, and drift-control routing
```

- [ ] **Step 3: Update docs-audit rows**

Update these literal-tab rows in
`docs/superpowers/documentation-accuracy-audit-ledger.tsv`:

```text
CONTRIBUTING.md	CONTRIBUTING.md	root	contributor	contributing; development; verification; feature-ownership; drift-control	AGENTS.md; docs/llms/README.md; docs/modules/feature-ownership.md; docs/INTEGRATION-TESTING.md; docs/operations/infrastructure.md; Cargo.toml; scripts/line-count-report.sh; scripts/maintainability-drift-report.sh	verified	corrected	Refreshed contributor workflow guidance with feature ownership map routing, focused proof expectations, broader interaction gates, line-count reporting, and warn-only maintainability drift reporting.
docs/llms/README.md	docs/llms/README.md	canonical	contributor	assistant-guidance; llm-map; feature-ownership; drift-control	AGENTS.md; docs/ARCHITECTURE.md; docs/INTEGRATION-TESTING.md; docs/modules/feature-ownership.md; docs/modules/test-fixtures.md; docs/operations/infrastructure.md; scripts/docs-audit-inventory.sh; scripts/check-doc-audit-ledger.sh; scripts/line-count-report.sh; scripts/maintainability-drift-report.sh	verified	corrected	Refreshed assistant routing to include feature ownership cards, interaction gates, line-count reporting, and warn-only maintainability drift reporting while preserving AGENTS.md as the repo-wide assistant contract.
docs/superpowers/documentation-accuracy-audit-summary.md	docs/superpowers/documentation-accuracy-audit-summary.md	planning	maintainer	audit-summary; verification; release-hardening; active-planning; maintainability; feature-ownership; drift-control	docs/superpowers/documentation-accuracy-audit-ledger.tsv; docs/superpowers/documentation-accuracy-audit-inventory.tsv; scripts/check-doc-audit-ledger.sh; scripts/docs-audit-inventory.sh; scripts/line-count-report.sh; scripts/maintainability-drift-report.sh; ROADMAP.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-agent-readiness-roadmap.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase1-repo-discipline-contract-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase2-dead-surface-pruning-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase3-test-fixture-discipline-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase4-install-hotspot-decomposition-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase5-6-feature-ownership-workflow-plan.md; docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md; docs/modules/test-fixtures.md; docs/modules/feature-ownership.md; docs/llms/README.md; docs/llms/subsystem-map.md; docs/modules/ccs.md; docs/modules/remi.md; CONTRIBUTING.md; .github/PULL_REQUEST_TEMPLATE.md; docs/superpowers/project-maintainability-dead-surface-inventory-2026-06-06.md	verified	corrected	Refreshed the audit summary for the active maintainability planning lane, current docs-audit counts, Phase 1 through Phase 7 maintainability packets, feature ownership workflow routing, and warn-only maintainability drift reporting.
```

- [ ] **Step 4: Update the audit summary implementation note**

Append this paragraph to the maintainability planning section in
`docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
The first Phase 7 implementation slice adds
`scripts/maintainability-drift-report.sh` as a warn-only local report for
changed-path owner hints, docs-audit health, focused proof suggestions, and
current Rust hotspot context. Contributor and assistant guidance now point to
the report as review support for broad maintenance work, not as a hard gate.
```

Counts stay unchanged after this task because it modifies existing tracked
doc-like files and adds scripts outside the docs-audit inventory.

- [ ] **Step 5: Verify docs-audit and docs diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

### Task 3: Final Verification And Commit The Slice

**Files:**
- Add: `scripts/maintainability-drift-report.sh`
- Add: `scripts/test-maintainability-drift-report.sh`
- Modify: `CONTRIBUTING.md`
- Modify: `docs/llms/README.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Run focused script verification**

```bash
bash scripts/test-maintainability-drift-report.sh
scripts/maintainability-drift-report.sh --all --limit 5
```

Expected: both commands exit 0. The report prints docs-audit status, changed
path hints, Rust hotspots, and the warn-only reminder.

- [ ] **Step 2: Run docs-audit and diff hygiene**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
git diff --cached --check
```

Expected: all commands exit 0.

- [ ] **Step 3: Run lightweight workspace sanity**

```bash
cargo fmt --check
```

Expected: exits 0. No Rust code changes are expected.

- [ ] **Step 4: Run docs-only search checks**

```bash
rg -n "maintainability-drift-report|drift report|warn-only" CONTRIBUTING.md docs/llms/README.md docs/superpowers/documentation-accuracy-audit-summary.md scripts/maintainability-drift-report.sh scripts/test-maintainability-drift-report.sh
rg -n 'T''BD|TO''DO|FI''XME|Cent''OS|RH''EL|Deb''ian stable|open''SUSE|Al''pine|CLAU''DE|Cla''ude|Open Review'' Questions' docs/superpowers/plans/archive/2026-06-06-project-maintainability-phase7-enforcement-drift-control-plan.md CONTRIBUTING.md docs/llms/README.md
```

Expected: the first command shows intentional references. The second command
has no matches.

- [ ] **Step 5: Review staged diff**

```bash
git status --short --branch
git diff --stat
git diff --cached --stat
```

Expected: only the Phase 7 docs, scripts, and audit metadata are staged.

- [ ] **Step 6: Commit the Phase 7 implementation slice**

```bash
git add scripts/maintainability-drift-report.sh scripts/test-maintainability-drift-report.sh CONTRIBUTING.md docs/llms/README.md docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: add maintainability drift report"
```

Expected: commit includes the warn-only report script, its focused test, and
the contributor/assistant routing docs.

- [ ] **Step 7: Stop for review**

After the implementation slice lands, stop. Do not add CI, hard thresholds,
license simplification, live-system mutation acknowledgement UX changes, or CCS
native package work until each has its own reviewed child plan.

## Final Verification For The Whole Packet

Run:

```bash
bash scripts/test-maintainability-drift-report.sh
scripts/maintainability-drift-report.sh --all --limit 5
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
cargo fmt --check
```

Expected: all commands exit 0.

Then verify:

```bash
git status --short --branch
git rev-list --left-right --count HEAD...origin/main
```

Expected after commit and push: clean `main`, `0 0` ahead/behind.
