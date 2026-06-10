# Feature Coherency Ledger Wave 1a Implementation Plan

> **Executed:** Wave 1a is closed in `docs/superpowers/feature-coherency-ledger.tsv`; do not rerun this plan. Treat embedded script and ledger snippets as planning-time snapshots and use the live files under `scripts/` and `docs/superpowers/` for current behavior.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the feature coherency ledger and validator, then execute Wave 1a against root CLI help, examples, dispatch coverage, and generated manpage behavior.

**Architecture:** Keep the existing documentation accuracy audit ledger as the tracked Markdown/doc-like inventory gate, and add a separate feature coherency ledger for implementation-to-claim surfaces. The first implementation slice creates `docs/superpowers/feature-coherency-ledger.tsv`, validates it with a focused shell script, tests, and PR-gate step, captures root CLI evidence into temporary files, then records or repairs Wave 1a rows until no selected-scope row remains open.

**Tech Stack:** Bash, TSV, Cargo, Clap root help, `clap_mangen` via `apps/conary/build.rs`, docs-audit scripts, `docs/modules/feature-ownership.md`.

---

## Current Repository Facts

- Repository root: `/home/peter/Conary`.
- The plan packet is already tracked in docs-audit metadata. Refresh `git rev-parse HEAD origin/main` and `date +%F` at execution time; do not reuse stale commit IDs or `last_verified` dates from this plan text.
- `docs/superpowers/specs/archive/2026-06-09-feature-coherency-ledger-design.md` defines the ledger columns, closure matrix, source-pointer grammar, Wave 1a scope, and verification gates.
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv` remains the tracked Markdown/doc-like inventory and documentation truth ledger. It must not track `docs/superpowers/feature-coherency-ledger.tsv` unless docs-audit is separately redesigned to include TSV artifacts.
- `scripts/check-doc-audit-ledger.sh` and `scripts/docs-audit-inventory.sh` are active PR-gate inputs.
- At planning time, `scripts/check-coherency-ledger.sh`, `scripts/test-coherency-ledger.sh`, and `docs/superpowers/feature-coherency-ledger.tsv` did not exist yet. They are active now; use the live files instead of this plan's embedded copies.
- `apps/conary/build.rs` writes ignored local generated manpage output to `apps/conary/man/conary.1` via `clap_mangen` during `cargo build -p conary`.
- `git ls-files apps/conary/man/conary.1 man/conary.1` returns no tracked manpage files today; generated manpage output is local evidence, not a committed artifact.
- `cargo run -p conary -- --help` currently renders root help and daily workflow examples successfully.
- At planning time, `cargo test -p conary --lib cli::tests -- --list` listed 25 CLI unit tests. Refresh this count at execution time rather than treating it as durable evidence.

## Non-Goals

- Do not audit every active doc, conaryd route, Remi route, MCP tool, or agent contract in Wave 1a.
- Do not add HTTP, MCP, or conaryd rows unless a selected root CLI surface directly advertises or depends on them.
- Do not commit generated manpage files under ignored `apps/conary/man/` or `/man/`.
- Do not leave any selected-scope row open at the end of Wave 1a.
- Do not replace `docs/superpowers/documentation-accuracy-audit-ledger.tsv`.
- Do not add `docs/superpowers/feature-coherency-ledger.tsv` to the documentation accuracy audit inventory, ledger, or summary counts.
- Do not hide untriaged findings in the durable coherency ledger. Use temporary scratch files until a finding has owner, status, decision, next slice, and verification.

## File Responsibility Map

| File | Responsibility |
| --- | --- |
| `docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md` | This implementation plan and execution checklist. |
| `docs/superpowers/feature-coherency-ledger.tsv` | Durable implementation-to-claim coherency ledger. |
| `scripts/check-coherency-ledger.sh` | Validates ledger header, IDs, status/disposition/decision values, closure matrix, source pointers, dates, and scope-completion rules. |
| `scripts/test-coherency-ledger.sh` | Shell tests for the coherency ledger validator using temporary ledger fixtures. |
| `.github/workflows/pr-gate.yml` | Adds the coherency ledger validator to CI after the ledger file exists. |
| `docs/superpowers/documentation-accuracy-audit-inventory.tsv` | Existing tracked-doc inventory. Verify it already contains this plan row; do not add the coherency TSV. |
| `docs/superpowers/documentation-accuracy-audit-ledger.tsv` | Existing tracked-doc ledger. Verify it already contains this plan row; do not add the coherency TSV. |
| `docs/superpowers/documentation-accuracy-audit-summary.md` | Summary counts and active-planning note for this plan. Verify current metadata; do not count the coherency TSV. |
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
works -> verified-no-change, resolved-repaired
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

Global allowed ID type prefixes:

```text
CLI
DOC
ROUTE
MCP
AGENT
OPS
```

Wave 1a should use `CLI` and, if a selected CLI claim is repeated in active docs, `DOC`. `ROUTE`, `MCP`, `AGENT`, and `OPS` are accepted by the global validator for later waves only; do not use them in `wave_scope=1a-root-cli` unless root CLI help directly advertises that surface and the row is narrowly scoped to that root CLI claim.

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

### Task 0: Verify The Already-Locked Plan Packet

**Files:**
- Read: `docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md`
- Read: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Read: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Read: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Confirm the plan packet is already tracked exactly once**

Do not add a second inventory or ledger row for this plan. It was locked before implementation began.

Run:

```bash
git ls-files docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md
rg -n '^docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan\.md\t' docs/superpowers/documentation-accuracy-audit-inventory.tsv
rg -n '^docs/superpowers/plans/2026-06-09-feature-coherency-ledger-wave1a-plan\.md\t' docs/superpowers/documentation-accuracy-audit-ledger.tsv
```

Expected:

```text
docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md
```

The two `rg` commands should each print exactly one row for the plan. If either command prints zero rows or more than one row, fix the docs-audit metadata before continuing.

- [ ] **Step 2: Refresh execution facts**

Run:

```bash
date +%F
git rev-parse HEAD origin/main
git status --short --branch
```

Use the date from this step for every `last_verified` value written during execution. Do not copy a hard-coded date from this plan.

- [ ] **Step 3: Verify the plan packet docs-audit metadata**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check -- docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md
```

