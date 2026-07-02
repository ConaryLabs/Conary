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
`docs/alpha.md`

## Paths owned
`a/*`

## Neighbor systems
beta runtime.

## Focused proof
`true`
`echo alpha-focused`

## Interaction gate
`echo alpha-gate`
when: alpha crosses beta.

## Docs to update
`docs/alpha.md`

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

# --- routing: most-specific wins, fallback table, no-hint ---

path_brief_out="$("$script" --path a/b.rs --brief --map "$fixture_map")"
grep -q '^Beta Feature |' <<<"$path_brief_out" \
    || fail "a/b.rs did not route to the more specific Beta card; got: $path_brief_out"

path_brief_out="$("$script" --path a/alpha.rs --brief --map "$fixture_map")"
grep -q '^Alpha Feature |' <<<"$path_brief_out" \
    || fail "a/alpha.rs did not route to Alpha; got: $path_brief_out"

"$script" --path a/alpha.rs --map "$fixture_map" > "$tmp/path-full.out"
grep -q '^# Task Packet: Alpha Feature$' "$tmp/path-full.out" \
    || fail "--path without --brief did not print the full packet"

fallback_out="$("$script" --path docs/superpowers/specs/2099-01-01-example-design.md --map "$fixture_map")"
grep -q '^Planning docs |' <<<"$fallback_out" \
    || fail "specs path did not use the planning fallback; got: $fallback_out"

fallback_out="$("$script" --path docs/modules/anything-at-all.md --map "$fixture_map")"
grep -q '^Canonical docs |' <<<"$fallback_out" \
    || fail "docs/modules path did not use the canonical docs fallback"

fallback_out="$("$script" --path AGENTS.md --map "$fixture_map")"
grep -q '^Assistant/contributor guidance |' <<<"$fallback_out" \
    || fail "AGENTS.md did not use the guidance fallback"

nohint_out="$("$script" --path zzz/nowhere.c --map "$fixture_map")"
grep -q '^No feature-card hint matched' <<<"$nohint_out" \
    || fail "unmatched path did not print the no-hint message"

# --- --changed and --changed --all collection (fixture git repo) ---

changed_repo="$tmp/changed-repo"
mkdir -p "$changed_repo/a"
git -C "$changed_repo" init -q
git -C "$changed_repo" config user.email "test@example.com"
git -C "$changed_repo" config user.name "test"
write_fixture_map "$changed_repo/map.md"
printf 'tracked one\n' > "$changed_repo/t1.rs"
printf 'alpha\n' > "$changed_repo/a/alpha.rs"
git -C "$changed_repo" add -A
git -C "$changed_repo" commit -qm init

printf 'tracked one modified\n' > "$changed_repo/t1.rs"
printf 'staged\n' > "$changed_repo/staged.rs"
git -C "$changed_repo" add staged.rs
printf 'untracked\n' > "$changed_repo/untracked.rs"

changed_out="$( (cd "$changed_repo" && bash "$script" --changed --map map.md) )"
grep -q '^changed_paths: 3$' <<<"$changed_out" \
    || fail "--changed did not count modified+staged+untracked; got: $changed_out"
grep -q -- '^- t1.rs$' <<<"$changed_out" || fail "--changed missed modified path"
grep -q -- '^- staged.rs$' <<<"$changed_out" || fail "--changed missed staged path"
grep -q -- '^- untracked.rs$' <<<"$changed_out" || fail "--changed missed untracked path"
if grep -q -- '^- a/alpha.rs$' <<<"$changed_out"; then
    fail "--changed included an unchanged tracked path"
fi

all_out="$( (cd "$changed_repo" && bash "$script" --changed --all --map map.md) )"
grep -q -- '^- a/alpha.rs$' <<<"$all_out" \
    || fail "--changed --all missed a tracked path"
grep -A1 -- '^- a/alpha.rs$' <<<"$all_out" | grep -q 'Alpha Feature |' \
    || fail "--changed --all did not route a/alpha.rs to Alpha"
grep -A1 -- '^- t1.rs$' <<<"$all_out" | grep -q 'No feature-card hint matched' \
    || fail "--changed --all did not print no-hint for unrouted path"

if (cd "$changed_repo" && bash "$script" --changed --base definitely-not-a-ref --map map.md) >"$tmp/bad-base.out" 2>&1; then
    fail "invalid base ref unexpectedly succeeded"
fi
grep -q "base ref not found" "$tmp/bad-base.out" \
    || fail "invalid base ref did not print a clear error"

clean_out="$( (cd "$changed_repo" && git stash -q --include-untracked && bash "$script" --changed --map map.md) )"
grep -q '^\[ok\] no changed paths detected$' <<<"$clean_out" \
    || fail "clean tree did not report no changed paths"

# --- --validate: good map passes; six distinct violations fail ---

make_validate_repo() {
    local dir="$1"
    mkdir -p "$dir/a" "$dir/b" "$dir/docs"
    git -C "$dir" init -q
    git -C "$dir" config user.email "test@example.com"
    git -C "$dir" config user.name "test"
    printf 'alpha\n' > "$dir/a/alpha.rs"
    printf 'b\n' > "$dir/a/b.rs"
    printf 'x\n' > "$dir/b/x.rs"
    printf 'alpha docs\n' > "$dir/docs/alpha.md"
    printf 'beta docs\n' > "$dir/docs/beta.md"
    git -C "$dir" add a b docs
    write_fixture_map "$dir/map.md"
}

