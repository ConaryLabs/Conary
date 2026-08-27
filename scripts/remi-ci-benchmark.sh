#!/usr/bin/env bash
# scripts/remi-ci-benchmark.sh -- Produce exact trusted-runner timing evidence.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: remi-ci-benchmark.sh --scope rust|integration|full \
  --commit <40-lowercase-hex-sha> --cache-namespace <name> --output <json>
       remi-ci-benchmark.sh --scope rust|integration|full --list

Run or list the consolidated phases used to compare a trusted persistent Remi
runner with the hosted PR gate. The benchmark is evidence only; it is not a
release or required-check authority.
EOF
}

SCOPE=""
COMMIT_SHA=""
CACHE_NAMESPACE=""
OUTPUT=""
LIST_ONLY="false"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scope)
            SCOPE="${2:-}"
            shift 2
            ;;
        --commit)
            COMMIT_SHA="${2:-}"
            shift 2
            ;;
        --cache-namespace)
            CACHE_NAMESPACE="${2:-}"
            shift 2
            ;;
        --output)
            OUTPUT="${2:-}"
            shift 2
            ;;
        --list)
            LIST_ONLY="true"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[remi-ci-benchmark] ERROR: unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

case "$SCOPE" in
    rust|integration|full)
        ;;
    *)
        echo "[remi-ci-benchmark] ERROR: invalid scope: ${SCOPE}" >&2
        exit 1
        ;;
esac

LIFECYCLE_DISTROS=(
    fedora44
    ubuntu-26.04
    arch
    artix
    cachyos
    opensuse-tumbleweed
    linux-mint-22.3
    pop-os-24.04
)
CORE_DISTROS=(fedora44 ubuntu-26.04 arch)
DEBIAN_DERIVATIVES=(linux-mint-22.3 pop-os-24.04)
ROLLING_DERIVATIVES=(cachyos opensuse-tumbleweed)

list_rust_phases() {
    printf '%s\n' \
        rust-clippy \
        workspace-tests \
        conary-test-crate \
        workspace-doctests
}

list_integration_phases() {
    printf '%s\n' \
        container-preflight \
        build-static-conary \
        build-integration-harness
    local distro
    for distro in "${LIFECYCLE_DISTROS[@]}"; do
        printf 'build-image-%s\n' "$distro"
    done
    for distro in "${LIFECYCLE_DISTROS[@]}"; do
        printf 'native-cross-source-lifecycle-%s\n' "$distro"
    done
    for distro in "${CORE_DISTROS[@]}"; do
        printf 'native-daily-driver-corpus-%s\n' "$distro"
    done
    for distro in "${CORE_DISTROS[@]}"; do
        printf 'native-pm-parity-%s\n' "$distro"
    done
    for distro in "${DEBIAN_DERIVATIVES[@]}"; do
        printf 'debian-derivative-acceptance-%s\n' "$distro"
    done
    for distro in "${ROLLING_DERIVATIVES[@]}"; do
        printf 'rolling-derivative-acceptance-%s\n' "$distro"
    done
}

list_phases() {
    if [[ "$SCOPE" == "rust" || "$SCOPE" == "full" ]]; then
        list_rust_phases
    fi
    if [[ "$SCOPE" == "integration" || "$SCOPE" == "full" ]]; then
        list_integration_phases
    fi
}

if [[ "$LIST_ONLY" == "true" ]]; then
    list_phases
    exit 0
fi

[[ "$COMMIT_SHA" =~ ^[0-9a-f]{40}$ ]] || {
    echo "[remi-ci-benchmark] ERROR: --commit must be exactly 40 lowercase hexadecimal digits" >&2
    exit 1
}
[[ "$CACHE_NAMESPACE" =~ ^[a-z0-9][a-z0-9._-]{0,63}$ ]] || {
    echo "[remi-ci-benchmark] ERROR: invalid --cache-namespace" >&2
    exit 1
}
[[ -n "$OUTPUT" ]] || {
    echo "[remi-ci-benchmark] ERROR: --output is required" >&2
    exit 1
}

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
actual_commit="$(git rev-parse HEAD)"
[[ "$actual_commit" == "$COMMIT_SHA" ]] || {
    echo "[remi-ci-benchmark] ERROR: checkout is ${actual_commit}, expected ${COMMIT_SHA}" >&2
    exit 1
}
git diff --quiet
git diff --cached --quiet
bash scripts/remi-ci-runner-preflight.sh --mode rust