Expected:

```text
Documentation audit ledger check passed (--require-complete).
```

The inventory diff and `git diff --check` commands should produce no output.

---

### Task 1: Add Coherency Ledger Validator Tests Locally

**Files:**
- Create: `scripts/test-coherency-ledger.sh`
- Later modify in Task 2: `scripts/check-coherency-ledger.sh`

- [ ] **Step 1: Create the failing test script**

Create `scripts/test-coherency-ledger.sh` with this content:

This block is a planning-time snapshot. After Wave 1a execution, use the live
`scripts/test-coherency-ledger.sh` file as source of truth.

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

real_repo_row=$'CLI-ROOT-005\tconary root dispatch fixture\tpath:apps/conary/src/dispatch/root.rs:1-40\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot dispatch paths exist in the repo\tLine-range and directory path pointers resolve against real repo files\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/dispatch/root.rs:1-40;path:apps/conary/src/dispatch/\tnone\ttest:cargo test -p conary --lib cli::tests\tverify\tKeep real path fixture green\tReal repo path fixture'
real_repo="$tmpdir/real-repo.tsv"
write_ledger "$real_repo" "$real_repo_row"
"$validator" "$real_repo" >/dev/null

valid_route_mcp="$tmpdir/valid-route-mcp.tsv"
route_row=$'ROUTE-CONARYD-001\tconaryd transactions route\troute:GET /v1/transactions\t\tlater-route-wave\tConaryd Daemon Routes\tRoute pointer grammar accepts method and path\tRoute pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\troute:GET /v1/transactions\tnone\troute:GET /v1/transactions\tverify\tValidate route pointer grammar\tRoute pointer fixture'
mcp_row=$'MCP-REMI-001\tRemi MCP tool fixture\tmcp:remi/tool-name\t\tlater-mcp-wave\tAgent And MCP Surface\tMCP pointer grammar accepts server and tool\tMCP pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\tmcp:remi/tool-name\tnone\tmcp:remi/tool-name\tverify\tValidate MCP pointer grammar\tMCP pointer fixture'
write_ledger "$valid_route_mcp" "$route_row" "$mcp_row"
"$validator" "$valid_route_mcp" >/dev/null

bad_status="$tmpdir/bad-status.tsv"
write_ledger "$bad_status" "${good_row/works/not-real-status}"
if "$validator" "$bad_status" >"$tmpdir/out" 2>&1; then
    fail "invalid status unexpectedly passed"
