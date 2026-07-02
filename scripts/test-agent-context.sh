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

# --- fixture map: parsing, --list, --feature, --brief ---

write_fixture_map() {
    cat > "$1" <<'EOF'
# Fixture Ownership Map

## How To Use This Map

Ignore me.

## Card Schema

Fields are described here; this section must not parse as a card.

## Alpha Feature

**Slug:** alpha

**Capability:** own alpha things.

**Start here:** `a/alpha.rs`;
`docs/alpha.md`.

**Neighbor systems:** beta runtime.

**Paths:** `a/*`.

**Focused proof:** `true`; `echo alpha-focused`.

**Interaction gate:** `echo alpha-gate` when alpha crosses beta.

**Docs to update:** `docs/alpha.md`.

**Safety notes:** never break alpha invariants.

## Beta Feature

**Slug:** beta

**Capability:** own beta things.

**Start here:** `a/b.rs`.

**Neighbor systems:** alpha runtime.

**Paths:** `a/b.rs`; `b/*`.

**Focused proof:** `echo beta-focused`.

**Interaction gate:** `echo beta-gate`.

**Docs to update:** `docs/beta.md`.

**Safety notes:** never break beta invariants.
EOF
}

fixture_map="$tmp/map.md"
write_fixture_map "$fixture_map"

list_out="$("$script" --list --map "$fixture_map")"
expected_list="$(printf 'alpha\town alpha things.\nbeta\town beta things.')"
[[ "$list_out" == "$expected_list" ]] \
    || fail "--list output mismatch; got: $list_out"

cat > "$tmp/alpha-packet.expected" <<'EOF'
# Task Packet: Alpha Feature
slug: alpha
capability: own alpha things.

## Read first
`a/alpha.rs`
`docs/alpha.md`.

## Paths owned
`a/*`.

## Neighbor systems
beta runtime.

## Focused proof
`true`
`echo alpha-focused`

## Interaction gate
`echo alpha-gate`
when: alpha crosses beta.

## Docs to update
`docs/alpha.md`.

## Safety invariants
never break alpha invariants.
EOF

"$script" --feature alpha --map "$fixture_map" > "$tmp/alpha-packet.out"
diff -u "$tmp/alpha-packet.expected" "$tmp/alpha-packet.out" \
    || fail "alpha task packet did not match expected format"

brief_out="$("$script" --feature alpha --brief --map "$fixture_map")"
expected_brief='Alpha Feature | focused: true; echo alpha-focused. | gate: echo alpha-gate when alpha crosses beta.'
[[ "$brief_out" == "$expected_brief" ]] \
    || fail "--brief output mismatch; got: $brief_out"

if "$script" --feature no-such-card --map "$fixture_map" >"$tmp/bad-slug.out" 2>&1; then
    fail "unknown slug unexpectedly succeeded"
fi
grep -q "unknown feature slug" "$tmp/bad-slug.out" \
    || fail "unknown slug did not produce a clear error"

echo "agent-context tests passed."
