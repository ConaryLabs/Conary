#!/usr/bin/env bash
# scripts/test-remi-health.sh -- Focused contract tests for bounded Remi health probes.
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TEST_DIR=$(mktemp -d)
trap 'rm -rf -- "$TEST_DIR"' EXIT

FAKE_CURL="$TEST_DIR/curl"
CURL_LOG="$TEST_DIR/curl.log"

cat >"$FAKE_CURL" <<'FAKE_CURL_EOF'
#!/usr/bin/env bash
set -euo pipefail

output=/dev/null
url=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -o) output="$2"; shift 2 ;;
        -w|--max-time) shift 2 ;;
        -sS|-f|-I) shift ;;
        *) url="$1"; shift ;;
    esac
done

printf '%s\n' "$url" >>"$REMI_CURL_LOG"
if [[ "${REMI_FAKE_TIMEOUT_URL:-}" == "$url" ]]; then
    printf '000'
    exit 28
fi
if [[ "${REMI_FAKE_HTTP_URL:-}" == "$url" ]]; then
    printf '503'
    exit 22
fi

code=200
body='{}'
case "$url" in
    */health/ready) body='{"ready":true}' ;;
    */v1/index/fedora\?*) body='{"distro":"fedora","source_profile":"fedora-44","packages":[],"total":0,"page":1,"per_page":1}' ;;
    */v1/index/ubuntu\?*) body='{"distro":"ubuntu","source_profile":"ubuntu-26.04","packages":[],"total":0,"page":1,"per_page":1}' ;;
    */v1/index/arch\?*) body='{"distro":"arch","source_profile":"arch","packages":[],"total":0,"page":1,"per_page":1}' ;;
    */v1/search\?*) body='{"packages":[{"name":"curl"}]}' ;;
    */v1/fedora/packages/curl) code=202 ;;
esac

if [[ "$output" != /dev/null ]]; then
    printf '%s' "$body" >"$output"
fi
printf '%s' "$code"
FAKE_CURL_EOF
chmod +x "$FAKE_CURL"

run_health() {
    REMI_CURL_BIN="$FAKE_CURL" REMI_CURL_LOG="$CURL_LOG" \
        bash "$REPO_ROOT/scripts/remi-health.sh" --full --endpoint https://fake.test
}

output=$(run_health)
grep -qF '[OK] All checks passed' <<<"$output"
grep -qF 'https://fake.test/v1/index/fedora?page=1&per_page=1' "$CURL_LOG"
grep -qF 'https://fake.test/v1/index/fedora/curl' "$CURL_LOG"
if grep -qE '/v1/[^/]+/metadata|/v1/packages/curl' "$CURL_LOG"; then
    echo "health script called an obsolete unbounded or nonexistent route" >&2
    exit 1
fi

: >"$CURL_LOG"
set +e
output=$(REMI_FAKE_TIMEOUT_URL=https://fake.test/v1/index/fedora/curl run_health 2>&1)
status=$?
set -e
if [[ "$status" -eq 0 ]]; then
    echo "health script accepted a timed-out sparse request" >&2
    exit 1
fi
grep -qF 'http=000 curl_exit=28 class=timeout' <<<"$output"

set +e
output=$(REMI_FAKE_HTTP_URL=https://fake.test/v1/index/fedora/curl run_health 2>&1)
status=$?
set -e
if [[ "$status" -eq 0 ]]; then
    echo "health script accepted an HTTP failure" >&2
    exit 1
fi
grep -qF 'http=503 curl_exit=22 class=http' <<<"$output"

echo "remi health script tests passed"
