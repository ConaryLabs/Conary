#!/usr/bin/env bash
set -euo pipefail

export SCCACHE_DIR="${SCCACHE_DIR:-/home/peter/Conary/.sccache}"
SCCACHE_BIN="${SCCACHE_BIN:-/home/peter/.cargo/bin/sccache}"

if [ -x "$SCCACHE_BIN" ]; then
  if "$SCCACHE_BIN" "$@"; then
    exit 0
  fi
fi

exec rustc "$@"
