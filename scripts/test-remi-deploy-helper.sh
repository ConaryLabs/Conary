#!/usr/bin/env bash
# scripts/test-remi-deploy-helper.sh -- Exercise the Remi deploy helper in a fake root.
set -euo pipefail

helper="${1:-deploy/remi-deploy-helper.sh}"
test -f "$helper" || {
    echo "missing helper: $helper" >&2
    exit 1
}

tmpdir="$(mktemp -d /tmp/remi-deploy-helper-test.XXXXXX)"
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

write_config() {
    local fake_root="$1"
    mkdir -p "$fake_root/etc/conary"
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
    exit 0
fi
exit 2
EOF
    chmod 0755 "$candidate"
    tar czf "$bundle" -C "$build_dir" "$(basename "$candidate")"
}

run_helper() {
    local fake_root="$1"
    shift

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
    CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
        bash "$helper" "$@"
}

expect_fail() {
    local description="$1"
    shift

    local output status
    set +e
    output="$("$@" 2>&1)"
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
    write_config "$fake_root"
    mkdir -p "$fake_root/usr/local/bin"
    make_fake_remi_bundle "$bundle" 0.8.0
    printf 'schema_version = 2\nrepositories = []\n' >"$repositories"

    run_helper "$fake_root" deploy-remi 0.8.0 "$bundle" "$repositories" 32

    test "$("$fake_root/usr/local/bin/remi" --version)" = "remi 0.8.0"
    test "$(cat "$fake_root/etc/conary/remi.toml.repository-keys-path")" = \
        "$fake_root/conary/repository-keys"
    test "$(stat -c '%a' "$fake_root/conary/repository-keys")" = "700"
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
    run_helper "$fake_root" inspect-remi --require-repopulated
    grep -Fx -- "--require-repopulated" \
        "$fake_root/etc/conary/remi.toml.inspect-args" >/dev/null
    expect_fail "unknown Remi inspection requirement" \
        run_helper "$fake_root" inspect-remi --require-something-vague
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
    test_deploy_remi_rejects_malformed_authority_root
    test_install_helper_requires_exact_digest

    echo "remi deploy helper smoke passed"
}

main "$@"
