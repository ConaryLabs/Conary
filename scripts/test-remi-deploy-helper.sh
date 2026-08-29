#!/usr/bin/env bash
# scripts/test-remi-deploy-helper.sh -- Exercise the Remi deploy helper in a fake root.
set -euo pipefail

helper="${1:-deploy/remi-deploy-helper.sh}"
test -f "$helper" || {
    echo "missing helper: $helper" >&2
    exit 1
}

tmpdir="$(mktemp -d /tmp/remi-deploy-helper-test.XXXXXX)"
benchmark_tmp_paths=()
cleanup() {
    local path
    for path in "${benchmark_tmp_paths[@]}"; do
        rm -f -- "$path"
    done
    rm -rf "$tmpdir"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

write_config() {
    local fake_root="$1"
    mkdir -p "$fake_root/etc/conary" "$fake_root/conary"
    chmod 0750 "$fake_root/conary"
    cat >"$fake_root/etc/conary/remi.toml" <<'TOML'
[server]
bind = "127.0.0.1:8080"

[conversion]
max_concurrent = 4

[r2]
enabled = false
TOML
}

make_release_staging() {
    local staging="$1"
    local include_sig="${2:-yes}"

    mkdir -p "$staging"
    printf 'ccs\n' >"$staging/conary-0.8.0.ccs"
    if [[ "$include_sig" == "yes" ]]; then
        printf 'sig\n' >"$staging/conary-0.8.0.ccs.sig"
    fi
    printf 'notes\n' >"$staging/metadata.json"
    (
        cd "$staging"
        sha256sum -- * > SHA256SUMS.tmp
        mv SHA256SUMS.tmp SHA256SUMS
    )
}

make_site_staging() {
    local staging="$1"

    mkdir -p "$staging/assets"
    printf '<!doctype html><title>Conary</title>\n' >"$staging/index.html"
    printf 'console.log("ok");\n' >"$staging/assets/app.js"
}

make_fake_remi_bundle() {
    local bundle="$1"
    local version="$2"
    local build_dir="${tmpdir}/fake-remi-${version}"
    local candidate="${build_dir}/remi-${version}-linux-x64"

    mkdir -p "$build_dir"
    cat >"$candidate" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ "\${1:-}" == "--version" ]]; then
    echo "remi ${version}"
    exit 0
