#!/usr/bin/env bash
# scripts/remi-health.sh -- Remi server health verification
#
# Usage:
#   ./scripts/remi-health.sh [--smoke|--full] [--endpoint URL]
#
# Modes:
#   --smoke   Quick health + metadata check (~5s)
#   --full    All endpoints + test conversion (~60s)
set -euo pipefail

ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"
MODE="smoke"

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

check() {
    local name="$1" url="$2" expect="${3:-200}"
    local http_code
    http_code=$(curl -sf -o /dev/null -w '%{http_code}' --max-time 10 "$url" 2>/dev/null || echo "000")

    if [[ "$http_code" == "$expect" ]]; then
        printf "  [PASS] %-40s %s\n" "$name" "$http_code"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s (expected %s)\n" "$name" "$http_code" "$expect"
        FAIL=$((FAIL + 1))
    fi
}

check_contains() {
    local name="$1" url="$2" needle="$3"
    local body
    body=$(curl -sf --max-time 10 "$url" 2>/dev/null || echo "")

    if echo "$body" | grep -qF "$needle"; then
        printf "  [PASS] %-40s contains '%s'\n" "$name" "$needle"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s missing '%s'\n" "$name" "$needle"
        FAIL=$((FAIL + 1))
    fi
}

echo "Remi Health Check ($MODE)"
echo "Endpoint: $ENDPOINT"
echo ""

# ── Smoke checks (always run) ────────────────────────────────────────────
echo "=== Core Endpoints ==="
check "health"           "$ENDPOINT/health"
check "stats overview"   "$ENDPOINT/v1/stats/overview"

echo ""
echo "=== Metadata (per distro) ==="
for distro in fedora ubuntu arch; do
    check "metadata ($distro)" "$ENDPOINT/v1/${distro}/metadata"
done

# ── Full checks (--full only) ────────────────────────────────────────────
if [[ "$MODE" == "full" ]]; then
    echo ""
    echo "=== Sparse Index ==="
    check "sparse index (curl)" "$ENDPOINT/v1/packages/curl"

    echo ""
    echo "=== Search ==="
    check_contains "search (curl)" "$ENDPOINT/v1/search?q=curl" "curl"

    echo ""
    echo "=== OCI Distribution ==="
    check "OCI catalog" "$ENDPOINT/v2/_catalog"

    echo ""
    echo "=== Conversion (async) ==="
    conv_code=$(curl -sf -o /dev/null -w '%{http_code}' --max-time 30 \
        -X POST "$ENDPOINT/v1/convert/fedora/curl" 2>/dev/null || echo "000")
    if [[ "$conv_code" == "200" ]] || [[ "$conv_code" == "202" ]]; then
        printf "  [PASS] %-40s %s\n" "conversion submit" "$conv_code"
        PASS=$((PASS + 1))
    else
        printf "  [FAIL] %-40s %s (expected 200 or 202)\n" "conversion submit" "$conv_code"
        FAIL=$((FAIL + 1))
    fi
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
