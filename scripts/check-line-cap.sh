#!/usr/bin/env bash
# scripts/check-line-cap.sh
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

exec cargo run -q -p conary-xtask -- \
    line-cap \
    --root "$repo_root" \
    --allowlist "$repo_root/scripts/line-cap-allowlist.txt" \
    "$@"
