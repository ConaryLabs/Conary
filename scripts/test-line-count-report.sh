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
