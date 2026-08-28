#!/usr/bin/env bash
# scripts/native-matrix-artifact.sh
set -euo pipefail

TARGET="x86_64-unknown-linux-musl"
PROFILE="debug"
BUNDLE_NAME="native-matrix-artifacts.tar.gz"
MANIFEST_NAME="native-matrix-artifact-manifest.json"
STATS_NAME="sccache-stats.json"
BUILD_METRICS_NAME="static-build-metrics.json"
BUILD_COMMAND="bash scripts/build-static-conary.sh --with-test-harness"
CACHE_BACKEND="local-disk-bulk-v1"
CACHE_NAMESPACE_PATTERN='^native-matrix-musl-local-v1-[0-9a-f]{64}$'
CACHE_VERSION="0.16.0"
ARTIFACT_PATHS=(
    "$TARGET/$PROFILE/conary"
    "$TARGET/$PROFILE/conary-test"
    "$TARGET/$PROFILE/conary-test-library-tests"
)

usage() {
    cat >&2 <<'EOF'
Usage:
  scripts/native-matrix-artifact.sh package OUTPUT_DIR COMMIT RUN_ID SETUP_MS CACHE_SETUP_MS CACHE_SAVE_MS BUILD_MS SCCACHE_STATS BUILD_METRICS
  scripts/native-matrix-artifact.sh verify ARTIFACT_DIR COMMIT RUN_ID EVENT
EOF
    exit 2
}

fail() {
    echo "native-matrix-artifact: $*" >&2
    exit 1
}

require_uint() {
    local name="$1"
    local value="$2"
    [[ "$value" =~ ^[0-9]+$ ]] || fail "$name must be a non-negative integer"
}

require_commit() {
    [[ "$1" =~ ^[0-9a-f]{40}$ ]] || fail "commit must be a full lowercase SHA"
}

require_regular_file() {
    local path="$1"
    [[ -f "$path" && ! -L "$path" ]] || fail "expected a regular non-symlink file: $path"
}

require_static_executable() {
    local path="$1"
    require_regular_file "$path"
    [[ -x "$path" ]] || fail "expected an executable file: $path"
    file "$path" | grep -Eq 'static-pie linked|statically linked' ||
        fail "artifact is not statically linked: $(file "$path")"
}

require_clean_checkout() {
    [[ -z "$(git status --porcelain --untracked-files=all)" ]] ||
        fail "artifact checkout must be clean"
}

sha256_file() {
    sha256sum "$1" | cut -d ' ' -f 1
}

file_size() {
    stat --printf='%s' "$1"
}

workspace_rust_version() {
    sed -n 's/^rust-version = "\([^"]\+\)"$/\1/p' Cargo.toml
}

