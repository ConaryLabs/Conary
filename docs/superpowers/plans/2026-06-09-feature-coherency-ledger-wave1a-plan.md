# Feature Coherency Ledger Wave 1a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the feature coherency ledger and validator, then execute Wave 1a against root CLI help, examples, dispatch coverage, and generated manpage behavior.

**Architecture:** Keep the existing documentation accuracy audit ledger as the tracked Markdown inventory gate, and add a separate feature coherency ledger for implementation-to-claim surfaces. The first implementation slice creates `docs/superpowers/feature-coherency-ledger.tsv`, validates it with a focused shell script and tests, captures root CLI evidence into temporary files, then records or repairs Wave 1a rows until no selected-scope `fix-now`, `misleading`, or `duplicate-stale` row remains open.

**Tech Stack:** Bash, TSV, Cargo, Clap root help, `clap_mangen` via `apps/conary/build.rs`, docs-audit scripts, `docs/modules/feature-ownership.md`.

---

## Current Repository Facts

- Repository root: `/home/peter/Conary`.
- Current pushed `HEAD` and `origin/main`: `4468765fc729943e5e00ea529e8c64bc531b5a79`.
- Local plan date from `date +%F`: `2026-06-09`.
- `docs/superpowers/specs/2026-06-09-feature-coherency-ledger-design.md` defines the ledger columns, closure matrix, source-pointer grammar, Wave 1a scope, and verification gates.
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv` remains the tracked Markdown inventory and documentation truth ledger.
- `scripts/check-doc-audit-ledger.sh` and `scripts/docs-audit-inventory.sh` are active PR-gate inputs.
- `scripts/check-coherency-ledger.sh`, `scripts/test-coherency-ledger.sh`, and `docs/superpowers/feature-coherency-ledger.tsv` do not exist yet.
- `apps/conary/build.rs` writes ignored local generated manpage output to `apps/conary/man/conary.1` via `clap_mangen` during `cargo build -p conary`.
- `git ls-files apps/conary/man/conary.1 man/conary.1` returns no tracked manpage files today; generated manpage output is local evidence, not a committed artifact.
- `cargo run -p conary -- --help` currently renders root help and daily workflow examples successfully.
- `cargo test -p conary --lib cli::tests -- --list` currently lists 25 CLI unit tests.

## Non-Goals

- Do not audit every active doc, conaryd route, Remi route, MCP tool, or agent contract in Wave 1a.
- Do not add HTTP, MCP, or conaryd rows unless a selected root CLI surface directly advertises or depends on them.
- Do not commit generated manpage files under ignored `apps/conary/man/` or `/man/`.
- Do not leave selected-scope `fix-now`, `misleading`, or `duplicate-stale` rows open at the end of Wave 1a.
- Do not replace `docs/superpowers/documentation-accuracy-audit-ledger.tsv`.
- Do not hide untriaged findings in the durable coherency ledger. Use temporary scratch files until a finding has owner, status, decision, next slice, and verification.

## File Responsibility Map

| File | Responsibility |
| --- | --- |
| `docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md` | This implementation plan and execution checklist. |
| `docs/superpowers/feature-coherency-ledger.tsv` | Durable implementation-to-claim coherency ledger. |
| `scripts/check-coherency-ledger.sh` | Validates ledger header, IDs, status/disposition/decision values, closure matrix, source pointers, dates, and scope-completion rules. |
| `scripts/test-coherency-ledger.sh` | Shell tests for the coherency ledger validator using temporary ledger fixtures. |
| `docs/superpowers/documentation-accuracy-audit-inventory.tsv` | Existing tracked-doc inventory, updated because this plan is a new tracked Markdown file. |
| `docs/superpowers/documentation-accuracy-audit-ledger.tsv` | Existing tracked-doc ledger, updated with this plan row. |
| `docs/superpowers/documentation-accuracy-audit-summary.md` | Summary counts and short active-planning note for this plan. |
| `apps/conary/src/cli/mod.rs` | Root Clap command definition, examples, root help source. Read during Wave 1a; edit only if evidence shows a bounded root-help repair. |
| `apps/conary/src/dispatch.rs` and `apps/conary/src/dispatch/` | Top-level dispatch routing. Read during Wave 1a; edit only if root command routing evidence shows a bounded repair. |
| `apps/conary/build.rs` | Generated manpage source. Read during Wave 1a; edit only if manpage generation itself is broken. |

## Ledger Header

The ledger header must be exactly:

```tsv
id	surface	source	related_ids	wave_scope	owner	claim	actual_or_gap	status	disposition	last_verified	evidence_sources	repro	verification	decision	next_slice	notes
```

## Allowed Values

Statuses:

```text
works
works-but-thin
fix-now
honest-deferred
misleading
duplicate-stale
```

Dispositions:

```text
open
verified-no-change
resolved-repaired
resolved-removed
resolved-merged
deferred-owned
```

Decisions:

```text
fix
defer
remove
merge
harden
verify
```

Closure matrix:

```text
works -> open, verified-no-change, resolved-repaired
works-but-thin -> open, resolved-repaired, verified-no-change, deferred-owned
fix-now -> open, resolved-repaired, resolved-removed, resolved-merged
misleading -> open, resolved-repaired, resolved-removed, resolved-merged
duplicate-stale -> open, resolved-merged, resolved-removed, resolved-repaired
honest-deferred -> open, deferred-owned
```

ID shape:

```text
{TYPE}-{SUBSYSTEM}-{NNN}
```

Allowed ID type prefixes for Wave 1a:

```text
CLI
DOC
ROUTE
MCP
AGENT
OPS
```

Accepted evidence pointer prefixes:

```text
path:apps/conary/src/cli/mod.rs:1-40
doc:README.md:120-130
cmd:cargo run -p conary -- --help
test:cargo test -p conary --lib cli::tests
route:GET /v1/transactions
mcp:remi/tool-name
```

---

### Task 0: Lock In The Plan Packet

**Files:**
- Create: `docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Stage the new plan before regenerating inventory**

