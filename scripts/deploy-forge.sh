#!/usr/bin/env bash
# scripts/deploy-forge.sh -- Managed Forge rollout wrapper
#
# Usage:
#   ./scripts/deploy-forge.sh (--unit NAME | --group NAME) [--ref REF | --path PATH]
#
# Source selection:
#   --ref REF   Trusted/default mode. Fetch REF from GitHub on Forge (default: main)
#   --path PATH Debug mode. Rsync a local snapshot directly over ~/Conary on Forge,
#               then invoke the managed rollout against that active checkout.
set -euo pipefail

FORGE_HOST="${FORGE_HOST:-peter@forge.conarylabs.com}"
FORGE_DEST="${FORGE_DEST:-/home/peter/Conary}"
TARGET_FLAG=""
TARGET_VALUE=""
SOURCE_MODE="ref"
SOURCE_VALUE="main"

usage() {
    cat <<'EOF'
Usage:
  ./scripts/deploy-forge.sh (--unit NAME | --group NAME) [--ref REF | --path PATH]

Managed Forge rollout wrapper. Trusted/default mode is --ref (default: main).
Debug/local-snapshot mode is --path, which rsyncs directly over the active
Forge checkout before invoking the managed rollout command there.

Options:
  --unit NAME      Roll out a single manifest unit
  --group NAME     Roll out a named manifest group
  --ref REF        Deploy an exact GitHub ref on Forge (default: main)
  --path PATH      Deploy a local snapshot by rsyncing PATH over the active Forge checkout
  --help           Show this help text

Examples:
  ./scripts/deploy-forge.sh --group control_plane --ref main
  ./scripts/deploy-forge.sh --unit conary_test --path "$(pwd)"
EOF
}

quote_remote_args() {
    local quoted=""
    local arg
    for arg in "$@"; do
        quoted+=" $(printf '%q' "$arg")"
    done
    printf '%s' "$quoted"
}

remote_rollout_command() {
    local quoted_args
    quoted_args="$(quote_remote_args deploy rollout "$@")"
    cat <<EOF
cd $(printf '%q' "$FORGE_DEST")
cargo run -p conary-test --${quoted_args}
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help)
            usage
            exit 0
            ;;
        --unit)
            [[ -z "$TARGET_FLAG" ]] || { echo "Exactly one of --unit or --group is required." >&2; exit 1; }
            TARGET_FLAG="--unit"
            TARGET_VALUE="${2:?missing value for --unit}"
            shift 2
            ;;
        --unit=*)
            [[ -z "$TARGET_FLAG" ]] || { echo "Exactly one of --unit or --group is required." >&2; exit 1; }
            TARGET_FLAG="--unit"
            TARGET_VALUE="${1#*=}"
            shift
            ;;
        --group)
            [[ -z "$TARGET_FLAG" ]] || { echo "Exactly one of --unit or --group is required." >&2; exit 1; }
            TARGET_FLAG="--group"
            TARGET_VALUE="${2:?missing value for --group}"
            shift 2
            ;;
        --group=*)
            [[ -z "$TARGET_FLAG" ]] || { echo "Exactly one of --unit or --group is required." >&2; exit 1; }
            TARGET_FLAG="--group"
            TARGET_VALUE="${1#*=}"
            shift
            ;;
        --ref)
            [[ "$SOURCE_MODE" == "ref" && "$SOURCE_VALUE" == "main" ]] || {
                echo "Exactly one of --ref or --path is allowed." >&2
                exit 1
            }
            SOURCE_MODE="ref"
            SOURCE_VALUE="${2:?missing value for --ref}"
            shift 2
            ;;
        --ref=*)
            [[ "$SOURCE_MODE" == "ref" && "$SOURCE_VALUE" == "main" ]] || {
                echo "Exactly one of --ref or --path is allowed." >&2
                exit 1
            }
            SOURCE_MODE="ref"
            SOURCE_VALUE="${1#*=}"
            shift
            ;;
        --path)
            [[ "$SOURCE_MODE" == "ref" && "$SOURCE_VALUE" == "main" ]] || {
                echo "Exactly one of --ref or --path is allowed." >&2
                exit 1
            }
            SOURCE_MODE="path"
            SOURCE_VALUE="${2:?missing value for --path}"
            shift 2
            ;;
        --path=*)
            [[ "$SOURCE_MODE" == "ref" && "$SOURCE_VALUE" == "main" ]] || {
                echo "Exactly one of --ref or --path is allowed." >&2
                exit 1
            }
            SOURCE_MODE="path"
            SOURCE_VALUE="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$TARGET_FLAG" || -z "$TARGET_VALUE" ]]; then
    echo "One of --unit or --group is required." >&2
    usage >&2
    exit 1
fi

if [[ "$SOURCE_MODE" == "path" ]]; then
    echo "Syncing local snapshot ${SOURCE_VALUE} -> ${FORGE_HOST}:${FORGE_DEST}"
    rsync -az --delete \
        --exclude target/ \
        --exclude '.git' \
        --exclude '.git/' \
        --exclude '.worktrees/' \
        "${SOURCE_VALUE}/" "${FORGE_HOST}:${FORGE_DEST}/"
    echo "Sync complete."
fi

if [[ "$SOURCE_MODE" == "ref" ]]; then
    echo "Running managed Forge rollout from GitHub ref ${SOURCE_VALUE}"
    REMOTE_CMD="$(remote_rollout_command "$TARGET_FLAG" "$TARGET_VALUE" --ref "$SOURCE_VALUE")"
else
    echo "Running managed Forge rollout from local snapshot now staged at ${FORGE_DEST}"
    REMOTE_CMD="$(remote_rollout_command "$TARGET_FLAG" "$TARGET_VALUE" --path "$FORGE_DEST")"
fi

ssh "$FORGE_HOST" "$REMOTE_CMD"
