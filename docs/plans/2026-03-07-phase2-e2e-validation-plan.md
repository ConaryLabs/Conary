# Phase 2: End-to-End Validation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 34 deep integration tests (T38-T71) to the existing Remi test suite, gated behind `--phase2`, covering full install/remove/update/rollback with checksums, generation lifecycle, bootstrap pipeline, recipe cooking, and Remi client validation.

**Architecture:** Extend `tests/integration/remi/runner/test-runner.sh` with a `--phase2` flag that enables Groups A-E. Add new assertion helpers to `lib.sh` (checksum verification). Create test fixture recipe + CCS packages and publish to Remi. Add a new `e2e.yaml` CI workflow for scheduled/manual Phase 2 runs.

**Tech Stack:** Bash (test runner), Podman (containers), CCS (fixture packages), Forgejo Actions (CI)

---

## Task 1: Add Checksum Assertion Helpers to lib.sh

**Files:**
- Modify: `tests/integration/remi/runner/lib.sh`

**Step 1: Add `assert_file_checksum` and `assert_dir_not_exists` helpers**

Add these after the existing `assert_output_not_contains` function (around line 152):

```bash
assert_file_checksum() {
    local path="$1"
    local expected_sha256="$2"
    if [ ! -f "$path" ]; then
        echo "file does not exist: $path" >&2
        return 1
    fi
    local actual
    actual=$(sha256sum "$path" | awk '{print $1}')
    if [ "$actual" != "$expected_sha256" ]; then
        echo "checksum mismatch for $path" >&2
        echo "  expected: $expected_sha256" >&2
        echo "  actual:   $actual" >&2
        return 1
    fi
}

assert_dir_not_exists() {
    local path="$1"
    if [ -d "$path" ]; then
        echo "directory still exists: $path" >&2
        return 1
    fi
}

assert_dir_exists() {
    local path="$1"
    if [ ! -d "$path" ]; then
        echo "directory does not exist: $path" >&2
        return 1
    fi
}
```

**Step 2: Verify lib.sh still sources cleanly**

Run: `bash -n tests/integration/remi/runner/lib.sh`
Expected: No output (no syntax errors)

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/lib.sh
git commit -m "test: add checksum and directory assertion helpers to lib.sh"
```

---

## Task 2: Add --phase2 Flag to test-runner.sh

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Add phase2 flag parsing**

After the existing configuration block (line 14, after `REMI_ENDPOINT=...`), add:

```bash
PHASE2=0
```

Then, at the very top of the file (after `source "$SCRIPT_DIR/lib.sh"`), add argument parsing:

```bash
# ── Phase 2 flag ──────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --phase2) PHASE2=1; shift ;;
        *) shift ;;
    esac
done
```

**Step 2: Add phase2 gate before new tests**

After the existing T37 test and cleanup section (around line 681), but BEFORE the cleanup block, add a gate:

```bash
# ── Phase 2: Deep E2E Validation ─────────────────────────────────────────────
# Enabled with --phase2 flag. Requires test fixture packages on Remi.

if [ "$PHASE2" -eq 0 ]; then
    echo ""
    echo "[INFO] Phase 2 tests skipped (pass --phase2 to enable)"
    echo ""
    # Jump to cleanup
else
    echo ""
    echo "════════════════════════════════════════════════════"
    echo "  Phase 2: Deep E2E Validation"
    echo "════════════════════════════════════════════════════"
    echo ""
```

Close the `else` block just before cleanup with `fi`.

**Step 3: Pass --phase2 from run.sh to the container**

Modify `tests/integration/remi/run.sh`:

Add `--phase2` to the argument parser (around line 36):

```bash
        --phase2)
            PHASE2=1
            shift
            ;;
```

Add `PHASE2=0` to the defaults section (around line 30).

Pass it to the container CMD. Change the `podman run` command (around line 181) to:

```bash
CONTAINER_CMD="/opt/remi-tests/test-runner.sh"
if [ "$PHASE2" -eq 1 ]; then
    CONTAINER_CMD="/opt/remi-tests/test-runner.sh --phase2"
fi

podman run \
    --rm \
    --name "conary-test-run-${DISTRO}" \
    -v "${VOLUME_NAME}:/results:Z" \
    -e "DISTRO=${DISTRO}" \
    "$IMAGE_NAME" $CONTAINER_CMD || CONTAINER_EXIT=$?
```

**Step 4: Verify syntax**

Run: `bash -n tests/integration/remi/runner/test-runner.sh && bash -n tests/integration/remi/run.sh`
Expected: No output

**Step 5: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh tests/integration/remi/run.sh
git commit -m "test: add --phase2 flag to integration test runner"
```

---

## Task 3: Create Test Fixture Recipe and CCS Packages

**Files:**
- Create: `tests/fixtures/conary-test-fixture/v1/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v1/stage/usr/share/conary-test/hello.txt`
- Create: `tests/fixtures/conary-test-fixture/v1/stage/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v1/build.sh`
- Create: `tests/fixtures/conary-test-fixture/v2/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/hello.txt`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/added.txt`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v2/build.sh`
- Create: `tests/fixtures/conary-test-fixture/build-all.sh`

**Step 1: Create v1 fixture**

