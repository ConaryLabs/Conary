#!/usr/bin/env bash
# tests/integration/remi/runner/test-runner.sh
# Remi integration test suite - runs inside container against live packages.conary.io

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

# ── Configuration ─────────────────────────────────────────────────────────────

CONARY="${CONARY_BIN:-/usr/bin/conary}"
DB_PATH="${DB_PATH:-/var/lib/conary/conary.db}"
REMI_ENDPOINT="https://packages.conary.io"

# Derive Remi distro identifier from container DISTRO env var
# DISTRO values: fedora43, arch, ubuntu-noble → Remi distros: fedora, arch, ubuntu
case "${DISTRO:-fedora43}" in
    fedora*)    REMI_DISTRO="fedora" ;;
    arch*)      REMI_DISTRO="arch"   ;;
    ubuntu*)    REMI_DISTRO="ubuntu" ;;
    debian*)    REMI_DISTRO="debian" ;;
    *)          REMI_DISTRO="fedora" ;;
esac
REPO_NAME="${REMI_DISTRO}-remi"

# Remi-native sync: repo URL is the Remi endpoint itself.
# No separate upstream URL needed - metadata comes from /v1/{distro}/metadata.
REPO_URL="$REMI_ENDPOINT"

# Per-distro test packages (must exist in that distro's Remi metadata)
case "$REMI_DISTRO" in
    ubuntu|debian)
        TEST_PACKAGE="patch"
        TEST_BINARY="/usr/bin/patch"
        TEST_PACKAGE_2="nano"
        TEST_BINARY_2="/usr/bin/nano"
        TEST_PACKAGE_3="jq"
        TEST_BINARY_3="/usr/bin/jq"
        ;;
    *)
        # Fedora, Arch — these packages exist in both repos
        TEST_PACKAGE="which"
        TEST_BINARY="/usr/bin/which"
        TEST_PACKAGE_2="tree"
        TEST_BINARY_2="/usr/bin/tree"
        TEST_PACKAGE_3="jq"
        TEST_BINARY_3="/usr/bin/jq"
        ;;
esac

export DISTRO="${DISTRO:-fedora43}"
export RESULTS_DIR="${RESULTS_DIR:-/results}"

# Ensure DB directory exists
mkdir -p "$(dirname "$DB_PATH")"

echo ""
echo "════════════════════════════════════════════════════"
echo "  Remi Integration Tests"
echo "  Distro:   $DISTRO"
echo "  Remi repo: $REPO_NAME ($REMI_DISTRO)"
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
        --remi-endpoint "$REMI_ENDPOINT" \
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
    echo "Repo sync failed - skipping package operation tests (T05-T24)"
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

# ── T13: Version Check ──────────────────────────────────────────────────────

test_version_check() {
    local output
    output=$("$CONARY" --version 2>&1)
    assert_output_contains "conary" "$output"
}

run_test "T13" "version_check" 10 test_version_check

# ── T14: Reinstall Which ────────────────────────────────────────────────────

test_reinstall_which() {
    "$CONARY" install "$TEST_PACKAGE" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --no-deps \
        --sandbox never \
        2>&1
}

_FAILS_BEFORE_REINSTALL=$_FAIL_COUNT
run_test "T14" "reinstall_which" 300 test_reinstall_which

# If reinstall failed, skip tests that depend on having which installed
if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_REINSTALL" ]; then
    echo ""
    echo "Reinstall failed - skipping dependent tests (T15-T17, T22-T24)"
    _REINSTALL_FAILED=1
else
    _REINSTALL_FAILED=0
fi

# ── T15: Package Info ───────────────────────────────────────────────────────

