#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

script="$repo_root/scripts/agent-context.sh"
[[ -x "$script" ]] || fail "scripts/agent-context.sh is not executable"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

help_output="$("$script" --help 2>&1)"
grep -q "Usage: scripts/agent-context.sh" <<<"$help_output" \
    || fail "help output did not include usage"

if "$script" >"$tmp/no-mode.out" 2>&1; then
    fail "missing mode unexpectedly succeeded"
fi
grep -q "Usage: scripts/agent-context.sh" "$tmp/no-mode.out" \
    || fail "missing mode did not print usage"

if "$script" --list --validate >"$tmp/two-modes.out" 2>&1; then
    fail "two modes unexpectedly succeeded"
fi

if "$script" --list --nonsense >"$tmp/bad-flag.out" 2>&1; then
    fail "unknown flag unexpectedly succeeded"
fi

if "$script" --list --map "$tmp/does-not-exist.md" >"$tmp/bad-map.out" 2>&1; then
    fail "missing map file unexpectedly succeeded"
fi
grep -q "map file not found" "$tmp/bad-map.out" \
    || fail "missing map file did not produce a clear error"

if "$script" --list --base HEAD >"$tmp/bad-base-combo.out" 2>&1; then
    fail "--base without --changed unexpectedly succeeded"
fi

if "$script" --changed --brief >"$tmp/bad-brief-combo.out" 2>&1; then
    fail "--brief with --changed unexpectedly succeeded"
fi

if "$script" --feature alpha --run nonsense >"$tmp/bad-run.out" 2>&1; then
    fail "invalid --run kind unexpectedly succeeded"
fi
grep -q "invalid --run kind" "$tmp/bad-run.out" \
    || fail "invalid --run kind did not produce a clear error"

echo "agent-context tests passed."