fi
grep -q "invalid status" "$tmpdir/out" || fail "invalid status error was not clear"

bad_evidence_pointer="$tmpdir/bad-evidence-pointer.tsv"
write_ledger "$bad_evidence_pointer" "${good_row/path:apps\/conary\/src\/cli\/mod.rs/path:no-such-file.rs}"
if "$validator" "$bad_evidence_pointer" >"$tmpdir/out" 2>&1; then
    fail "missing path pointer unexpectedly passed"
fi
grep -q "referenced path does not exist" "$tmpdir/out" || fail "missing path error was not clear"

bad_source="$tmpdir/bad-source.tsv"
write_ledger "$bad_source" "${good_row/cmd:cargo run -p conary -- --help/not-a-typed-source}"
if "$validator" "$bad_source" >"$tmpdir/out" 2>&1; then
    fail "bad source pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad source error was not clear"

bad_repro="$tmpdir/bad-repro.tsv"
bad_repro_row=$'CLI-ROOT-002\tconary root help\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help lists top-level commands and daily examples\tRoot help renders and examples are visible\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help\tbad-repro\tcmd:cargo run -p conary -- --help\tverify\tWave 1a root help evidence captured\tBad repro should fail'
write_ledger "$bad_repro" "$bad_repro_row"
if "$validator" "$bad_repro" >"$tmpdir/out" 2>&1; then
    fail "bad repro pointer unexpectedly passed"
fi
grep -q "invalid typed repro pointer" "$tmpdir/out" || fail "bad repro error was not clear"

bad_route="$tmpdir/bad-route.tsv"
bad_route_row=$'ROUTE-CONARYD-002\tconaryd transactions route\troute:GET/v1/transactions\t\tlater-route-wave\tConaryd Daemon Routes\tRoute pointer grammar rejects missing method/path space\tRoute pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\troute:GET/v1/transactions\tnone\troute:GET/v1/transactions\tverify\tValidate bad route pointer grammar\tBad route pointer fixture'
write_ledger "$bad_route" "$bad_route_row"
if "$validator" "$bad_route" >"$tmpdir/out" 2>&1; then
    fail "bad route pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad route pointer error was not clear"

bad_mcp="$tmpdir/bad-mcp.tsv"
bad_mcp_row=$'MCP-REMI-002\tRemi MCP tool fixture\tmcp:remi\t\tlater-mcp-wave\tAgent And MCP Surface\tMCP pointer grammar rejects missing tool name\tMCP pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\tmcp:remi\tnone\tmcp:remi\tverify\tValidate bad MCP pointer grammar\tBad MCP pointer fixture'
write_ledger "$bad_mcp" "$bad_mcp_row"
if "$validator" "$bad_mcp" >"$tmpdir/out" 2>&1; then
    fail "bad MCP pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad MCP pointer error was not clear"

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

bad_open_works="$tmpdir/bad-open-works.tsv"
open_works=$'CLI-ROOT-003\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tRoot example route proof is still being captured\tworks\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tverify\tFinish root example proof\tOpen works row should fail normal validation'
write_ledger "$bad_open_works" "$open_works"
if "$validator" "$bad_open_works" >"$tmpdir/out" 2>&1; then
    fail "open works row unexpectedly passed"
fi
grep -q "invalid disposition" "$tmpdir/out" || fail "open works matrix error was not clear"

scope_open_thin="$tmpdir/scope-open-thin.tsv"
open_thin=$'CLI-ROOT-004\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tRoute proof is thin and still open\tworks-but-thin\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tharden\tFinish root example proof\tOpen works-but-thin row should block scope completion'
write_ledger "$scope_open_thin" "$open_thin"
"$validator" "$scope_open_thin" >/dev/null
if "$validator" "$scope_open_thin" --scope-complete 1a-root-cli >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed open works-but-thin row"
fi
grep -q "scope completion blocked" "$tmpdir/out" || fail "open works-but-thin scope error was not clear"

empty_scope="$tmpdir/empty-scope.tsv"
write_ledger "$empty_scope" "$good_row"
if "$validator" "$empty_scope" --scope-complete other-scope >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed an empty scope"
fi
grep -q "scope completion found no rows" "$tmpdir/out" || fail "empty scope error was not clear"

