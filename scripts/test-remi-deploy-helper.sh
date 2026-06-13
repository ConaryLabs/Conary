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

test_configure_concurrency_updates_config() {
    local fake_root="${tmpdir}/root-config"
    write_config "$fake_root"

    run_helper "$fake_root" configure-concurrency 32

    grep -q '^max_concurrent = 32$' "$fake_root/etc/conary/remi.toml"
}

test_configure_concurrency_accepts_skip_restart_flag() {
    local fake_root="${tmpdir}/root-config-skip"
    write_config "$fake_root"

    CONARY_REMI_DEPLOY_ROOT="$fake_root" \
        bash "$helper" configure-concurrency 16 --skip-restart

    grep -q '^max_concurrent = 16$' "$fake_root/etc/conary/remi.toml"
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
    test_configure_concurrency_updates_config
    test_configure_concurrency_accepts_skip_restart_flag

    echo "remi deploy helper smoke passed"
}

main "$@"
