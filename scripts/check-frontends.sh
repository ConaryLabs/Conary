#!/usr/bin/env bash
# scripts/check-frontends.sh -- Clean-install, type-check, and build both SvelteKit frontends.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for frontend in site web; do
    echo "[$frontend] Installing locked frontend dependencies..."
    (
        cd "$repo_root/$frontend"
        npm ci
        npm run check
        npm run build
    )
done

echo "Frontend checks and builds passed."