`scripts/docs-audit-inventory.sh` reads `git ls-files`, so stage the new plan first.

Run:

```bash
git add docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Expected:

```text
169
```

- [ ] **Step 2: Add the inventory row**

Add this row to `docs/superpowers/documentation-accuracy-audit-inventory.tsv` in sorted order near the other active plans:

```tsv
docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md	planning	maintainer
```

- [ ] **Step 3: Add the docs-audit ledger row**

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the active planning rows:

```tsv
docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md	docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md	planning	maintainer	feature-coherency; wave1a; cli-root-help; implementation-plan	docs/superpowers/specs/2026-06-09-feature-coherency-ledger-design.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md; apps/conary/src/cli/mod.rs; apps/conary/src/dispatch.rs; apps/conary/src/dispatch/; apps/conary/build.rs; scripts/check-doc-audit-ledger.sh; scripts/docs-audit-inventory.sh; scripts/check-doc-truth.sh	verified	corrected	Added the Wave 1a implementation plan for creating the feature coherency ledger, adding a ledger validator, auditing root CLI help/examples/dispatch/generated-manpage behavior, and closing selected-scope findings through repair, merge, removal, verified no-change, or honest deferral.
```

- [ ] **Step 4: Update the docs-audit summary counts and note**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, update the final counts to:

```markdown
- Total tracked doc-like files audited: 169
- `verified-no-change`: 12
- `corrected`: 70
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

Add this paragraph near the active maintainability planning paragraphs:

```markdown
The feature coherency Wave 1a implementation plan starts the reviewed
coherency ledger program with root CLI help, examples, top-level dispatch, and
generated manpage behavior only. It keeps the existing documentation accuracy
audit as the tracked Markdown gate while adding a separate implementation-to-
claim ledger and validator.
```

