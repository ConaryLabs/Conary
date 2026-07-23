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
    chmod -R u+rwX "$tmp" 2>/dev/null || true
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
if [[ "$*" != "--version" &&
    "${CONARY_TEST_REQUIRE_PRIVILEGED_DB:-0}" == "1" &&
    "${CONARY_TEST_PRIVILEGED_DB:-0}" != "1" ]]; then
    printf 'attempt to write a readonly database\n' >&2
    exit 1
fi
if [[ "$*" != "--version" &&
    "${CONARY_TEST_REQUIRE_PRIVILEGED_DB:-0}" == "1" &&
    "${CONARY_DB:-}" != "${CONARY_TEST_EXPECT_DB:?}" ]]; then
    printf 'custom database path was not preserved across privilege escalation\n' >&2
    exit 1
fi
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
    "distro info")
        printf 'No distro pin set. System is distro-agnostic.\n'
        printf 'Selection mode: policy (runtime default)\n'
        ;;
    "repo list --all")
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
if [[ "${CONARY_TEST_REQUIRE_PRIVILEGED_DB:-0}" == "1" &&
    "${CONARY_TEST_PRIVILEGED_DB:-0}" != "1" ]]; then
    printf 'attempt to write a readonly database\n' >&2
    exit 1
fi
case "$*" in
    *"PRAGMA integrity_check;"*)
        printf 'ok\n'
        ;;
    *"SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"*)
        printf 'changesets\nrepositories\ntroves\n'
        ;;
    *"SELECT key || '=' || value FROM settings"*)
        printf 'system.host-profile=arch\n'
        ;;
    *)
        printf 'unexpected sqlite3 command: %s\n' "$*" >&2
        exit 64
        ;;
esac
EOF
chmod +x "$fake_bin/sqlite3"

cat > "$fake_bin/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${CONARY_TEST_SUDO_DENY:-0}" == "1" ]]; then
    exit 1
fi
if [[ "${1:-}" == "-n" ]]; then
    shift
fi
if [[ "${1:-}" == "--" ]]; then
    shift
fi
printf '%q ' "$@" >> "${CONARY_TEST_SUDO_LOG:?}"
printf '\n' >> "$CONARY_TEST_SUDO_LOG"
unset CONARY_DB
export CONARY_TEST_PRIVILEGED_DB=1
exec "$@"
EOF
chmod +x "$fake_bin/sudo"

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
assert_file "$bundle/distro-info.txt"
assert_file "$bundle/repo-list.txt"
assert_file "$bundle/uname.txt"
assert_file "$bundle/os-release.txt"
assert_file "$bundle/db-integrity-check.txt"
assert_file "$bundle/db-tables.txt"
assert_file "$bundle/source-settings.txt"

assert_contains "$bundle/README.txt" "Review this local support bundle before attaching it to an issue."
assert_contains "$bundle/README.txt" "does not include conary.db"
assert_contains "$bundle/README.txt" "Database-backed diagnostics used: current user."
assert_contains "$bundle/conary-version.txt" "conary 0.8.0"
assert_contains "$bundle/db-integrity-check.txt" "ok"
assert_contains "$bundle/db-tables.txt" "troves"
assert_contains "$bundle/distro-info.txt" "System is distro-agnostic."
assert_contains "$bundle/source-settings.txt" "system.host-profile=arch"

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

sudo_log="$tmp/sudo.log"
privileged_bundle="$tmp/privileged-bundle"
force_privilege=()
if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    force_privilege=(CONARY_TEST_FORCE_SUPPORT_PRIVILEGE=1)
fi
chmod 0444 "$db_dir/conary.db"
chmod 0555 "$db_dir"
env \
    PATH="$fake_bin:$PATH" \
    CONARY_DB="$db_dir/conary.db" \
    CONARY_TEST_EXPECT_DB="$db_dir/conary.db" \
    CONARY_TEST_REQUIRE_PRIVILEGED_DB=1 \
    CONARY_TEST_SUDO_LOG="$sudo_log" \
    "${force_privilege[@]}" \
    "$script" "$privileged_bundle"

assert_contains "$privileged_bundle/README.txt" "Database-backed diagnostics used: targeted sudo -n."
assert_not_contains "$privileged_bundle/adoption-status.txt" "readonly database"
assert_not_contains "$privileged_bundle/db-integrity-check.txt" "readonly database"
assert_contains "$sudo_log" "conary system adopt --status"
assert_contains "$sudo_log" "conary distro info"
assert_contains "$sudo_log" "conary repo list --all"
assert_contains "$sudo_log" "sqlite3"
assert_not_contains "$sudo_log" "conary --version"

denied_bundle="$tmp/denied-bundle"
if env \
    PATH="$fake_bin:$PATH" \
    CONARY_DB="$db_dir/conary.db" \
    CONARY_TEST_EXPECT_DB="$db_dir/conary.db" \
    CONARY_TEST_REQUIRE_PRIVILEGED_DB=1 \
    CONARY_TEST_SUDO_DENY=1 \
    CONARY_TEST_SUDO_LOG="$sudo_log" \
    "${force_privilege[@]}" \
    "$script" "$denied_bundle" > "$tmp/denied.out" 2> "$tmp/denied.err"; then
    fail "expected unavailable cached sudo authorization to be rejected"
fi
assert_contains "$tmp/denied.err" 'Run "sudo -v", then rerun this command.'
[[ ! -e "$denied_bundle" ]] || fail "denied privilege preflight must not create a bundle"
chmod 0755 "$db_dir"
chmod 0644 "$db_dir/conary.db"

dirty_bundle="$tmp/dirty-bundle"
mkdir -p "$dirty_bundle"
printf 'do not keep this\n' > "$dirty_bundle/conary.db"
if PATH="$fake_bin:$PATH" CONARY_DB="$db_dir/conary.db" "$script" "$dirty_bundle" > "$tmp/dirty.out" 2> "$tmp/dirty.err"; then
    fail "expected non-empty support bundle directory to be rejected"
fi
assert_contains "$tmp/dirty.err" "Refusing to write support bundle into non-empty directory"

echo "support bundle self-tests passed."