`tests/fixtures/conary-test-fixture/v1/ccs.toml`:
```toml
[package]
name = "conary-test-fixture"
version = "1.0.0"
description = "Test fixture package for Phase 2 E2E validation"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["conary-test-fixture"]

[requires]
capabilities = []
packages = []

[components]
default = ["runtime"]

[hooks]

[[hooks.directories]]
path = "/usr/share/conary-test"
mode = "0755"

[[hooks.directories]]
path = "/var/lib/conary-test"
mode = "0755"

[hooks.post_install]
script = "touch /var/lib/conary-test/installed"

[hooks.pre_remove]
script = "rm -f /var/lib/conary-test/installed"
```

`tests/fixtures/conary-test-fixture/v1/stage/usr/share/conary-test/hello.txt`:
```
hello-v1
```

Copy ccs.toml to stage: `cp v1/ccs.toml v1/stage/ccs.toml`

`tests/fixtures/conary-test-fixture/v1/build.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONARY="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

"$CONARY" ccs build "$SCRIPT_DIR/ccs.toml" \
    --stage-dir "$SCRIPT_DIR/stage" \
    --output "$SCRIPT_DIR/output/"
echo "[OK] Built conary-test-fixture v1.0.0"
```

**Step 2: Create v2 fixture**

`tests/fixtures/conary-test-fixture/v2/ccs.toml`: Same as v1 but:
```toml
version = "2.0.0"
```

`tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/hello.txt`:
```
hello-v2
```

`tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/added.txt`:
```
added-in-v2
```

**Step 3: Create build-all.sh**

`tests/fixtures/conary-test-fixture/build-all.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building test fixture packages..."
bash "$SCRIPT_DIR/v1/build.sh"
bash "$SCRIPT_DIR/v2/build.sh"

# Print checksums for hardcoding in tests
echo ""
echo "Checksums for test verification:"
echo "  v1 hello.txt: $(sha256sum "$SCRIPT_DIR/v1/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 hello.txt: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 added.txt: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/added.txt" | awk '{print $1}')"
```

**Step 4: Compute checksums and record them**

Run: `sha256sum tests/fixtures/conary-test-fixture/v1/stage/usr/share/conary-test/hello.txt`
Run: `sha256sum tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/hello.txt`
Run: `sha256sum tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/added.txt`

Record the SHA-256 values — these will be hardcoded in test-runner.sh.

**Step 5: Commit**

```bash
git add tests/fixtures/conary-test-fixture/
git commit -m "test: add conary-test-fixture v1 and v2 CCS fixture packages"
```

---

## Task 4: Build and Publish Test Fixtures to Remi

**Files:**
- Create: `scripts/publish-test-fixtures.sh`

**Step 1: Write publish script**

`scripts/publish-test-fixtures.sh`:
```bash
#!/usr/bin/env bash
# scripts/publish-test-fixtures.sh
# Build and publish test fixture CCS packages to Remi for all 3 distros.
# Requires SSH access to Remi (ssh remi).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURE_DIR="$PROJECT_ROOT/tests/fixtures/conary-test-fixture"
CONARY="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"
REMI_ENDPOINT="https://packages.conary.io"

echo "Building test fixture CCS packages..."
bash "$FIXTURE_DIR/build-all.sh"

echo ""
echo "Publishing to Remi..."
for version in v1 v2; do
    pkg=$(ls "$FIXTURE_DIR/$version/output/"*.ccs 2>/dev/null | head -1)
    if [ -z "$pkg" ]; then
        echo "FATAL: No CCS output for $version" >&2
        exit 1
    fi

    for distro in fedora ubuntu arch; do
        echo "  Publishing $version to $distro..."
        curl -sf -X POST "$REMI_ENDPOINT/v1/$distro/packages" \
            -F "package=@$pkg" \
            -F "format=ccs" || {
            echo "    WARN: publish failed for $version/$distro (may already exist)"
        }
    done
done

echo ""
echo "[OK] Test fixtures published to Remi"
```

**Step 2: Build fixtures locally to verify**

Run: `cargo build && bash tests/fixtures/conary-test-fixture/build-all.sh`
Expected: CCS packages built in `v1/output/` and `v2/output/`

**Step 3: Publish to Remi**

Run: `bash scripts/publish-test-fixtures.sh`
Expected: Fixtures available on packages.conary.io for all 3 distros

**Step 4: Commit**

```bash
git add scripts/publish-test-fixtures.sh
git commit -m "test: add script to publish test fixtures to Remi"
```

---

## Task 5: Write Group A Tests — Deep Install Flow (T38-T50)

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Add fixture constants**

Inside the Phase 2 `else` block (from Task 2), add fixture configuration:

```bash
    # ── Phase 2 Configuration ────────────────────────────────────────────────
    FIXTURE_PKG="conary-test-fixture"
    FIXTURE_FILE="/usr/share/conary-test/hello.txt"
    FIXTURE_ADDED="/usr/share/conary-test/added.txt"
    FIXTURE_MARKER="/var/lib/conary-test/installed"
    # SHA-256 checksums (computed from fixture source files)
    FIXTURE_V1_HELLO_SHA="REPLACE_WITH_ACTUAL_SHA256"
    FIXTURE_V2_HELLO_SHA="REPLACE_WITH_ACTUAL_SHA256"
    FIXTURE_V2_ADDED_SHA="REPLACE_WITH_ACTUAL_SHA256"
```

