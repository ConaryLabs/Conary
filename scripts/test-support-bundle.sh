#!/usr/bin/env bash
# scripts/test-support-bundle.sh
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="$repo_root/scripts/conary-support-bundle.sh"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_file() {
    local file="$1"
    [[ -f "$file" ]] || fail "expected file: $file"
}

assert_contains() {
    local file="$1"
    local needle="$2"
    rg -q --fixed-strings "$needle" "$file" || fail "expected $file to contain: $needle"
}

assert_not_contains() {
    local file="$1"
    local needle="$2"
    if rg -q --fixed-strings "$needle" "$file"; then
        fail "expected $file not to contain: $needle"
    fi
}

assert_no_path_named() {
    local root="$1"
    local name="$2"
    if find "$root" -name "$name" -print -quit | rg -q .; then
        fail "bundle unexpectedly contains path named $name"
    fi
}

tmp="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT

fake_bin="$tmp/bin"
bundle="$tmp/bundle"
db_dir="$tmp/db"
mkdir -p "$fake_bin" "$db_dir"

cat > "$fake_bin/conary" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
    "--version")
        printf 'conary 0.8.0\n'
        ;;
    "system adopt --status")
        printf 'Conary Adoption Status\n  Adopted (track): 2\n'
        ;;
    "system generation list")
        printf 'No generations found\n'
        ;;
    "system generation pending")
        printf 'No pending generation publication debt\n'
        ;;
    "repo list")
        printf 'remi https://user:password@example.invalid/repo?access_token=token123\n'
        printf 'Authorization: Bearer secret-token\n'
        printf 'X-Remi-Admin-Token: admin-secret\n'
        ;;
    *)
        printf 'unexpected conary command: %s\n' "$*" >&2
        exit 64
        ;;
esac
EOF
chmod +x "$fake_bin/conary"

cat > "$fake_bin/sqlite3" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
    *"PRAGMA integrity_check;"*)
        printf 'ok\n'
        ;;
    *"SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"*)
        printf 'changesets\nrepositories\ntroves\n'
        ;;
    *)
        printf 'unexpected sqlite3 command: %s\n' "$*" >&2
        exit 64
        ;;
esac
EOF
chmod +x "$fake_bin/sqlite3"

cat > "$fake_bin/lsb_release" <<'EOF'
#!/usr/bin/env bash
printf 'Distributor ID:\tConaryTest\nDescription:\tConary Test Linux\nRelease:\t1\n'
EOF
chmod +x "$fake_bin/lsb_release"

printf 'not a real sqlite db\n' > "$db_dir/conary.db"
printf 'raw log with password=should-not-appear\n' > "$tmp/conary.log"
printf 'env secret=should-not-appear\n' > "$tmp/environment"

PATH="$fake_bin:$PATH" CONARY_DB="$db_dir/conary.db" "$script" "$bundle"

assert_file "$bundle/README.txt"
assert_file "$bundle/conary-version.txt"
assert_file "$bundle/adoption-status.txt"
assert_file "$bundle/generation-list.txt"
assert_file "$bundle/generation-pending.txt"
assert_file "$bundle/repo-list.txt"
assert_file "$bundle/uname.txt"
assert_file "$bundle/os-release.txt"
assert_file "$bundle/db-integrity-check.txt"
assert_file "$bundle/db-tables.txt"

assert_contains "$bundle/README.txt" "Review this local support bundle before attaching it to an issue."
assert_contains "$bundle/README.txt" "does not include conary.db"
assert_contains "$bundle/conary-version.txt" "conary 0.8.0"
assert_contains "$bundle/db-integrity-check.txt" "ok"
assert_contains "$bundle/db-tables.txt" "troves"

assert_not_contains "$bundle/repo-list.txt" "user:password"
assert_not_contains "$bundle/repo-list.txt" "token123"
assert_not_contains "$bundle/repo-list.txt" "secret-token"
assert_not_contains "$bundle/repo-list.txt" "admin-secret"
assert_contains "$bundle/repo-list.txt" "https://[REDACTED]@example.invalid/repo?access_token=[REDACTED]"
assert_contains "$bundle/repo-list.txt" "Authorization: [REDACTED]"
assert_contains "$bundle/repo-list.txt" "X-Remi-Admin-Token: [REDACTED]"

assert_no_path_named "$bundle" "conary.db"
assert_no_path_named "$bundle" "conary.log"
assert_no_path_named "$bundle" "environment"

dirty_bundle="$tmp/dirty-bundle"
mkdir -p "$dirty_bundle"
printf 'do not keep this\n' > "$dirty_bundle/conary.db"
if PATH="$fake_bin:$PATH" CONARY_DB="$db_dir/conary.db" "$script" "$dirty_bundle" > "$tmp/dirty.out" 2> "$tmp/dirty.err"; then
    fail "expected non-empty support bundle directory to be rejected"
fi
assert_contains "$tmp/dirty.err" "Refusing to write support bundle into non-empty directory"

echo "support bundle self-tests passed."
