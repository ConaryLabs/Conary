#!/bin/sh
# apps/conary/tests/fixtures/native/retry-command.sh
set -eu

attempts="${1:-}"
case "$attempts" in
    ''|*[!0-9]*)
        echo "usage: retry-command.sh <attempts:1..10> <command> [args...]" >&2
        exit 2
        ;;
esac
[ "$attempts" -ge 1 ] && [ "$attempts" -le 10 ] && [ "$#" -ge 2 ] || {
    echo "usage: retry-command.sh <attempts:1..10> <command> [args...]" >&2
    exit 2
}
shift

attempt=1
delay=1
while true; do
    if "$@"; then
        exit 0
    else
        status=$?
    fi
    if [ "$attempt" -ge "$attempts" ]; then
        exit "$status"
    fi
    echo "command failed; retrying in ${delay}s (${attempt}/${attempts}): $*" >&2
    sleep "$delay"
    attempt=$((attempt + 1))
    delay=$((delay * 2))
done
