#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

export SCCACHE_DIR="${SCCACHE_DIR:-$REPO_ROOT/.sccache}"
SCCACHE_BIN="${SCCACHE_BIN:-$(command -v sccache || true)}"

if [[ -n "$SCCACHE_BIN" && -x "$SCCACHE_BIN" ]]; then
    if "$SCCACHE_BIN" "$@"; then
        exit 0
    fi
fi

exec rustc "$@"
