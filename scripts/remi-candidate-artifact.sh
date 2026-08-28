#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
Usage:
  scripts/remi-candidate-artifact.sh package BINARY OUTPUT_DIR VERSION COMMIT RUN_ID SETUP_MS CACHE_SETUP_MS CACHE_SAVE_MS BUILD_MS LINK_TIMINGS RUSTC_TIMINGS SCCACHE_STATS
  scripts/remi-candidate-artifact.sh verify ARTIFACT_DIR COMMIT RUN_ID EVENT
EOF
    exit 2
}

fail() {
    echo "remi-candidate-artifact: $*" >&2
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

sha256_file() {
    sha256sum "$1" | cut -d ' ' -f 1
}

file_size() {
    stat --printf='%s' "$1"
}

workspace_rust_version() {
    sed -n 's/^rust-version = "\([^"]\+\)"$/\1/p' Cargo.toml
}

workspace_version() {
    sed -n 's/^version = "\([^"]\+\)"$/\1/p' Cargo.toml
}

require_regular_file() {
    local path="$1"
    [[ -f "$path" && ! -L "$path" ]] || fail "expected a regular non-symlink file: $path"
}

require_clean_checkout() {
    local dirty
    dirty="$(git status --porcelain --untracked-files=all)"
    [[ -z "$dirty" ]] || fail "candidate checkout must be clean before packaging"
}

package_artifact() {
    [[ $# -eq 12 ]] || usage
    local binary="$1"
    local output_dir="$2"
    local version="$3"
    local expected_commit="$4"
    local workflow_run_id="$5"
    local setup_ms="$6"
    local cache_setup_ms="$7"
    local cache_save_ms="$8"
    local build_ms="$9"
    local link_timings="${10}"
    local rustc_timings="${11}"
    local sccache_stats="${12}"

    require_commit "$expected_commit"
    require_uint workflow_run_id "$workflow_run_id"
    require_uint setup_ms "$setup_ms"
    require_uint cache_setup_ms "$cache_setup_ms"
    require_uint cache_save_ms "$cache_save_ms"
    require_uint build_ms "$build_ms"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]] ||
        fail "version is not safe for an artifact filename"
    require_regular_file "$binary"
    require_regular_file "$link_timings"
    require_regular_file "$rustc_timings"
    require_regular_file "$sccache_stats"
    jq -e 'type == "object"' "$sccache_stats" >/dev/null ||
        fail "sccache statistics must be a JSON object"
    require_clean_checkout

    local actual_commit tree_sha lock_sha workspace_manifest_sha
    actual_commit="$(git rev-parse HEAD)"
    [[ "$actual_commit" == "$expected_commit" ]] ||
        fail "checkout commit does not match requested candidate"
    tree_sha="$(git rev-parse 'HEAD^{tree}')"
    lock_sha="$(sha256_file Cargo.lock)"
    workspace_manifest_sha="$(sha256_file Cargo.toml)"

    local rust_toolchain rustc_verbose rustc_verbose_sha cargo_verbose target_triple
    rust_toolchain="$(workspace_rust_version)"
    [[ -n "$rust_toolchain" ]] || fail "workspace rust-version is missing"
    rustc_verbose="$(rustc -vV)"
    rustc_verbose_sha="$(printf '%s' "$rustc_verbose" | sha256sum | cut -d ' ' -f 1)"
    cargo_verbose="$(cargo -vV)"
    target_triple="$(sed -n 's/^host: //p' <<<"$rustc_verbose")"
    [[ -n "$target_triple" ]] || fail "rustc did not report its host target"
    [[ "$(sed -n 's/^rustc \([^ ]\+\).*/\1/p' <<<"$rustc_verbose")" == "$rust_toolchain" ]] ||
        fail "active rustc does not match the workspace rust-version"

    local artifact_name bundle_name artifact_path bundle_path manifest_path
    local retained_link_timings retained_rustc_timings retained_sccache_stats
    artifact_name="remi-${version}-linux-x64"
    bundle_name="${artifact_name}.tar.gz"
    mkdir -p "$output_dir"
    output_dir="$(realpath "$output_dir")"
    artifact_path="${output_dir}/${artifact_name}"
    bundle_path="${output_dir}/${bundle_name}"
    manifest_path="${output_dir}/remi-candidate-manifest.json"
    retained_link_timings="${output_dir}/link-timings.tsv"
    retained_rustc_timings="${output_dir}/rustc-timings.tsv"
    retained_sccache_stats="${output_dir}/sccache-stats.json"
    [[ ! -e "$artifact_path" && ! -e "$bundle_path" && ! -e "$manifest_path" \
        && ! -e "$retained_link_timings" && ! -e "$retained_rustc_timings" \
        && ! -e "$retained_sccache_stats" ]] ||
        fail "output directory already contains candidate artifact files"

    install -m 0755 "$binary" "$artifact_path"
    install -m 0644 "$link_timings" "$retained_link_timings"
    install -m 0644 "$rustc_timings" "$retained_rustc_timings"
    install -m 0644 "$sccache_stats" "$retained_sccache_stats"
    local bundle_started_ns bundle_finished_ns bundle_ms
    bundle_started_ns="$(date -u +%s%N)"
    tar --create --format=gnu --sort=name --mtime='UTC 1970-01-01' \
        --owner=0 --group=0 --numeric-owner -C "$output_dir" "$artifact_name" |
        gzip --no-name >"$bundle_path"
    bundle_finished_ns="$(date -u +%s%N)"
    bundle_ms=$(( (bundle_finished_ns - bundle_started_ns) / 1000000 ))

    local binary_sha bundle_sha binary_bytes bundle_bytes
    binary_sha="$(sha256_file "$artifact_path")"
    bundle_sha="$(sha256_file "$bundle_path")"
    binary_bytes="$(file_size "$artifact_path")"
    bundle_bytes="$(file_size "$bundle_path")"

    local link_invocations link_ms successful_link_ms remi_link_ms
    link_invocations="$(awk -F '\t' 'NF == 3 { count += 1 } END { print count + 0 }' "$link_timings")"
    link_ms="$(awk -F '\t' 'NF == 3 { total += $1 } END { print total + 0 }' "$link_timings")"
    successful_link_ms="$(awk -F '\t' 'NF == 3 && $2 == 0 { total += $1 } END { print total + 0 }' "$link_timings")"
    remi_link_ms="$(awk -F '\t' 'NF == 3 && $2 == 0 && $3 ~ /^remi-[0-9a-f]+$/ { if ($1 > max) max = $1 } END { print max + 0 }' "$link_timings")"

    awk -F '\t' '
        NF != 4 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $3 == "" || $4 == "" {
            exit 1
        }
    ' "$rustc_timings" || fail "Rust compiler timings are malformed"
    local rustc_invocations rustc_ms successful_rustc_ms slowest_rustc
    local slowest_rustc_ms slowest_rustc_status slowest_rustc_crate slowest_rustc_type
    rustc_invocations="$(awk -F '\t' 'NF == 4 { count += 1 } END { print count + 0 }' "$rustc_timings")"
    (( rustc_invocations > 0 )) || fail "Rust compiler timings contain no invocations"
    rustc_ms="$(awk -F '\t' 'NF == 4 { total += $1 } END { print total + 0 }' "$rustc_timings")"
    successful_rustc_ms="$(awk -F '\t' 'NF == 4 && $2 == 0 { total += $1 } END { print total + 0 }' "$rustc_timings")"
    slowest_rustc="$(awk -F '\t' '
        NF == 4 && (!seen || $1 > max) {
            seen = 1
            max = $1
            status = $2
            crate = $3
            crate_type = $4
        }
        END {
            if (seen) printf "%s\t%s\t%s\t%s\n", max, status, crate, crate_type
        }
    ' "$rustc_timings")"
    IFS=$'\t' read -r slowest_rustc_ms slowest_rustc_status \
        slowest_rustc_crate slowest_rustc_type <<<"$slowest_rustc"
    require_uint slowest_rustc_ms "$slowest_rustc_ms"
    require_uint slowest_rustc_status "$slowest_rustc_status"

    local linker_path linker_sha rustc_wrapper_path rustc_wrapper_sha
    linker_path="$(realpath scripts/timed-linker.sh)"
    linker_sha="$(sha256_file "$linker_path")"
    rustc_wrapper_path="$(realpath scripts/timed-rustc-wrapper.sh)"
    rustc_wrapper_sha="$(sha256_file "$rustc_wrapper_path")"
    local link_timings_sha link_timings_bytes rustc_timings_sha rustc_timings_bytes
    local sccache_stats_sha sccache_stats_bytes
    link_timings_sha="$(sha256_file "$retained_link_timings")"
    link_timings_bytes="$(file_size "$retained_link_timings")"
    rustc_timings_sha="$(sha256_file "$retained_rustc_timings")"
    rustc_timings_bytes="$(file_size "$retained_rustc_timings")"
    sccache_stats_sha="$(sha256_file "$retained_sccache_stats")"
    sccache_stats_bytes="$(file_size "$retained_sccache_stats")"

    jq -n \
        --arg commit_sha "$actual_commit" \
        --arg tree_sha "$tree_sha" \
        --arg lock_sha256 "$lock_sha" \
        --arg workspace_manifest_sha256 "$workspace_manifest_sha" \
        --arg rust_toolchain "$rust_toolchain" \
        --arg rustc_verbose "$rustc_verbose" \
        --arg rustc_verbose_sha256 "$rustc_verbose_sha" \
        --arg cargo_verbose "$cargo_verbose" \
        --arg target "$target_triple" \
        --arg linker_sha256 "$linker_sha" \
        --arg rustc_wrapper_sha256 "$rustc_wrapper_sha" \
        --arg repository "${GITHUB_REPOSITORY:-unknown}" \
        --arg workflow "${GITHUB_WORKFLOW:-unknown}" \
        --arg event "${GITHUB_EVENT_NAME:-unknown}" \
        --arg runner_os "${RUNNER_OS:-unknown}" \
        --arg runner_arch "${RUNNER_ARCH:-unknown}" \
        --arg runner_image_os "${ImageOS:-unknown}" \
        --arg runner_image_version "${ImageVersion:-unknown}" \
        --arg version "$version" \
        --arg artifact_name "$artifact_name" \
        --arg artifact_sha256 "$binary_sha" \
        --arg bundle_name "$bundle_name" \
        --arg bundle_sha256 "$bundle_sha" \
        --arg rustflags "${RUSTFLAGS:-}" \
        --arg encoded_rustflags "${CARGO_ENCODED_RUSTFLAGS:-}" \
        --arg cargo_incremental "${CARGO_INCREMENTAL:-}" \
        --arg cargo_profile_dev_debug "${CARGO_PROFILE_DEV_DEBUG:-}" \
        --arg cargo_profile_test_debug "${CARGO_PROFILE_TEST_DEBUG:-}" \
        --arg sccache_version "${SCCACHE_VERSION:-unknown}" \
        --arg compiler_cache_backend "${SCCACHE_CACHE_BACKEND:-unknown}" \
        --arg compiler_cache_namespace "${CONARY_COMPILER_CACHE_NAMESPACE:-unknown}" \
        --arg conary_git_commit "${CONARY_GIT_COMMIT:-}" \
        --arg conary_git_dirty "${CONARY_GIT_DIRTY:-}" \
        --arg link_timings_sha256 "$link_timings_sha" \
        --arg rustc_timings_sha256 "$rustc_timings_sha" \
        --arg sccache_stats_sha256 "$sccache_stats_sha" \
        --argjson workflow_run_id "$workflow_run_id" \
        --argjson setup_ms "$setup_ms" \
        --argjson cache_setup_ms "$cache_setup_ms" \
        --argjson cache_save_ms "$cache_save_ms" \
        --argjson build_ms "$build_ms" \
        --argjson bundle_ms "$bundle_ms" \
        --argjson link_invocations "$link_invocations" \
        --argjson link_ms "$link_ms" \
        --argjson successful_link_ms "$successful_link_ms" \
        --argjson remi_link_ms "$remi_link_ms" \
        --argjson rustc_invocations "$rustc_invocations" \
        --argjson rustc_ms "$rustc_ms" \
        --argjson successful_rustc_ms "$successful_rustc_ms" \
        --argjson slowest_rustc_ms "$slowest_rustc_ms" \
        --argjson slowest_rustc_status "$slowest_rustc_status" \
        --arg slowest_rustc_crate "$slowest_rustc_crate" \
        --arg slowest_rustc_type "$slowest_rustc_type" \
        --argjson artifact_bytes "$binary_bytes" \
        --argjson bundle_bytes "$bundle_bytes" '
          {
            schema_version: 2,
            source: {
              commit_sha: $commit_sha,
              tree_sha: $tree_sha,
              clean: true,
              cargo_lock_sha256: $lock_sha256,
              workspace_manifest_sha256: $workspace_manifest_sha256
            },
            build: {
              package: "remi",
              version: $version,
              rust_toolchain: $rust_toolchain,
              rustc_verbose: $rustc_verbose,
              rustc_verbose_sha256: $rustc_verbose_sha256,
              cargo_verbose: $cargo_verbose,
              target: $target,
              profile: "release",
              features: "default",
              command: "cargo build -p remi --release --locked",
              linker: "scripts/timed-linker.sh",
              linker_sha256: $linker_sha256,
              compiler_timing_wrapper: "scripts/timed-rustc-wrapper.sh",
              compiler_timing_wrapper_sha256: $rustc_wrapper_sha256,
              rustflags: $rustflags,
              cargo_encoded_rustflags: $encoded_rustflags,
              cargo_incremental: $cargo_incremental,
              cargo_profile_dev_debug: $cargo_profile_dev_debug,
              cargo_profile_test_debug: $cargo_profile_test_debug,
              sccache_version: $sccache_version,
              conary_git_commit: $conary_git_commit,
              conary_git_dirty: $conary_git_dirty
            },
            compiler_cache: {
              backend: $compiler_cache_backend,
              namespace: $compiler_cache_namespace
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
              binary: $artifact_name,
              binary_sha256: $artifact_sha256,
              binary_bytes: $artifact_bytes,
              bundle: $bundle_name,
              bundle_sha256: $bundle_sha256,
              bundle_bytes: $bundle_bytes,
              link_timings: "link-timings.tsv",
              link_timings_sha256: $link_timings_sha256,
              link_timings_bytes: $link_timings_bytes,
              compiler_timings: "rustc-timings.tsv",
              compiler_timings_sha256: $rustc_timings_sha256,
              compiler_timings_bytes: $rustc_timings_bytes,
              compiler_cache_stats: "sccache-stats.json",
              compiler_cache_stats_sha256: $sccache_stats_sha256,
              compiler_cache_stats_bytes: $sccache_stats_bytes
            },
            compiler_timing: {
              slowest_ms: $slowest_rustc_ms,
              slowest_status: $slowest_rustc_status,
              slowest_crate: $slowest_rustc_crate,
              slowest_crate_type: $slowest_rustc_type
            },
            measurements: {
              dependency_setup_ms: $setup_ms,
              compiler_cache_setup_ms: $cache_setup_ms,
              compiler_cache_save_ms: $cache_save_ms,
              cargo_build_ms: $build_ms,
              rustc_invocations: $rustc_invocations,
              rustc_ms_total: $rustc_ms,
              successful_rustc_ms_total: $successful_rustc_ms,
              bundle_ms: $bundle_ms,
              linker_invocations: $link_invocations,
              linker_ms_total: $link_ms,
              successful_link_ms_total: $successful_link_ms,
              remi_final_link_ms: $remi_link_ms
            }
          }
        ' \
        --argjson link_timings_bytes "$link_timings_bytes" \
        --argjson rustc_timings_bytes "$rustc_timings_bytes" \
        --argjson sccache_stats_bytes "$sccache_stats_bytes" \
        >"$manifest_path"

    jq -e . "$manifest_path" >/dev/null
    require_clean_checkout
    printf '%s\n' "$manifest_path"
}

verify_artifact() {
    [[ $# -eq 4 ]] || usage
    local artifact_dir="$1"
    local expected_commit="$2"
    local expected_run_id="$3"
    local expected_event="$4"
    require_commit "$expected_commit"
    require_uint workflow_run_id "$expected_run_id"
    [[ "$expected_event" == "push" || "$expected_event" == "workflow_dispatch" ]] ||
        fail "unsupported workflow event"

    artifact_dir="$(realpath "$artifact_dir")"
    local manifest="${artifact_dir}/remi-candidate-manifest.json"
    require_regular_file "$manifest"
    local expected_version manifest_rustc_sha manifest_target
    expected_version="$(workspace_version)"
    [[ -n "$expected_version" ]] || fail "workspace version is missing"
    manifest_rustc_sha="$(
        jq -rj '.build.rustc_verbose' "$manifest" | sha256sum | cut -d ' ' -f 1
    )"
    manifest_target="$(
        jq -r '.build.rustc_verbose' "$manifest" | sed -n 's/^host: //p'
    )"
    [[ -n "$manifest_target" ]] || fail "candidate manifest rustc target is missing"
    jq -e \
        --arg commit "$expected_commit" \
        --arg tree "$(git rev-parse 'HEAD^{tree}')" \
        --arg lock "$(sha256_file Cargo.lock)" \
        --arg workspace_manifest "$(sha256_file Cargo.toml)" \
        --arg version "$expected_version" \
        --arg rust_toolchain "$(workspace_rust_version)" \
        --arg rustc_verbose_sha256 "$manifest_rustc_sha" \
        --arg target "$manifest_target" \
        --arg repository "${GITHUB_REPOSITORY:-ConaryLabs/Conary}" \
        --arg event "$expected_event" \
        --argjson run_id "$expected_run_id" '
          .schema_version == 2
          and .source.commit_sha == $commit
          and .source.tree_sha == $tree
          and .source.clean == true
          and .source.cargo_lock_sha256 == $lock
          and .source.workspace_manifest_sha256 == $workspace_manifest
          and .build.package == "remi"
          and .build.version == $version
          and .build.rust_toolchain == $rust_toolchain
          and .build.profile == "release"
          and .build.features == "default"
          and .build.command == "cargo build -p remi --release --locked"
          and .build.linker == "scripts/timed-linker.sh"
          and .build.linker_sha256 == $linker_sha256
          and .build.compiler_timing_wrapper == "scripts/timed-rustc-wrapper.sh"
          and .build.compiler_timing_wrapper_sha256 == $rustc_wrapper_sha256
          and .build.rustflags == ""
          and .build.cargo_encoded_rustflags == ""
          and .build.cargo_incremental == "0"
          and .build.cargo_profile_dev_debug == "0"
          and .build.cargo_profile_test_debug == "0"
          and .build.sccache_version == "0.16.0"
          and .compiler_cache.backend == "local-disk-bulk-v1"
          and (.compiler_cache.namespace | test("^remi-release-local-v1-[0-9a-f]{64}$"))
          and .provenance.repository == $repository
          and .provenance.workflow == "build-remi-candidate"
          and .provenance.event == $event
          and .provenance.workflow_run_id == $run_id
          and .provenance.runner_os == "Linux"
          and .provenance.runner_arch == "X64"
          and (.build.version | type == "string")
          and (.build.rustc_verbose | type == "string")
          and .build.rustc_verbose_sha256 == $rustc_verbose_sha256
          and (.build.cargo_verbose | type == "string")
          and .build.target == $target
          and .build.conary_git_commit == $commit
          and .build.conary_git_dirty == "false"
          and .artifact.binary == ("remi-" + $version + "-linux-x64")
          and (.artifact.binary_sha256 | test("^[0-9a-f]{64}$"))
          and (.artifact.binary_bytes | type == "number" and . > 0)
          and .artifact.bundle == (.artifact.binary + ".tar.gz")
          and (.artifact.bundle_sha256 | test("^[0-9a-f]{64}$"))
          and (.artifact.bundle_bytes | type == "number" and . > 0)
          and .artifact.link_timings == "link-timings.tsv"
          and (.artifact.link_timings_sha256 | test("^[0-9a-f]{64}$"))
          and (.artifact.link_timings_bytes | type == "number" and . > 0)
          and .artifact.compiler_timings == "rustc-timings.tsv"
          and (.artifact.compiler_timings_sha256 | test("^[0-9a-f]{64}$"))
          and (.artifact.compiler_timings_bytes | type == "number" and . > 0)
          and .artifact.compiler_cache_stats == "sccache-stats.json"
          and (.artifact.compiler_cache_stats_sha256 | test("^[0-9a-f]{64}$"))
          and (.artifact.compiler_cache_stats_bytes | type == "number" and . > 0)
          and (.compiler_timing.slowest_ms | type == "number" and . >= 0)
          and (.compiler_timing.slowest_status | type == "number" and . >= 0)
          and (.compiler_timing.slowest_crate | type == "string" and length > 0)
          and (.compiler_timing.slowest_crate_type | type == "string" and length > 0)
          and (.measurements | type == "object")
          and all(.measurements[]; type == "number" and . >= 0)
          and .measurements.rustc_invocations > 0
          and .compiler_timing.slowest_ms <= .measurements.rustc_ms_total
        ' --arg linker_sha256 "$(sha256_file scripts/timed-linker.sh)" \
        --arg rustc_wrapper_sha256 "$(sha256_file scripts/timed-rustc-wrapper.sh)" \
        "$manifest" >/dev/null || fail "candidate manifest bindings are invalid"

    local binary_name bundle_name binary bundle link_timings rustc_timings sccache_stats
    binary_name="$(jq -r '.artifact.binary' "$manifest")"
    bundle_name="$(jq -r '.artifact.bundle' "$manifest")"
    binary="${artifact_dir}/${binary_name}"
    bundle="${artifact_dir}/${bundle_name}"
    link_timings="${artifact_dir}/link-timings.tsv"
    rustc_timings="${artifact_dir}/rustc-timings.tsv"
    sccache_stats="${artifact_dir}/sccache-stats.json"
    require_regular_file "$bundle"
    require_regular_file "$link_timings"
    require_regular_file "$rustc_timings"
    require_regular_file "$sccache_stats"

    if [[ -e "$binary" || -L "$binary" ]]; then
        require_regular_file "$binary"
        [[ "$(sha256_file "$binary")" == "$(jq -r '.artifact.binary_sha256' "$manifest")" ]] ||
            fail "candidate binary digest does not match its manifest"
        [[ "$(file_size "$binary")" == "$(jq -r '.artifact.binary_bytes' "$manifest")" ]] ||
            fail "candidate binary size does not match its manifest"
    fi
    [[ "$(sha256_file "$bundle")" == "$(jq -r '.artifact.bundle_sha256' "$manifest")" ]] ||
        fail "candidate bundle digest does not match its manifest"
    [[ "$(file_size "$bundle")" == "$(jq -r '.artifact.bundle_bytes' "$manifest")" ]] ||
        fail "candidate bundle size does not match its manifest"
    [[ "$(sha256_file "$link_timings")" == "$(jq -r '.artifact.link_timings_sha256' "$manifest")" ]] ||
        fail "link timing evidence digest does not match its manifest"
    [[ "$(file_size "$link_timings")" == "$(jq -r '.artifact.link_timings_bytes' "$manifest")" ]] ||
        fail "link timing evidence size does not match its manifest"
    [[ "$(sha256_file "$rustc_timings")" == "$(jq -r '.artifact.compiler_timings_sha256' "$manifest")" ]] ||
        fail "compiler timing evidence digest does not match its manifest"
    [[ "$(file_size "$rustc_timings")" == "$(jq -r '.artifact.compiler_timings_bytes' "$manifest")" ]] ||
        fail "compiler timing evidence size does not match its manifest"
    awk -F '\t' '
        NF != 4 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $3 == "" || $4 == "" {
            exit 1
        }
    ' "$rustc_timings" || fail "compiler timing evidence is malformed"
    local observed_rustc_invocations observed_rustc_ms observed_successful_rustc_ms
    local observed_slowest_rustc
    observed_rustc_invocations="$(awk -F '\t' 'NF == 4 { count += 1 } END { print count + 0 }' "$rustc_timings")"
    observed_rustc_ms="$(awk -F '\t' 'NF == 4 { total += $1 } END { print total + 0 }' "$rustc_timings")"
    observed_successful_rustc_ms="$(awk -F '\t' 'NF == 4 && $2 == 0 { total += $1 } END { print total + 0 }' "$rustc_timings")"
    observed_slowest_rustc="$(awk -F '\t' '
        NF == 4 && (!seen || $1 > max) {
            seen = 1
            max = $1
            status = $2
            crate = $3
            crate_type = $4
        }
        END {
            if (seen) printf "%s\t%s\t%s\t%s", max, status, crate, crate_type
        }
    ' "$rustc_timings")"
    [[ "$observed_rustc_invocations" == "$(jq -r '.measurements.rustc_invocations' "$manifest")" \
        && "$observed_rustc_ms" == "$(jq -r '.measurements.rustc_ms_total' "$manifest")" \
        && "$observed_successful_rustc_ms" == "$(jq -r '.measurements.successful_rustc_ms_total' "$manifest")" \
        && "$observed_slowest_rustc" == "$(jq -r '[.compiler_timing.slowest_ms, .compiler_timing.slowest_status, .compiler_timing.slowest_crate, .compiler_timing.slowest_crate_type] | @tsv' "$manifest")" ]] ||
        fail "compiler timing measurements do not match their retained evidence"
    [[ "$(sha256_file "$sccache_stats")" == "$(jq -r '.artifact.compiler_cache_stats_sha256' "$manifest")" ]] ||
        fail "compiler-cache evidence digest does not match its manifest"
    [[ "$(file_size "$sccache_stats")" == "$(jq -r '.artifact.compiler_cache_stats_bytes' "$manifest")" ]] ||
        fail "compiler-cache evidence size does not match its manifest"
    jq -e 'type == "object"' "$sccache_stats" >/dev/null ||
        fail "compiler-cache evidence is not a JSON object"

    local listing
    listing="$(tar -tzf "$bundle")"
    [[ "$listing" == "$binary_name" ]] || fail "candidate bundle has unexpected members"
    local bundled_binary_sha
    bundled_binary_sha="$(tar -xOzf "$bundle" "$binary_name" | sha256sum | cut -d ' ' -f 1)"
    [[ "$bundled_binary_sha" == "$(jq -r '.artifact.binary_sha256' "$manifest")" ]] ||
        fail "candidate bundle payload does not match the standalone binary"
    local bundled_binary_bytes
    bundled_binary_bytes="$(tar -xOzf "$bundle" "$binary_name" | wc -c)"
    [[ "$bundled_binary_bytes" == "$(jq -r '.artifact.binary_bytes' "$manifest")" ]] ||
        fail "candidate bundle payload size does not match its manifest"

    jq -n \
        --arg version "$(jq -r '.build.version' "$manifest")" \
        --arg bundle "$bundle" \
        --arg binary_sha256 "$(jq -r '.artifact.binary_sha256' "$manifest")" \
        --arg bundle_sha256 "$(jq -r '.artifact.bundle_sha256' "$manifest")" \
        --arg manifest "$manifest" '{
          schema_version: 2,
          version: $version,
          bundle: $bundle,
          binary_sha256: $binary_sha256,
          bundle_sha256: $bundle_sha256,
          manifest: $manifest
        }'
}

[[ $# -ge 1 ]] || usage
command="$1"
shift
case "$command" in
    package) package_artifact "$@" ;;
    verify) verify_artifact "$@" ;;
    *) usage ;;
esac
