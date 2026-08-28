#!/usr/bin/env bash
set -uo pipefail

real_linker="${CONARY_REAL_LINKER:-/usr/bin/cc}"
timings_path="${CONARY_LINK_TIMINGS_PATH:-}"

start_ns="$(date -u +%s%N)"
set +e
"$real_linker" "$@"
status=$?
set -e
finished_ns="$(date -u +%s%N)"

if [[ -n "$timings_path" ]]; then
    output="unknown"
    expect_output=false
    for argument in "$@"; do
        if [[ "$expect_output" == true ]]; then
            output="${argument##*/}"
            break
        fi
        if [[ "$argument" == "-o" ]]; then
            expect_output=true
        fi
    done
    output="${output//$'\t'/_}"
    output="${output//$'\n'/_}"
    duration_ms=$(( (finished_ns - start_ns) / 1000000 ))
    mkdir -p "$(dirname "$timings_path")"
    {
        flock 9
        printf '%s\t%s\t%s\n' "$duration_ms" "$status" "$output" >&9
    } 9>>"$timings_path"
fi

exit "$status"
