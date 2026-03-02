#!/usr/bin/env bash
# tests/integration/remi/runner/lib.sh
# Assertion helpers and JSON result writer for Remi integration tests

set -euo pipefail

# ── State ─────────────────────────────────────────────────────────────────────

DISTRO="${DISTRO:-fedora43}"
RESULTS_DIR="${RESULTS_DIR:-/results}"
_PASS_COUNT=0
_FAIL_COUNT=0
_SKIP_COUNT=0
_RESULTS=()
_CURRENT_TEST_ID=""
_CURRENT_TEST_NAME=""
_CURRENT_OUTPUT=""
_FATAL=0

# ── Colors (only if stdout is a terminal) ─────────────────────────────────────

if [ -t 1 ]; then
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
else
    GREEN='' RED='' YELLOW='' BLUE='' NC=''
fi

# ── Test runner ───────────────────────────────────────────────────────────────

# wait_with_timeout PID SECONDS
# Waits for a process to finish, returns 1 if it exceeds the timeout.
# run_test ID NAME TIMEOUT_SECONDS FUNCTION
# Executes FUNCTION with a timeout, captures stdout+stderr, records result.
run_test() {
    local id="$1" name="$2" timeout_secs="$3" func="$4"
    _CURRENT_TEST_ID="$id"
    _CURRENT_TEST_NAME="$name"
    _CURRENT_OUTPUT=""

    # Check for fatal flag (prior critical test failed)
    if [ "$_FATAL" -eq 1 ]; then
        record_skip "$id" "$name" "skipped due to prior critical failure"
        return
    fi

    printf "${BLUE}[%s]${NC} %-30s " "$id" "$name"

    local start_time
    start_time=$(date +%s%N)

    # Call the function directly in the current shell (so it can access
    # other functions and variables). Capture output via temp file.
    local output exit_code tmpfile
    tmpfile=$(mktemp)
    set +e
    "$func" > "$tmpfile" 2>&1
    exit_code=$?
    set -e
    output=$(cat "$tmpfile")
    rm -f "$tmpfile"
    _CURRENT_OUTPUT="$output"

    local end_time duration_ms
    end_time=$(date +%s%N)
    duration_ms=$(( (end_time - start_time) / 1000000 ))

    if [ "$exit_code" -eq 124 ]; then
        # timeout(1) returns 124 on timeout
        record_fail "$id" "$name" "timed out after ${timeout_secs}s" "$duration_ms"
        printf "${RED}TIMEOUT${NC} (%dms)\n" "$duration_ms"
    elif [ "$exit_code" -ne 0 ]; then
        record_fail "$id" "$name" "$output" "$duration_ms"
        printf "${RED}FAIL${NC} (%dms)\n" "$duration_ms"
    else
        record_pass "$id" "$name" "$duration_ms"
        printf "${GREEN}PASS${NC} (%dms)\n" "$duration_ms"
    fi

    return 0
}

# ── Assertions ────────────────────────────────────────────────────────────────
# Each assertion prints a message on failure and returns non-zero.

assert_exit_code() {
    local expected="$1"
    shift
    local actual
    "$@" >/dev/null 2>&1 && actual=0 || actual=$?
    if [ "$actual" -ne "$expected" ]; then
        echo "expected exit code $expected, got $actual" >&2
        return 1
    fi
}

assert_exit_code_with_output() {
    local expected="$1"
    shift
    local actual
    "$@" 2>&1 && actual=0 || actual=$?
    if [ "$actual" -ne "$expected" ]; then
        echo "expected exit code $expected, got $actual" >&2
        return 1
    fi
}

assert_file_exists() {
    local path="$1"
    if [ ! -f "$path" ]; then
        echo "file does not exist: $path" >&2
        return 1
    fi
}

assert_file_not_exists() {
    local path="$1"
    if [ -f "$path" ]; then
        echo "file still exists: $path" >&2
        return 1
    fi
}

