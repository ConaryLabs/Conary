#!/usr/bin/env bash
# deploy/configure-site-routing.sh -- Configure nginx for prerendered conary.io routes.
set -euo pipefail

PATH=/usr/sbin:/usr/bin:/sbin:/bin

NGINX_ROOT="${CONARY_SITE_NGINX_ROOT:-/etc/nginx}"
SKIP_RELOAD="${CONARY_SITE_NGINX_SKIP_RELOAD:-0}"

die() {
    echo "site routing: $*" >&2
    exit 1
}

if [[ -z "${CONARY_SITE_NGINX_ROOT:-}" && "$(id -u)" != "0" ]]; then
    die "must run as root"
fi

search_paths=()
for path in "$NGINX_ROOT/sites-available" "$NGINX_ROOT/conf.d"; do
    [[ -d "$path" ]] && search_paths+=("$path")
done
[[ -f "$NGINX_ROOT/nginx.conf" ]] && search_paths+=("$NGINX_ROOT/nginx.conf")
(( ${#search_paths[@]} > 0 )) || die "no nginx configuration files found under $NGINX_ROOT"

declare -A seen=()
candidates=()
while IFS= read -r -d '' path; do
    resolved="$(realpath -e "$path")" || continue
    [[ -f "$resolved" && ! -L "$resolved" ]] || continue
    [[ -z "${seen[$resolved]:-}" ]] || continue
    seen[$resolved]=1
    if grep -Eq '^[[:space:]]*root[[:space:]]+/conary/site/?;[[:space:]]*$' "$resolved" &&
        grep -Eq '^[[:space:]]*server_name[[:space:]][^;]*\bconary\.io\b[^;]*;' "$resolved"; then
        candidates+=("$resolved")
    fi
done < <(find -L "${search_paths[@]}" -type f -print0)

(( ${#candidates[@]} == 1 )) ||
    die "expected exactly one conary.io config owning /conary/site, found ${#candidates[@]}"

config="${candidates[0]}"
desired_route='^[[:space:]]*try_files[[:space:]]+\$uri[[:space:]]+\$uri/[[:space:]]+=404;[[:space:]]*$'
desired_error='^[[:space:]]*error_page[[:space:]]+404[[:space:]]+/404\.html;[[:space:]]*$'
legacy_route='^[[:space:]]*try_files[[:space:]]+\$uri[[:space:]]+\$uri/[[:space:]]+/index\.html([[:space:]]+=404)?;[[:space:]]*$'

desired_route_count="$(grep -Ec "$desired_route" "$config" || true)"
desired_error_count="$(grep -Ec "$desired_error" "$config" || true)"
legacy_route_count="$(grep -Ec "$legacy_route" "$config" || true)"

if [[ "$desired_route_count" == "1" && "$desired_error_count" == "1" && "$legacy_route_count" == "0" ]]; then
    echo "site routing already configured in $config"
    exit 0
fi

[[ "$desired_route_count" == "0" && "$desired_error_count" == "0" && "$legacy_route_count" == "1" ]] ||
    die "refusing to modify unexpected routing directives in $config"

backup_dir="${CONARY_SITE_NGINX_BACKUP_DIR:-/var/backups/conary-nginx}"
if [[ -n "${CONARY_SITE_NGINX_ROOT:-}" && -z "${CONARY_SITE_NGINX_BACKUP_DIR:-}" ]]; then
    backup_dir="$NGINX_ROOT/backups"
fi
mkdir -p "$backup_dir"
backup="$backup_dir/$(basename "$config").$(date -u +%Y%m%dT%H%M%SZ)"
cp -a "$config" "$backup"

tmp="$(mktemp "${config}.conary.XXXXXX")"
cleanup() {
    rm -f "$tmp"
}
trap cleanup EXIT

awk '
    /^[[:space:]]*try_files[[:space:]]+\$uri[[:space:]]+\$uri\/[[:space:]]+\/index\.html([[:space:]]+=404)?;[[:space:]]*$/ {
        match($0, /^[[:space:]]*/)
        indent = substr($0, 1, RLENGTH)
        print indent "try_files $uri $uri/ =404;"
        print indent "error_page 404 /404.html;"
        next
    }
    { print }
' "$config" > "$tmp"

install -m "$(stat -c '%a' "$config")" "$tmp" "$config"

restore() {
    cp -a "$backup" "$config"
}

if [[ "$SKIP_RELOAD" != "1" ]]; then
    if ! nginx -t; then
        restore
        nginx -t || true
        die "nginx validation failed; restored $backup"
    fi
    if ! systemctl reload nginx; then
        restore
        nginx -t || true
        systemctl reload nginx || true
        die "nginx reload failed; restored $backup"
    fi
fi

echo "configured branded 404 routing in $config (backup: $backup)"
