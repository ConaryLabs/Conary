#!/usr/bin/env bash
# scripts/test-random-package-smoke.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

output="$(
    PACKAGE_CANDIDATES=$'htop\njq\ntree' \
        bash "$repo_root/scripts/random-package-smoke.sh" \
            --plan-only \
            --seed issue35 \
            --count 2 \
            --modes satisfy,adopt
)"

grep -q '^Selected packages: htop jq$' <<<"$output"
grep -q '^Modes: satisfy adopt$' <<<"$output"
grep -q -- '--dep-mode satisfy' <<<"$output"
grep -q -- '--dep-mode adopt' <<<"$output"
grep -q -- '--dry-run' <<<"$output"

plan_count="$(grep -c '^PLAN ' <<<"$output")"
if [[ "$plan_count" != "4" ]]; then
    echo "expected 4 planned commands, got $plan_count"
    echo "$output"
    exit 1
fi

echo "random package smoke planner test passed"