- [ ] **Step 5: Verify the plan packet docs-audit metadata**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check -- docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
```

Expected:

```text
Documentation audit ledger check passed (--require-complete).
```

The inventory diff and `git diff --check` commands should produce no output.

- [ ] **Step 6: Commit the plan packet**

Run:

```bash
git status --short
git add docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan feature coherency wave 1a"
```

Expected:

```text
[main <sha>] docs: plan feature coherency wave 1a
```

---

### Task 1: Add Coherency Ledger Validator Tests

**Files:**
- Create: `scripts/test-coherency-ledger.sh`
- Later modify in Task 2: `scripts/check-coherency-ledger.sh`

- [ ] **Step 1: Create the failing test script**

Create `scripts/test-coherency-ledger.sh` with this content:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

validator="scripts/check-coherency-ledger.sh"
header=$'id\tsurface\tsource\trelated_ids\twave_scope\towner\tclaim\tactual_or_gap\tstatus\tdisposition\tlast_verified\tevidence_sources\trepro\tverification\tdecision\tnext_slice\tnotes'

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

write_ledger() {
    local path="$1"
    shift
    {
        printf '%s\n' "$header"
        printf '%s\n' "$@"
    } > "$path"
}

good_row=$'CLI-ROOT-001\tconary root help\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help lists top-level commands and daily examples\tRoot help renders and examples are visible\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tverify\tWave 1a root help evidence captured\tInitial root help row'

good="$tmpdir/good.tsv"
write_ledger "$good" "$good_row"
"$validator" "$good" >/dev/null
"$validator" "$good" --scope-complete 1a-root-cli >/dev/null

bad_status="$tmpdir/bad-status.tsv"
write_ledger "$bad_status" "${good_row/works/not-real-status}"
if "$validator" "$bad_status" >"$tmpdir/out" 2>&1; then
    fail "invalid status unexpectedly passed"
fi
grep -q "invalid status" "$tmpdir/out" || fail "invalid status error was not clear"

bad_pointer="$tmpdir/bad-pointer.tsv"
write_ledger "$bad_pointer" "${good_row/path:apps\/conary\/src\/cli\/mod.rs/path:no-such-file.rs}"
if "$validator" "$bad_pointer" >"$tmpdir/out" 2>&1; then
    fail "missing path pointer unexpectedly passed"
fi
grep -q "referenced path does not exist" "$tmpdir/out" || fail "missing path error was not clear"

missing_gap="$tmpdir/missing-gap.tsv"
misleading_row=$'CLI-ROOT-002\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\t\tmisleading\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tfix\tFix or remove misleading example\tMissing gap should fail'
write_ledger "$missing_gap" "$misleading_row"
if "$validator" "$missing_gap" >"$tmpdir/out" 2>&1; then
    fail "misleading row without actual_or_gap unexpectedly passed"
fi
grep -q "actual_or_gap is required" "$tmpdir/out" || fail "missing actual_or_gap error was not clear"

scope_open="$tmpdir/scope-open.tsv"
open_misleading=$'CLI-ROOT-003\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tExample reaches a dead end\tmisleading\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tfix\tFix root example\tOpen misleading row should block scope completion'
write_ledger "$scope_open" "$open_misleading"
"$validator" "$scope_open" >/dev/null
if "$validator" "$scope_open" --scope-complete 1a-root-cli >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed open misleading row"
fi
grep -q "scope completion blocked" "$tmpdir/out" || fail "scope completion error was not clear"

bad_matrix="$tmpdir/bad-matrix.tsv"
matrix_row=$'CLI-ROOT-004\tconary root help duplicate\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tDuplicate root path should be merged\tTwo root surfaces overlap\tduplicate-stale\tverified-no-change\t2026-06-09\tcmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tmerge\tMerge duplicate root surface\tBad matrix should fail'
write_ledger "$bad_matrix" "$matrix_row"
if "$validator" "$bad_matrix" >"$tmpdir/out" 2>&1; then
    fail "invalid status/disposition matrix unexpectedly passed"
fi
grep -q "invalid disposition" "$tmpdir/out" || fail "bad matrix error was not clear"

echo "Coherency ledger validator tests passed."
```

- [ ] **Step 2: Make the test script executable**

Run:

```bash
chmod +x scripts/test-coherency-ledger.sh
```

- [ ] **Step 3: Run the test to verify it fails before implementation**

Run:

```bash
scripts/test-coherency-ledger.sh
```

Expected:

```text
scripts/test-coherency-ledger.sh: line ...: scripts/check-coherency-ledger.sh: No such file or directory
```

- [ ] **Step 4: Commit the failing test**

Run:

```bash
git add scripts/test-coherency-ledger.sh
git commit -m "test: add coherency ledger validator coverage"
```

Expected:

```text
[main <sha>] test: add coherency ledger validator coverage
```

---

### Task 2: Implement The Coherency Ledger Validator

**Files:**
- Create: `scripts/check-coherency-ledger.sh`
- Test: `scripts/test-coherency-ledger.sh`

- [ ] **Step 1: Create the validator script**

Create `scripts/check-coherency-ledger.sh` with this content:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

usage() {
    cat >&2 <<'EOF'
Usage: scripts/check-coherency-ledger.sh <ledger-path> [--scope-complete <wave-scope>]
EOF
    exit 2
}

if [[ $# -ne 1 && $# -ne 3 ]]; then
    usage
fi

ledger_path="$1"
scope_complete=""

if [[ $# -eq 3 ]]; then
    [[ "$2" == "--scope-complete" ]] || usage
    scope_complete="$3"
    [[ -n "$scope_complete" ]] || usage
fi

[[ -f "$ledger_path" ]] || fail "ledger file not found: $ledger_path"

expected_header=$'id\tsurface\tsource\trelated_ids\twave_scope\towner\tclaim\tactual_or_gap\tstatus\tdisposition\tlast_verified\tevidence_sources\trepro\tverification\tdecision\tnext_slice\tnotes'

is_allowed_status() {
    case "$1" in
        works|works-but-thin|fix-now|honest-deferred|misleading|duplicate-stale)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_allowed_disposition() {
    case "$1" in
        open|verified-no-change|resolved-repaired|resolved-removed|resolved-merged|deferred-owned)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_allowed_decision() {
    case "$1" in
        fix|defer|remove|merge|harden|verify)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

matrix_allows() {
    local status="$1"
    local disposition="$2"

    case "$status:$disposition" in
        works:open|works:verified-no-change|works:resolved-repaired)
            return 0
            ;;
        works-but-thin:open|works-but-thin:resolved-repaired|works-but-thin:verified-no-change|works-but-thin:deferred-owned)
            return 0
            ;;
        fix-now:open|fix-now:resolved-repaired|fix-now:resolved-removed|fix-now:resolved-merged)
            return 0
            ;;
        misleading:open|misleading:resolved-repaired|misleading:resolved-removed|misleading:resolved-merged)
            return 0
            ;;
        duplicate-stale:open|duplicate-stale:resolved-merged|duplicate-stale:resolved-removed|duplicate-stale:resolved-repaired)
            return 0
            ;;
        honest-deferred:open|honest-deferred:deferred-owned)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

