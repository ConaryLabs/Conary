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

fake_root="${tmpdir}/root"
staging="${tmpdir}/staging"
mkdir -p "$fake_root/etc/conary" "$staging"

cat >"$fake_root/etc/conary/remi.toml" <<'TOML'
[server]
bind = "127.0.0.1:8080"

[conversion]
chunking = true
max_concurrent = 4

[r2]
enabled = false
TOML

printf 'ccs\n' >"$staging/conary-0.8.0.ccs"
printf 'sig\n' >"$staging/conary-0.8.0.ccs.sig"
printf 'notes\n' >"$staging/metadata.json"

CONARY_REMI_DEPLOY_ROOT="$fake_root" \
CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
    bash "$helper" deploy-conary 0.8.0 "$staging"

test -f "$fake_root/conary/releases/0.8.0/conary-0.8.0.ccs"
test -f "$fake_root/conary/releases/0.8.0/SHA256SUMS"
test -L "$fake_root/conary/releases/latest"
test -f "$fake_root/conary/self-update/conary-0.8.0.ccs"
test ! -e "$staging"

CONARY_REMI_DEPLOY_ROOT="$fake_root" \
CONARY_REMI_DEPLOY_SKIP_RESTART=1 \
    bash "$helper" configure-concurrency 32

grep -q '^max_concurrent = 32$' "$fake_root/etc/conary/remi.toml"

echo "remi deploy helper smoke passed"