cache_root="${CONARY_CI_CACHE_ROOT:-${HOME}/.cache/conary-ci}"
[[ "$cache_root" == /* ]] || {
    echo "[remi-ci-benchmark] ERROR: CONARY_CI_CACHE_ROOT must be absolute" >&2
    exit 1
}
cache_dir="${cache_root}/benchmarks/${CACHE_NAMESPACE}"
success_marker="${cache_dir}/last-successful-commit"
cache_disposition="cold"
if [[ -f "$success_marker" ]]; then
    cache_disposition="warm-other-commit"
    grep -Fxq "$COMMIT_SHA" "$success_marker" && cache_disposition="warm-same-commit"
fi

mkdir -p \
    "$cache_dir/cargo-home" \
    "$cache_dir/sccache" \
    "$cache_dir/target" \
    "$cache_dir/xdg-cache"
export CARGO_HOME="${cache_dir}/cargo-home"
export CARGO_TARGET_DIR="${cache_dir}/target"
export SCCACHE_DIR="${cache_dir}/sccache"
export XDG_CACHE_HOME="${cache_dir}/xdg-cache"
export RUSTC_WRAPPER=sccache

# Give the single trusted job all execution capacity visible to the runner.
# The preflight rejects service-level CPU and memory ceilings before this runs.
logical_cpus="$(nproc)"
memory_kib="$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)"
export CARGO_BUILD_JOBS="$logical_cpus"
export CMAKE_BUILD_PARALLEL_LEVEL="$logical_cpus"
export MAKEFLAGS="-j${logical_cpus}"
export RUST_TEST_THREADS="$logical_cpus"
integration_parallel_jobs="$logical_cpus"
[[ "$integration_parallel_jobs" -le "${#LIFECYCLE_DISTROS[@]}" ]] ||
    integration_parallel_jobs="${#LIFECYCLE_DISTROS[@]}"

output_parent="$(dirname "$OUTPUT")"
mkdir -p "$output_parent"
phases_file="$(mktemp)"
phase_results_dir="$(mktemp -d)"
run_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/conary-ci-benchmark.XXXXXX")"
started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
started_ns="$(date +%s%N)"

finalize() {
    local benchmark_status="$?"
    trap - EXIT
    set +e

    local finished_at finished_ns duration_ms passed phase_name
    finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    finished_ns="$(date +%s%N)"
    duration_ms=$(( (finished_ns - started_ns) / 1000000 ))
    passed="false"
    if [[ "$benchmark_status" -eq 0 ]]; then
        passed="true"
        printf '%s\n' "$COMMIT_SHA" > "$success_marker"
    fi

    while IFS= read -r phase_name; do
        [[ -f "${phase_results_dir}/${phase_name}.json" ]] || continue
        jq -c . "${phase_results_dir}/${phase_name}.json" >> "$phases_file"
    done < <(list_phases)

    jq -s \
        --arg commit_sha "$COMMIT_SHA" \
        --arg scope "$SCOPE" \
        --arg cache_namespace "$CACHE_NAMESPACE" \
        --arg cache_disposition "$cache_disposition" \
        --arg runner_name "${RUNNER_NAME:-local}" \
        --arg runner_os "${RUNNER_OS:-Linux}" \
        --arg runner_arch "${RUNNER_ARCH:-X64}" \
        --argjson logical_cpus "$logical_cpus" \
        --argjson memory_kib "$memory_kib" \
        --argjson parallel_jobs "$CARGO_BUILD_JOBS" \
        --argjson integration_parallel_jobs "$integration_parallel_jobs" \
        --arg github_run_id "${GITHUB_RUN_ID:-}" \
        --arg github_run_attempt "${GITHUB_RUN_ATTEMPT:-}" \
        --arg started_at "$started_at" \
        --arg finished_at "$finished_at" \
        --argjson duration_ms "$duration_ms" \
        --argjson passed "$passed" \
        '{
            schema_version: 1,
            issue: 620,
            commit_sha: $commit_sha,
            scope: $scope,
            cache: {
                namespace: $cache_namespace,
                disposition: $cache_disposition
            },
            runner: {
                name: $runner_name,
                os: $runner_os,
                architecture: $runner_arch,
                resources: {
                    logical_cpus: $logical_cpus,
                    memory_kib: $memory_kib,
                    parallel_jobs: $parallel_jobs,
                    integration_parallel_jobs: $integration_parallel_jobs,
                    cgroup_cpu_limit: "none",
                    cgroup_memory_limit: "none"
                }
            },
            github: {
                run_id: $github_run_id,
                run_attempt: $github_run_attempt
            },
            started_at: $started_at,
            finished_at: $finished_at,
            duration_ms: $duration_ms,
            passed: $passed,
            phases: .
        }' "$phases_file" > "${OUTPUT}.tmp"
    mv "${OUTPUT}.tmp" "$OUTPUT"
    sccache --show-stats || true
    rm -f "$phases_file"
    rm -rf "$phase_results_dir"
    rm -rf "$run_root"
    exit "$benchmark_status"
}
trap finalize EXIT

run_phase() {
    local name="$1"
    shift
    local phase_started_ns phase_finished_ns duration_ms status_code phase_status phase_output
    phase_started_ns="$(date +%s%N)"
    echo "[remi-ci-benchmark] phase ${name}"
    set +e
    "$@"
    status_code="$?"
    set -e
    phase_finished_ns="$(date +%s%N)"
    duration_ms=$(( (phase_finished_ns - phase_started_ns) / 1000000 ))
    phase_status="passed"
    [[ "$status_code" -eq 0 ]] || phase_status="failed"
    phase_output="$(mktemp "${phase_results_dir}/${name}.XXXXXX")"
    jq -cn \
        --arg name "$name" \
        --arg status "$phase_status" \
        --argjson duration_ms "$duration_ms" \
        '{name: $name, status: $status, duration_ms: $duration_ms}' > "$phase_output"
    mv "$phase_output" "${phase_results_dir}/${name}.json"
    return "$status_code"
}

queued_phase_pids=()
queued_phase_failed="false"

reap_queued_phase() {
    local pid="${queued_phase_pids[0]}"
    if ! wait "$pid"; then
        queued_phase_failed="true"
    fi
    queued_phase_pids=("${queued_phase_pids[@]:1}")
}

queue_phase() {
    while [[ "${#queued_phase_pids[@]}" -ge "$integration_parallel_jobs" ]]; do
        reap_queued_phase
    done
    (
        trap - EXIT
        run_phase "$@"
    ) &
    queued_phase_pids+=("$!")
}

finish_queued_phases() {
    while [[ "${#queued_phase_pids[@]}" -gt 0 ]]; do
        reap_queued_phase
    done
    [[ "$queued_phase_failed" == "false" ]]
}

build_image() {
    cargo run -p conary-test -- images build --distro "$1"
}

run_suite() {
    local phase_name="$1"
    local suite="$2"
    local distro="$3"
    local corpus_gate="$4"
    local result_dir="${run_root}/results/${phase_name}"
    local result_file="${result_dir}/${distro}-phase4.json"
    mkdir -p "$result_dir"
    CONARY_TEST_RESULTS_DIR="$result_dir" \
        CONARY_TEST_REUSE_IMAGE=1 \
        cargo run -p conary-test -- \
        run --distro "$distro" --phase 4 --suite "$suite" || return
    bash scripts/check-conary-test-result-gate.sh "$result_file" || return
    if [[ "$corpus_gate" == "true" ]]; then
        bash scripts/check-conary-corpus-result-gate.sh "$result_file" || return
    fi
}

run_rust_tests_as_root() {
    # Match hosted container-root semantics without granting host root: Podman
    # maps inner root to conary-ci and the remaining IDs to its subordinate
    # UID/GID ranges, so signed numeric ownership remains representable.
    podman unshare "$@"
}

sccache --start-server >/dev/null
sccache --zero-stats >/dev/null

if [[ "$SCOPE" == "rust" || "$SCOPE" == "full" ]]; then
    run_phase rust-clippy cargo clippy --workspace --all-targets -- -D warnings
    run_phase workspace-tests run_rust_tests_as_root \
        cargo test --workspace --exclude conary-test --verbose
    run_phase conary-test-crate run_rust_tests_as_root \
        cargo test -p conary-test --verbose
    run_phase workspace-doctests run_rust_tests_as_root \
        cargo test --doc --workspace --verbose
fi

if [[ "$SCOPE" == "integration" || "$SCOPE" == "full" ]]; then
    run_phase container-preflight bash scripts/remi-ci-runner-preflight.sh --mode container
    run_phase build-static-conary bash scripts/build-static-conary.sh
    run_phase build-integration-harness cargo build -p conary -p conary-test

    for distro in "${LIFECYCLE_DISTROS[@]}"; do
        queue_phase "build-image-${distro}" build_image "$distro"
    done
    finish_queued_phases

    queued_phase_failed="false"
    for distro in "${LIFECYCLE_DISTROS[@]}"; do
        phase="native-cross-source-lifecycle-${distro}"
        queue_phase "$phase" run_suite \
            "$phase" native-cross-source-lifecycle "$distro" true
    done
    for distro in "${CORE_DISTROS[@]}"; do
        phase="native-daily-driver-corpus-${distro}"
        queue_phase "$phase" run_suite \
            "$phase" phase4-native-daily-driver-corpus "$distro" true
    done
    for distro in "${CORE_DISTROS[@]}"; do
        phase="native-pm-parity-${distro}"
        queue_phase "$phase" run_suite \
            "$phase" phase4-native-pm-parity "$distro" false
    done
    for distro in "${DEBIAN_DERIVATIVES[@]}"; do
        phase="debian-derivative-acceptance-${distro}"
        queue_phase "$phase" run_suite \
            "$phase" debian-derivative-acceptance "$distro" false
    done
    for distro in "${ROLLING_DERIVATIVES[@]}"; do
        phase="rolling-derivative-acceptance-${distro}"
        queue_phase "$phase" run_suite \
            "$phase" rolling-derivative-acceptance "$distro" false
    done
    finish_queued_phases
fi
