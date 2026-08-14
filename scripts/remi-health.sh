#!/usr/bin/env bash
# scripts/remi-health.sh -- Remi server health verification
#
# Usage:
#   ./scripts/remi-health.sh [--smoke|--full] [--endpoint URL]
#
# Modes:
#   --smoke   Quick health + metadata check (~5s)
#   --full    All endpoints + package metadata/conversion probe (~60s)
set -euo pipefail

ENDPOINT="${REMI_ENDPOINT:-https://remi.conary.io}"
MODE="smoke"
EXPECTED_UBUNTU_PROFILE="${REMI_EXPECTED_UBUNTU_PROFILE:-ubuntu-26.04}"
CURL_BIN="${REMI_CURL_BIN:-curl}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --smoke) MODE="smoke"; shift ;;
        --full)  MODE="full"; shift ;;
        --endpoint) ENDPOINT="$2"; shift 2 ;;
        --endpoint=*) ENDPOINT="${1#*=}"; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

PASS=0
FAIL=0
HTTP_CODE="000"
CURL_EXIT=0
CURL_CLASS="ok"

classify_curl_exit() {
    case "$1" in
        0)  printf '%s' "ok" ;;
        6)  printf '%s' "dns" ;;
        7)  printf '%s' "connect" ;;
        22) printf '%s' "http" ;;
        28) printf '%s' "timeout" ;;
        35) printf '%s' "tls" ;;
        52) printf '%s' "empty-reply" ;;
        56) printf '%s' "receive" ;;
        *)  printf 'curl-exit-%s' "$1" ;;
    esac
}

probe() {
    local method="$1" url="$2" output="$3"
    local -a method_args=()
    if [[ "$method" == "HEAD" ]]; then
        method_args=(-I)
    fi

    if HTTP_CODE=$("$CURL_BIN" -sS -f "${method_args[@]}" -o "$output" \
        -w '%{http_code}' --max-time 30 "$url" 2>/dev/null); then
        CURL_EXIT=0
    else
        CURL_EXIT=$?
    fi
    CURL_CLASS=$(classify_curl_exit "$CURL_EXIT")
}

probe_result() {
    printf 'http=%s curl_exit=%s class=%s' "$HTTP_CODE" "$CURL_EXIT" "$CURL_CLASS"
}

check() {
    local name="$1" url="$2" expect="${3:-200}" method="${4:-HEAD}"
    probe "$method" "$url" /dev/null

    if [[ "$CURL_EXIT" -eq 0 && "$HTTP_CODE" =~ ^($expect)$ ]]; then
        printf "  [PASS] %-40s %s\n" "$name" "$(probe_result)"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s (expected %s)\n" "$name" "$(probe_result)" "$expect"
        FAIL=$((FAIL + 1))
    fi
}

check_contains() {
    local name="$1" url="$2" needle="$3"
    local body_file
    body_file=$(mktemp)
    probe GET "$url" "$body_file"

    if [[ "$CURL_EXIT" -eq 0 && "$HTTP_CODE" == "200" ]] && grep -qF "$needle" "$body_file"; then
        printf "  [PASS] %-40s contains '%s'\n" "$name" "$needle"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s; missing '%s'\n" "$name" "$(probe_result)" "$needle"
        FAIL=$((FAIL + 1))
    fi
    rm -f -- "$body_file"
}

check_source_profile() {
    local distro="$1" expected="$2"
    check_contains "source profile ($distro)" \
        "$ENDPOINT/v1/index/$distro?page=1&per_page=1" \
        "\"source_profile\":\"$expected\""
}

echo "Remi Health Check ($MODE)"
echo "Endpoint: $ENDPOINT"
echo ""

# ── Smoke checks (always run) ────────────────────────────────────────────
echo "=== Core Endpoints ==="
check "health"           "$ENDPOINT/health"
# /health is an unconditional liveness reply. Readiness is the evidence-bearing
# one: it opens the database, requires the expected schema revision, and checks
# the serving directories and free space, failing closed when a probe cannot run.
check_contains "readiness" "$ENDPOINT/health/ready" '"ready":true'
check "stats overview"   "$ENDPOINT/v1/stats/overview"

echo ""
echo "=== Bounded Sparse Metadata ==="
check_source_profile fedora fedora-44
check_source_profile ubuntu "$EXPECTED_UBUNTU_PROFILE"
check_source_profile arch arch

# ── Full checks (--full only) ────────────────────────────────────────────
if [[ "$MODE" == "full" ]]; then
    echo ""
    echo "=== Sparse Index ==="
    check "sparse index (curl)" "$ENDPOINT/v1/index/fedora/curl" 200 GET

    echo ""
    echo "=== Search ==="
    check_contains "search (curl)" "$ENDPOINT/v1/search?q=curl" "curl"

    echo ""
    echo "=== OCI Distribution ==="
    check "OCI catalog" "$ENDPOINT/v2/_catalog"

    echo ""
    echo "=== Package Metadata / Conversion ==="
    check "package metadata / conversion" \
        "$ENDPOINT/v1/fedora/packages/curl" "200|202"
fi

# ── Summary ──────────────────────────────────────────────────────────────
echo ""
TOTAL=$((PASS + FAIL))
echo "Results: $PASS/$TOTAL passed"

if [[ "$FAIL" -gt 0 ]]; then
    echo "[FAILED] $FAIL checks failed"
    exit 1
fi

echo "[OK] All checks passed"
exit 0
