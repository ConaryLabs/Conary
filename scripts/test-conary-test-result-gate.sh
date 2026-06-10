#!/usr/bin/env bash
set -euo pipefail

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

gate="scripts/check-conary-test-result-gate.sh"

write_json() {
    local name="$1"
    local body="$2"
    printf '%s\n' "$body" > "${tmpdir}/${name}.json"
}

expect_pass() {
    local name="$1"
    if ! bash "$gate" "${tmpdir}/${name}.json" >"${tmpdir}/${name}.out" 2>"${tmpdir}/${name}.err"; then
        echo "expected ${name} to pass" >&2
        cat "${tmpdir}/${name}.out" >&2
        cat "${tmpdir}/${name}.err" >&2
        exit 1
    fi
}

expect_fail() {
    local name="$1"
    if bash "$gate" "${tmpdir}/${name}.json" >"${tmpdir}/${name}.out" 2>"${tmpdir}/${name}.err"; then
        echo "expected ${name} to fail" >&2
        cat "${tmpdir}/${name}.out" >&2
        cat "${tmpdir}/${name}.err" >&2
        exit 1
    fi
}

write_json good '{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","name":"smoke","status":"passed"}]}'
write_json empty '{}'
write_json empty_results '{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[]}'
write_json missing_total '{"summary":{"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"passed"}]}'
write_json total_mismatch '{"summary":{"total":2,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"passed"}]}'
write_json status_mismatch '{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"id":"T01","status":"failed"}]}'
write_json skipped '{"summary":{"total":1,"passed":0,"failed":0,"skipped":1,"cancelled":0},"results":[{"id":"T01","name":"skipped","status":"skipped"}]}'

expect_pass good
expect_fail empty
expect_fail empty_results
expect_fail missing_total
expect_fail total_mismatch
expect_fail status_mismatch
expect_fail skipped

echo "conary-test result gate fixtures passed"
