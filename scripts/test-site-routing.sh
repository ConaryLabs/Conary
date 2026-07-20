#!/usr/bin/env bash
# scripts/test-site-routing.sh -- Exercise the conary.io nginx routing installer.
set -euo pipefail

script="${1:-deploy/configure-site-routing.sh}"
test -f "$script" || {
    echo "missing routing installer: $script" >&2
    exit 1
}

tmpdir="$(mktemp -d /tmp/conary-site-routing-test.XXXXXX)"
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

write_legacy_config() {
    local root="$1"
    local name="${2:-conary}"
    mkdir -p "$root/etc/nginx/sites-available"
    printf '%s\n' \
        'server {' \
        '    server_name conary.io www.conary.io;' \
        '    root /conary/site;' \
        '    location / {' \
        '        try_files $uri $uri/ /index.html;' \
        '    }' \
        '}' > "$root/etc/nginx/sites-available/$name"
}

run_installer() {
    local root="$1"
    CONARY_SITE_NGINX_ROOT="$root/etc/nginx" \
    CONARY_SITE_NGINX_SKIP_RELOAD=1 \
        bash "$script"
}

root="$tmpdir/positive"
write_legacy_config "$root"
run_installer "$root"
config="$root/etc/nginx/sites-available/conary"
grep -Fq 'try_files $uri $uri/ =404;' "$config" || fail "missing status-aware try_files route"
grep -Fq 'error_page 404 /404.html;' "$config" || fail "missing branded error_page route"
! grep -Fq 'try_files $uri $uri/ /index.html;' "$config" || fail "SPA fallback remains"
test -f "$root/etc/nginx/backups/conary."* || fail "backup was not created"

before="$(sha256sum "$config")"
run_installer "$root"
after="$(sha256sum "$config")"
[[ "$before" == "$after" ]] || fail "idempotent rerun changed the config"

ambiguous="$tmpdir/ambiguous"
write_legacy_config "$ambiguous" conary-a
write_legacy_config "$ambiguous" conary-b
if run_installer "$ambiguous" >/dev/null 2>&1; then
    fail "ambiguous config unexpectedly succeeded"
fi

unexpected="$tmpdir/unexpected"
write_legacy_config "$unexpected"
sed -i 's|try_files $uri $uri/ /index.html;|proxy_pass http://127.0.0.1:8080;|' \
    "$unexpected/etc/nginx/sites-available/conary"
if run_installer "$unexpected" >/dev/null 2>&1; then
    fail "unexpected routing config unexpectedly succeeded"
fi

echo "site routing installer smoke passed"
