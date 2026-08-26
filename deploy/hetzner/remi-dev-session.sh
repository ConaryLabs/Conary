#!/usr/bin/env bash
# deploy/hetzner/remi-dev-session.sh -- Enter a project-owned Remi tmux session.

set -euo pipefail

readonly DEV_SOURCE_ROOT="${DEV_SOURCE_ROOT:-/data/dev/src}"
project="${1:-conary}"

case "$project" in
    conary)
        checkout="Conary"
        session="conary"
        ;;
    rpm-rs)
        checkout="rpm-rs"
        session="rpm-rs"
        ;;
    signed-world)
        checkout="signed-world"
        session="signed-world"
        ;;
    *)
        printf 'usage: dev [conary|rpm-rs|signed-world]\n' >&2
        exit 2
        ;;
esac

repo="$DEV_SOURCE_ROOT/$checkout"
if [[ ! -d "$repo/.git" ]]; then
    printf 'dev: missing checkout at %s\n' "$repo" >&2
    exit 1
fi

if tmux has-session -t "$session" 2>/dev/null; then
    exec tmux attach-session -t "$session"
fi

exec tmux new-session -s "$session" -c "$repo"
