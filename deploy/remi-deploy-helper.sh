#!/usr/bin/env bash
# deploy/remi-deploy-helper.sh -- Root-owned Remi deployment helper.
set -euo pipefail

PATH=/usr/sbin:/usr/bin:/sbin:/bin

ROOT="${CONARY_REMI_DEPLOY_ROOT:-}"
SKIP_RESTART="${CONARY_REMI_DEPLOY_SKIP_RESTART:-0}"
HEALTH_URL="${CONARY_REMI_DEPLOY_HEALTH_URL:-http://localhost:8081/health}"

die() {
    echo "remi deploy helper: $*" >&2
    exit 1
}

usage() {
    cat >&2 <<'USAGE'
usage:
  conary-remi-deploy deploy-conary <version> <staging-dir>
  conary-remi-deploy deploy-remi <version> <bundle.tar.gz>
  conary-remi-deploy configure-concurrency <max-concurrent>
  conary-remi-deploy verify-access
USAGE
    exit 2
}

root_path() {
    local path="$1"
    if [[ -n "$ROOT" ]]; then
        printf '%s%s' "$ROOT" "$path"
    else
        printf '%s' "$path"
    fi
}

owner_args() {
    if [[ -z "$ROOT" ]]; then
        printf '%s\n' -o conary -g conary
    fi
}

validate_version() {
    local version="$1"
    [[ "$version" =~ ^[0-9A-Za-z._+-]+$ ]] || die "invalid version: $version"
}

validate_positive_int() {
    local value="$1"
    [[ "$value" =~ ^[0-9]+$ ]] || die "expected positive integer, got: $value"
    (( value >= 1 && value <= 128 )) || die "value out of allowed range 1..128: $value"
}

