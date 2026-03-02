#!/usr/bin/env bash
# tests/integration/remi/runner/test-runner.sh
# Remi integration test suite - runs inside container against live packages.conary.io

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ── Configuration ─────────────────────────────────────────────────────────────

CONARY="${CONARY_BIN:-/usr/local/bin/conary}"
DB_PATH="${DB_PATH:-/var/lib/conary/conary.db}"
REMI_ENDPOINT="https://packages.conary.io"
REMI_DISTRO="fedora"
REPO_NAME="fedora-remi"
# Remi-native sync: repo URL is the Remi endpoint itself.
# No separate upstream URL needed - metadata comes from /v1/{distro}/metadata.
REPO_URL="$REMI_ENDPOINT"
TEST_PACKAGE="which"
TEST_BINARY="/usr/bin/which"

export DISTRO="${DISTRO:-fedora43}"
export RESULTS_DIR="${RESULTS_DIR:-/results}"

# Ensure DB directory exists
mkdir -p "$(dirname "$DB_PATH")"

echo ""
echo "════════════════════════════════════════════════════"
echo "  Remi Integration Tests"
echo "  Distro:   $DISTRO"
echo "  Endpoint: $REMI_ENDPOINT"
echo "  Binary:   $CONARY"
echo "  DB:       $DB_PATH"
echo "════════════════════════════════════════════════════"
echo ""

# ── T00: Initialize Database ──────────────────────────────────────────────────

# Initialize the Conary DB. system init creates default repos - we'll remove
# them to keep syncs fast and focused on just the Remi-backed repo.
echo "[SETUP] Initializing database at $DB_PATH ..."
"$CONARY" system init --db-path "$DB_PATH" 2>/dev/null || {
    echo "FATAL: Failed to initialize database"
    exit 1
}
# Remove default repos to avoid slow syncs and name conflicts
for default_repo in arch-core arch-extra arch-multilib fedora-43 ubuntu-noble; do
    "$CONARY" repo remove "$default_repo" --db-path "$DB_PATH" 2>/dev/null || true
done
echo "[SETUP] Database ready"
echo ""

# ── T01: Health Check ─────────────────────────────────────────────────────────

test_health_check() {
    curl -sf "${REMI_ENDPOINT}/health" >/dev/null
}

run_test "T01" "health_check" 10 test_health_check

# If health check failed, skip everything else
if [ "$_FAIL_COUNT" -gt 0 ]; then
    echo ""
    echo "Remi server unreachable at $REMI_ENDPOINT - skipping remaining tests"
    set_fatal
fi

# ── T02: Repo Add ────────────────────────────────────────────────────────────

test_repo_add() {
    "$CONARY" repo add "$REPO_NAME" "$REPO_URL" \
        --db-path "$DB_PATH" \
        --default-strategy remi \
        --remi-distro "$REMI_DISTRO" \
        --no-gpg-check \
        2>&1
}

run_test "T02" "repo_add" 10 test_repo_add

# ── T03: Repo List ───────────────────────────────────────────────────────────

test_repo_list() {
    local output
    output=$("$CONARY" repo list --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$REPO_NAME" "$output"
}

run_test "T03" "repo_list" 10 test_repo_list

# ── T04: Repo Sync ───────────────────────────────────────────────────────────

test_repo_sync() {
    local output
    output=$("$CONARY" repo sync "$REPO_NAME" --db-path "$DB_PATH" --force 2>&1)
    # Check for successful sync indicator
    assert_output_contains "[OK]" "$output"
}

_FAILS_BEFORE_SYNC=$_FAIL_COUNT
run_test "T04" "repo_sync" 300 test_repo_sync

# If sync failed, skip package operation tests
if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_SYNC" ]; then
    echo ""
    echo "Repo sync failed - skipping package operation tests (T05-T12)"
    set_fatal
fi

# ── T05: Search Exists ───────────────────────────────────────────────────────

test_search_exists() {
    local output
    output=$("$CONARY" search "$TEST_PACKAGE" --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_PACKAGE" "$output"
    assert_output_not_contains "No packages found" "$output"
}

run_test "T05" "search_exists" 30 test_search_exists

# ── T06: Search Nonexistent ──────────────────────────────────────────────────

test_search_nonexistent() {
    local output
    output=$("$CONARY" search "zzz-nonexistent-pkg-12345" --db-path "$DB_PATH" 2>&1)
    assert_output_contains "No packages found" "$output"
}

run_test "T06" "search_nonexistent" 10 test_search_nonexistent

# ── T07: Install Package ─────────────────────────────────────────────────────

test_install_package() {
    "$CONARY" install "$TEST_PACKAGE" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --no-deps \
        --sandbox never \
        2>&1
}

run_test "T07" "install_package" 300 test_install_package

# ── T08: Verify Files ────────────────────────────────────────────────────────

test_verify_files() {
    assert_file_exists "$TEST_BINARY"
    assert_file_executable "$TEST_BINARY"
}

run_test "T08" "verify_files" 10 test_verify_files

# ── T09: List Installed ──────────────────────────────────────────────────────

test_list_installed() {
    local output
    output=$("$CONARY" list --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_PACKAGE" "$output"
}

run_test "T09" "list_installed" 10 test_list_installed

# ── T10: Install Nonexistent ─────────────────────────────────────────────────

test_install_nonexistent() {
    local output exit_code
    output=$("$CONARY" install "zzz-nonexistent-pkg-12345" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --no-deps \
        --sandbox never \
        2>&1) && exit_code=0 || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        echo "expected non-zero exit code for nonexistent package" >&2
        return 1
    fi
    # Should fail gracefully (not crash/segfault)
    return 0
}

run_test "T10" "install_nonexistent" 30 test_install_nonexistent

# ── T11: Remove Package ──────────────────────────────────────────────────────

test_remove_package() {
    "$CONARY" remove "$TEST_PACKAGE" \
        --db-path "$DB_PATH" \
        --no-scripts \
        2>&1
}

run_test "T11" "remove_package" 60 test_remove_package

# ── T12: Verify Removed ──────────────────────────────────────────────────────

test_verify_removed() {
    assert_file_not_exists "$TEST_BINARY"

    local output
    output=$("$CONARY" list --db-path "$DB_PATH" 2>&1)
    # Should either be empty or not contain the test package
    if echo "$output" | grep -qF "$TEST_PACKAGE"; then
        echo "package '$TEST_PACKAGE' still appears in list after removal" >&2
        return 1
    fi
}

run_test "T12" "verify_removed" 10 test_verify_removed

# ── Finalize ──────────────────────────────────────────────────────────────────

finalize_results
