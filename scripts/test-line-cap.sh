#!/usr/bin/env bash
# scripts/test-line-cap.sh
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
checker="$repo_root/scripts/check-line-cap.sh"
[[ -x "$checker" ]] || {
    echo "ERROR: scripts/check-line-cap.sh is not executable" >&2
    exit 1
}

fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT
mkdir -p "$fixture_root/crates/fixture/src/tests"
allowlist="$fixture_root/allowlist.txt"
: > "$allowlist"

write_lines() {
    local path="$1"
    local count="$2"
    awk -v count="$count" 'BEGIN { for (i = 1; i <= count; i++) print "// fixture line " i }' > "$path"
}

write_lines "$fixture_root/crates/fixture/src/at_cap.rs" 1000
write_lines "$fixture_root/crates/fixture/src/inline_tests.rs" 900
{
cat <<'EOF'
#[cfg(test)]
mod tests {
EOF
awk 'BEGIN { for (i = 1; i <= 150; i++) print "    // test line " i }'
echo '}'
} >> "$fixture_root/crates/fixture/src/inline_tests.rs"
{
cat <<'EOF'
#[cfg(test)]
mod tests_at_cap {
EOF
awk 'BEGIN { for (i = 1; i <= 297; i++) print "    // test line " i }'
echo '}'
} > "$fixture_root/crates/fixture/src/inline_tests_at_cap.rs"
write_lines "$fixture_root/crates/fixture/src/tests.rs" 1200
write_lines "$fixture_root/crates/fixture/src/tests/helper.rs" 1200

"$checker" --root "$fixture_root" --allowlist "$allowlist" >/dev/null

{
awk 'BEGIN { for (i = 1; i <= 400; i++) print "// fixture line " i }'
cat <<'EOF'
#[cfg(test)]
mod middle_tests {
    const OPEN_BRACE: &str = "{";
    // }
}
EOF
awk 'BEGIN { for (i = 1; i <= 300; i++) print "// middle production line " i }'
cat <<'EOF'
#[cfg(test)] mod later_tests {
    const CLOSE_BRACE: &str = r#"}"#;
}
EOF
awk 'BEGIN { for (i = 1; i <= 301; i++) print "// trailing production line " i }'
} > "$fixture_root/crates/fixture/src/production_after_inline.rs"
if "$checker" --root "$fixture_root" --allowlist "$allowlist" >"$fixture_root/after-inline.out" 2>&1; then
    echo "ERROR: production after an inline test module was not counted" >&2
    exit 1
fi
grep -q 'production_after_inline.rs has 1001 non-test lines' "$fixture_root/after-inline.out"
rm "$fixture_root/crates/fixture/src/production_after_inline.rs"

write_lines "$fixture_root/crates/fixture/src/over_cap.rs" 1001
if "$checker" --root "$fixture_root" --allowlist "$allowlist" >"$fixture_root/over.out" 2>&1; then
    echo "ERROR: unallowlisted over-cap fixture unexpectedly passed" >&2
    exit 1
fi
grep -q 'over_cap.rs has 1001 non-test lines' "$fixture_root/over.out"

echo 'crates/fixture/src/over_cap.rs #846' > "$allowlist"
"$checker" --root "$fixture_root" --allowlist "$allowlist" >/dev/null

rm "$fixture_root/crates/fixture/src/over_cap.rs"
if "$checker" --root "$fixture_root" --allowlist "$allowlist" >"$fixture_root/stale.out" 2>&1; then
    echo "ERROR: stale allowlist fixture unexpectedly passed" >&2
    exit 1
fi
grep -q 'stale line-cap allowlist entry' "$fixture_root/stale.out"

: > "$allowlist"
{
cat <<'EOF'
#[cfg(test)]
mod oversized_tests {
EOF
awk 'BEGIN { for (i = 1; i <= 298; i++) print "    // test line " i }'
echo '}'
} > "$fixture_root/crates/fixture/src/oversized_inline_tests.rs"
if "$checker" --root "$fixture_root" --allowlist "$allowlist" >"$fixture_root/inline-size.out" 2>&1; then
    echo "ERROR: oversized inline test module unexpectedly passed" >&2
    exit 1
fi
grep -q 'inline #\[cfg(test)\] module at line 1 with 301 lines' "$fixture_root/inline-size.out"

echo 'crates/fixture/src/oversized_inline_tests.rs #846' > "$allowlist"
"$checker" --root "$fixture_root" --allowlist "$allowlist" >/dev/null
rm "$fixture_root/crates/fixture/src/oversized_inline_tests.rs"

: > "$allowlist"
{
cat <<'EOF'
#[cfg(test)]
mod tests;
EOF
awk 'BEGIN { for (i = 1; i <= 1001; i++) print "// production line " i }'
} > "$fixture_root/crates/fixture/src/external_tests.rs"
if "$checker" --root "$fixture_root" --allowlist "$allowlist" >"$fixture_root/external.out" 2>&1; then
    echo "ERROR: external test module incorrectly truncated the production count" >&2
    exit 1
fi
grep -q 'external_tests.rs has 1003 non-test lines' "$fixture_root/external.out"

echo "line-cap tests passed."