bad_matrix="$tmpdir/bad-matrix.tsv"
matrix_row=$'CLI-ROOT-004\tconary root help duplicate\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tDuplicate root path should be merged\tTwo root surfaces overlap\tduplicate-stale\tverified-no-change\t2026-06-09\tcmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tmerge\tMerge duplicate root surface\tBad matrix should fail'
write_ledger "$bad_matrix" "$matrix_row"
if "$validator" "$bad_matrix" >"$tmpdir/out" 2>&1; then
    fail "invalid status/disposition matrix unexpectedly passed"
fi
grep -q "invalid disposition" "$tmpdir/out" || fail "bad matrix error was not clear"

dangling_related="$tmpdir/dangling-related.tsv"
dangling_row=$'CLI-ROOT-005\tconary root help duplicate\tcmd:cargo run -p conary -- --help\tCLI-ROOT-999\t1a-root-cli\tCLI Dispatch And Command Routing\tDuplicate root path should be merged\tTwo root surfaces overlap\tduplicate-stale\tresolved-merged\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tmerge\tMerge duplicate root surface\tDangling related ID should fail'
write_ledger "$dangling_related" "$dangling_row"
if "$validator" "$dangling_related" >"$tmpdir/out" 2>&1; then
    fail "dangling related_id unexpectedly passed"
fi
grep -q "dangling related_id" "$tmpdir/out" || fail "dangling related_id error was not clear"

echo "Coherency ledger validator tests passed."
```

- [ ] **Step 2: Make the test script executable**

Run:

```bash
chmod +x scripts/test-coherency-ledger.sh
```

- [ ] **Step 3: Run the test to verify it fails before implementation, but do not commit yet**

Run:

```bash
scripts/test-coherency-ledger.sh
```

Expected:

```text
scripts/test-coherency-ledger.sh: line ...: scripts/check-coherency-ledger.sh: No such file or directory
```

Keep this red check local. Commit `scripts/test-coherency-ledger.sh` only after Task 2 creates the validator and the test script passes.

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
        works:verified-no-change|works:resolved-repaired)
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
    if [[ "$pointer" =~ ^(.+):[0-9]+(-[0-9]+)?$ ]]; then
        pointer="${BASH_REMATCH[1]}"
    fi
    printf '%s\n' "$pointer"
}

validate_pointer() {
    local pointer="$1"
    local line_no="$2"
    local field_name="${3:-evidence}"
    local ptr_path

    [[ -n "$pointer" ]] || fail "empty $field_name pointer at $ledger_path:$line_no"

    case "$pointer" in
        path:*|doc:*)
            ptr_path="$(path_from_pointer "$pointer")"
            [[ -e "$ptr_path" ]] || fail "referenced path does not exist at $ledger_path:$line_no: $pointer"
            ;;
        cmd:*)
            # Syntax-only: execution is proven by the explicit verification commands.
            [[ "${pointer#cmd:}" == *[![:space:]]* ]] || fail "empty $field_name command pointer at $ledger_path:$line_no"
            ;;
        test:*)
            # Syntax-only: execution is proven by the explicit verification commands.
            [[ "${pointer#test:}" == *[![:space:]]* ]] || fail "empty $field_name test pointer at $ledger_path:$line_no"
            ;;
        route:*)
            [[ "$pointer" =~ ^route:(GET|POST|PUT|PATCH|DELETE)[[:space:]]/ ]] \
                || fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
        mcp:*)
            [[ "$pointer" =~ ^mcp:[^/[:space:]]+/[^/[:space:]]+$ ]] \
                || fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
        *)
            fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
    esac
}

validate_pointer_list() {
    local value="$1"
    local line_no="$2"
    local field_name="$3"

    IFS=';' read -ra pointers <<< "$value"
    for pointer in "${pointers[@]}"; do
        validate_pointer "$pointer" "$line_no" "$field_name"
    done
}

validate_optional_pointer_list() {
    local value="$1"
    local line_no="$2"
    local field_name="$3"

    [[ "$value" == "none" ]] && return 0
    validate_pointer_list "$value" "$line_no" "$field_name"
}

validate_related_id_shape() {
    local related_id="$1"
    local line_no="$2"
    [[ "$related_id" =~ ^(CLI|DOC|ROUTE|MCP|AGENT|OPS)-[A-Z0-9][A-Z0-9-]*-[0-9]{3}$ ]] \
        || fail "invalid related_id at $ledger_path:$line_no: $related_id"
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
declare -a related_refs=()
line_no=0
header_seen=0
scope_row_count=0

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

    if [[ -n "$related_ids" ]]; then
        IFS=';' read -ra related_id_values <<< "$related_ids"
        for related_id in "${related_id_values[@]}"; do
            validate_related_id_shape "$related_id" "$line_no"
            related_refs+=("$id|$related_id|$line_no")
        done
    fi

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
    validate_pointer_list "$source" "$line_no" "source"
    validate_pointer_list "$evidence_sources" "$line_no" "evidence"
    validate_optional_pointer_list "$repro" "$line_no" "repro"
    validate_pointer_list "$verification" "$line_no" "verification"

    case "$status" in
        fix-now|misleading|duplicate-stale|works-but-thin)
            [[ -n "$actual_or_gap" ]] || fail "actual_or_gap is required at $ledger_path:$line_no for status $status"
            ;;
    esac

    if [[ "$disposition" == "open" ]]; then
        [[ -n "$owner" && -n "$decision" && -n "$next_slice" && -n "$verification" && -n "$last_verified" ]] \
            || fail "open row lacks required active-wave fields at $ledger_path:$line_no"
    fi

    if [[ -n "$scope_complete" && "$wave_scope" == "$scope_complete" ]]; then
        scope_row_count=$((scope_row_count + 1))
        if [[ "$disposition" == "open" ]]; then
            fail "scope completion blocked by open $status row at $ledger_path:$line_no: $id"
        fi
    fi

    maybe_warn_stale "$status" "$last_verified" "$id"
done < "$ledger_path"

[[ "$header_seen" -eq 1 ]] || fail "ledger header missing from $ledger_path"

for ref in "${related_refs[@]}"; do
    IFS='|' read -r referrer related_id ref_line <<< "$ref"
    [[ -n "${seen_ids["$related_id"]:-}" ]] \
        || fail "dangling related_id at $ledger_path:$ref_line: $referrer references $related_id"
done

if [[ -n "$scope_complete" && "$scope_row_count" -eq 0 ]]; then
    fail "scope completion found no rows for scope: $scope_complete"
fi

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
- Modify: `.github/workflows/pr-gate.yml`

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

- [ ] **Step 3: Confirm the ledger is not part of docs-audit**

`scripts/docs-audit-inventory.sh` only tracks Markdown/RST/ADOC/MDX and `*.toml.example` documentation-like files. Do not add `docs/superpowers/feature-coherency-ledger.tsv` to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, or `docs/superpowers/documentation-accuracy-audit-summary.md`.

Run:

```bash
git add docs/superpowers/feature-coherency-ledger.tsv
if bash scripts/docs-audit-inventory.sh | cut -f1 | rg -x 'docs/superpowers/feature-coherency-ledger.tsv'; then
  echo "ERROR: docs-audit unexpectedly tracks feature-coherency-ledger.tsv" >&2
  exit 1