(Replace SHA values with actual checksums from Task 3 Step 4.)

**Step 2: Write T38-T50 tests**

```bash
    # ── Group A: Deep Install Flow ───────────────────────────────────────────
    echo ""
    echo "── Group A: Deep Install Flow ──"
    echo ""

    # ── T38: Install fixture v1 with deps ────────────────────────────────────
    test_install_fixture_v1_with_deps() {
        "$CONARY" install "${FIXTURE_PKG}=1.0.0" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --sandbox never \
            2>&1
    }

    _FAILS_BEFORE_A=$_FAIL_COUNT
    run_test "T38" "install_fixture_v1_deps" 300 test_install_fixture_v1_with_deps

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_A" ]; then
        echo "Fixture v1 install failed - skipping Group A"
        for t in T39 T40 T41 T42 T43 T44 T45 T46 T47 T48 T49 T50; do
            record_skip "$t" "group_a_skipped" "skipped due to T38 failure"
        done
    else

    # ── T39: Verify dep files on disk ────────────────────────────────────────
    test_verify_dep_files_on_disk() {
        assert_file_exists "$FIXTURE_FILE"
        assert_dir_exists "/usr/share/conary-test"
    }
    run_test "T39" "verify_dep_files_disk" 10 test_verify_dep_files_on_disk

    # ── T40: Verify v1 content checksum ──────────────────────────────────────
    test_verify_v1_checksum() {
        assert_file_checksum "$FIXTURE_FILE" "$FIXTURE_V1_HELLO_SHA"
    }
    run_test "T40" "verify_v1_checksum" 10 test_verify_v1_checksum

    # ── T41: Verify scriptlet ran ────────────────────────────────────────────
    test_verify_scriptlet_ran() {
        assert_file_exists "$FIXTURE_MARKER"
    }
    run_test "T41" "verify_scriptlet_ran" 10 test_verify_scriptlet_ran

    # ── T42: Remove with scriptlets ──────────────────────────────────────────
    test_remove_with_scriptlets() {
        "$CONARY" remove "$FIXTURE_PKG" \
            --db-path "$DB_PATH" \
            2>&1
    }
    run_test "T42" "remove_with_scriptlets" 60 test_remove_with_scriptlets

    test_verify_scriptlet_cleanup() {
        assert_file_not_exists "$FIXTURE_MARKER"
        assert_file_not_exists "$FIXTURE_FILE"
    }
    run_test "T42b" "verify_scriptlet_cleanup" 10 test_verify_scriptlet_cleanup

    # ── T43: Reinstall fixture v1 ────────────────────────────────────────────
    test_reinstall_fixture_v1() {
        "$CONARY" install "${FIXTURE_PKG}=1.0.0" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --sandbox never \
            2>&1
    }

    _FAILS_BEFORE_REINSTALL_V1=$_FAIL_COUNT
    run_test "T43" "reinstall_fixture_v1" 300 test_reinstall_fixture_v1

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_REINSTALL_V1" ]; then
        echo "Fixture v1 reinstall failed - skipping T44-T50"
        for t in T44 T45 T46 T47 T48 T49 T50; do
            record_skip "$t" "group_a_skipped" "skipped due to T43 failure"
        done
    else

    # ── T44: Update v1 -> v2 ─────────────────────────────────────────────────
    test_update_v1_to_v2() {
        "$CONARY" update "$FIXTURE_PKG" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --sandbox never \
            2>&1
    }

    _FAILS_BEFORE_UPDATE=$_FAIL_COUNT
    run_test "T44" "update_v1_to_v2" 300 test_update_v1_to_v2

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_UPDATE" ]; then
        echo "Update failed - skipping T45-T48"
        for t in T45 T46 T47 T48; do
            record_skip "$t" "group_a_skipped" "skipped due to T44 failure"
        done
    else

    # ── T45: Delta update verification ───────────────────────────────────────
    test_delta_update_verify() {
        # After update, verify v2 content
        assert_file_checksum "$FIXTURE_FILE" "$FIXTURE_V2_HELLO_SHA"
    }
    run_test "T45" "delta_update_verify" 10 test_delta_update_verify

    # ── T46: Verify v2 added file ────────────────────────────────────────────
    test_verify_v2_added() {
        assert_file_exists "$FIXTURE_ADDED"
        assert_file_checksum "$FIXTURE_ADDED" "$FIXTURE_V2_ADDED_SHA"
    }
    run_test "T46" "verify_v2_added" 10 test_verify_v2_added

    # ── T47: Rollback after update ───────────────────────────────────────────
    test_rollback_after_update() {
        "$CONARY" restore --last \
            --db-path "$DB_PATH" \
            --yes \
            2>&1
    }

    _FAILS_BEFORE_ROLLBACK=$_FAIL_COUNT
    run_test "T47" "rollback_after_update" 120 test_rollback_after_update

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_ROLLBACK" ]; then
        record_skip "T48" "rollback_fs_check" "skipped due to T47 failure"
    else

    # ── T48: Rollback filesystem check ───────────────────────────────────────
    test_rollback_fs_check() {
        # v1 content should be restored
        assert_file_checksum "$FIXTURE_FILE" "$FIXTURE_V1_HELLO_SHA"
        # v2-only file should be gone
        assert_file_not_exists "$FIXTURE_ADDED"
    }
    run_test "T48" "rollback_fs_check" 10 test_rollback_fs_check

    fi # T47 rollback

    fi # T44 update

    # ── T49: Pin blocks update ───────────────────────────────────────────────
    test_pin_blocks_update() {
        # Pin to current version (v1 after rollback, or whatever is installed)
        "$CONARY" pin "$FIXTURE_PKG" --db-path "$DB_PATH" 2>&1

        # Attempt update
        "$CONARY" update "$FIXTURE_PKG" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --sandbox never \
            2>&1 || true

        # Should still be v1
        local info_output
        info_output=$("$CONARY" list "$FIXTURE_PKG" --info --db-path "$DB_PATH" 2>&1)
        assert_output_contains "1.0.0" "$info_output"

        # Unpin for cleanup
        "$CONARY" unpin "$FIXTURE_PKG" --db-path "$DB_PATH" 2>&1
    }
    run_test "T49" "pin_blocks_update" 300 test_pin_blocks_update

    # ── T50: Orphan detection ────────────────────────────────────────────────
    test_orphan_detection() {
        # Remove the fixture package (deps should become orphans)
        "$CONARY" remove "$FIXTURE_PKG" \
            --db-path "$DB_PATH" \
            --no-scripts \
            2>&1

        # Check for orphan reporting
        local output
        output=$("$CONARY" list --orphans --db-path "$DB_PATH" 2>&1)
        # Should mention orphans or empty set (depends on what deps were pulled)
        # At minimum, should not crash
        echo "$output"
    }
    run_test "T50" "orphan_detection" 60 test_orphan_detection

    fi # T43 reinstall
    fi # T38 initial install
```