real_tmp_path() {
    local path="$1"
    local resolved
    resolved="$(realpath -e "$path")" || die "missing path: $path"
    [[ "$resolved" == /tmp/* ]] || die "staging path must be under /tmp: $resolved"
    printf '%s' "$resolved"
}

install_owned_dir() {
    local mode="$1"
    shift
    local owners=()
    mapfile -t owners < <(owner_args)
    install -d -m "$mode" "${owners[@]}" "$@"
}

install_owned_file() {
    local mode="$1"
    local src="$2"
    local dest="$3"
    local owners=()
    mapfile -t owners < <(owner_args)
    install -m "$mode" "${owners[@]}" "$src" "$dest"
}

restart_remi() {
    [[ "$SKIP_RESTART" == "1" ]] && return 0
    systemctl restart remi
    sleep 2
    curl -fsS "$HEALTH_URL" >/dev/null
}

deploy_conary() {
    local version="$1"
    local staging
    validate_version "$version"
    staging="$(real_tmp_path "$2")"
    [[ -d "$staging" && ! -L "$staging" ]] || die "staging path is not a plain directory: $staging"

    local conary_root releases_root release_dir self_update_dir
    conary_root="$(root_path /conary)"
    releases_root="$(root_path /conary/releases)"
    release_dir="$(root_path "/conary/releases/${version}")"
    self_update_dir="$(root_path /conary/self-update)"

    install_owned_dir 0750 "$conary_root" "$releases_root" "$release_dir" "$self_update_dir"

    shopt -s nullglob
    local files=("$staging"/*)
    shopt -u nullglob
    (( ${#files[@]} > 0 )) || die "staging directory is empty: $staging"

    local file base
    for file in "${files[@]}"; do
        [[ -f "$file" && ! -L "$file" ]] || die "refusing non-regular release artifact: $file"
        base="$(basename "$file")"
        install_owned_file 0644 "$file" "${release_dir}/${base}"
    done

    local ccs_source=""
    shopt -s nullglob
    for file in "$staging"/*.ccs; do
        [[ -f "$file" && ! -L "$file" ]] || die "refusing non-regular CCS artifact: $file"
        ccs_source="$file"
        break
    done
    shopt -u nullglob

    if [[ -n "$ccs_source" ]]; then
        install_owned_file 0644 "$ccs_source" "${self_update_dir}/conary-${version}.ccs"
        if [[ -f "${ccs_source}.sig" && ! -L "${ccs_source}.sig" ]]; then
            install_owned_file 0644 "${ccs_source}.sig" "${self_update_dir}/conary-${version}.ccs.sig"
        fi
    fi

    (
        cd "$release_dir"
        rm -f SHA256SUMS SHA256SUMS.tmp
        sha256sum -- * > SHA256SUMS.tmp
        mv SHA256SUMS.tmp SHA256SUMS
    )
    if [[ -z "$ROOT" ]]; then
        chown conary:conary "${release_dir}/SHA256SUMS"
    fi

    ln -sfn "$version" "${releases_root}/latest"
    if [[ -z "$ROOT" ]]; then
        chown -h conary:conary "${releases_root}/latest"
    fi

    rm -rf "$staging"
}

deploy_remi() {
    local version="$1"
    local bundle
    validate_version "$version"
    bundle="$(real_tmp_path "$2")"
    [[ -f "$bundle" && ! -L "$bundle" ]] || die "bundle path is not a plain file: $bundle"

    local tmpdir bin candidate backup had_previous
    tmpdir="$(mktemp -d /tmp/remi-install.XXXXXX)"
    backup="${tmpdir}/remi.previous"
    bin="$(root_path /usr/local/bin/remi)"
    had_previous=false
    trap 'rm -rf "$tmpdir"' RETURN

    tar xzf "$bundle" -C "$tmpdir"
    candidate="${tmpdir}/remi-${version}-linux-x64"
    [[ -f "$candidate" && ! -L "$candidate" ]] || die "bundle did not contain remi-${version}-linux-x64"

    if [[ -f "$bin" ]]; then
        cp "$bin" "$backup"
        had_previous=true
    fi

    if [[ "$SKIP_RESTART" != "1" ]]; then
        systemctl stop remi
    fi

    if ! install -m 0755 "$candidate" "$bin"; then
        [[ "$SKIP_RESTART" == "1" ]] || systemctl start remi || true
        die "failed to install Remi binary"
    fi

    if ! restart_remi; then
        if [[ "$had_previous" == true ]]; then
            install -m 0755 "$backup" "$bin" || true
            restart_remi || true
        fi
        die "Remi health check failed after deployment"
    fi
}

configure_concurrency() {
    local value="$1"
    validate_positive_int "$value"

    local config tmp
    config="$(root_path /etc/conary/remi.toml)"
    [[ -f "$config" && ! -L "$config" ]] || die "missing plain Remi config: $config"
    tmp="$(mktemp)"
    awk -v value="$value" '
        BEGIN { in_conversion = 0; wrote = 0; saw_conversion = 0 }
        /^\[[^]]+\][[:space:]]*$/ {
            if (in_conversion && !wrote) {
                print "max_concurrent = " value
                wrote = 1
            }
            in_conversion = ($0 == "[conversion]")
            if (in_conversion) {
                saw_conversion = 1
                wrote = 0
            }
            print
            next
        }
        in_conversion && /^[[:space:]]*max_concurrent[[:space:]]*=/ {
            print "max_concurrent = " value
            wrote = 1
            next
        }
        { print }
        END {
            if (in_conversion && !wrote) {
                print "max_concurrent = " value
            } else if (!saw_conversion) {
                print ""
                print "[conversion]"
                print "max_concurrent = " value
            }
        }
    ' "$config" > "$tmp"
    install -m 0644 "$tmp" "$config"
    rm -f "$tmp"
    restart_remi
}

verify_access() {
    [[ "$(id -u)" == "0" ]] || die "helper must run as root"
    [[ -f "$(root_path /etc/conary/remi.toml)" ]] || die "missing /etc/conary/remi.toml"
    [[ "$SKIP_RESTART" == "1" ]] || systemctl status --no-pager remi >/dev/null
}

case "${1:-}" in
    deploy-conary)
        [[ $# -eq 3 ]] || usage
        deploy_conary "$2" "$3"
        ;;
    deploy-remi)
        [[ $# -eq 3 ]] || usage
        deploy_remi "$2" "$3"
        ;;
    configure-concurrency)
        [[ $# -eq 2 ]] || usage
        configure_concurrency "$2"
        ;;
    verify-access)
        [[ $# -eq 1 ]] || usage
        verify_access
        ;;
    *)
        usage
        ;;
esac
