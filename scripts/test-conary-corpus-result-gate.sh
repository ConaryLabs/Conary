#!/usr/bin/env bash
set -euo pipefail

root="$(mktemp -d)"
cleanup() {
  find "${root}" -depth -mindepth 1 -delete
  rmdir "${root}"
}
trap cleanup EXIT
gate="scripts/check-conary-corpus-result-gate.sh"

write_result() {
  local path="$1"
  local digest="$2"
  local completed="$3"
  local failed="$4"
  local outcome="$5"
  printf '%s\n' "{\"corpus\":{\"schema_version\":2,\"expected_cases\":1,\"aggregate\":{\"cases\":1,\"completed\":${completed},\"failed\":${failed},\"skipped\":0,\"stages\":[],\"unclassified\":0},\"coverage\":{\"required\":[\"identity_exact_version\"],\"covered\":[\"identity_exact_version\"],\"missing\":[],\"unattributed_claims\":0},\"cases\":[{\"case_id\":\"rpm-fedora\",\"source_profile\":{\"profile\":\"fedora-44\",\"format\":\"rpm\"},\"source_artifacts\":[{\"role\":\"install_request\",\"digest_source\":\"fixture_build_manifest\",\"name\":\"fixture\",\"version\":\"1-1\",\"architecture\":\"x86_64\",\"digest\":\"${digest}\"}],\"target_profile\":{\"profile\":\"fedora44\",\"architecture\":\"x86_64\",\"init_system\":\"systemd\",\"capabilities\":[\"native_lifecycle\"]},\"coverage_claims\":[{\"semantic\":\"identity_exact_version\",\"artifact_roles\":[\"install_request\"]}],\"stage_results\":[{\"stage\":\"installation\",\"outcome\":\"${outcome}\"}],\"final_outcome\":{\"outcome\":\"completed\"}}]}}" > "${path}"
}

write_result "${root}/passing.json" "$(printf 'a%.0s' {1..64})" 1 0 passed
"${gate}" "${root}/passing.json"

write_result "${root}/bad-digest.json" "short" 1 0 passed
if "${gate}" "${root}/bad-digest.json" >/dev/null 2>&1; then
  echo "bad digest unexpectedly passed corpus gate" >&2
  exit 1
fi

write_result "${root}/failed.json" "$(printf 'b%.0s' {1..64})" 0 1 failed
if "${gate}" "${root}/failed.json" >/dev/null 2>&1; then
  echo "failed case unexpectedly passed corpus gate" >&2
  exit 1
fi

sed 's/"expected_cases":1/"expected_cases":2/' \
  "${root}/passing.json" > "${root}/missing-case.json"
if "${gate}" "${root}/missing-case.json" >/dev/null 2>&1; then
  echo "missing emitted case unexpectedly passed corpus gate" >&2
  exit 1
fi

sed 's/"missing":\[\]/"missing":["identity_exact_version"]/' \
  "${root}/passing.json" > "${root}/missing-coverage.json"
if "${gate}" "${root}/missing-coverage.json" >/dev/null 2>&1; then
  echo "missing semantic coverage unexpectedly passed corpus gate" >&2
  exit 1
fi

sed 's/"artifact_roles":\["install_request"\]/"artifact_roles":["update_request"]/' \
  "${root}/passing.json" > "${root}/unbound-coverage.json"
if "${gate}" "${root}/unbound-coverage.json" >/dev/null 2>&1; then
  echo "unbound semantic coverage unexpectedly passed corpus gate" >&2
  exit 1
fi

sed 's/"covered":\["identity_exact_version"\]/"covered":["payload_files"]/' \
  "${root}/passing.json" > "${root}/contradictory-coverage.json"
if "${gate}" "${root}/contradictory-coverage.json" >/dev/null 2>&1; then
  echo "contradictory semantic coverage unexpectedly passed corpus gate" >&2
  exit 1
fi

printf '%s\n' '{"summary":{"total":1,"passed":1,"failed":0,"skipped":0,"cancelled":0},"results":[{"status":"passed"}]}' > "${root}/missing.json"
if "${gate}" "${root}/missing.json" >/dev/null 2>&1; then
  echo "missing corpus unexpectedly passed corpus gate" >&2
  exit 1
fi

echo "corpus result gate tests passed"
