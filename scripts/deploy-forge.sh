#!/usr/bin/env bash
# scripts/deploy-forge.sh -- Rsync current source to Forge for testing
#
# Usage:
#   ./scripts/deploy-forge.sh [--build] [--path PATH]
#
# Options:
#   --build   Also run cargo build on Forge after syncing
#   --path    Source path to sync (default: current repo root)
set -euo pipefail

FORGE_HOST="peter@forge.conarylabs.com"
FORGE_DEST="~/Conary"
DO_BUILD=false
SOURCE_PATH="$(cd "$(dirname "$0")/.." && pwd)"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build) DO_BUILD=true; shift ;;
        --path)  SOURCE_PATH="$2"; shift 2 ;;
        --path=*) SOURCE_PATH="${1#*=}"; shift ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

echo "Syncing ${SOURCE_PATH} -> ${FORGE_HOST}:${FORGE_DEST}"
rsync -az --delete \
    --exclude target/ \
    --exclude '.git/' \
    --exclude '.worktrees/' \
    "$SOURCE_PATH/" "${FORGE_HOST}:${FORGE_DEST}/"

echo "Sync complete."

if [[ "$DO_BUILD" == true ]]; then
    echo "Building on Forge..."
    ssh "$FORGE_HOST" "cd ${FORGE_DEST} && cargo build"
    echo "Build complete."
fi
