#!/usr/bin/env bash
# scripts/test-retry-command.sh
set -euo pipefail

retry_command="${1:-apps/conary/tests/fixtures/native/retry-command.sh}"
tmpdir="$(mktemp -d /tmp/conary-retry-command-test.XXXXXX)"
trap 'rm -rf -- "$tmpdir"' EXIT

cat >"$tmpdir/flaky" <<'EOF'
#!/bin/sh
set -eu
count=0
if [ -f "$RETRY_TEST_COUNT" ]; then
    count="$(cat "$RETRY_TEST_COUNT")"
fi
count=$((count + 1))
printf '%s\n' "$count" >"$RETRY_TEST_COUNT"
[ "$count" -ge "$RETRY_TEST_SUCCEED_ON" ]
EOF
chmod 0755 "$tmpdir/flaky"

RETRY_TEST_COUNT="$tmpdir/success-count" \
RETRY_TEST_SUCCEED_ON=2 \
    "$retry_command" 5 "$tmpdir/flaky"
[[ "$(cat "$tmpdir/success-count")" == "2" ]]

set +e
RETRY_TEST_COUNT="$tmpdir/failure-count" \
RETRY_TEST_SUCCEED_ON=3 \
    "$retry_command" 2 "$tmpdir/flaky" >/dev/null 2>&1
status=$?
set -e
[[ "$status" != "0" ]]
[[ "$(cat "$tmpdir/failure-count")" == "2" ]]

if "$retry_command" 0 true >/dev/null 2>&1; then
    echo "zero-attempt retry unexpectedly succeeded" >&2
    exit 1
fi

echo "bounded command retry tests passed"