package_artifact() {
    [[ $# -eq 9 ]] || usage
    local output_dir="$1"
    local expected_commit="$2"
    local workflow_run_id="$3"
    local setup_ms="$4"
    local cache_setup_ms="$5"
    local cache_save_ms="$6"
    local build_ms="$7"
    local cache_stats="$8"
    local build_metrics="$9"

    require_commit "$expected_commit"
    require_uint workflow_run_id "$workflow_run_id"
    require_uint setup_ms "$setup_ms"
    require_uint cache_setup_ms "$cache_setup_ms"
    require_uint cache_save_ms "$cache_save_ms"
    require_uint build_ms "$build_ms"
    require_regular_file "$cache_stats"
    require_regular_file "$build_metrics"
    jq -e --arg version "$CACHE_VERSION" '
      .version == $version
      and (.cache_location | startswith("Local disk: "))
      and (.stats.compile_requests | type == "number")
      and (.stats.cache_hits.counts | type == "object")
      and (.stats.cache_misses.counts | type == "object")
      and (.stats.cache_errors.counts | type == "object")
      and (.stats.cache_writes | type == "number")
      and (.stats.cache_read_errors | type == "number")
      and (.stats.cache_write_errors | type == "number")
      and (.stats.cache_timeouts | type == "number")
    ' "$cache_stats" >/dev/null ||
        fail "sccache statistics do not prove the protected local backend"
    jq -e '
      .schema_version == 1
      and (.static_dependency_cache_hit | type == "boolean")
      and (.static_dependency_ms | type == "number")
      and (.static_runtime_build_ms | type == "number")
      and (.library_test_build_ms | type == "number")
      and .with_test_harness == true
    ' "$build_metrics" >/dev/null || fail "static build metrics are invalid"
    require_clean_checkout
    [[ "$(git rev-parse HEAD)" == "$expected_commit" ]] ||
        fail "checkout commit does not match requested matrix source"

    local repo_root target_dir
    repo_root="$(git rev-parse --show-toplevel)"
    target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
    local relative
    for relative in "${ARTIFACT_PATHS[@]}"; do
        require_static_executable "$target_dir/$relative"
    done

    mkdir -p "$output_dir"
    output_dir="$(realpath "$output_dir")"
    local bundle="$output_dir/$BUNDLE_NAME"
    local manifest="$output_dir/$MANIFEST_NAME"
    local retained_stats="$output_dir/$STATS_NAME"
    local retained_build_metrics="$output_dir/$BUILD_METRICS_NAME"
    [[ ! -e "$bundle" && ! -e "$manifest" && ! -e "$retained_stats" \
        && ! -e "$retained_build_metrics" ]] ||
        fail "output directory already contains matrix artifact files"
    install -m 0644 "$cache_stats" "$retained_stats"
    install -m 0644 "$build_metrics" "$retained_build_metrics"

    local bundle_started_ns bundle_finished_ns bundle_ms
    bundle_started_ns="$(date -u +%s%N)"
    tar --create --format=gnu --sort=name --mtime='UTC 1970-01-01' \
        --owner=0 --group=0 --numeric-owner -C "$target_dir" \
        "${ARTIFACT_PATHS[@]}" | gzip --no-name >"$bundle"
    bundle_finished_ns="$(date -u +%s%N)"
    bundle_ms=$(( (bundle_finished_ns - bundle_started_ns) / 1000000 ))

    local cache_backend cache_namespace
    cache_backend="${SCCACHE_CACHE_BACKEND:-unknown}"
    cache_namespace="${SCCACHE_CACHE_NAMESPACE:-unknown}"
    [[ "$cache_backend" == "$CACHE_BACKEND" ]] ||
        fail "compiler-cache backend is not the protected local bulk backend"
    [[ "$cache_namespace" =~ $CACHE_NAMESPACE_PATTERN ]] ||
        fail "compiler-cache namespace is not an exact native matrix identity"

    local rust_toolchain rustc_verbose rustc_verbose_sha cargo_verbose
    rust_toolchain="$(workspace_rust_version)"
    [[ -n "$rust_toolchain" ]] || fail "workspace rust-version is missing"
    rustc_verbose="$(rustc -vV)"
    [[ "$(sed -n 's/^rustc \([^ ]\+\).*/\1/p' <<<"$rustc_verbose")" == "$rust_toolchain" ]] ||
        fail "active rustc does not match the workspace rust-version"
    rustc_verbose_sha="$(printf '%s' "$rustc_verbose" | sha256sum | cut -d ' ' -f 1)"
    cargo_verbose="$(cargo -vV)"

    local conary="$target_dir/${ARTIFACT_PATHS[0]}"
    local harness="$target_dir/${ARTIFACT_PATHS[1]}"
    local tests="$target_dir/${ARTIFACT_PATHS[2]}"
    jq -n \
        --arg commit_sha "$expected_commit" \
        --arg tree_sha "$(git rev-parse 'HEAD^{tree}')" \
        --arg lock_sha256 "$(sha256_file Cargo.lock)" \
        --arg workspace_manifest_sha256 "$(sha256_file Cargo.toml)" \
        --arg builder_sha256 "$(sha256_file scripts/build-static-conary.sh)" \
        --arg header_probe_sha256 "$(sha256_file scripts/kernel-header-roots.sh)" \
        --arg action_sha256 "$(sha256_file .github/actions/build-static-conary/action.yml)" \
        --arg rust_toolchain "$rust_toolchain" \
        --arg rustc_verbose "$rustc_verbose" \
        --arg rustc_verbose_sha256 "$rustc_verbose_sha" \
        --arg cargo_verbose "$cargo_verbose" \
        --arg repository "${GITHUB_REPOSITORY:-unknown}" \
        --arg workflow "${GITHUB_WORKFLOW:-unknown}" \
        --arg event "${GITHUB_EVENT_NAME:-unknown}" \
        --arg runner_os "${RUNNER_OS:-unknown}" \
        --arg runner_arch "${RUNNER_ARCH:-unknown}" \
        --arg runner_image_os "${ImageOS:-unknown}" \
        --arg runner_image_version "${ImageVersion:-unknown}" \
        --arg bundle_sha256 "$(sha256_file "$bundle")" \
        --arg stats_sha256 "$(sha256_file "$retained_stats")" \
        --arg build_metrics_sha256 "$(sha256_file "$retained_build_metrics")" \
        --arg conary_sha256 "$(sha256_file "$conary")" \
        --arg harness_sha256 "$(sha256_file "$harness")" \
        --arg tests_sha256 "$(sha256_file "$tests")" \
        --arg rustflags "${RUSTFLAGS:-}" \
        --arg encoded_rustflags "${CARGO_ENCODED_RUSTFLAGS:-}" \
        --arg cargo_incremental "${CARGO_INCREMENTAL:-}" \
        --arg cargo_profile_dev_debug "${CARGO_PROFILE_DEV_DEBUG:-}" \
        --arg cargo_profile_test_debug "${CARGO_PROFILE_TEST_DEBUG:-}" \
        --arg build_command "$BUILD_COMMAND" \
        --arg sccache_version "${SCCACHE_VERSION:-unknown}" \
        --arg sccache_cache_backend "$cache_backend" \
        --arg sccache_cache_namespace "$cache_namespace" \
        --argjson workflow_run_id "$workflow_run_id" \
        --argjson setup_ms "$setup_ms" \
        --argjson cache_setup_ms "$cache_setup_ms" \
        --argjson cache_save_ms "$cache_save_ms" \
        --argjson build_ms "$build_ms" \
        --argjson bundle_ms "$bundle_ms" \
        --argjson bundle_bytes "$(file_size "$bundle")" \
        --argjson stats_bytes "$(file_size "$retained_stats")" \
        --argjson build_metrics_bytes "$(file_size "$retained_build_metrics")" \
        --argjson conary_bytes "$(file_size "$conary")" \
        --argjson harness_bytes "$(file_size "$harness")" \
        --argjson tests_bytes "$(file_size "$tests")" \
        --slurpfile build_metrics "$retained_build_metrics" '
          {
            schema_version: 1,
            source: {
              commit_sha: $commit_sha,
              tree_sha: $tree_sha,
              clean: true,
              cargo_lock_sha256: $lock_sha256,
              workspace_manifest_sha256: $workspace_manifest_sha256,
              builder_sha256: $builder_sha256,
              header_probe_sha256: $header_probe_sha256,
              action_sha256: $action_sha256
            },
            build: {
              rust_toolchain: $rust_toolchain,
              rustc_verbose: $rustc_verbose,
              rustc_verbose_sha256: $rustc_verbose_sha256,
              cargo_verbose: $cargo_verbose,
              target: "x86_64-unknown-linux-musl",
              profile: "dev-and-test",
              features: "default",
              command: $build_command,
              rustflags: $rustflags,
              cargo_encoded_rustflags: $encoded_rustflags,
              cargo_incremental: $cargo_incremental,
              cargo_profile_dev_debug: $cargo_profile_dev_debug,
              cargo_profile_test_debug: $cargo_profile_test_debug,
              static_libseccomp_version: "2.6.0",
              sccache_version: $sccache_version,
              sccache_cache_backend: $sccache_cache_backend,
              sccache_cache_namespace: $sccache_cache_namespace
            },
            provenance: {
              repository: $repository,
              workflow: $workflow,
              event: $event,
              workflow_run_id: $workflow_run_id,
              runner_os: $runner_os,
              runner_arch: $runner_arch,
              runner_image_os: $runner_image_os,
              runner_image_version: $runner_image_version
            },
            artifact: {
              bundle: "native-matrix-artifacts.tar.gz",
              bundle_sha256: $bundle_sha256,
              bundle_bytes: $bundle_bytes,
              compiler_cache_stats: "sccache-stats.json",
              compiler_cache_stats_sha256: $stats_sha256,
              compiler_cache_stats_bytes: $stats_bytes,
              static_build_metrics: "static-build-metrics.json",
              static_build_metrics_sha256: $build_metrics_sha256,
              static_build_metrics_bytes: $build_metrics_bytes,
              binaries: [
                {path: "x86_64-unknown-linux-musl/debug/conary", sha256: $conary_sha256, bytes: $conary_bytes},
                {path: "x86_64-unknown-linux-musl/debug/conary-test", sha256: $harness_sha256, bytes: $harness_bytes},
                {path: "x86_64-unknown-linux-musl/debug/conary-test-library-tests", sha256: $tests_sha256, bytes: $tests_bytes}
              ]
            },
            measurements: {
              dependency_setup_ms: $setup_ms,
              compiler_cache_setup_ms: $cache_setup_ms,
              compiler_cache_save_ms: $cache_save_ms,
              artifact_build_ms: $build_ms,
              bundle_ms: $bundle_ms,
              static_dependency_cache_hit: $build_metrics[0].static_dependency_cache_hit,
              static_dependency_ms: $build_metrics[0].static_dependency_ms,
              static_runtime_build_ms: $build_metrics[0].static_runtime_build_ms,
              library_test_build_ms: $build_metrics[0].library_test_build_ms
            }
          }
        ' >"$manifest"
    jq -e . "$manifest" >/dev/null
    require_clean_checkout
    printf '%s\n' "$manifest"
}

verify_artifact() {
    [[ $# -eq 4 ]] || usage
    local artifact_dir="$1"
    local expected_commit="$2"
    local expected_run_id="$3"
    local expected_event="$4"
    require_commit "$expected_commit"
    require_uint workflow_run_id "$expected_run_id"
    [[ "$expected_event" == "pull_request" || "$expected_event" == "workflow_dispatch" ]] ||
        fail "unsupported workflow event"

    artifact_dir="$(realpath "$artifact_dir")"
    local manifest="$artifact_dir/$MANIFEST_NAME"
    local bundle="$artifact_dir/$BUNDLE_NAME"
    local stats="$artifact_dir/$STATS_NAME"
    local build_metrics="$artifact_dir/$BUILD_METRICS_NAME"
    require_regular_file "$manifest"
    require_regular_file "$bundle"
    require_regular_file "$stats"
    require_regular_file "$build_metrics"
    require_clean_checkout
    [[ "$(git rev-parse HEAD)" == "$expected_commit" ]] ||
        fail "consumer checkout does not match requested matrix source"

    local manifest_rustc_sha
    manifest_rustc_sha="$(jq -rj '.build.rustc_verbose' "$manifest" | sha256sum | cut -d ' ' -f 1)"
    jq -e \
        --arg commit "$expected_commit" \
        --arg tree "$(git rev-parse 'HEAD^{tree}')" \
        --arg lock "$(sha256_file Cargo.lock)" \
        --arg workspace_manifest "$(sha256_file Cargo.toml)" \
        --arg builder "$(sha256_file scripts/build-static-conary.sh)" \
        --arg header_probe "$(sha256_file scripts/kernel-header-roots.sh)" \
        --arg action "$(sha256_file .github/actions/build-static-conary/action.yml)" \
        --arg rust_toolchain "$(workspace_rust_version)" \
        --arg rustc_verbose_sha256 "$manifest_rustc_sha" \
        --arg repository "${GITHUB_REPOSITORY:-ConaryLabs/Conary}" \
        --arg event "$expected_event" \
        --argjson run_id "$expected_run_id" '
          .schema_version == 1
          and .source.commit_sha == $commit
          and .source.tree_sha == $tree
          and .source.clean == true
          and .source.cargo_lock_sha256 == $lock
          and .source.workspace_manifest_sha256 == $workspace_manifest
          and .source.builder_sha256 == $builder
          and .source.header_probe_sha256 == $header_probe
          and .source.action_sha256 == $action
          and .build.rust_toolchain == $rust_toolchain
          and .build.rustc_verbose_sha256 == $rustc_verbose_sha256
          and (.build.rustc_verbose | type == "string")
          and (.build.cargo_verbose | type == "string")
          and .build.target == "x86_64-unknown-linux-musl"
          and .build.profile == "dev-and-test"
          and .build.features == "default"
          and .build.command == "bash scripts/build-static-conary.sh --with-test-harness"
          and .build.rustflags == ""
          and .build.cargo_encoded_rustflags == ""
          and .build.cargo_incremental == "0"
          and .build.cargo_profile_dev_debug == "0"
          and .build.cargo_profile_test_debug == "0"
          and .build.static_libseccomp_version == "2.6.0"
          and .build.sccache_version == "0.16.0"
          and .build.sccache_cache_backend == "local-disk-bulk-v1"
          and (.build.sccache_cache_namespace | test("^native-matrix-musl-local-v1-[0-9a-f]{64}$"))
          and .provenance.repository == $repository
          and .provenance.workflow == "pr-gate"
          and .provenance.event == $event
          and .provenance.workflow_run_id == $run_id
          and .provenance.runner_os == "Linux"
          and .provenance.runner_arch == "X64"
          and .artifact.bundle == "native-matrix-artifacts.tar.gz"
          and .artifact.compiler_cache_stats == "sccache-stats.json"
          and .artifact.static_build_metrics == "static-build-metrics.json"
          and [.artifact.binaries[].path] == [
            "x86_64-unknown-linux-musl/debug/conary",
            "x86_64-unknown-linux-musl/debug/conary-test",
            "x86_64-unknown-linux-musl/debug/conary-test-library-tests"
          ]
          and (.artifact.binaries | length == 3)
          and (.artifact.binaries | all((.sha256 | test("^[0-9a-f]{64}$")) and (.bytes > 0)))
          and (.measurements.dependency_setup_ms | type == "number")
          and (.measurements.compiler_cache_setup_ms | type == "number")
          and (.measurements.compiler_cache_save_ms | type == "number")
          and (.measurements.artifact_build_ms | type == "number")
          and (.measurements.bundle_ms | type == "number")
          and (.measurements.static_dependency_cache_hit | type == "boolean")
          and (.measurements.static_dependency_ms | type == "number")
          and (.measurements.static_runtime_build_ms | type == "number")
          and (.measurements.library_test_build_ms | type == "number")
        ' "$manifest" >/dev/null || fail "matrix artifact manifest bindings are invalid"

    [[ "$(sha256_file "$bundle")" == "$(jq -r '.artifact.bundle_sha256' "$manifest")" ]] ||
        fail "matrix artifact bundle digest does not match its manifest"
    [[ "$(file_size "$bundle")" == "$(jq -r '.artifact.bundle_bytes' "$manifest")" ]] ||
        fail "matrix artifact bundle size does not match its manifest"
    [[ "$(sha256_file "$stats")" == "$(jq -r '.artifact.compiler_cache_stats_sha256' "$manifest")" ]] ||
        fail "matrix compiler-cache statistics digest does not match its manifest"
    [[ "$(file_size "$stats")" == "$(jq -r '.artifact.compiler_cache_stats_bytes' "$manifest")" ]] ||
        fail "matrix compiler-cache statistics size does not match its manifest"
    jq -e '
      .version == "0.16.0"
      and (.cache_location | startswith("Local disk: "))
      and (.stats.compile_requests | type == "number")
      and (.stats.cache_hits.counts | type == "object")
      and (.stats.cache_misses.counts | type == "object")
      and (.stats.cache_errors.counts | type == "object")
      and (.stats.cache_writes | type == "number")
      and (.stats.cache_read_errors | type == "number")
      and (.stats.cache_write_errors | type == "number")
      and (.stats.cache_timeouts | type == "number")
    ' "$stats" >/dev/null ||
        fail "matrix compiler-cache statistics do not prove the protected local backend"
    [[ "$(sha256_file "$build_metrics")" == "$(jq -r '.artifact.static_build_metrics_sha256' "$manifest")" ]] ||
        fail "matrix static-build metrics digest does not match its manifest"
    [[ "$(file_size "$build_metrics")" == "$(jq -r '.artifact.static_build_metrics_bytes' "$manifest")" ]] ||
        fail "matrix static-build metrics size does not match its manifest"
    jq -e --slurpfile metrics "$build_metrics" '
      .measurements.static_dependency_cache_hit == $metrics[0].static_dependency_cache_hit
      and .measurements.static_dependency_ms == $metrics[0].static_dependency_ms
      and .measurements.static_runtime_build_ms == $metrics[0].static_runtime_build_ms
      and .measurements.library_test_build_ms == $metrics[0].library_test_build_ms
    ' "$manifest" >/dev/null || fail "matrix static-build measurements do not match their source"

    local expected_listing actual_listing
    expected_listing="$(printf '%s\n' "${ARTIFACT_PATHS[@]}" | LC_ALL=C sort)"
    actual_listing="$(tar -tzf "$bundle" | LC_ALL=C sort)"
    [[ "$actual_listing" == "$expected_listing" ]] ||
        fail "matrix artifact bundle member list is invalid"

    local unpacked
    unpacked="$(mktemp -d)"
    trap 'rm -rf "$unpacked"' RETURN
    tar --extract --gzip --no-same-owner --no-same-permissions -f "$bundle" -C "$unpacked"
    local index path expected_sha expected_bytes
    for index in "${!ARTIFACT_PATHS[@]}"; do
        path="$unpacked/${ARTIFACT_PATHS[$index]}"
        require_static_executable "$path"
        expected_sha="$(jq -r ".artifact.binaries[$index].sha256" "$manifest")"
        expected_bytes="$(jq -r ".artifact.binaries[$index].bytes" "$manifest")"
        [[ "$(sha256_file "$path")" == "$expected_sha" ]] ||
            fail "matrix binary digest does not match its manifest: ${ARTIFACT_PATHS[$index]}"
        [[ "$(file_size "$path")" == "$expected_bytes" ]] ||
            fail "matrix binary size does not match its manifest: ${ARTIFACT_PATHS[$index]}"
    done

    local repo_root target_dir destination
    repo_root="$(git rev-parse --show-toplevel)"
    target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
    destination="$target_dir/$TARGET/$PROFILE"
    mkdir -p "$destination"
    for index in "${!ARTIFACT_PATHS[@]}"; do
        install -m 0755 "$unpacked/${ARTIFACT_PATHS[$index]}" \
            "$destination/$(basename "${ARTIFACT_PATHS[$index]}")"
    done
    "$destination/conary" --version >/dev/null
    "$destination/conary-test" --version >/dev/null
    "$destination/conary-test-library-tests" --list >/dev/null

    jq -n \
        --arg commit_sha "$expected_commit" \
        --arg bundle_sha256 "$(sha256_file "$bundle")" \
        --argjson workflow_run_id "$expected_run_id" \
        --slurpfile manifest "$manifest" '
          {
            schema_version: 1,
            commit_sha: $commit_sha,
            workflow_run_id: $workflow_run_id,
            bundle_sha256: $bundle_sha256,
            measurements: $manifest[0].measurements
          }
        '
}

[[ $# -ge 1 ]] || usage
command="$1"
shift
case "$command" in
    package) package_artifact "$@" ;;
    verify) verify_artifact "$@" ;;
    *) usage ;;
esac
