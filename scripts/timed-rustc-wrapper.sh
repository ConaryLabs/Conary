#!/usr/bin/env bash
# scripts/timed-rustc-wrapper.sh
set -uo pipefail

real_wrapper="${CONARY_REAL_RUSTC_WRAPPER:-}"
timings_path="${CONARY_RUSTC_TIMINGS_PATH:-}"

if [[ -z "$real_wrapper" || ! -x "$real_wrapper" ]]; then
    echo "timed-rustc-wrapper: CONARY_REAL_RUSTC_WRAPPER must name an executable" >&2
    exit 1
fi

crate_name=unknown
crate_type=unknown
previous=""
for argument in "$@"; do
    case "$previous" in
        crate-name) crate_name="$argument" ;;
        crate-type) crate_type="$argument" ;;
    esac
    previous=""
    case "$argument" in
        --crate-name) previous=crate-name ;;
        --crate-name=*) crate_name="${argument#--crate-name=}" ;;
        --crate-type) previous=crate-type ;;
        --crate-type=*) crate_type="${argument#--crate-type=}" ;;
    esac
done

start_ns="$(date -u +%s%N)"
set +e
"$real_wrapper" "$@"
status=$?
set -e
finished_ns="$(date -u +%s%N)"

if [[ -n "$timings_path" ]]; then
    crate_name="${crate_name//$'\t'/_}"
    crate_name="${crate_name//$'\n'/_}"
    crate_type="${crate_type//$'\t'/_}"
    crate_type="${crate_type//$'\n'/_}"
    duration_ms=$(( (finished_ns - start_ns) / 1000000 ))
    {
        flock 9
        printf '%s\t%s\t%s\t%s\n' \
            "$duration_ms" "$status" "$crate_name" "$crate_type" >&9
    } 9>>"$timings_path" || {
        echo "timed-rustc-wrapper: could not retain compiler timing" >&2
        exit 1
    }
fi

exit "$status"