path_from_pointer() {
    local pointer="$1"
    case "$pointer" in
        path:*)
            pointer="${pointer#path:}"
            ;;
        doc:*)
            pointer="${pointer#doc:}"
            ;;
        *)
            return 1
            ;;
    esac
    printf '%s\n' "$pointer" | sed -E 's/:[0-9]+(-[0-9]+)?$//'
}

validate_pointer() {
    local pointer="$1"
    local line_no="$2"
    local ptr_path

    [[ -n "$pointer" ]] || fail "empty evidence pointer at $ledger_path:$line_no"

    case "$pointer" in
        path:*|doc:*)
            ptr_path="$(path_from_pointer "$pointer")"
            [[ -e "$ptr_path" ]] || fail "referenced path does not exist at $ledger_path:$line_no: $pointer"
            ;;
        cmd:*)
            [[ "${pointer#cmd:}" == *[![:space:]]* ]] || fail "empty command pointer at $ledger_path:$line_no"
            ;;
        test:*)
            [[ "${pointer#test:}" == *[![:space:]]* ]] || fail "empty test pointer at $ledger_path:$line_no"
            ;;
        route:*)
            [[ "$pointer" =~ ^route:(GET|POST|PUT|PATCH|DELETE)[[:space:]]/ ]] \
                || fail "invalid route pointer at $ledger_path:$line_no: $pointer"
            ;;
        mcp:*)
            [[ "$pointer" =~ ^mcp:[^/[:space:]]+/[^/[:space:]]+$ ]] \
                || fail "invalid MCP pointer at $ledger_path:$line_no: $pointer"
            ;;
        *)
            fail "invalid typed source pointer at $ledger_path:$line_no: $pointer"
            ;;
    esac
}

validate_date() {
    local value="$1"
    local line_no="$2"
    [[ "$value" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] \
        || fail "invalid last_verified date at $ledger_path:$line_no: $value"
}

maybe_warn_stale() {
    local status="$1"
    local last_verified="$2"
    local id="$3"

    case "$status" in
        works|works-but-thin)
            ;;
        *)
            return 0
            ;;
    esac

    if command -v date >/dev/null 2>&1; then
        local verified_epoch now_epoch age_days
        verified_epoch="$(date -d "$last_verified" +%s 2>/dev/null || true)"
        now_epoch="$(date +%s)"
        if [[ -n "$verified_epoch" ]]; then
            age_days=$(( (now_epoch - verified_epoch) / 86400 ))
            if (( age_days > 90 )); then
                echo "WARN: stale verification date for $id: $last_verified ($age_days days)" >&2
            fi
        fi
    fi
}

declare -A seen_ids=()
line_no=0
header_seen=0