fi
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected:

```text
```

No output.

- [ ] **Step 4: Add the coherency ledger validator to the PR gate**

In `.github/workflows/pr-gate.yml`, add this step to the `docs-truth` job after `Check docs truth invariants`:

```yaml
      - name: Check feature coherency ledger
        run: bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
```

Do not run `--scope-complete` in CI until Wave 1a rows have been recorded and closed in Task 5.

- [ ] **Step 5: Verify docs-audit remains unchanged and coherency CI input works**

Run:

```bash
bash -n scripts/check-coherency-ledger.sh
bash -n scripts/test-coherency-ledger.sh
scripts/test-coherency-ledger.sh
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected:

```text
Coherency ledger validator tests passed.
Coherency ledger check passed.
Documentation audit ledger check passed (--require-complete).
```

The inventory diff and `git diff --check` commands should produce no output.

- [ ] **Step 6: Commit the ledger scaffold**

Run:

```bash
git add docs/superpowers/feature-coherency-ledger.tsv .github/workflows/pr-gate.yml
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
cargo run -p conary -- system adopt --help > "$scratch/system-adopt-help.txt"
cargo run -p conary -- system generation export --help > "$scratch/system-generation-export-help.txt"
cargo run -p conary -- system completions --help > "$scratch/system-completions-help.txt"
cargo run -p conaryd -- --help > "$scratch/conaryd-help.txt"
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
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test live_host_mutation_safety
cargo test -p conaryd
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
bash scripts/check-doc-truth.sh
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
for pattern in \
  "Daily workflow examples" \
  "conary install nginx" \
  "dry-run" \
  "conary system completions bash"