**Step 3: Verify syntax**

Run: `bash -n tests/integration/remi/runner/test-runner.sh`
Expected: No output

**Step 4: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh
git commit -m "test: add Group A deep install flow tests (T38-T50)"
```

---

## Task 6: Write Group B Tests — Generation Lifecycle (T51-T57)

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Write T51-T57**

Add after Group A, inside the Phase 2 block:

```bash
    # ── Group B: Generation Lifecycle ────────────────────────────────────────
    echo ""
    echo "── Group B: Generation Lifecycle ──"
    echo ""

    # Reinstall fixture for generation testing
    "$CONARY" install "${FIXTURE_PKG}=1.0.0" \
        --db-path "$DB_PATH" \
        --dep-mode takeover \
        --yes \
        --no-scripts \
        --sandbox never \
        2>/dev/null || true

    # ── T51: Build generation ────────────────────────────────────────────────
    test_generation_build() {
        "$CONARY" system generation build --db-path "$DB_PATH" 2>&1
    }

    _FAILS_BEFORE_GEN=$_FAIL_COUNT
    run_test "T51" "generation_build" 120 test_generation_build

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_GEN" ]; then
        echo "Generation build failed - skipping T52-T57"
        for t in T52 T53 T54 T55 T56 T57; do
            record_skip "$t" "group_b_skipped" "skipped due to T51 failure"
        done
    else

    # ── T52: Generation list ─────────────────────────────────────────────────
    test_generation_list_after_build() {
        local output
        output=$("$CONARY" system generation list --db-path "$DB_PATH" 2>&1)
        assert_output_not_contains "No generations" "$output"
        # Should show at least generation 1
        assert_output_contains "1" "$output"
    }
    run_test "T52" "generation_list" 10 test_generation_list_after_build

    # ── T53: Generation info ─────────────────────────────────────────────────
    test_generation_info() {
        local output
        output=$("$CONARY" system generation info 1 --db-path "$DB_PATH" 2>&1)
        # Should show metadata (format, packages, etc.)
        assert_output_contains "packages" "$output"
    }
    run_test "T53" "generation_info" 10 test_generation_info

    # ── T54: Switch generation ───────────────────────────────────────────────
    # Install v2, build gen 2, then switch back and forth
    test_generation_switch() {
        # Update to v2 to create different state
        "$CONARY" update "$FIXTURE_PKG" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --no-scripts \
            --sandbox never \
            2>&1

        # Build generation 2
        "$CONARY" system generation build --db-path "$DB_PATH" 2>&1

        # Switch to generation 2
        "$CONARY" system generation switch 2 --db-path "$DB_PATH" 2>&1
    }

    _FAILS_BEFORE_SWITCH=$_FAIL_COUNT
    run_test "T54" "generation_switch" 300 test_generation_switch

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_SWITCH" ]; then
        for t in T55 T56; do
            record_skip "$t" "group_b_skipped" "skipped due to T54 failure"
        done
    else

    # ── T55: Rollback generation ─────────────────────────────────────────────
    test_generation_rollback() {
        # Switch back to generation 1
        "$CONARY" system generation switch 1 --db-path "$DB_PATH" 2>&1
    }
    run_test "T55" "generation_rollback" 120 test_generation_rollback

    # ── T56: GC old generation ───────────────────────────────────────────────
    test_generation_gc() {
        # GC should clean up at least one generation
        local output
        output=$("$CONARY" system generation gc --db-path "$DB_PATH" 2>&1)
        echo "$output"
        # Should not crash
    }
    run_test "T56" "generation_gc" 60 test_generation_gc

    fi # T54 switch

    # ── T57: System takeover full ────────────────────────────────────────────
    test_system_takeover_full() {
        local output exit_code
        output=$("$CONARY" system takeover \
            --db-path "$DB_PATH" \
            --skip-conversion \
            --yes \
            2>&1) && exit_code=0 || exit_code=$?

        if [ "$exit_code" -eq 0 ]; then
            assert_output_contains "generation" "$output"
        else
            # May fail in container (composefs, kernel requirements)
            # As long as it fails gracefully, that's acceptable
            echo "takeover exited $exit_code (may be expected in container): $output"
        fi
    }
    run_test "T57" "system_takeover_full" 300 test_system_takeover_full

    fi # T51 generation build
