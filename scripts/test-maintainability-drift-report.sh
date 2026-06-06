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
