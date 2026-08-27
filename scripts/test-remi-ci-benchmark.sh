#!/usr/bin/env bash
# scripts/test-remi-ci-benchmark.sh -- Contract tests for the Remi CI benchmark.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "[test-remi-ci-benchmark] ERROR: $*" >&2
    exit 1
}

mapfile -t rust_phases < <(bash scripts/remi-ci-benchmark.sh --scope rust --list)
mapfile -t integration_phases < <(
    bash scripts/remi-ci-benchmark.sh --scope integration --list
)
mapfile -t full_phases < <(bash scripts/remi-ci-benchmark.sh --scope full --list)

[[ "${#rust_phases[@]}" -eq 4 ]] || fail "rust scope must contain 4 phases"
[[ "${#integration_phases[@]}" -eq 29 ]] ||
    fail "integration scope must contain 29 phases"
[[ "${#full_phases[@]}" -eq 33 ]] || fail "full scope must contain 33 phases"
[[ "${full_phases[*]}" == "${rust_phases[*]} ${integration_phases[*]}" ]] ||
    fail "full scope must be the exact rust and integration phase concatenation"

duplicates="$(printf '%s\n' "${full_phases[@]}" | sort | uniq -d)"
[[ -z "$duplicates" ]] || fail "benchmark phases are duplicated: ${duplicates}"

for expected in \
    build-image-opensuse-tumbleweed \
    native-cross-source-lifecycle-pop-os-24.04 \
    native-daily-driver-corpus-fedora44 \
    native-pm-parity-arch \
    debian-derivative-acceptance-linux-mint-22.3 \
    rolling-derivative-acceptance-cachyos; do
    printf '%s\n' "${full_phases[@]}" | grep -Fxq "$expected" ||
        fail "full scope is missing ${expected}"
done

if bash scripts/remi-ci-benchmark.sh --scope invalid --list >/dev/null 2>&1; then
    fail "invalid benchmark scope unexpectedly succeeded"
fi

workflow=".github/workflows/remi-ci-benchmark.yml"
[[ -f "$workflow" ]] || fail "benchmark workflow is missing"
rg -q '^  workflow_dispatch:$' "$workflow" || fail "manual dispatch trigger is missing"
if rg -q 'pull_request:|pull_request_target:|^  push:' "$workflow"; then
    fail "benchmark workflow must not accept automatic code triggers"
fi
rg -q 'runs-on: \[self-hosted, linux, x64, remi-ci-trusted\]' "$workflow" ||
    fail "benchmark workflow must select only the restricted trusted runner"
rg -q '^  contents: read$' "$workflow" || fail "workflow contents permission is not read-only"
if rg -q 'secrets\.' "$workflow"; then
    fail "benchmark workflow must not receive repository or environment secrets"
fi
rg -q 'scripts/remi-ci-runner-preflight\.sh --mode rust' "$workflow" ||
    fail "runner isolation preflight is missing"
rg -q 'git merge-base --is-ancestor "\$EXPECTED_COMMIT" origin/main' "$workflow" ||
    fail "benchmark commit is not constrained to merged main history"
rg -q 'scripts/remi-ci-benchmark\.sh' "$workflow" ||
    fail "canonical benchmark script is not invoked"
rg -q 'actions/upload-artifact@[0-9a-f]{40}' "$workflow" ||
    fail "machine-readable evidence is not retained by a pinned action"

rg -q 'CONARY_CI_MIN_LOGICAL_CPUS:-12' scripts/remi-ci-runner-preflight.sh ||
    fail "runner preflight does not require Remi's 12 logical CPUs"
rg -q 'CONARY_CI_MIN_MEMORY_GIB:-48' scripts/remi-ci-runner-preflight.sh ||
    fail "runner preflight does not require the Remi memory floor"
rg -q 'check_cgroup_v2_limits' scripts/remi-ci-runner-preflight.sh ||
    fail "runner preflight does not reject cgroup resource ceilings"
rg -q 'export CARGO_BUILD_JOBS="\$logical_cpus"' scripts/remi-ci-benchmark.sh ||
    fail "Cargo builds do not receive all runner CPUs"
rg -q 'export RUST_TEST_THREADS="\$logical_cpus"' scripts/remi-ci-benchmark.sh ||
    fail "Rust tests do not receive all runner CPUs"
rg -q 'integration_parallel_jobs="\$logical_cpus"' scripts/remi-ci-benchmark.sh ||
    fail "container phase concurrency does not start from all runner CPUs"
rg -q 'queue_phase "build-image-\$\{distro\}"' scripts/remi-ci-benchmark.sh ||
    fail "distro image builds are not scheduled concurrently"
rg -q 'queue_phase "\$phase" run_suite' scripts/remi-ci-benchmark.sh ||
    fail "distro suite phases are not scheduled concurrently"

installer="deploy/setup-remi-ci-runner.sh"
service="deploy/systemd/github-actions-remi-ci-runner.service"
[[ -x "$installer" ]] || fail "Remi runner installer is missing or not executable"
[[ -f "$service" ]] || fail "Remi runner service template is missing"
rg -q 'RUNNER_USER="conary-ci"' "$installer" ||
    fail "installer does not use the dedicated runner identity"
rg -q 'RUNNER_VERSION="2\.337\.0"' "$installer" ||
    fail "installer runner version is not pinned"
rg -q 'RUNNER_SHA256="[0-9a-f]{64}"' "$installer" ||
    fail "installer runner archive digest is not pinned"
rg -q 'rustup.*1\.98\.0|RUST_VERSION="1\.98\.0"' "$installer" ||
    fail "installer does not provision the exact workspace Rust version"
rg -q 'rustup target add.*x86_64-unknown-linux-musl' "$installer" ||
    fail "installer does not provision the static-build Rust target"
rg -q 'sccache' "$installer" || fail "installer does not provision sccache"
rg -q 'podman\.socket' "$installer" ||
    fail "installer does not provision the rootless Podman socket"
rg -q 'CPUQuotaPerSecUSec.*infinity' "$installer" ||
    fail "installer does not verify an uncapped runner CPU allocation"
rg -q 'MemoryMax.*infinity' "$installer" ||
    fail "installer does not verify an uncapped runner memory allocation"
if rg -q '^(CPUQuota|MemoryMax)=' "$service"; then
    fail "runner service template must not impose CPU or memory ceilings"
fi

echo "Remi CI benchmark contracts passed."