while IFS= read -r line; do
    line_no=$((line_no + 1))

    if [[ "$line_no" -eq 1 ]]; then
        [[ "$line" == "$expected_header" ]] || fail "unexpected ledger header in $ledger_path"
        header_seen=1
        continue
    fi

    normalized_line="${line//$'\t'/$'\x1f'}"
    IFS=$'\x1f' read -r id surface source related_ids wave_scope owner claim actual_or_gap status disposition last_verified evidence_sources repro verification decision next_slice notes extra <<< "$normalized_line"

    [[ -z "${extra:-}" ]] || fail "unexpected extra fields at $ledger_path:$line_no"
    [[ "$id" =~ ^(CLI|DOC|ROUTE|MCP|AGENT|OPS)-[A-Z0-9][A-Z0-9-]*-[0-9]{3}$ ]] \
        || fail "invalid id at $ledger_path:$line_no: $id"
    [[ -z "${seen_ids["$id"]:-}" ]] || fail "duplicate id at $ledger_path:$line_no: $id"
    seen_ids["$id"]=1

    [[ -n "$surface" ]] || fail "empty surface at $ledger_path:$line_no"
    [[ -n "$source" ]] || fail "empty source at $ledger_path:$line_no"
    [[ -n "$wave_scope" ]] || fail "empty wave_scope at $ledger_path:$line_no"
    [[ -n "$owner" ]] || fail "empty owner at $ledger_path:$line_no"
    [[ -n "$claim" ]] || fail "empty claim at $ledger_path:$line_no"
    [[ -n "$status" ]] || fail "empty status at $ledger_path:$line_no"
    [[ -n "$disposition" ]] || fail "empty disposition at $ledger_path:$line_no"
    [[ -n "$last_verified" ]] || fail "empty last_verified at $ledger_path:$line_no"
    [[ -n "$evidence_sources" ]] || fail "empty evidence_sources at $ledger_path:$line_no"
    [[ -n "$repro" ]] || fail "empty repro at $ledger_path:$line_no"
    [[ -n "$verification" ]] || fail "empty verification at $ledger_path:$line_no"
    [[ -n "$decision" ]] || fail "empty decision at $ledger_path:$line_no"
    [[ -n "$next_slice" ]] || fail "empty next_slice at $ledger_path:$line_no"

    is_allowed_status "$status" || fail "invalid status at $ledger_path:$line_no: $status"
    is_allowed_disposition "$disposition" || fail "invalid disposition at $ledger_path:$line_no: $disposition"
    is_allowed_decision "$decision" || fail "invalid decision at $ledger_path:$line_no: $decision"
    matrix_allows "$status" "$disposition" || fail "invalid disposition for status at $ledger_path:$line_no: $status/$disposition"
    validate_date "$last_verified" "$line_no"

    case "$status" in
        fix-now|misleading|duplicate-stale|works-but-thin)
            [[ -n "$actual_or_gap" ]] || fail "actual_or_gap is required at $ledger_path:$line_no for status $status"
            ;;
    esac

    if [[ "$disposition" == "open" ]]; then
        [[ -n "$owner" && -n "$decision" && -n "$next_slice" && -n "$verification" && -n "$last_verified" ]] \
            || fail "open row lacks required active-wave fields at $ledger_path:$line_no"
    fi

    IFS=';' read -ra pointers <<< "$evidence_sources"
    for pointer in "${pointers[@]}"; do
        validate_pointer "$pointer" "$line_no"
    done

    if [[ -n "$scope_complete" && "$wave_scope" == "$scope_complete" && "$disposition" == "open" ]]; then
        case "$status" in
            fix-now|misleading|duplicate-stale)
                fail "scope completion blocked by open $status row at $ledger_path:$line_no: $id"
                ;;
        esac
    fi

    maybe_warn_stale "$status" "$last_verified" "$id"
done < "$ledger_path"

[[ "$header_seen" -eq 1 ]] || fail "ledger header missing from $ledger_path"

echo "Coherency ledger check passed."
```

- [ ] **Step 2: Make the validator executable**

Run:

```bash
chmod +x scripts/check-coherency-ledger.sh
```

- [ ] **Step 3: Run the validator tests**

Run:

```bash
scripts/test-coherency-ledger.sh
```

Expected:

```text
Coherency ledger validator tests passed.
```

- [ ] **Step 4: Commit the validator**

Run:

```bash
git add scripts/check-coherency-ledger.sh scripts/test-coherency-ledger.sh
git commit -m "docs: add coherency ledger validator"
```

Expected:

```text
[main <sha>] docs: add coherency ledger validator
```

---

### Task 3: Create The Feature Coherency Ledger

**Files:**
- Create: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Create the empty ledger with the exact header**

Create `docs/superpowers/feature-coherency-ledger.tsv` with:

```tsv
id	surface	source	related_ids	wave_scope	owner	claim	actual_or_gap	status	disposition	last_verified	evidence_sources	repro	verification	decision	next_slice	notes
```

- [ ] **Step 2: Validate the empty ledger**

Run:

```bash
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
```

Expected:

```text
Coherency ledger check passed.
```

- [ ] **Step 3: Stage the ledger before regenerating docs-audit inventory**

Run:

```bash
git add docs/superpowers/feature-coherency-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Expected after Task 0 already added this plan:

```text
170
```

- [ ] **Step 4: Add the docs-audit metadata for the ledger**

Add this row to `docs/superpowers/documentation-accuracy-audit-inventory.tsv` in sorted order:

```tsv
docs/superpowers/feature-coherency-ledger.tsv	planning	maintainer
```

