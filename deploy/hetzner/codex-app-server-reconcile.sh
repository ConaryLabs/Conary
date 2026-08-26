#!/usr/bin/env bash
# deploy/hetzner/codex-app-server-reconcile.sh

set -euo pipefail

state_dir="${CODEX_APP_SERVER_DAEMON_STATE_DIR:-$HOME/.codex/app-server-daemon}"

if [[ ! -d "$state_dir" ]]; then
    exit 0
fi

for command in flock jq ps; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf 'codex daemon reconciliation requires %s\n' "$command" >&2
        exit 1
    fi
done

exec 9>"$state_dir/daemon.lock"
flock -x 9

for name in app-server app-server-updater; do
    pid_file="$state_dir/$name.pid"
    [[ -e "$pid_file" ]] || continue

    if [[ ! -f "$pid_file" ]]; then
        printf 'refusing non-regular Codex daemon PID record: %s\n' "$pid_file" >&2
        exit 1
    fi

    if ! pid="$(jq -er '.pid | numbers' "$pid_file")" ||
        ! expected_start="$(jq -er '.processStartTime | strings | select(length > 0)' "$pid_file")" ||
        [[ ! "$pid" =~ ^[1-9][0-9]*$ ]]; then
        printf 'refusing malformed Codex daemon PID record: %s\n' "$pid_file" >&2
        exit 1
    fi

    if actual_start="$(LC_ALL=C ps -p "$pid" -o lstart= 2>/dev/null)"; then
        actual_start="${actual_start#"${actual_start%%[![:space:]]*}"}"
        actual_start="${actual_start%"${actual_start##*[![:space:]]}"}"
        if [[ "$actual_start" == "$expected_start" ]]; then
            continue
        fi
        reason="start time mismatch"
    else
        reason="process absent"
    fi

    rm -f -- "$pid_file"
    printf 'removed stale Codex daemon PID record (%s): %s\n' "$reason" "$pid_file"
done