run_validate_expect_fail() {
    local dir="$1" expect="$2" out
    if out="$( (cd "$dir" && bash "$script" --map map.md --validate) 2>&1 )"; then
        fail "validate unexpectedly passed; wanted error: $expect"
    fi
    grep -q "$expect" <<<"$out" \
        || fail "validate error missing '$expect'; got: $out"
}

good_repo="$tmp/validate-good"
make_validate_repo "$good_repo"
good_out="$( (cd "$good_repo" && bash "$script" --map map.md --validate) )"
grep -q "validation passed" <<<"$good_out" \
    || fail "well-formed fixture map did not validate; got: $good_out"

vr="$tmp/validate-missing-field"
make_validate_repo "$vr"
sed -i '/^\*\*Safety notes:\*\* never break beta invariants\.$/d' "$vr/map.md"
run_validate_expect_fail "$vr" "card 'Beta Feature' is missing field: Safety notes"

vr="$tmp/validate-dup-slug"
make_validate_repo "$vr"
sed -i 's/^\*\*Slug:\*\* beta$/**Slug:** alpha/' "$vr/map.md"
run_validate_expect_fail "$vr" "duplicates slug: alpha"

vr="$tmp/validate-dead-glob"
make_validate_repo "$vr"
sed -i 's|`b/\*`|`c/*`|' "$vr/map.md"
run_validate_expect_fail "$vr" "dead Paths glob: c/\*"

vr="$tmp/validate-overlap"
make_validate_repo "$vr"
cat >> "$vr/map.md" <<'EOF'

## Gamma Feature

**Slug:** gamma

**Capability:** own gamma things.

**Start here:** `a/alpha.rs`.

**Neighbor systems:** alpha runtime.

**Paths:** `a/*`.

**Focused proof:** `echo gamma-focused`.

**Interaction gate:** `echo gamma-gate`.

**Docs to update:** `docs/alpha.md`.

**Safety notes:** never break gamma invariants.
EOF
run_validate_expect_fail "$vr" "equal-specificity Paths overlap for a/alpha.rs"

vr="$tmp/validate-missing-start"
make_validate_repo "$vr"
sed -i 's|`a/alpha.rs`;|`a/missing.rs`;|' "$vr/map.md"
run_validate_expect_fail "$vr" "references untracked path: a/missing.rs"

vr="$tmp/validate-no-proof-command"
make_validate_repo "$vr"
sed -i 's/^\*\*Focused proof:\*\* `true`; `echo alpha-focused`\.$/**Focused proof:** run the alpha tests by hand./' "$vr/map.md"
run_validate_expect_fail "$vr" "Focused proof has no backticked command"

# --- --run focused|gate: executes card commands, fail-fast ---

run_out="$("$script" --feature alpha --run focused --map "$fixture_map")"
grep -q '^+ true$' <<<"$run_out" || fail "--run did not echo the first command"
grep -q '^alpha-focused$' <<<"$run_out" || fail "--run did not execute echo command"
grep -q 'command(s) passed for Alpha Feature' <<<"$run_out" \
    || fail "--run did not print the success footer"

run_out="$("$script" --feature alpha --run gate --map "$fixture_map")"
grep -q '^alpha-gate$' <<<"$run_out" || fail "--run gate did not execute the gate command"

failing_map="$tmp/failing-map.md"
write_fixture_map "$failing_map"
sed -i 's/^\*\*Focused proof:\*\* `true`; `echo alpha-focused`\.$/**Focused proof:** `false`; `echo never-runs`./' "$failing_map"
if "$script" --feature alpha --run focused --map "$failing_map" >"$tmp/run-fail.out" 2>&1; then
    fail "--run with failing command unexpectedly succeeded"
fi
if grep -q "never-runs" "$tmp/run-fail.out"; then
    fail "--run did not stop at the first failing command"
fi
grep -q "command failed: false" "$tmp/run-fail.out" \
    || fail "--run failure did not name the failing command"

# --- real-map smoke assertions (default --map) ---

"$script" --validate >/dev/null \
    || fail "real feature-ownership map failed --validate"

real_list="$("$script" --list)"
grep -q $'^packaging\t' <<<"$real_list" || fail "real map --list missing packaging slug"
grep -q $'^profiles\t' <<<"$real_list" || fail "real map --list missing profiles slug"
[[ "$(wc -l <<<"$real_list")" -eq 13 ]] || fail "real map --list did not print 13 cards"

"$script" --path apps/conary/src/commands/install/mod.rs > "$tmp/real-install.out"
grep -q '^slug: install$' "$tmp/real-install.out" \
    || fail "install path did not route to the install card"
"$script" --path apps/remi/src/server/mcp.rs > "$tmp/real-mcp.out"
grep -q '^slug: agent-mcp$' "$tmp/real-mcp.out" \
    || fail "remi mcp.rs did not route to agent-mcp (specificity)"
"$script" --path apps/conary-test/src/bootstrap.rs > "$tmp/real-bootstrap.out"
grep -q '^slug: bootstrap$' "$tmp/real-bootstrap.out" \
    || fail "conary-test bootstrap.rs did not route to bootstrap (specificity)"
"$script" --path apps/remi/src/federation/mod.rs > "$tmp/real-federation.out"
grep -q '^slug: remi$' "$tmp/real-federation.out" \
    || fail "federation path did not fold into the remi card"

echo "agent-context tests passed."