assert_file_executable() {
    local path="$1"
    if [ ! -x "$path" ]; then
        echo "file is not executable: $path" >&2
        return 1
    fi
}

assert_output_contains() {
    local needle="$1"
    local haystack="$2"
    if ! echo "$haystack" | grep -qF "$needle"; then
        echo "output does not contain '$needle'" >&2
        echo "output was: $haystack" >&2
        return 1
    fi
}

assert_output_not_contains() {
    local needle="$1"
    local haystack="$2"
    if echo "$haystack" | grep -qF "$needle"; then
        echo "output unexpectedly contains '$needle'" >&2
        return 1
    fi
}

assert_output_empty() {
    local output="$1"
    local trimmed
    trimmed=$(echo "$output" | tr -d '[:space:]')
    if [ -n "$trimmed" ]; then
        echo "expected empty output, got: $output" >&2
        return 1
    fi
}

# ── Result recording ─────────────────────────────────────────────────────────

record_pass() {
    local id="$1" name="$2" duration_ms="${3:-0}"
    _PASS_COUNT=$((_PASS_COUNT + 1))
    _RESULTS+=("{\"id\":\"$id\",\"name\":\"$name\",\"status\":\"pass\",\"duration_ms\":$duration_ms}")
}

record_fail() {
    local id="$1" name="$2" message="$3" duration_ms="${4:-0}"
    _FAIL_COUNT=$((_FAIL_COUNT + 1))
    # Escape JSON special characters in message
    local escaped
    escaped=$(echo "$message" | head -c 500 | sed 's/\\/\\\\/g; s/"/\\"/g; s/\t/\\t/g' | tr '\n' ' ')
    _RESULTS+=("{\"id\":\"$id\",\"name\":\"$name\",\"status\":\"fail\",\"message\":\"$escaped\",\"duration_ms\":$duration_ms}")
}

record_skip() {
    local id="$1" name="$2" reason="$3"
    _SKIP_COUNT=$((_SKIP_COUNT + 1))
    _RESULTS+=("{\"id\":\"$id\",\"name\":\"$name\",\"status\":\"skip\",\"reason\":\"$reason\"}")
    printf "${BLUE}[%s]${NC} %-30s ${YELLOW}SKIP${NC} (%s)\n" "$id" "$name" "$reason"
}

# Mark remaining tests as skipped (called after a critical failure)
set_fatal() {
    _FATAL=1
}

# ── Result output ─────────────────────────────────────────────────────────────

finalize_results() {
    local total=$((_PASS_COUNT + _FAIL_COUNT + _SKIP_COUNT))

    echo ""
    echo "════════════════════════════════════════════════════"
    printf "  Results: ${GREEN}%d passed${NC}  ${RED}%d failed${NC}  ${YELLOW}%d skipped${NC}  %d total\n" \
        "$_PASS_COUNT" "$_FAIL_COUNT" "$_SKIP_COUNT" "$total"
    echo "════════════════════════════════════════════════════"

    # Write JSON results
    mkdir -p "$RESULTS_DIR"
    local json_file="$RESULTS_DIR/${DISTRO}.json"

    {
        echo "{"
        echo "  \"distro\": \"$DISTRO\","
        echo "  \"timestamp\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
        echo "  \"summary\": {"
        echo "    \"total\": $total,"
        echo "    \"passed\": $_PASS_COUNT,"
        echo "    \"failed\": $_FAIL_COUNT,"
        echo "    \"skipped\": $_SKIP_COUNT"
        echo "  },"
        echo "  \"tests\": ["

        local i=0
        for result in "${_RESULTS[@]}"; do
            if [ $i -gt 0 ]; then
                echo "    ,$result"
            else
                echo "    $result"
            fi
            i=$((i + 1))
        done

        echo "  ]"
        echo "}"
    } > "$json_file"

    echo "Results written to $json_file"

    # Exit with failure if any tests failed
    if [ "$_FAIL_COUNT" -gt 0 ]; then
        return 1
    fi
    return 0
}