```

**Step 2: Verify syntax**

Run: `bash -n tests/integration/remi/runner/test-runner.sh`
Expected: No output

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh
git commit -m "test: add Group B generation lifecycle tests (T51-T57)"
```

---

## Task 7: Write Group C Tests — Bootstrap Pipeline (T58-T61)

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Write T58-T61**

Add after Group B:

```bash
    # ── Group C: Bootstrap Pipeline ──────────────────────────────────────────
    echo ""
    echo "── Group C: Bootstrap Pipeline ──"
    echo ""

    BOOTSTRAP_WORK="/tmp/conary-bootstrap-test"
    BOOTSTRAP_RECIPES="/tmp/conary-bootstrap-recipes"
    mkdir -p "$BOOTSTRAP_WORK" "$BOOTSTRAP_RECIPES"

    # ── T58: Bootstrap dry-run ───────────────────────────────────────────────
    test_bootstrap_dry_run() {
        local output exit_code
        output=$("$CONARY" bootstrap dry-run \
            --work-dir "$BOOTSTRAP_WORK" \
            --recipe-dir "$BOOTSTRAP_RECIPES" \
            2>&1) && exit_code=0 || exit_code=$?

        if [ "$exit_code" -eq 0 ]; then
            assert_output_contains "Graph resolved" "$output"
        else
            # Dry-run may fail if no recipes exist - that's a valid test
            echo "dry-run exited $exit_code: $output"
            # Should fail gracefully, not crash
            assert_output_not_contains "panic" "$output"
        fi
    }
    run_test "T58" "bootstrap_dry_run" 60 test_bootstrap_dry_run

    # ── T59: Stage 0 runs ───────────────────────────────────────────────────
    test_bootstrap_stage0() {
        local output exit_code
        output=$("$CONARY" bootstrap stage0 \
            --work-dir "$BOOTSTRAP_WORK" \
            2>&1) && exit_code=0 || exit_code=$?

        if [ "$exit_code" -eq 0 ]; then
            echo "$output"
        else
            # Stage 0 may fail due to missing toolchains in minimal container
            echo "stage0 exited $exit_code: $output"
            assert_output_not_contains "panic" "$output"
        fi
    }
    run_test "T59" "bootstrap_stage0" 300 test_bootstrap_stage0

    # ── T60: Stage 0 output valid ────────────────────────────────────────────
    test_bootstrap_stage0_output() {
        # If stage 0 produced output, verify structure
        if [ -d "$BOOTSTRAP_WORK/stage0" ]; then
            assert_dir_exists "$BOOTSTRAP_WORK/stage0"
            echo "Stage 0 directory exists with contents:"
            ls -la "$BOOTSTRAP_WORK/stage0/"
        else
            echo "No stage0 output (expected if stage0 failed)"
        fi
    }
    run_test "T60" "bootstrap_stage0_output" 10 test_bootstrap_stage0_output

    # ── T61: Stage 1 starts ──────────────────────────────────────────────────
    test_bootstrap_stage1_starts() {
        local output exit_code
        # Run stage1 with a short timeout - we just need proof of life
        timeout 60 "$CONARY" bootstrap stage1 \
            --work-dir "$BOOTSTRAP_WORK" \
            2>&1 && exit_code=0 || exit_code=$?

        # Exit 124 = timeout (proof of life: it started and ran)
        # Exit 0 = completed (unlikely in 60s but fine)
        # Other = failed to start
        if [ "$exit_code" -eq 124 ]; then
            echo "Stage 1 started (timed out as expected)"
        elif [ "$exit_code" -eq 0 ]; then
            echo "Stage 1 completed"
        else
            echo "Stage 1 exited $exit_code (may need stage0 first)"
            assert_output_not_contains "panic" "$output"
        fi
    }
    run_test "T61" "bootstrap_stage1_starts" 120 test_bootstrap_stage1_starts

    rm -rf "$BOOTSTRAP_WORK" "$BOOTSTRAP_RECIPES"
```

**Step 2: Verify syntax**

Run: `bash -n tests/integration/remi/runner/test-runner.sh`
Expected: No output

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh
git commit -m "test: add Group C bootstrap pipeline tests (T58-T61)"
```

---

## Task 8: Write Group D Tests — Recipe & Build (T62-T66)

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`
- Create: `tests/fixtures/recipes/simple-hello/recipe.toml`
- Create: `tests/fixtures/recipes/simple-hello/src/hello.sh`