fi
if [[ "\${1:-}" == "deployment" && "\${2:-}" == "prepare" ]]; then
    shift 2
    config=""
    repository_keys_dir=""
    while [[ \$# -gt 0 ]]; do
        case "\$1" in
            --config)
                config="\$2"
                shift 2
                ;;
            --repository-keys-dir)
                repository_keys_dir="\$2"
                shift 2
                ;;
            *)
                shift
                [[ \$# -gt 0 ]] && shift
                ;;
        esac
    done
    [[ -d "\$repository_keys_dir" && ! -L "\$repository_keys_dir" ]]
    runtime_root="\${config%/etc/conary/remi.toml}/conary"
    runtime_lock="\${runtime_root}/.remi-runtime.lock"
    [[ -f "\$runtime_lock" && ! -L "\$runtime_lock" ]]
    [[ "\$(stat -c '%a' "\$runtime_lock")" == "600" ]]
    exec 9<>"\$runtime_lock"
    transition="\${config}.transition.json"
    printf '{}\n' >"\$transition"
    printf '%s\n' "\$repository_keys_dir" >"\${config}.repository-keys-path"
    echo "\$transition"
    exit 0
fi
if [[ "\${1:-}" == "deployment" && "\${2:-}" == "rollback" ]]; then
    exit 0
fi
if [[ "\${1:-}" == "deployment" && "\${2:-}" == "baseline" ]]; then
    shift 2
    config=""
    while [[ \$# -gt 0 ]]; do
        case "\$1" in
            --config)
                config="\$2"
                shift 2
                ;;
            *)
                exit 2
                ;;
        esac
    done
    printf '{"baseline_schema_version":1,"config":"%s","owner":"candidate"}\n' "\$config"
    exit 0
fi
if [[ "\${1:-}" == "deployment" && "\${2:-}" == "inspect" ]]; then
    shift 2
    config=""
    args=("\$@")
    while [[ \$# -gt 0 ]]; do
        case "\$1" in
            --config)
                config="\$2"
                shift 2
                ;;
            *)
                shift
                ;;
        esac
    done
    printf '%s\n' "\${args[@]}" >"\${config}.inspect-args"
    if [[ "\${CONARY_FAKE_INSPECT_DIAGNOSTIC:-0}" == "1" ]]; then
        printf 'INFO immutable catalog reopen completed\n' >&2
        printf '%s\n' '{"schema_epoch":"test-v1","schema_revision":1,"configured_profiles":3,"populated_profiles":0,"candidate_profiles":3,"profiles":[],"candidates":[]}'
    fi
    exit 0
fi
if [[ "\${1:-}" == "native-oracle-input" ]]; then
    shift
    output_dir=""
    args=("\$@")
    while [[ \$# -gt 0 ]]; do
        case "\$1" in
            --output-dir)
                output_dir="\$2"
                shift 2
                ;;
            *)
                shift
                [[ \$# -gt 0 ]] && shift
                ;;
        esac
    done
    [[ -n "\$output_dir" && ! -e "\$output_dir" ]]
    mkdir "\$output_dir"
    printf '%s\n' "\${args[@]}" >"\${output_dir}/command-args"
    exit 0
fi
exit 2
EOF
    chmod 0755 "$candidate"
    tar czf "$bundle" -C "$build_dir" "$(basename "$candidate")"
}

make_fake_benchmark_remi() {
    local fake_root="$1"
    local bin="${fake_root}/usr/local/bin/remi"
    mkdir -p "$(dirname "$bin")"
    cat >"$bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "conversion-benchmark" && $# -eq 17 ]]
printf '%s\n' "$@" >"$CONARY_FAKE_BENCHMARK_ARGS"
[[ "$(cat "$CONARY_FAKE_SERVICE_STATE")" == "stopped" ]]
if [[ "${CONARY_FAKE_BENCHMARK_FAIL:-0}" == "1" ]]; then
    exit 41
fi
shift
work_root=""
while (( $# > 0 )); do
    case "$1" in
        --config|--profile|--revision|--package-key|--source-artifact|--hardware-label|--iterations)
            shift 2
            ;;
        --work-root)
            work_root="$2"
            shift 2
            ;;
        *) exit 2 ;;
    esac
done
[[ -n "$work_root" && ! -e "$work_root" ]]
mkdir -m 0700 "$work_root"
raw="${work_root}/conversion-benchmark-v3.json"
public="${work_root}/conversion-benchmark-public-v1.json"
printf '%s\n' '{"schema_version":3}' >"$raw"
chmod "${CONARY_FAKE_RAW_REPORT_MODE:-0600}" "$raw"
raw_sha256="$(sha256sum "$raw" | cut -d ' ' -f 1)"
raw_bytes="$(stat -c '%s' "$raw")"
if [[ "${CONARY_FAKE_BAD_PUBLIC_BINDING:-0}" == "1" ]]; then
    raw_sha256=0000000000000000000000000000000000000000000000000000000000000000
fi
jq -n \
    --arg raw_sha256 "$raw_sha256" \
    --argjson raw_bytes "$raw_bytes" '
    {
      schema_version: 1,
      raw_report: {
        schema_version: 3,
        sha256: $raw_sha256,
        size_bytes: $raw_bytes
      },
      repetitions: [
        {iteration: 1, cache_state: "cold"},
        {iteration: 2, cache_state: "hot"}
      ]
    }
' >"$public"
chmod 0600 "$public"
EOF
    chmod 0755 "$bin"
}

make_fake_benchmark_systemctl() {
    local fake_root="$1"
    local command="${fake_root}/fake-systemctl"
    cat >"$command" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$CONARY_FAKE_SERVICE_LOG"
case "${1:-}" in
    is-active)
        [[ "${2:-}" == "--quiet" && "${3:-}" == "remi" && $# -eq 3 ]]
        [[ "$(cat "$CONARY_FAKE_SERVICE_STATE")" == "active" ]]
        ;;
    stop)
        [[ "${2:-}" == "remi" && $# -eq 2 ]]
        printf 'stopped\n' >"$CONARY_FAKE_SERVICE_STATE"
        ;;
    start)
        [[ "${2:-}" == "remi" && $# -eq 2 ]]
        [[ ! -e "$CONARY_FAKE_FAIL_START" ]]
        printf 'active\n' >"$CONARY_FAKE_SERVICE_STATE"
        ;;
    *) exit 2 ;;
esac
EOF
    chmod 0700 "$command"
}

make_benchmark_fixture() {
    local fake_root="$1"
    local run_id="$2"
    write_config "$fake_root"
    chmod 0644 "$fake_root/etc/conary/remi.toml"
    mkdir -m 0755 "$fake_root/work"
    make_fake_benchmark_remi "$fake_root"
    make_fake_benchmark_systemctl "$fake_root"
    printf 'active\n' >"$fake_root/service-state"
    : >"$fake_root/service-log"
    printf 'ok\n' >"$fake_root/health"
    local source="/tmp/remi-conversion-source-${run_id}.native"
    local transport="/tmp/remi-conversion-benchmark-${run_id}.json"
    printf 'fixed native benchmark source for %s\n' "$run_id" >"$source"
    chmod 0600 "$source"
    benchmark_tmp_paths+=("$source" "$transport")
}

run_helper() {
    local fake_root="$1"
    shift

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
    CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
    CONARY_FAKE_INSPECT_DIAGNOSTIC="${CONARY_FAKE_INSPECT_DIAGNOSTIC:-0}" \
        bash "$helper" "$@"
}

run_helper_with_ingress() {
    local fake_root="$1"
    shift

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
    CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
    CONARY_REMI_DEPLOY_SITE_HOME_URL="file://${fake_root}/conary/site/index.html" \
    CONARY_REMI_DEPLOY_SITE_INSTALLER_URL="file://${fake_root}/conary/site/install-conary-preview.sh" \
    CONARY_REMI_DEPLOY_SITE_ORIGIN_RESOLVE='' \
        bash "$helper" "$@"
}

run_benchmark_helper() {
    local fake_root="$1"
    shift

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
    CONARY_REMI_DEPLOY_SKIP_RESTART=0 \
    CONARY_REMI_DEPLOY_HEALTH_URL="file://${CONARY_FAKE_HEALTH_PATH:-${fake_root}/health}" \
    CONARY_REMI_DEPLOY_TEST_SYSTEMCTL="${fake_root}/fake-systemctl" \
    CONARY_REMI_DEPLOY_TEST_FILESYSTEM_TYPE="${CONARY_FAKE_FILESYSTEM_TYPE:-xfs}" \
    CONARY_REMI_DEPLOY_TEST_ROOT_FILESYSTEM_TYPE="${CONARY_FAKE_ROOT_FILESYSTEM_TYPE:-}" \
    CONARY_REMI_DEPLOY_TEST_WORK_FILESYSTEM_TYPE="${CONARY_FAKE_WORK_FILESYSTEM_TYPE:-}" \
    CONARY_REMI_DEPLOY_TEST_FILESYSTEM_DEVICE="${CONARY_FAKE_FILESYSTEM_DEVICE:-101}" \
    CONARY_REMI_DEPLOY_TEST_WORK_FILESYSTEM_DEVICE="${CONARY_FAKE_WORK_FILESYSTEM_DEVICE:-}" \
    CONARY_FAKE_SERVICE_STATE="${fake_root}/service-state" \
    CONARY_FAKE_SERVICE_LOG="${fake_root}/service-log" \
    CONARY_FAKE_FAIL_START="${fake_root}/fail-start" \
    CONARY_FAKE_BENCHMARK_ARGS="${fake_root}/benchmark-args" \
    CONARY_FAKE_BENCHMARK_FAIL="${CONARY_FAKE_BENCHMARK_FAIL:-0}" \
    CONARY_FAKE_BAD_PUBLIC_BINDING="${CONARY_FAKE_BAD_PUBLIC_BINDING:-0}" \
    CONARY_FAKE_RAW_REPORT_MODE="${CONARY_FAKE_RAW_REPORT_MODE:-0600}" \
        bash "$helper" benchmark-remi-conversion "$@"
}

expect_fail() {
    local description="$1"
    shift

    local status
    set +e
    "$@" >/dev/null 2>&1
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
        fail "$description unexpectedly succeeded"
    fi
}

test_deploy_conary_accepts_verified_release() {
    local fake_root="${tmpdir}/root-positive"
    local staging="${tmpdir}/staging-positive"
    write_config "$fake_root"
    make_release_staging "$staging" yes

    run_helper "$fake_root" deploy-conary 0.8.0 "$staging"

    test -f "$fake_root/conary/releases/0.8.0/conary-0.8.0.ccs"
    test -f "$fake_root/conary/releases/0.8.0/SHA256SUMS"
    test -L "$fake_root/conary/releases/latest"
    test -f "$fake_root/conary/self-update/conary-0.8.0.ccs"
    test -f "$fake_root/conary/self-update/conary-0.8.0.ccs.sig"
    test ! -e "$staging"
}

test_deploy_conary_rejects_checksum_mismatch() {
    local fake_root="${tmpdir}/root-checksum"
    local staging="${tmpdir}/staging-checksum"
    write_config "$fake_root"
    make_release_staging "$staging" yes
    printf 'tampered\n' >"$staging/metadata.json"

    expect_fail "checksum mismatch" run_helper "$fake_root" deploy-conary 0.8.0 "$staging"
}

test_deploy_conary_requires_ccs_signature() {
    local fake_root="${tmpdir}/root-missing-sig"
    local staging="${tmpdir}/staging-missing-sig"
    write_config "$fake_root"
    make_release_staging "$staging" no

    expect_fail "missing CCS signature" run_helper "$fake_root" deploy-conary 0.8.0 "$staging"
}

test_deploy_conary_rejects_symlinked_checksums() {
    local fake_root="${tmpdir}/root-symlink-checksums"
    local staging="${tmpdir}/staging-symlink-checksums"
    local checksum_target="${tmpdir}/external-SHA256SUMS"
    write_config "$fake_root"
    make_release_staging "$staging" yes
    mv "$staging/SHA256SUMS" "$checksum_target"
    ln -s "$checksum_target" "$staging/SHA256SUMS"

    expect_fail "symlinked checksum file" run_helper "$fake_root" deploy-conary 0.8.0 "$staging"
}

test_deploy_conary_rejects_symlinked_ccs_signature() {
    local fake_root="${tmpdir}/root-symlink-sig"
    local staging="${tmpdir}/staging-symlink-sig"
    local sig_target="${tmpdir}/external.ccs.sig"
    write_config "$fake_root"
    make_release_staging "$staging" yes
    mv "$staging/conary-0.8.0.ccs.sig" "$sig_target"
    ln -s "$sig_target" "$staging/conary-0.8.0.ccs.sig"

    expect_fail "symlinked CCS signature" run_helper "$fake_root" deploy-conary 0.8.0 "$staging"
}

test_deploy_site_replaces_site_root_from_staging() {
    local fake_root="${tmpdir}/root-site"
    local staging="${tmpdir}/staging-site"
    write_config "$fake_root"
    make_site_staging "$staging"
    mkdir -p "$fake_root/conary/site"
    printf 'old\n' >"$fake_root/conary/site/stale.txt"

    run_helper "$fake_root" deploy-site site "$staging"

    test -f "$fake_root/conary/site/index.html"
    test -f "$fake_root/conary/site/assets/app.js"
    test ! -e "$fake_root/conary/site/stale.txt"
    test ! -e "$staging"
}

test_deploy_site_replaces_web_root_from_staging() {
    local fake_root="${tmpdir}/root-web"
    local staging="${tmpdir}/staging-web"
    write_config "$fake_root"
    make_site_staging "$staging"

    run_helper "$fake_root" deploy-site web "$staging"

    test -f "$fake_root/conary/web/index.html"
    test -f "$fake_root/conary/web/assets/app.js"
    test ! -e "$staging"
}

test_deploy_site_rejects_unknown_target() {
    local fake_root="${tmpdir}/root-site-unknown"
    local staging="${tmpdir}/staging-site-unknown"
    write_config "$fake_root"
    make_site_staging "$staging"

    expect_fail "unknown site target" run_helper "$fake_root" deploy-site admin "$staging"
}

test_publish_test_artifact_is_verified_atomic_and_idempotent() {
    local fake_root="${tmpdir}/root-test-artifact"
    local staged="${tmpdir}/fedora44-guest-v1.qcow2"
    local digest
    write_config "$fake_root"
    printf 'qcow2-test-bytes\n' >"$staged"
    digest="$(sha256sum "$staged" | cut -d ' ' -f 1)"

    run_helper "$fake_root" publish-test-artifact \
        fedora44-guest-v1.qcow2 "$digest" "$staged"

    local published="$fake_root/conary/test-artifacts/fedora44-guest-v1.qcow2"
    test -f "$published"
    test ! -L "$published"
    test ! -e "$staged"
    test "$(sha256sum "$published" | cut -d ' ' -f 1)" = "$digest"
    test "$(stat -c '%a' "$published")" = "644"

    printf 'qcow2-test-bytes\n' >"$staged"
    run_helper "$fake_root" publish-test-artifact \
        fedora44-guest-v1.qcow2 "$digest" "$staged"
    test ! -e "$staged"
    test "$(sha256sum "$published" | cut -d ' ' -f 1)" = "$digest"
}

test_publish_test_artifact_rejects_unverified_or_mutating_inputs() {
    local fake_root="${tmpdir}/root-test-artifact-rejections"
    local staged="${tmpdir}/fedora44-guest-v1-rejected.qcow2"
    local digest
    write_config "$fake_root"
    printf 'original\n' >"$staged"
    digest="$(sha256sum "$staged" | cut -d ' ' -f 1)"

    expect_fail "invalid test-artifact filename" \
        run_helper "$fake_root" publish-test-artifact \
        ../escape.qcow2 "$digest" "$staged"
    expect_fail "test-artifact digest mismatch" \
        run_helper "$fake_root" publish-test-artifact \
        fedora44-guest-v1.qcow2 \
        0000000000000000000000000000000000000000000000000000000000000000 \
        "$staged"

    local empty="${tmpdir}/empty.qcow2"
    : >"$empty"
    expect_fail "empty test artifact" \
        run_helper "$fake_root" publish-test-artifact \
        empty.qcow2 \
        e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 \
        "$empty"

    local oversized="${tmpdir}/oversized.qcow2"
    truncate -s 8589934593 "$oversized"
    expect_fail "oversized test artifact" \
        run_helper "$fake_root" publish-test-artifact \
        oversized.qcow2 \
        0000000000000000000000000000000000000000000000000000000000000000 \
        "$oversized"

    local symlinked="${tmpdir}/symlinked.qcow2"
    ln -s "$staged" "$symlinked"
    expect_fail "symlinked test artifact" \
        run_helper "$fake_root" publish-test-artifact \
        symlinked.qcow2 "$digest" "$symlinked"

    local directory="${tmpdir}/directory.qcow2"
    mkdir "$directory"
    expect_fail "directory test artifact" \
        run_helper "$fake_root" publish-test-artifact \
        directory.qcow2 "$digest" "$directory"

    run_helper "$fake_root" publish-test-artifact \
        fedora44-guest-v1.qcow2 "$digest" "$staged"
    printf 'replacement\n' >"$staged"
    local replacement_digest
    replacement_digest="$(sha256sum "$staged" | cut -d ' ' -f 1)"
    expect_fail "immutable test-artifact replacement" \
        run_helper "$fake_root" publish-test-artifact \
        fedora44-guest-v1.qcow2 "$replacement_digest" "$staged"
    test -e "$staged"
    test "$(sha256sum "$fake_root/conary/test-artifacts/fedora44-guest-v1.qcow2" |
        cut -d ' ' -f 1)" = "$digest"
}

test_deploy_remi_uses_candidate_owned_transition() {
    local fake_root="${tmpdir}/root-remi"
    local bundle="${tmpdir}/remi-0.8.0.tar.gz"
    local repositories="${tmpdir}/repositories.toml"
    local inspection_stdout="${tmpdir}/inspect-remi.stdout"
    local inspection_stderr="${tmpdir}/inspect-remi.stderr"
    write_config "$fake_root"
    mkdir -p "$fake_root/usr/local/bin"
    make_fake_remi_bundle "$bundle" 0.8.0
    printf 'schema_version = 2\nrepositories = []\n' >"$repositories"

    run_helper "$fake_root" deploy-remi 0.8.0 "$bundle" "$repositories" 32

    test "$("$fake_root/usr/local/bin/remi" --version)" = "remi 0.8.0"
    test "$(cat "$fake_root/etc/conary/remi.toml.repository-keys-path")" = \
        "$fake_root/conary/repository-keys"
    test "$(stat -c '%a' "$fake_root/conary/repository-keys")" = "700"
    test "$(stat -c '%a' "$fake_root/conary/metadata")" = "750"
    test -f "$fake_root/conary/.remi-runtime.lock"
    test ! -L "$fake_root/conary/.remi-runtime.lock"
    test "$(stat -c '%a' "$fake_root/conary/.remi-runtime.lock")" = "600"
    printf 'stable-authority\n' >"$fake_root/conary/repository-keys/preserved"
    test ! -e "$bundle"
    test ! -e "$repositories"

    bundle="${tmpdir}/remi-0.8.1.tar.gz"
    repositories="${tmpdir}/repositories-repeat.toml"
    make_fake_remi_bundle "$bundle" 0.8.1
    printf 'schema_version = 2\nrepositories = []\n' >"$repositories"
    run_helper "$fake_root" deploy-remi 0.8.1 "$bundle" "$repositories" 32

    test "$("$fake_root/usr/local/bin/remi" --version)" = "remi 0.8.1"
    test "$(cat "$fake_root/conary/repository-keys/preserved")" = "stable-authority"

    run_helper "$fake_root" inspect-remi --require-private-candidates
    grep -Fx -- "--require-private-candidates" \
        "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    run_helper "$fake_root" inspect-remi --require-private-candidates \
        --accept-candidates-completed-after 123
    grep -Fx -- "--require-private-candidates" \
        "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    grep -Fx -- "--accept-candidates-completed-after" \
        "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    grep -Fx -- "123" "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    run_helper "$fake_root" inspect-remi --require-repopulated
    grep -Fx -- "--require-repopulated" \
        "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    CONARY_FAKE_INSPECT_DIAGNOSTIC=1 \
        run_helper "$fake_root" inspect-remi --require-private-candidates \
        >"$inspection_stdout" 2>"$inspection_stderr"
    jq -e '
        .schema_epoch == "test-v1"
        and .candidate_profiles == 3
    ' "$inspection_stdout" >/dev/null
    grep -Fx 'INFO immutable catalog reopen completed' \
        "$inspection_stderr" >/dev/null
    ! rg -q 'INFO immutable catalog' "$inspection_stdout" ||
        fail "Remi inspection diagnostics contaminated JSON stdout"
    expect_fail "unknown Remi inspection requirement" \
        run_helper "$fake_root" inspect-remi --require-something-vague
    expect_fail "completion floor without private-candidate requirement" \
        run_helper "$fake_root" inspect-remi --accept-candidates-completed-after 123
    expect_fail "missing private-candidate completion floor" \
        run_helper "$fake_root" inspect-remi --require-private-candidates \
        --accept-candidates-completed-after
    expect_fail "nonpositive private-candidate completion floor" \
        run_helper "$fake_root" inspect-remi --require-private-candidates \
        --accept-candidates-completed-after 0
    expect_fail "conflicting Remi inspection requirements" \
        run_helper "$fake_root" inspect-remi --require-private-candidates \
        --require-repopulated
}

test_candidate_baseline_uses_exact_staged_binary_without_mutation() {
    local fake_root="${tmpdir}/root-remi-baseline"
    local bundle="${tmpdir}/remi-baseline.tar.gz"
    local digest inspection
    write_config "$fake_root"
    make_fake_remi_bundle "$bundle" 0.8.0
    digest="$(tar xOzf "$bundle" remi-0.8.0-linux-x64 | sha256sum | cut -d ' ' -f 1)"

    inspection="$(run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.0 "$digest" "$bundle")"
    jq -e \
        --arg config "$fake_root/etc/conary/remi.toml" \
        '.baseline_schema_version == 1 and .config == $config and .owner == "candidate"' \
        <<<"$inspection" >/dev/null
    test -f "$bundle"
    test ! -e "$fake_root/usr/local/bin/remi"

    mkdir -p "$fake_root/conary/metadata"
    printf 'persisted database\n' >"$fake_root/conary/metadata/conary.db"
    inspection="$(run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.0 "$digest" "$bundle")"
    jq -e '.owner == "candidate"' <<<"$inspection" >/dev/null

    expect_fail "candidate baseline digest mismatch" \
        run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.0 \
        0000000000000000000000000000000000000000000000000000000000000000 \
        "$bundle"
    expect_fail "candidate baseline version mismatch" \
        run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.1 "$digest" "$bundle"
}

test_candidate_baseline_uses_installed_schema_owner_after_candidate_verification() {
    local fake_root="${tmpdir}/root-remi-live-baseline"
    local bundle="${tmpdir}/remi-live-baseline.tar.gz"
    local digest inspection installed
    write_config "$fake_root"
    make_fake_remi_bundle "$bundle" 0.8.0
    digest="$(tar xOzf "$bundle" remi-0.8.0-linux-x64 | sha256sum | cut -d ' ' -f 1)"
    installed="$fake_root/usr/local/bin/remi"
    mkdir -p "$(dirname "$installed")"
    cat >"$installed" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "deployment" && "${2:-}" == "baseline" ]]
shift 2
[[ "${1:-}" == "--config" && -n "${2:-}" && $# -eq 2 ]]
printf '{"baseline_schema_version":1,"config":"%s","owner":"installed"}\n' "$2"
EOF
    chmod 0755 "$installed"

    inspection="$(run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.0 "$digest" "$bundle")"
    jq -e \
        --arg config "$fake_root/etc/conary/remi.toml" \
        '.baseline_schema_version == 1 and .config == $config and .owner == "installed"' \
        <<<"$inspection" >/dev/null
    test -f "$bundle"
    test "$($installed deployment baseline --config "$fake_root/etc/conary/remi.toml" \
        | jq -r .owner)" = "installed"

    mv "$installed" "${installed}.real"
    ln -s "${installed}.real" "$installed"
    expect_fail "symlinked installed baseline owner" \
        run_helper "$fake_root" inspect-remi-candidate-baseline \
        0.8.0 "$digest" "$bundle"
}

test_shared_conary_root_is_preserved_and_drift_fails_closed() {
    local fake_root="${tmpdir}/root-shared-contract"
    local before after
    write_config "$fake_root"
    before="$(stat -c '%u:%g:%a' "$fake_root/conary")"

    run_helper "$fake_root" verify-access
    after="$(stat -c '%u:%g:%a' "$fake_root/conary")"
    test "$after" = "$before"

    chmod 0755 "$fake_root/conary"
    expect_fail "shared Conary root mode drift" run_helper "$fake_root" verify-access
    test "$(stat -c '%a' "$fake_root/conary")" = "755"

    chmod 0750 "$fake_root/conary"
    mv "$fake_root/conary" "$fake_root/conary-real"
    ln -s "$fake_root/conary-real" "$fake_root/conary"
    expect_fail "symlinked shared Conary root" run_helper "$fake_root" verify-access
}

test_verify_ingress_requires_exact_deployed_bytes() {
    local fake_root="${tmpdir}/root-ingress"
    write_config "$fake_root"
    mkdir -p "$fake_root/conary/site"
    printf '<!doctype html><title>Conary</title>\n' >"$fake_root/conary/site/index.html"
    printf '#!/usr/bin/env bash\n' >"$fake_root/conary/site/install-conary-preview.sh"

    run_helper_with_ingress "$fake_root" verify-ingress

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
    CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
    CONARY_REMI_DEPLOY_SITE_HOME_URL="file://${fake_root}/conary/site/index.html" \
    CONARY_REMI_DEPLOY_SITE_INSTALLER_URL="file://${fake_root}/conary/site/index.html" \
    CONARY_REMI_DEPLOY_SITE_ORIGIN_RESOLVE='' \
        expect_fail "installer byte mismatch" bash "$helper" verify-ingress
}

test_deploy_remi_rejects_malformed_authority_root() {
    local fake_root="${tmpdir}/root-remi-malformed"
    local bundle="${tmpdir}/remi-malformed.tar.gz"
    local repositories="${tmpdir}/repositories-malformed.toml"
    write_config "$fake_root"
    mkdir -p "$fake_root/usr/local/bin" "$fake_root/conary/repository-keys"
    chmod 0755 "$fake_root/conary/repository-keys"
    make_fake_remi_bundle "$bundle" 0.8.0
    printf 'schema_version = 2\nrepositories = []\n' >"$repositories"

    expect_fail "insecure repository authority root" \
        run_helper "$fake_root" deploy-remi 0.8.0 "$bundle" "$repositories" 32
    test ! -e "$fake_root/usr/local/bin/remi"

    chmod 0700 "$fake_root/conary/repository-keys"
    rmdir "$fake_root/conary/repository-keys"
    ln -s "${tmpdir}" "$fake_root/conary/repository-keys"
    expect_fail "symlinked repository authority root" \
        run_helper "$fake_root" deploy-remi 0.8.0 "$bundle" "$repositories" 32
    test ! -e "$fake_root/usr/local/bin/remi"
}

test_inspect_remi_storage_reports_bounded_numeric_evidence() {
    local fake_root="${tmpdir}/root-storage-inspection"
    local inspection
    write_config "$fake_root"
    mkdir -p \
        "$fake_root/conary/metadata" \
        "$fake_root/conary/deployment-backups/first" \
        "$fake_root/conary/deployment-backups/second"
    truncate -s 1048576 "$fake_root/conary/metadata/conary.db"
    printf 'first\n' >"$fake_root/conary/deployment-backups/first/transition.json"
    printf 'second\n' >"$fake_root/conary/deployment-backups/second/transition.json"

    inspection="$(run_helper "$fake_root" inspect-remi-storage)"
    jq -e '
        .schema_version == 1
        and .filesystem.available_bytes > 0
        and .database.files == 1
        and .database.logical_bytes == 1048576
        and .database.allocated_bytes >= 0
        and .transition_backups.directories == 2
        and .transition_backups.logical_bytes > 0
        and .transition_backups.allocated_bytes >= 0
    ' <<<"$inspection" >/dev/null

    ln -s "$fake_root/conary/metadata/conary.db" \
        "$fake_root/conary/deployment-backups/first/database-link"
    expect_fail "symlinked deployment backup storage" \
        run_helper "$fake_root" inspect-remi-storage
}

test_export_native_oracle_inputs_uses_exact_public_candidates() {
    local fake_root="${tmpdir}/root-native-input"
    local bundle="${tmpdir}/remi-native-input.tar.gz"
    local repositories="${tmpdir}/repositories-native-input.toml"
    local export_id="slice6-$$"
    local fedora_sha ubuntu_sha arch_sha transport unpacked
    fedora_sha="$(printf 'a%.0s' {1..64})"
    ubuntu_sha="$(printf 'b%.0s' {1..64})"
    arch_sha="$(printf 'c%.0s' {1..64})"
    transport="/tmp/remi-native-oracle-input-${export_id}.tar"
    unpacked="${tmpdir}/native-input-unpacked"
    write_config "$fake_root"
    mkdir -p "$fake_root/usr/local/bin"
    make_fake_remi_bundle "$bundle" 0.8.0
    printf 'schema_version = 2\nrepositories = []\n' >"$repositories"
    run_helper "$fake_root" deploy-remi 0.8.0 "$bundle" "$repositories" 32

    run_helper "$fake_root" export-native-oracle-inputs \
        "$export_id" "$fedora_sha" "$ubuntu_sha" "$arch_sha"
    test -f "$transport"
    mkdir "$unpacked"
    tar -xf "$transport" -C "$unpacked"
    grep -Fx -- "fedora-44=${fedora_sha}" \
        "$unpacked/$export_id/command-args" >/dev/null
    grep -Fx -- "ubuntu-26.04=${ubuntu_sha}" \
        "$unpacked/$export_id/command-args" >/dev/null
    grep -Fx -- "arch=${arch_sha}" \
        "$unpacked/$export_id/command-args" >/dev/null
    expect_fail "repeated native-oracle export" \
        run_helper "$fake_root" export-native-oracle-inputs \
        "$export_id" "$fedora_sha" "$ubuntu_sha" "$arch_sha"
    expect_fail "uppercase native-oracle candidate digest" \
        run_helper "$fake_root" export-native-oracle-inputs \
        "${export_id}-upper" "${fedora_sha^^}" "$ubuntu_sha" "$arch_sha"
    rm -f "$transport"
}

run_valid_conversion_benchmark() {
    local fake_root="$1"
    local run_id="$2"
    local bin_sha256 source source_sha256 source_size
    bin_sha256="$(sha256sum "$fake_root/usr/local/bin/remi" | cut -d ' ' -f 1)"
    source="/tmp/remi-conversion-source-${run_id}.native"
    source_sha256="$(sha256sum "$source" | cut -d ' ' -f 1)"
    source_size="$(stat -c '%s' "$source")"
    run_benchmark_helper "$fake_root" \
        "$run_id" \
        "$bin_sha256" \
        fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" \
        "$source_size"
}

assert_benchmark_service_sequence() {
    local fake_root="$1"
    local expected=$'is-active --quiet remi\nstop remi\nstart remi'
    [[ "$(cat "$fake_root/service-log")" == "$expected" ]] ||
        fail "unexpected benchmark service sequence: $(cat "$fake_root/service-log")"
}

assert_benchmark_recovery_retry_sequence() {
    local fake_root="$1"
    local expected=$'is-active --quiet remi\nstop remi\nstart remi\nstart remi'
    [[ "$(cat "$fake_root/service-log")" == "$expected" ]] ||
        fail "unexpected benchmark recovery retry sequence: $(cat "$fake_root/service-log")"
}

test_conversion_benchmark_uses_fixed_paths_arguments_and_service_sequence() {
    local run_id="benchmark-success-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    local output transport work_root raw public bin_sha256 source source_sha256 source_size
    make_benchmark_fixture "$fake_root" "$run_id"
    chmod 0640 "$fake_root/etc/conary/remi.toml"
    output="$(CONARY_FAKE_ROOT_FILESYSTEM_TYPE=ext4 \
        CONARY_FAKE_FILESYSTEM_DEVICE=101 \
        CONARY_FAKE_WORK_FILESYSTEM_DEVICE=101 \
        run_valid_conversion_benchmark "$fake_root" "$run_id")"
    transport="/tmp/remi-conversion-benchmark-${run_id}.json"
    work_root="$fake_root/work/remi-conversion-benchmarks/$run_id/work"
    raw="$work_root/conversion-benchmark-v3.json"
    public="$work_root/conversion-benchmark-public-v1.json"
    bin_sha256="$(sha256sum "$fake_root/usr/local/bin/remi" | cut -d ' ' -f 1)"
    source="/tmp/remi-conversion-source-${run_id}.native"
    source_sha256="$(sha256sum "$source" | cut -d ' ' -f 1)"
    source_size="$(stat -c '%s' "$source")"

    local transport_sha256 transport_bytes
    transport_sha256="$(sha256sum "$transport" | cut -d ' ' -f 1)"
    transport_bytes="$(stat -c '%s' "$transport")"
    [[ "$output" == "Conversion benchmark: run=${run_id} transport=${transport} sha256=${transport_sha256} bytes=${transport_bytes}" ]] ||
        fail "conversion benchmark returned an unexpected publication line: $output"
    [[ -f "$raw" && ! -L "$raw" && "$(stat -c '%a' "$raw")" == "600" ]]
    [[ -f "$public" && ! -L "$public" && "$(stat -c '%a' "$public")" == "600" ]]
    [[ -f "$transport" && ! -L "$transport" && "$(stat -c '%a' "$transport")" == "600" ]]
    cmp -s "$public" "$transport"
    local private_root
    private_root="$fake_root/work/remi-conversion-benchmarks/$run_id"
    [[ -f "$private_root/source.native" ]]
    [[ "$(stat -c '%a' "$private_root/source.native")" == "400" ]]
    [[ -f "$private_root/remi.toml" && ! -L "$private_root/remi.toml" ]]
    [[ "$(stat -c '%a' "$private_root/remi.toml")" == "400" ]]
    cmp -s "$fake_root/etc/conary/remi.toml" "$private_root/remi.toml"
    [[ "$(stat -c '%a' "$fake_root/work/remi-conversion-benchmarks")" == "700" ]]
    jq -e \
        --arg sha "$(sha256sum "$raw" | cut -d ' ' -f 1)" \
        --argjson bytes "$(stat -c '%s' "$raw")" '
        .schema_version == 1
        and .raw_report.sha256 == $sha
        and .raw_report.size_bytes == $bytes
    ' "$transport" >/dev/null

    local expected_args=(
        conversion-benchmark
        --config "$private_root/remi.toml"
        --work-root "$work_root"
        --profile fedora-44
        --revision aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        --package-key bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
        --source-artifact "$private_root/source.native"
        --hardware-label remi-production-i7-8700-xfs
        --iterations 2
    )
    local actual_args=()
    mapfile -t actual_args <"$fake_root/benchmark-args"
    [[ "${#actual_args[@]}" == "${#expected_args[@]}" ]] ||
        fail "conversion benchmark argv length changed"
    local index
    for index in "${!expected_args[@]}"; do
        [[ "${actual_args[$index]}" == "${expected_args[$index]}" ]] ||
            fail "conversion benchmark argv changed at index $index"
    done
    [[ "$bin_sha256" =~ ^[0-9a-f]{64}$ && "$source_sha256" =~ ^[0-9a-f]{64}$ ]]
    [[ "$source_size" =~ ^[1-9][0-9]*$ ]]
    assert_benchmark_service_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
}

test_conversion_benchmark_failure_restarts_without_publication() {
    local run_id="benchmark-command-failure-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"

    CONARY_FAKE_BENCHMARK_FAIL=1 \
        expect_fail "failed conversion benchmark" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    assert_benchmark_service_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]
}

test_conversion_benchmark_rejects_unbound_or_public_raw_evidence() {
    local run_id="benchmark-unbound-sidecar-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"

    CONARY_FAKE_BAD_PUBLIC_BINDING=1 \
        expect_fail "public sidecar with the wrong raw binding" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    assert_benchmark_service_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]

    run_id="benchmark-public-raw-report-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    CONARY_FAKE_RAW_REPORT_MODE=0644 \
        expect_fail "raw benchmark report with public mode" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    assert_benchmark_service_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]
}

test_conversion_benchmark_restart_and_health_fail_closed() {
    local run_id="benchmark-restart-failure-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    : >"$fake_root/fail-start"

    expect_fail "failed benchmark service restart" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    assert_benchmark_recovery_retry_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "stopped" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]

    run_id="benchmark-health-failure-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    CONARY_FAKE_HEALTH_PATH="$fake_root/missing-health" \
        expect_fail "failed benchmark liveness probe" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    assert_benchmark_recovery_retry_sequence "$fake_root"
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]
}