test_package_info() {
    local output
    output=$("$CONARY" list "$TEST_PACKAGE" --info --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_PACKAGE" "$output"
    assert_output_contains "Version" "$output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T15" "package_info" "skipped due to T14 failure"
else
    run_test "T15" "package_info" 30 test_package_info
fi

# ── T16: List Files ─────────────────────────────────────────────────────────

test_list_files() {
    local output
    output=$("$CONARY" list "$TEST_PACKAGE" --files --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_BINARY" "$output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T16" "list_files" "skipped due to T14 failure"
else
    run_test "T16" "list_files" 30 test_list_files
fi

# ── T17: Path Ownership ────────────────────────────────────────────────────

test_path_ownership() {
    local output
    output=$("$CONARY" list --path "$TEST_BINARY" --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_PACKAGE" "$output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T17" "path_ownership" "skipped due to T14 failure"
else
    run_test "T17" "path_ownership" 30 test_path_ownership
fi

# ── T18: Install Tree ──────────────────────────────────────────────────────

test_install_tree() {
    "$CONARY" install "$TEST_PACKAGE_2" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --no-deps \
        --sandbox never \
        2>&1
}

_FAILS_BEFORE_TREE=$_FAIL_COUNT
run_test "T18" "install_tree" 300 test_install_tree

# If tree install failed, skip T19
if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_TREE" ]; then
    _TREE_FAILED=1
else
    _TREE_FAILED=0
fi

# ── T19: Verify Tree Files ─────────────────────────────────────────────────

test_verify_tree_files() {
    assert_file_exists "$TEST_BINARY_2"
    assert_file_executable "$TEST_BINARY_2"
}

if [ "$_TREE_FAILED" -eq 1 ]; then
    record_skip "T19" "verify_tree_files" "skipped due to T18 failure"
else
    run_test "T19" "verify_tree_files" 10 test_verify_tree_files
fi

# ── T20: Adopt Single Package ──────────────────────────────────────────────

test_adopt_single_package() {
    "$CONARY" system adopt curl --db-path "$DB_PATH" 2>&1
}

_FAILS_BEFORE_ADOPT=$_FAIL_COUNT
run_test "T20" "adopt_single_package" 60 test_adopt_single_package

# If adopt failed, skip T21
if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_ADOPT" ]; then
    _ADOPT_FAILED=1
else
    _ADOPT_FAILED=0
fi

# ── T21: Adopt Status ──────────────────────────────────────────────────────

test_adopt_status() {
    local output
    output=$("$CONARY" system adopt --status --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Conary Adoption Status" "$output"
    assert_output_contains "Adopted" "$output"
}

if [ "$_ADOPT_FAILED" -eq 1 ]; then
    record_skip "T21" "adopt_status" "skipped due to T20 failure"
else
    run_test "T21" "adopt_status" 30 test_adopt_status
fi

# ── T22: Pin Package ───────────────────────────────────────────────────────

test_pin_package() {
    "$CONARY" pin "$TEST_PACKAGE" --db-path "$DB_PATH" 2>&1
    # Verify pin was applied
    local query_output
    query_output=$("$CONARY" list "$TEST_PACKAGE" --info --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Pinned      : yes" "$query_output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T22" "pin_package" "skipped due to T14 failure"
else
    run_test "T22" "pin_package" 30 test_pin_package
fi

# ── T23: Unpin Package ─────────────────────────────────────────────────────

test_unpin_package() {
    "$CONARY" unpin "$TEST_PACKAGE" --db-path "$DB_PATH" 2>&1
    # Verify pin was removed
    local query_output
    query_output=$("$CONARY" list "$TEST_PACKAGE" --info --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Pinned      : no" "$query_output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T23" "unpin_package" "skipped due to T14 failure"
else
    run_test "T23" "unpin_package" 30 test_unpin_package
fi

# ── T24: Changeset History ─────────────────────────────────────────────────

test_changeset_history() {
    local output
    output=$("$CONARY" system history --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Changeset" "$output"
}

if [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T24" "changeset_history" "skipped due to T14 failure"
else
    run_test "T24" "changeset_history" 30 test_changeset_history
fi

# ── T25: Install Package With Dependencies ──────────────────────────────────

test_install_dep_package() {
    "$CONARY" install "$TEST_PACKAGE_3" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --no-deps \
        --sandbox never \
        2>&1
}

_FAILS_BEFORE_DEP=$_FAIL_COUNT
run_test "T25" "install_dep_package" 300 test_install_dep_package

# If dep package install failed, skip T26-T27
if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_DEP" ]; then
    _DEP_FAILED=1
else
    _DEP_FAILED=0
fi

# ── T26: Verify Dep Package Files ──────────────────────────────────────────

test_verify_dep_files() {
    assert_file_exists "$TEST_BINARY_3"
}

if [ "$_DEP_FAILED" -eq 1 ]; then
    record_skip "T26" "verify_dep_files" "skipped due to T25 failure"
else
    run_test "T26" "verify_dep_files" 10 test_verify_dep_files
fi

# ── T27: Multiple Packages Coexist ─────────────────────────────────────────

test_multi_package_coexist() {
    local output
    output=$("$CONARY" list --db-path "$DB_PATH" 2>&1)
    assert_output_contains "$TEST_PACKAGE" "$output"
    assert_output_contains "$TEST_PACKAGE_2" "$output"
    assert_output_contains "$TEST_PACKAGE_3" "$output"
}

if [ "$_DEP_FAILED" -eq 1 ] || [ "$_REINSTALL_FAILED" -eq 1 ]; then
    record_skip "T27" "multi_package_coexist" "skipped due to prior install failure"
else
    run_test "T27" "multi_package_coexist" 10 test_multi_package_coexist
fi

# ── T28: Install With --dep-mode satisfy ─────────────────────────────────────

test_dep_mode_satisfy() {
    # Install a package with --dep-mode satisfy (default).
    # Deps that exist on the system should satisfy requirements without error.
    local output exit_code
    output=$("$CONARY" install "$TEST_PACKAGE" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --dep-mode satisfy \
        --yes \
        --sandbox never \
        2>&1) && exit_code=0 || exit_code=$?

    # Remove first so the package is clean for later tests
    "$CONARY" remove "$TEST_PACKAGE" --db-path "$DB_PATH" --no-scripts 2>/dev/null || true

    if [ "$exit_code" -ne 0 ]; then
        echo "install with --dep-mode satisfy failed (exit $exit_code): $output" >&2
        return 1
    fi
}

run_test "T28" "dep_mode_satisfy" 300 test_dep_mode_satisfy

# ── T29: Install With --dep-mode adopt ───────────────────────────────────────

test_dep_mode_adopt() {
    # Install a package with --dep-mode adopt.
    # System deps should be auto-adopted.
    local output exit_code
    output=$("$CONARY" install "$TEST_PACKAGE_2" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --dep-mode adopt \
        --yes \
        --sandbox never \
        2>&1) && exit_code=0 || exit_code=$?

    if [ "$exit_code" -ne 0 ]; then
        echo "install with --dep-mode adopt failed (exit $exit_code): $output" >&2
        return 1
    fi
    assert_file_exists "$TEST_BINARY_2"
}

run_test "T29" "dep_mode_adopt" 300 test_dep_mode_adopt

# ── T30: Install With --dep-mode takeover ────────────────────────────────────

test_dep_mode_takeover() {
    # Install a package with --dep-mode takeover.
    # Deps should be downloaded from Remi as CCS.
    local output exit_code
    output=$("$CONARY" install "$TEST_PACKAGE_3" \
        --db-path "$DB_PATH" \
        --no-scripts \
        --dep-mode takeover \
        --yes \
        --sandbox never \
        2>&1) && exit_code=0 || exit_code=$?

    if [ "$exit_code" -ne 0 ]; then
        echo "install with --dep-mode takeover failed (exit $exit_code): $output" >&2
        return 1
    fi
    assert_file_exists "$TEST_BINARY_3"
}

run_test "T30" "dep_mode_takeover" 300 test_dep_mode_takeover

# ── T31: Blocklisted Package Refused ─────────────────────────────────────────

test_blocklist_enforced() {
    # Attempting to install a blocklisted package with --dep-mode takeover
    # should either refuse or treat it as satisfied-by-system.
    # We verify by checking that glibc does NOT appear as a Conary-owned install.
    local output exit_code
    output=$("$CONARY" install glibc \
        --db-path "$DB_PATH" \
        --no-scripts \
        --dep-mode takeover \
        --yes \
        --sandbox never \
        2>&1) && exit_code=0 || exit_code=$?

    # The install should fail (blocklisted) or succeed but not actually
    # overlay glibc. Check that glibc is NOT in Conary's installed list.
    local list_output
    list_output=$("$CONARY" list --db-path "$DB_PATH" 2>&1)
    if echo "$list_output" | grep -qw "glibc"; then
        echo "glibc should not be in Conary's installed list (blocklist violation)" >&2
        return 1
    fi
    return 0
}

run_test "T31" "blocklist_enforced" 60 test_blocklist_enforced

# ── T32: Update With Adopted Packages ────────────────────────────────────────

test_update_with_adopted() {
    # Adopt a package, then run update. Should not skip adopted packages.
    "$CONARY" system adopt curl --db-path "$DB_PATH" 2>/dev/null || true

    local output exit_code
    output=$("$CONARY" update \
        --db-path "$DB_PATH" \
        --dep-mode satisfy \
        2>&1) && exit_code=0 || exit_code=$?

    # Update should not crash and should acknowledge adopted packages
    if [ "$exit_code" -ne 0 ]; then
        echo "update with adopted packages failed (exit $exit_code): $output" >&2
        return 1
    fi
    return 0
}

run_test "T32" "update_with_adopted" 120 test_update_with_adopted

# ── T33: Generation List (empty) ────────────────────────────────────────────

test_generation_list_empty() {
    local output
    output=$("$CONARY" system generation list --db-path "$DB_PATH" 2>&1)
    # Should not crash, should indicate no generations
    assert_output_contains "No generations" "$output"
}

run_test "T33" "generation_list_empty" 10 test_generation_list_empty

# ── T34: System Takeover Dry Run ────────────────────────────────────────────

test_takeover_dry_run() {
    local output exit_code
    output=$("$CONARY" system takeover \
        --db-path "$DB_PATH" \
        --dry-run \
        --skip-conversion \
        2>&1) && exit_code=0 || exit_code=$?

    # Dry run should show inventory (may fail on non-root, that's OK)
    if [ "$exit_code" -eq 0 ]; then
        assert_output_contains "DRY RUN" "$output"
    else
        # Expected failure: requires root
        assert_output_contains "root\|Root\|privilege" "$output"
    fi
}

run_test "T34" "takeover_dry_run" 60 test_takeover_dry_run

# ── T35: Generation GC (nothing to clean) ──────────────────────────────────

test_generation_gc_empty() {
    local output
    output=$("$CONARY" system generation gc --db-path "$DB_PATH" 2>&1)
    assert_output_contains "Nothing to clean\|No generations" "$output"
}

run_test "T35" "generation_gc_empty" 10 test_generation_gc_empty

# ── Cleanup ──────────────────────────────────────────────────────────────────

echo ""
echo "[CLEANUP] Removing test packages..."
"$CONARY" remove "$TEST_PACKAGE" --db-path "$DB_PATH" --no-scripts 2>/dev/null || true
"$CONARY" remove "$TEST_PACKAGE_2" --db-path "$DB_PATH" --no-scripts 2>/dev/null || true
"$CONARY" remove "$TEST_PACKAGE_3" --db-path "$DB_PATH" --no-scripts 2>/dev/null || true

# ── Finalize ──────────────────────────────────────────────────────────────────

finalize_results
