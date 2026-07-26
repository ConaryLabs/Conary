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
chunking = true
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
    while [[ \$# -gt 0 ]]; do
        case "\$1" in
            --config)
                config="\$2"
                shift 2
                ;;
            *)
                shift
                [[ \$# -gt 0 ]] && shift
                ;;
        esac
    done
    transition="\${config}.transition.json"
    printf '{}\n' >"\$transition"
    echo "\$transition"
    exit 0
fi
if [[ "\${1:-}" == "deployment" && "\${2:-}" == "rollback" ]]; then
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
    test ! -e "$bundle"
    test ! -e "$repositories"
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
    test_deploy_remi_uses_candidate_owned_transition
    test_install_helper_requires_exact_digest

    echo "remi deploy helper smoke passed"
}

main "$@"