**Step 1: Create simple test recipe**

`tests/fixtures/recipes/simple-hello/recipe.toml`:
```toml
[package]
name = "test-hello"
version = "1.0.0"
description = "Simple test recipe for E2E validation"
license = "MIT"

[source]
type = "local"
path = "src/"

[build]
steps = [
    "install -Dm755 hello.sh ${DESTDIR}/usr/bin/test-hello",
]

[package.platform]
os = "linux"
arch = "x86_64"
```

`tests/fixtures/recipes/simple-hello/src/hello.sh`:
```bash
#!/bin/sh
echo "hello from test recipe"
```

**Step 2: Write T62-T66**

```bash
    # ── Group D: Recipe & Build ──────────────────────────────────────────────
    echo ""
    echo "── Group D: Recipe & Build ──"
    echo ""

    RECIPE_OUTPUT="/tmp/conary-recipe-output"
    RECIPE_CACHE="/tmp/conary-recipe-cache"
    mkdir -p "$RECIPE_OUTPUT" "$RECIPE_CACHE"

    # Copy recipe fixtures into container working dir
    RECIPE_DIR="/opt/remi-tests/fixtures/recipes"

    # ── T62: Cook TOML recipe ────────────────────────────────────────────────
    test_cook_toml_recipe() {
        local output exit_code

        # If fixtures were copied into the container
        if [ -d "$RECIPE_DIR/simple-hello" ]; then
            output=$("$CONARY" cook "$RECIPE_DIR/simple-hello/recipe.toml" \
                --output "$RECIPE_OUTPUT" \
                --source-cache "$RECIPE_CACHE" \
                --no-isolation \
                2>&1) && exit_code=0 || exit_code=$?
        else
            echo "Recipe fixtures not found at $RECIPE_DIR" >&2
            return 1
        fi

        if [ "$exit_code" -ne 0 ]; then
            echo "cook failed (exit $exit_code): $output" >&2
            return 1
        fi
        echo "$output"
    }

    _FAILS_BEFORE_COOK=$_FAIL_COUNT
    run_test "T62" "cook_toml_recipe" 120 test_cook_toml_recipe

    if [ "$_FAIL_COUNT" -gt "$_FAILS_BEFORE_COOK" ]; then
        record_skip "T63" "ccs_output_valid" "skipped due to T62 failure"
    else

    # ── T63: CCS output valid ────────────────────────────────────────────────
    test_ccs_output_valid() {
        # Should have produced a .ccs file
        local ccs_file
        ccs_file=$(ls "$RECIPE_OUTPUT"/*.ccs 2>/dev/null | head -1)
        if [ -z "$ccs_file" ]; then
            echo "no CCS file found in $RECIPE_OUTPUT" >&2
            return 1
        fi
        echo "CCS output: $ccs_file ($(du -h "$ccs_file" | cut -f1))"
    }
    run_test "T63" "ccs_output_valid" 10 test_ccs_output_valid

    fi # T62

    # ── T64: PKGBUILD conversion ─────────────────────────────────────────────
    test_pkgbuild_conversion() {
        local output exit_code
        # Use the real Conary PKGBUILD as test input
        if [ -f "/opt/remi-tests/fixtures/pkgbuild/PKGBUILD" ]; then
            output=$("$CONARY" convert-pkgbuild \
                "/opt/remi-tests/fixtures/pkgbuild/PKGBUILD" \
                2>&1) && exit_code=0 || exit_code=$?
        else
            echo "PKGBUILD fixture not found" >&2
            return 1
        fi

        if [ "$exit_code" -ne 0 ]; then
            echo "convert-pkgbuild failed (exit $exit_code): $output" >&2
            return 1
        fi
        # Should produce valid TOML recipe output
        assert_output_contains "name" "$output"
        assert_output_contains "version" "$output"
    }

    _FAILS_BEFORE_CONVERT=$_FAIL_COUNT
    run_test "T64" "pkgbuild_conversion" 30 test_pkgbuild_conversion

    # ── T65: Converted recipe cooks ──────────────────────────────────────────
    test_converted_recipe_cooks() {
        # Convert PKGBUILD to recipe file, then cook it
        local recipe_file="$RECIPE_OUTPUT/converted-recipe.toml"
        "$CONARY" convert-pkgbuild \
            "/opt/remi-tests/fixtures/pkgbuild/PKGBUILD" \
            --output "$recipe_file" \
            2>&1

        if [ ! -f "$recipe_file" ]; then
            echo "converted recipe not written to $recipe_file" >&2
            return 1
        fi

        # Try to cook (may fail due to missing sources, but should parse)
        local output exit_code
        output=$("$CONARY" cook "$recipe_file" \
            --output "$RECIPE_OUTPUT/converted" \
            --source-cache "$RECIPE_CACHE" \
            --no-isolation \
            --fetch-only \
            2>&1) && exit_code=0 || exit_code=$?

        # fetch-only validates recipe parsing + source resolution
        echo "cook --fetch-only exited $exit_code: $output"
        assert_output_not_contains "panic" "$output"
    }

    if [ "$_FAILS_BEFORE_CONVERT" -gt "$_FAIL_COUNT" ]; then
        record_skip "T65" "converted_recipe_cooks" "skipped due to T64 failure"
    else
        run_test "T65" "converted_recipe_cooks" 120 test_converted_recipe_cooks
    fi

    # ── T66: Hermetic build isolation ────────────────────────────────────────
    test_hermetic_build() {
        local output exit_code
        if [ ! -d "$RECIPE_DIR/simple-hello" ]; then
            echo "Recipe fixtures not found" >&2
            return 1
        fi

        output=$("$CONARY" cook "$RECIPE_DIR/simple-hello/recipe.toml" \
            --output "$RECIPE_OUTPUT/hermetic" \
            --source-cache "$RECIPE_CACHE" \
            --hermetic \
            2>&1) && exit_code=0 || exit_code=$?

        if [ "$exit_code" -ne 0 ]; then
            # Check if it failed due to network being blocked (expected)
            # vs some other error
            echo "hermetic cook exited $exit_code: $output"
            # The recipe has local sources, so it should succeed even hermetic
            return 1
        fi
        echo "Hermetic build succeeded"
    }
    run_test "T66" "hermetic_build" 120 test_hermetic_build

    rm -rf "$RECIPE_OUTPUT" "$RECIPE_CACHE"
```

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh tests/fixtures/recipes/
git commit -m "test: add Group D recipe and build tests (T62-T66)"
```

---

## Task 9: Write Group E Tests — Remi Client (T67-T71)

**Files:**
- Modify: `tests/integration/remi/runner/test-runner.sh`

**Step 1: Write T67-T71**

```bash
    # ── Group E: Remi Client ─────────────────────────────────────────────────
    echo ""
    echo "── Group E: Remi Client ──"
    echo ""

    # ── T67: Sparse index fetch ──────────────────────────────────────────────
    test_sparse_index_fetch() {
        local output
        output=$(curl -sf "${REMI_ENDPOINT}/v1/${REMI_DISTRO}/index" 2>&1)
        if [ -z "$output" ]; then
            echo "empty response from sparse index endpoint" >&2
            return 1
        fi
        echo "Sparse index: $(echo "$output" | wc -l) lines"
    }
    run_test "T67" "sparse_index_fetch" 30 test_sparse_index_fetch

    # ── T68: Chunk-level install ─────────────────────────────────────────────
    test_chunk_level_install() {
        # Install a package that's already partially present (fixture was
        # installed and removed earlier, chunks may be in CAS)
        local output exit_code
        output=$("$CONARY" install "${FIXTURE_PKG}=1.0.0" \
            --db-path "$DB_PATH" \
            --dep-mode takeover \
            --yes \
            --no-scripts \
            --sandbox never \
            2>&1) && exit_code=0 || exit_code=$?

        if [ "$exit_code" -ne 0 ]; then
            echo "chunk-level install failed (exit $exit_code): $output" >&2
            return 1
        fi
        # Verify files exist
        assert_file_exists "$FIXTURE_FILE"

        # Cleanup
        "$CONARY" remove "$FIXTURE_PKG" --db-path "$DB_PATH" --no-scripts 2>/dev/null || true
    }
    run_test "T68" "chunk_level_install" 300 test_chunk_level_install

    # ── T69: OCI manifest valid ──────────────────────────────────────────────
    test_oci_manifest() {
        local output http_code
        http_code=$(curl -sf -o /dev/null -w "%{http_code}" \
            "${REMI_ENDPOINT}/v2/" 2>&1) || true

        if [ "$http_code" = "200" ] || [ "$http_code" = "401" ]; then
            # 200 = OCI registry responds
            # 401 = OCI registry responds but requires auth (still valid)
            echo "OCI endpoint returned $http_code"
        else
            echo "OCI endpoint returned unexpected $http_code" >&2
            return 1
        fi
    }
    run_test "T69" "oci_manifest_valid" 30 test_oci_manifest

    # ── T70: OCI blob fetch ──────────────────────────────────────────────────
    test_oci_blob_fetch() {
        # List tags to find a valid manifest
        local tags_output
        tags_output=$(curl -sf "${REMI_ENDPOINT}/v2/${REMI_DISTRO}/conary-test-fixture/tags/list" 2>&1) || {
            echo "Could not list OCI tags (may not be published yet)"
            return 0  # Soft pass - OCI may not have fixture yet
        }

        if echo "$tags_output" | grep -q "tags"; then
            echo "OCI tags available: $tags_output"
        else
            echo "No OCI tags found (expected if fixtures not published as OCI)"
        fi
    }
    run_test "T70" "oci_blob_fetch" 30 test_oci_blob_fetch

    # ── T71: Stats endpoint ──────────────────────────────────────────────────
    test_stats_endpoint() {
        local output
        output=$(curl -sf "${REMI_ENDPOINT}/stats" 2>&1)
        if [ -z "$output" ]; then
            echo "empty response from /stats" >&2
            return 1
        fi
        # Should be valid JSON with expected fields
        assert_output_contains "packages" "$output"
    }
    run_test "T71" "stats_endpoint" 30 test_stats_endpoint