test_conversion_benchmark_rejects_non_xfs_before_downtime() {
    local run_id="benchmark-non-xfs-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"

    CONARY_FAKE_WORK_FILESYSTEM_TYPE=ext4 \
        expect_fail "non-XFS conversion benchmark work carrier" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    [[ ! -s "$fake_root/service-log" ]]
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "$fake_root/work/remi-conversion-benchmarks/$run_id" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]
}

test_conversion_benchmark_rejects_distinct_xfs_device_before_downtime() {
    local run_id="benchmark-distinct-xfs-device-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"

    CONARY_FAKE_FILESYSTEM_DEVICE=101 \
        CONARY_FAKE_WORK_FILESYSTEM_DEVICE=202 \
        expect_fail "conversion benchmark on a distinct XFS device" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    [[ ! -s "$fake_root/service-log" ]]
    [[ "$(cat "$fake_root/service-state")" == "active" ]]
    [[ ! -e "$fake_root/work/remi-conversion-benchmarks/$run_id" ]]
    [[ ! -e "/tmp/remi-conversion-benchmark-${run_id}.json" ]]
}

test_conversion_benchmark_rejects_invalid_inputs_and_existing_targets() {
    local run_id="benchmark-invalid-binary-$$"
    local fake_root="${tmpdir}/root-${run_id}"
    local source source_sha256 source_size bin_sha256
    make_benchmark_fixture "$fake_root" "$run_id"
    source="/tmp/remi-conversion-source-${run_id}.native"
    source_sha256="$(sha256sum "$source" | cut -d ' ' -f 1)"
    source_size="$(stat -c '%s' "$source")"
    bin_sha256="$(sha256sum "$fake_root/usr/local/bin/remi" | cut -d ' ' -f 1)"
    expect_fail "noncanonical benchmark profile" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" 'Fedora 44' \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" "$source_size"
    expect_fail "unsupported canonical benchmark profile" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" debian-13 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" "$source_size"
    expect_fail "oversized benchmark source declaration" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" 8589934593
    expect_fail "wrong installed benchmark binary digest" \
        run_benchmark_helper "$fake_root" \
        "$run_id" \
        0000000000000000000000000000000000000000000000000000000000000000 \
        fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" "$source_size"

    run_id="benchmark-unreadable-bin-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    chmod 0700 "$fake_root/usr/local/bin/remi"
    expect_fail "service-inaccessible benchmark binary" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-unreadable-config-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    chmod 0600 "$fake_root/etc/conary/remi.toml"
    expect_fail "service-inaccessible benchmark configuration" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-invalid-source-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    source="/tmp/remi-conversion-source-${run_id}.native"
    source_size="$(stat -c '%s' "$source")"
    bin_sha256="$(sha256sum "$fake_root/usr/local/bin/remi" | cut -d ' ' -f 1)"
    expect_fail "wrong staged benchmark source digest" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        0000000000000000000000000000000000000000000000000000000000000000 \
        "$source_size"
    source_sha256="$(sha256sum "$source" | cut -d ' ' -f 1)"
    expect_fail "wrong staged benchmark source size" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" 1
    chmod 0666 "$source"
    expect_fail "writable staged benchmark source" \
        run_benchmark_helper "$fake_root" \
        "$run_id" "$bin_sha256" fedora-44 \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
        "$source_sha256" "$source_size"

    run_id="benchmark-symlink-source-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    source="/tmp/remi-conversion-source-${run_id}.native"
    mv "$source" "$fake_root/source-real"
    ln -s "$fake_root/source-real" "$source"
    expect_fail "symlinked staged benchmark source" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-symlink-config-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    mv "$fake_root/etc/conary/remi.toml" "$fake_root/config-real.toml"
    ln -s "$fake_root/config-real.toml" "$fake_root/etc/conary/remi.toml"
    expect_fail "symlinked benchmark configuration" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-symlink-work-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    rmdir "$fake_root/work"
    ln -s "$fake_root/conary" "$fake_root/work"
    expect_fail "symlinked benchmark XFS container" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-existing-run-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    mkdir -m 0700 "$fake_root/work/remi-conversion-benchmarks"
    mkdir "$fake_root/work/remi-conversion-benchmarks/$run_id"
    expect_fail "existing benchmark run root" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    run_id="benchmark-existing-transport-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    printf 'preserve\n' >"/tmp/remi-conversion-benchmark-${run_id}.json"
    expect_fail "existing benchmark transport" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"
    [[ "$(cat "/tmp/remi-conversion-benchmark-${run_id}.json")" == "preserve" ]]

    run_id="benchmark-insecure-root-$$"
    fake_root="${tmpdir}/root-${run_id}"
    make_benchmark_fixture "$fake_root" "$run_id"
    mkdir -m 0755 "$fake_root/work/remi-conversion-benchmarks"
    chmod 0755 "$fake_root/work/remi-conversion-benchmarks"
    expect_fail "insecure preexisting benchmark root" \
        run_valid_conversion_benchmark "$fake_root" "$run_id"

    local root
    for root in \
        "${tmpdir}/root-benchmark-invalid-binary-$$" \
        "${tmpdir}/root-benchmark-unreadable-bin-$$" \
        "${tmpdir}/root-benchmark-unreadable-config-$$" \
        "${tmpdir}/root-benchmark-invalid-source-$$" \
        "${tmpdir}/root-benchmark-symlink-source-$$" \
        "${tmpdir}/root-benchmark-symlink-config-$$" \
        "${tmpdir}/root-benchmark-symlink-work-$$" \
        "${tmpdir}/root-benchmark-existing-run-$$" \
        "${tmpdir}/root-benchmark-existing-transport-$$" \
        "${tmpdir}/root-benchmark-insecure-root-$$"; do
        [[ ! -s "$root/service-log" ]] || fail "invalid benchmark input caused downtime"
    done
}