Add this row to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` near the other `docs/superpowers/*.tsv` rows:

```tsv
docs/superpowers/feature-coherency-ledger.tsv	docs/superpowers/feature-coherency-ledger.tsv	planning	maintainer	feature-coherency; ledger; implementation-to-claim-truth	docs/superpowers/specs/2026-06-09-feature-coherency-ledger-design.md; docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md; scripts/check-coherency-ledger.sh	verified	corrected	Added the feature coherency ledger as the implementation-to-claim repair queue that supplements the existing documentation accuracy audit ledger.
```

Update final counts in `docs/superpowers/documentation-accuracy-audit-summary.md`:

```markdown
- Total tracked doc-like files audited: 170
- `verified-no-change`: 12
- `corrected`: 71
- `archived`: 73
- `retained-historical`: 14
- Remaining pending rows: 0
```

- [ ] **Step 5: Verify docs-audit and coherency metadata**

Run:

```bash
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected:

```text
Coherency ledger check passed.
Documentation audit ledger check passed (--require-complete).
```

The inventory diff and `git diff --check` commands should produce no output.

- [ ] **Step 6: Commit the ledger scaffold**

Run:

```bash
git add docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: add feature coherency ledger"
```

Expected:

```text
[main <sha>] docs: add feature coherency ledger
```

---

### Task 4: Capture Wave 1a CLI Evidence

**Files:**
- Read: `apps/conary/src/cli/mod.rs`
- Read: `apps/conary/src/dispatch.rs`
- Read: `apps/conary/src/dispatch/`
- Read: `apps/conary/build.rs`
- Read: `docs/modules/feature-ownership.md`
- Modify later in Task 5: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify only if evidence requires repair: `apps/conary/src/cli/mod.rs`, `apps/conary/src/dispatch.rs`, `apps/conary/src/dispatch/*`, selected active docs.

- [ ] **Step 1: Create scratch directory for Wave 1a**

Run:

```bash
scratch="$(mktemp -d /tmp/conary-coherency-wave1a.XXXXXX)"
printf '%s\n' "$scratch"
```

Expected:

```text
/tmp/conary-coherency-wave1a.<suffix>
```

- [ ] **Step 2: Capture root help and version**

Run:

```bash
cargo run -p conary -- --help > "$scratch/root-help.txt"
cargo run -p conary -- --version > "$scratch/root-version.txt"
sed -n '1,120p' "$scratch/root-help.txt"
cat "$scratch/root-version.txt"
```

Expected root help must include:

```text
Usage: conary [OPTIONS] [COMMAND]
Commands:
Daily workflow examples:
```

Expected version output must include:

```text
conary
```

- [ ] **Step 3: Capture selected subcommand help used by root examples**

Run:

```bash
cargo run -p conary -- install --help > "$scratch/install-help.txt"
cargo run -p conary -- update --help > "$scratch/update-help.txt"
cargo run -p conary -- system --help > "$scratch/system-help.txt"
cargo run -p conary -- system generation export --help > "$scratch/system-generation-export-help.txt"
cargo run -p conary -- system completions --help > "$scratch/system-completions-help.txt"
```

Expected:

```text
```

The commands should exit 0. No output is expected on stdout from this shell step because output is redirected to scratch files.

- [ ] **Step 4: Run focused CLI proof commands**

Run:

```bash
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --test live_host_mutation_safety
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
```

Expected:

```text
test result: ok
```

`cargo check` and completion rendering should exit 0.

- [ ] **Step 5: Regenerate and capture ignored local manpage output**

Run:

```bash
cargo build -p conary
test -f apps/conary/man/conary.1
cp apps/conary/man/conary.1 "$scratch/conary.1"
rg -n "Daily workflow examples|conary install nginx --dry-run|conary system completions bash" "$scratch/conary.1"
```

Expected:

```text
```

The `rg` command should print matching manpage lines. Do not stage `apps/conary/man/conary.1`; it is ignored generated output.

- [ ] **Step 6: Sweep the selected Wave 1a scope**

Run:

```bash
rg -n --glob '!target/**' --glob '!docs/superpowers/plans/archive/**' --glob '!docs/superpowers/specs/archive/**' 'TODO|not implemented|stub|future|unsupported|broken' \
  apps/conary/src/cli \
  apps/conary/src/dispatch.rs \
  apps/conary/src/dispatch \
  apps/conary/build.rs \
  "$scratch/root-help.txt" \
  "$scratch/install-help.txt" \
  "$scratch/update-help.txt" \
  "$scratch/system-help.txt" \
  "$scratch/system-generation-export-help.txt" \
  "$scratch/system-completions-help.txt" \
  "$scratch/conary.1" \
  README.md \
  docs/conaryopedia-v2.md \
  docs/modules/feature-ownership.md \
  docs/llms/subsystem-map.md \
  > "$scratch/wave1a-sweep.txt" || true
sed -n '1,200p' "$scratch/wave1a-sweep.txt"
```

Expected:

```text
```

The sweep may print lines. Each line must be fixed, ledgered, or marked non-public/out-of-scope in the Wave 1a notes before scope completion.

- [ ] **Step 7: Inspect root dispatch ownership**

Run:

```bash
sed -n '1,220p' apps/conary/src/dispatch/root.rs > "$scratch/dispatch-root.rs.txt"
sed -n '1,220p' apps/conary/src/dispatch.rs > "$scratch/dispatch.rs.txt"
sed -n '120,170p' apps/conary/src/cli/mod.rs > "$scratch/cli-root-examples.rs.txt"
```

Expected:

```text
```

The commands should exit 0.

- [ ] **Step 8: Write a scratch classification note**

Create `$scratch/classification.txt` with exactly these section headers and fill them with concrete evidence from the previous steps:

```text
Wave scope: 1a-root-cli

Root help:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Root examples:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Generated manpage:
- Claim:
- Actual:
- Status:
- Decision:
- Verification:

Top-level dispatch:
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

- [ ] **Step 9: Commit no files for evidence capture**

Run:

```bash
git status --short --ignored apps/conary/man
```

Expected:

```text
!! apps/conary/man/
```

Do not commit the scratch directory or ignored manpage output.

---

### Task 5: Record And Close Wave 1a Rows

**Files:**
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify only if evidence requires repair: `apps/conary/src/cli/mod.rs`, `apps/conary/src/dispatch.rs`, `apps/conary/src/dispatch/*`, `README.md`, `docs/conaryopedia-v2.md`, `docs/modules/feature-ownership.md`, `docs/llms/subsystem-map.md`

- [ ] **Step 1: Add baseline Wave 1a rows**

Append rows to `docs/superpowers/feature-coherency-ledger.tsv` using the classification note. If the evidence shows no gap, use these rows:

```tsv
CLI-ROOT-001	conary root help	cmd:cargo run -p conary -- --help		1a-root-cli	CLI Dispatch And Command Routing	Root help renders top-level command list and daily workflow examples	Root help renders successfully and includes Usage, Commands, Options, and Daily workflow examples	works	verified-no-change	2026-06-09	path:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help	none	cmd:cargo run -p conary -- --help	verify	Re-run root help during CLI help changes	Root help evidence captured in Wave 1a scratch output
CLI-ROOT-002	root daily examples	cmd:cargo run -p conary -- --help		1a-root-cli	CLI Dispatch And Command Routing	Daily examples point at routed CLI commands and dry-run-first workflows	Root examples reference install dry-run, install yes, update dry-run, system adopt refresh, system completions, generation export, and conaryd apply-intent boundary	works	verified-no-change	2026-06-09	path:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help;test:cargo test -p conary --test cli_daily_ux	none	cargo test -p conary --test cli_daily_ux	verify	Re-run root example route proof during CLI help changes	Example route proof captured and closed in Wave 1a
CLI-ROOT-003	generated root manpage	cmd:cargo build -p conary		1a-root-cli	CLI Dispatch And Command Routing	Generated manpage mirrors root Clap help closely enough for root examples	Generated ignored local manpage exists after cargo build and includes root example strings	works	verified-no-change	2026-06-09	path:apps/conary/build.rs;cmd:cargo build -p conary	none	cmd:cargo build -p conary;rg -n "Daily workflow examples|conary install nginx --dry-run" apps/conary/man/conary.1	verify	Re-run build and sweep generated manpage during root help changes	Generated manpage remains ignored and should not be committed
CLI-ROOT-004	top-level dispatch coverage	test:cargo test -p conary --lib cli::tests		1a-root-cli	CLI Dispatch And Command Routing	Top-level parsed commands remain covered by CLI and live-mutation routing tests	CLI tests and live-host mutation safety tests pass for the selected root scope	works	verified-no-change	2026-06-09	path:apps/conary/src/dispatch.rs;path:apps/conary/src/dispatch/root.rs;test:cargo test -p conary --lib cli::tests;test:cargo test -p conary --test live_host_mutation_safety	none	cargo check -p conary;cargo test -p conary --lib cli::tests;cargo test -p conary --test live_host_mutation_safety	verify	Re-run focused CLI routing proof before changing root dispatch	Root dispatch evidence captured through focused CLI tests
```

If evidence shows a gap, change only the affected row:

- `status=misleading` when a root help/doc claim sends a user to a dead end or contradicted behavior.
- `status=duplicate-stale` when two root surfaces claim the same job with conflicting contracts.
- `status=fix-now` when a bounded root help/dispatch/manpage repair is required before Wave 1a can close.
- `actual_or_gap` must state the concrete observed behavior.
- `repro` must include the exact command or pointer that demonstrates the gap.
- `next_slice` must name the exact repair or honest-deferral action.
- Any temporary `disposition=open` row in `wave_scope=1a-root-cli` must be converted to a resolved, verified, merged, removed, or `deferred-owned` row before Wave 1a is called complete.

- [ ] **Step 2: Validate before scope completion**

Run:

```bash
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
```

Expected:

```text
Coherency ledger check passed.
```

- [ ] **Step 3: Repair selected-scope blockers before completion**

If Task 5 Step 1 added any `1a-root-cli` row with `status=fix-now`, `status=misleading`, or `status=duplicate-stale` and `disposition=open`, do the smallest repair inside the selected scope:

- For root help wording, edit `apps/conary/src/cli/mod.rs`.
- For dispatch mismatch, edit `apps/conary/src/dispatch.rs` or the specific child module under `apps/conary/src/dispatch/`.
- For active-doc overclaim tied to root examples, edit only the exact active doc that repeats the root claim.
- For generated manpage drift caused by Clap source, edit the Clap source and regenerate by running `cargo build -p conary`.

Then update the affected ledger row to one of:

```text
resolved-repaired
resolved-removed
resolved-merged
```

If the feature is intentionally unavailable, first make the active root help or doc surface honest, verify that honest refusal or wording, then change the row to:

```text
status=honest-deferred
disposition=deferred-owned
decision=defer
```

- [ ] **Step 4: Run scope-completion validation**

Run:

```bash
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
```

Expected:

```text
Coherency ledger check passed.
```

- [ ] **Step 5: Run focused Wave 1a verification**

Run:

```bash
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --test live_host_mutation_safety
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
cargo build -p conary
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected:

```text
test result: ok
Coherency ledger check passed.
Documentation audit ledger check passed (--require-complete).
```

The inventory diff and `git diff --check` commands should produce no output.

- [ ] **Step 6: Commit Wave 1a closure**

Run:

```bash
git status --short
git add docs/superpowers/feature-coherency-ledger.tsv apps/conary/src/cli/mod.rs apps/conary/src/dispatch.rs apps/conary/src/dispatch README.md docs/conaryopedia-v2.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md
git diff --cached --name-only
git commit -m "docs: record feature coherency wave 1a"
```

Expected:

```text
[main <sha>] docs: record feature coherency wave 1a
```

If only the ledger changed, `git add` will ignore unchanged paths and stage just `docs/superpowers/feature-coherency-ledger.tsv`.

---

### Task 6: Final Verification And Handoff

**Files:**
- Read: `docs/superpowers/feature-coherency-ledger.tsv`
- Read: `docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan.md`

- [ ] **Step 1: Run final verification**

Run:

```bash
scripts/test-coherency-ledger.sh
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --test live_host_mutation_safety
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
git diff --check
```

Expected:

```text
Coherency ledger validator tests passed.
Coherency ledger check passed.
test result: ok
Documentation audit ledger check passed (--require-complete).
Documentation truth checks passed.
```

The inventory diff and `git diff --check` commands should produce no output.

- [ ] **Step 2: Confirm no ignored manpage output is staged**

Run:

```bash
git status --short --ignored apps/conary/man man
git diff --cached --name-only | rg '(^apps/conary/man/|^man/)' && exit 1 || true
```

Expected:

```text
!! apps/conary/man/
```

The second command should produce no output and exit 0.

- [ ] **Step 3: Commit any final verification-only metadata**

If Task 6 changed only ledger verification dates or notes, commit them:

```bash
git status --short
git add docs/superpowers/feature-coherency-ledger.tsv
git commit -m "docs: finalize feature coherency wave 1a"
```

Expected when there are staged metadata changes:

```text
[main <sha>] docs: finalize feature coherency wave 1a
```

If `git status --short` is clean, skip this commit.

- [ ] **Step 4: Push and verify sync**

Run:

```bash
git status --short --branch
git push
git status --short --branch
git rev-list --left-right --count HEAD...origin/main
git rev-parse HEAD origin/main
```

Expected:

```text
## main...origin/main
0	0
```

Both `git rev-parse` lines should be identical.

## Self-Review Checklist

- [ ] The feature coherency ledger exists and has the exact header.
- [ ] The ledger validator rejects bad status, bad source pointer, bad closure matrix, and open selected-scope blockers.
- [ ] Wave 1a has no open row in `wave_scope=1a-root-cli`.
- [ ] Generated manpage output was inspected but not committed.
- [ ] Docs-audit ledger and inventory are in sync.
- [ ] `docs/modules/feature-ownership.md` and `docs/llms/subsystem-map.md` are updated if root routing or "look here first" paths changed.
- [ ] Final verification commands in Task 6 pass before claiming completion.