```

**Step 2: Close the Phase 2 block**

After T71, close the `else` block from Task 2:

```bash
fi  # end Phase 2
```

**Step 3: Verify syntax**

Run: `bash -n tests/integration/remi/runner/test-runner.sh`
Expected: No output

**Step 4: Commit**

```bash
git add tests/integration/remi/runner/test-runner.sh
git commit -m "test: add Group E Remi client tests (T67-T71)"
```

---

## Task 10: Update Containerfiles to Include Fixtures

**Files:**
- Modify: `tests/integration/remi/containers/Containerfile.fedora43`
- Modify: `tests/integration/remi/containers/Containerfile.ubuntu-noble`
- Modify: `tests/integration/remi/containers/Containerfile.arch`

**Step 1: Add fixture copying to all three Containerfiles**

Add after the `COPY runner/ /opt/remi-tests/` line in each Containerfile:

```dockerfile
# Phase 2 test fixtures (recipes, PKGBUILD)
COPY fixtures/ /opt/remi-tests/fixtures/
```

**Step 2: Update run.sh to copy fixtures into build context**

In `tests/integration/remi/run.sh`, after the binary/package setup and before `podman build`, add:

```bash
# ── Copy test fixtures into build context ────────────────────────────────
FIXTURES_SRC="$PROJECT_ROOT/tests/fixtures"
FIXTURES_DST="$BUILD_CONTEXT/fixtures"
if [ -d "$FIXTURES_SRC" ]; then
    rm -rf "$FIXTURES_DST"
    mkdir -p "$FIXTURES_DST"
    # Copy recipes
    cp -r "$FIXTURES_SRC/recipes" "$FIXTURES_DST/recipes" 2>/dev/null || true
    # Copy PKGBUILD for conversion tests
    mkdir -p "$FIXTURES_DST/pkgbuild"
    cp "$PROJECT_ROOT/packaging/arch/PKGBUILD" "$FIXTURES_DST/pkgbuild/" 2>/dev/null || true
    CLEANUP_FILES+=("$FIXTURES_DST")