do
  rg -n -- "$pattern" "$scratch/conary.1"
done
```

Expected:

```text
```

Each required pattern must print at least one matching manpage line. If roff escaping prevents a literal dashed-option match, check stable surrounding text that proves the same example is present and record that exact inspection in the ledger row. Do not stage `apps/conary/man/conary.1`; it is ignored generated output.

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
  "$scratch/system-adopt-help.txt" \
  "$scratch/system-generation-export-help.txt" \
  "$scratch/system-completions-help.txt" \
  "$scratch/conaryd-help.txt" \
  "$scratch/conary.1" \
  > "$scratch/wave1a-sweep.txt" || true
sed -n '1,200p' "$scratch/wave1a-sweep.txt"
```

Expected:

```text
```

The sweep may print lines. Each line must be fixed, ledgered, or marked non-public/out-of-scope in the Wave 1a notes before scope completion. Do not broaden this sweep to `README.md`, `docs/conaryopedia-v2.md`, `docs/modules/*.md`, or `docs/operations/*.md` during Wave 1a. If evidence in the selected CLI scope points at a specific active-doc overclaim, inspect and repair that exact doc claim only; broad active-doc claim sweeps belong to later waves.

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

Before adding rows, verify the scratch classification note has been filled in:

```bash
test -f "$scratch/classification.txt"
if rg -n '^- (Claim|Actual|Status|Decision|Verification|Public in scope|Non-public or out of scope|Requires repair before scope completion):[[:space:]]*$' "$scratch/classification.txt"; then
  echo "ERROR: classification note has empty required fields" >&2
  exit 1
fi
```

Append rows to `docs/superpowers/feature-coherency-ledger.tsv` using the classification note. If the evidence shows no gap, run:

```bash
verified_date="$(date +%F)"
cat >> docs/superpowers/feature-coherency-ledger.tsv <<EOF
CLI-ROOT-001	conary root help	cmd:cargo run -p conary -- --help		1a-root-cli	CLI Dispatch And Command Routing	Root help renders top-level command list and daily workflow examples	Root help renders successfully and includes Usage, Commands, Options, and Daily workflow examples	works	verified-no-change	${verified_date}	path:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help	none	cmd:cargo run -p conary -- --help	verify	Re-run root help during CLI help changes	Root help evidence captured in Wave 1a scratch output
CLI-ROOT-002	root daily examples	cmd:cargo run -p conary -- --help		1a-root-cli	CLI Dispatch And Command Routing	Daily examples point at routed CLI commands, dry-run-first workflows, and the conaryd durable package-job boundary	Root examples route through install, update, system adopt refresh, system completions, generation export, and conaryd package-job proof	works	verified-no-change	${verified_date}	path:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help;cmd:cargo run -p conary -- system adopt --help;test:cargo test -p conary --test cli_daily_ux;cmd:cargo run -p conaryd -- --help;test:cargo test -p conaryd;cmd:bash scripts/check-doc-truth.sh	none	test:cargo test -p conary --test cli_daily_ux;test:cargo test -p conaryd;cmd:bash scripts/check-doc-truth.sh	verify	Re-run root example route and conaryd-boundary proof during CLI help changes	Example route and conaryd-boundary proof captured and closed in Wave 1a
CLI-ROOT-003	generated root manpage	cmd:cargo build -p conary		1a-root-cli	CLI Dispatch And Command Routing	Generated manpage mirrors root Clap help closely enough for root examples	Generated ignored local manpage exists after cargo build and includes each required root example string	works	verified-no-change	${verified_date}	path:apps/conary/build.rs;cmd:cargo build -p conary	none	cmd:cargo build -p conary;cmd:rg -n -- "Daily workflow examples" apps/conary/man/conary.1;cmd:rg -n -- "conary install nginx" apps/conary/man/conary.1;cmd:rg -n -- "dry-run" apps/conary/man/conary.1;cmd:rg -n -- "conary system completions bash" apps/conary/man/conary.1	verify	Re-run build and per-string generated manpage checks during root help changes	Generated manpage remains ignored and should not be committed
CLI-ROOT-004	top-level dispatch coverage	test:cargo test -p conary --lib cli::tests		1a-root-cli	CLI Dispatch And Command Routing	Top-level parsed commands remain covered by CLI, daily UX, and live-mutation routing tests	CLI tests, daily UX tests, and live-host mutation safety tests pass for the selected root scope	works	verified-no-change	${verified_date}	path:apps/conary/src/dispatch.rs;path:apps/conary/src/dispatch/root.rs;test:cargo test -p conary --lib cli::tests;test:cargo test -p conary --test cli_daily_ux;test:cargo test -p conary --test live_host_mutation_safety	none	cmd:cargo check -p conary;test:cargo test -p conary --lib cli::tests;test:cargo test -p conary --test cli_daily_ux;test:cargo test -p conary --test live_host_mutation_safety	verify	Re-run focused CLI routing proof before changing root dispatch	Root dispatch evidence captured through focused CLI tests
EOF
```