test_install_helper_requires_exact_digest() {
    local fake_root="${tmpdir}/root-helper"
    local staged="${tmpdir}/staged-helper"
    local digest
    mkdir -p "$fake_root/usr/local/sbin"
    cp "$helper" "$staged"
    digest="$(sha256sum "$staged" | cut -d ' ' -f 1)"

    run_helper "$fake_root" install-helper "$digest" "$staged"
    test -x "$fake_root/usr/local/sbin/conary-remi-deploy"

    cp "$helper" "$staged"
    expect_fail "helper digest mismatch" \
        run_helper "$fake_root" install-helper \
        0000000000000000000000000000000000000000000000000000000000000000 \
        "$staged"
}

test_verify_access_does_not_require_a_running_service() {
    local fake_root="${tmpdir}/root-verify-access"

    expect_fail "deploy access without Remi configuration" \
        run_helper "$fake_root" verify-access
    write_config "$fake_root"
    run_helper "$fake_root" verify-access
}

main() {
    test_deploy_conary_accepts_verified_release
    test_deploy_conary_rejects_checksum_mismatch
    test_deploy_conary_requires_ccs_signature
    test_deploy_conary_rejects_symlinked_checksums
    test_deploy_conary_rejects_symlinked_ccs_signature
    test_deploy_site_replaces_site_root_from_staging
    test_deploy_site_replaces_web_root_from_staging
    test_deploy_site_rejects_unknown_target
    test_publish_test_artifact_is_verified_atomic_and_idempotent
    test_publish_test_artifact_rejects_unverified_or_mutating_inputs
    test_deploy_remi_uses_candidate_owned_transition
    test_candidate_baseline_uses_exact_staged_binary_without_mutation
    test_candidate_baseline_uses_installed_schema_owner_after_candidate_verification
    test_shared_conary_root_is_preserved_and_drift_fails_closed
    test_verify_ingress_requires_exact_deployed_bytes
    test_deploy_remi_rejects_malformed_authority_root
    test_inspect_remi_storage_reports_bounded_numeric_evidence
    test_export_native_oracle_inputs_uses_exact_public_candidates
    test_conversion_benchmark_uses_fixed_paths_arguments_and_service_sequence
    test_conversion_benchmark_failure_restarts_without_publication
    test_conversion_benchmark_rejects_unbound_or_public_raw_evidence
    test_conversion_benchmark_restart_and_health_fail_closed
    test_conversion_benchmark_rejects_non_xfs_before_downtime
    test_conversion_benchmark_rejects_distinct_xfs_device_before_downtime
    test_conversion_benchmark_rejects_invalid_inputs_and_existing_targets
    test_install_helper_requires_exact_digest
    test_verify_access_does_not_require_a_running_service

    echo "remi deploy helper smoke passed"
}

main "$@"