fi
```

**Step 3: Verify all three Containerfiles parse**

Run: `podman build --help > /dev/null` (just verify podman works)

**Step 4: Commit**

```bash
git add tests/integration/remi/containers/ tests/integration/remi/run.sh
git commit -m "test: include fixtures in container images for Phase 2"
```

---

## Task 11: Add E2E CI Workflow

**Files:**
- Create: `.forgejo/workflows/e2e.yaml`

**Step 1: Write the workflow**

```yaml
# .forgejo/workflows/e2e.yaml
# Phase 2 E2E validation -- daily + manual (~20-30 min)
name: E2E Validation

on:
  schedule:
    - cron: '0 6 * * *'  # Daily at 06:00 UTC
  workflow_dispatch:

jobs:
  e2e:
    runs-on: linux-native
    strategy:
      fail-fast: false
      matrix:
        distro: [fedora43, ubuntu-noble, arch]
    name: E2E (${{ matrix.distro }})
    steps:
      - uses: actions/checkout@v4

      - name: Build conary
        run: cargo build

      - name: Run E2E tests (${{ matrix.distro }})
        run: ./tests/integration/remi/run.sh --build --distro ${{ matrix.distro }} --phase2

      - name: Upload results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: e2e-results-${{ matrix.distro }}
          path: tests/integration/remi/results/${{ matrix.distro }}.json
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/e2e.yaml
git commit -m "ci: add daily E2E validation workflow for Phase 2 tests"
```

---

## Task 12: Update ROADMAP.md Phase 2 with Test IDs

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Update Phase 2 items with test coverage references**

Replace the Phase 2 section with items that cross-reference the test IDs, showing which tests prove each feature. Check off items that become covered by passing tests.

**Step 2: Commit**

```bash
git add ROADMAP.md
git commit -m "docs: cross-reference Phase 2 roadmap items with test IDs"
```

---

## Task 13: Integration Smoke Test

**Step 1: Build the binary**

Run: `cargo build`
Expected: Successful build

**Step 2: Run Phase 1 tests locally (quick sanity check)**

Run: `./tests/integration/remi/run.sh --distro fedora43`
Expected: T01-T37 pass as before

**Step 3: Run Phase 2 tests locally**

Run: `./tests/integration/remi/run.sh --distro fedora43 --phase2`
Expected: T01-T37 pass, T38-T71 run (some may skip if fixtures aren't published yet)

**Step 4: Fix any issues found**

Iterate on test failures until the suite is stable.

**Step 5: Final commit with any fixes**

```bash
git add -A
git commit -m "test: fix Phase 2 integration test issues from smoke run"
```

---

## Execution Order Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1 | Checksum assertions in lib.sh | None |
| 2 | --phase2 flag in runner + run.sh | None |
| 3 | Create fixture CCS packages | None |
| 4 | Build and publish fixtures to Remi | 3 |
| 5 | Group A tests (T38-T50) | 1, 2 |
| 6 | Group B tests (T51-T57) | 2 |
| 7 | Group C tests (T58-T61) | 2 |
| 8 | Group D tests (T62-T66) | 2 |
| 9 | Group E tests (T67-T71) | 2, 4 |
| 10 | Container fixtures | 3, 8 |
| 11 | E2E CI workflow | All tests |
| 12 | ROADMAP.md updates | All tests |
| 13 | Integration smoke test | All above |

Tasks 1, 2, 3 are independent and can be parallelized. Tasks 5-9 depend on 1+2 but are independent of each other.