If evidence shows a gap, change only the affected row:

- `status=misleading` when a root help/doc claim sends a user to a dead end or contradicted behavior.
- `status=duplicate-stale` when two root surfaces claim the same job with conflicting contracts.
- `status=fix-now` when a bounded root help/dispatch/manpage repair is required before Wave 1a can close.
- `actual_or_gap` must state the concrete observed behavior.
- `repro` must include the exact command or pointer that demonstrates the gap.
- `next_slice` must name the exact repair or honest-deferral action.
- Any temporary `disposition=open` row in `wave_scope=1a-root-cli` must be converted to a resolved, verified, merged, removed, or `deferred-owned` row before Wave 1a is called complete.
- If the root help keeps a conaryd durable-package-job claim, `CLI-ROOT-002` cannot close as `works` until the conaryd proof above passes. If that proof is intentionally deferred, reword root help to be honest or reclassify the row as `honest-deferred` with `disposition=deferred-owned`.

- [ ] **Step 2: Validate before scope completion**

Run:

```bash
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
```

Expected:

```text
Coherency ledger check passed.
```

- [ ] **Step 3: Close every selected-scope open row before completion**

If Task 5 Step 1 added any `1a-root-cli` row with `disposition=open`, close it before scope completion. For `status=fix-now`, `status=misleading`, or `status=duplicate-stale`, do the smallest repair inside the selected scope:

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
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test live_host_mutation_safety
cargo test -p conaryd
cargo run -p conary -- system completions bash >/tmp/conary-completion.bash
cargo build -p conary
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-truth.sh
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
git commit -m "docs: close feature coherency wave 1a rows"
```

Expected:

```text
[main <sha>] docs: close feature coherency wave 1a rows
```

If only the ledger changed, `git add` will ignore unchanged paths and stage just `docs/superpowers/feature-coherency-ledger.tsv`.

---

### Task 6: Final Verification And Handoff

**Files:**
- Read: `docs/superpowers/feature-coherency-ledger.tsv`
- Read: `docs/superpowers/plans/archive/2026-06-09-feature-coherency-ledger-wave1a-plan.md`

- [ ] **Step 1: Run final verification**

Run:

```bash
scripts/test-coherency-ledger.sh
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv --scope-complete 1a-root-cli
cargo check -p conary
cargo test -p conary --lib cli::tests
cargo test -p conary --test cli_daily_ux
cargo test -p conary --test live_host_mutation_safety
cargo test -p conaryd
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
- [ ] The coherency TSV is not registered in the docs-audit inventory, ledger, or summary counts.
- [ ] The ledger validator is wired into `.github/workflows/pr-gate.yml`.
- [ ] The ledger validator rejects bad status, bad source/repro/evidence/route/MCP pointers, bad closure matrix, dangling related IDs, empty completed scopes, and open selected-scope rows.
- [ ] Wave 1a has no open row in `wave_scope=1a-root-cli`.
- [ ] Root example proof includes `system adopt`, `cli_daily_ux`, and either conaryd proof or an honest conaryd deferral/wording repair.
- [ ] Generated manpage output was checked with per-string assertions, not one broad alternation.
- [ ] Generated manpage output was inspected but not committed.
- [ ] Docs-audit ledger and inventory are in sync.
- [ ] `docs/modules/feature-ownership.md` and `docs/llms/subsystem-map.md` are updated if root routing or "look here first" paths changed.
- [ ] Final verification commands in Task 6 pass before claiming completion.
