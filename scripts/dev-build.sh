#!/usr/bin/env bash
# scripts/dev-build.sh -- Shared compiler-cache environment with isolated Cargo targets.
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage:
  scripts/dev-build.sh [--cache auto|off|required] run -- <command> [args...]
  scripts/dev-build.sh [--cache auto|off|required] cargo -- <cargo args...>
  scripts/dev-build.sh [--cache auto|off|required] status
  scripts/dev-build.sh [--cache auto|off|required] stop
  scripts/dev-build.sh [--cache auto|off|required] clean --yes

Environment precedence:
  RUSTC_WRAPPER          Existing value, including empty, is never replaced.
  CARGO_TARGET_DIR       Existing value is never replaced or cleaned.
  CONARY_COMPILER_CACHE  auto (default), off, or required; overridden by --cache.
  CONARY_SCCACHE         Optional sccache executable override for discovery.
  SCCACHE_DIR            Optional cache directory override.
  SCCACHE_CACHE_SIZE     Optional cache bound; defaults to 10G when selected.
USAGE
}

fail() {
    echo "dev-build: $*" >&2
    exit 1
}

cache_mode="${CONARY_COMPILER_CACHE:-auto}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --cache)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            cache_mode="$2"
            shift 2
            ;;
        --cache=*)
            cache_mode="${1#--cache=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            break
            ;;
    esac
done

case "$cache_mode" in
    auto|off|required) ;;
    *) fail "invalid compiler-cache mode: $cache_mode (expected auto, off, or required)" ;;
esac

[[ $# -ge 1 ]] || { usage; exit 2; }
action="$1"
shift

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" ||
    fail "run this command inside a Git worktree"
common_git_dir="$(git rev-parse --git-common-dir)"
if [[ "$common_git_dir" != /* ]]; then
    common_git_dir="${repo_root}/${common_git_dir}"
fi
common_git_dir="$(cd "$common_git_dir" && pwd -P)"
default_cache_dir="${common_git_dir}/conary-dev/sccache"
cache_dir="${SCCACHE_DIR:-$default_cache_dir}"
cache_size="${SCCACHE_CACHE_SIZE:-10G}"
cache_marker=".conary-sccache-cache-v1"

resolve_sccache() {
    local candidate
    if [[ ${CONARY_SCCACHE+x} ]]; then
        candidate="$CONARY_SCCACHE"
    else
        candidate="$(command -v sccache 2>/dev/null || true)"
    fi
    [[ -n "$candidate" ]] || return 1
    if [[ "$candidate" == */* ]]; then
        [[ -x "$candidate" ]] || return 1
        printf '%s\n' "$candidate"
        return
    fi
    command -v "$candidate" 2>/dev/null
}

ensure_cache_dir() {
    if [[ -e "$cache_dir" || -L "$cache_dir" ]]; then
        [[ -d "$cache_dir" && ! -L "$cache_dir" ]] ||
            fail "compiler cache is not a plain directory: $cache_dir"
    else
        mkdir -p -- "$cache_dir"
    fi
    local marker="${cache_dir}/${cache_marker}"
    if [[ -e "$marker" || -L "$marker" ]]; then
        [[ -f "$marker" && ! -L "$marker" ]] ||
            fail "compiler cache marker is not a plain file: $marker"
    else
        install -m 0600 /dev/null "$marker"
    fi
}

target_dir_display() {
    if [[ ${CARGO_TARGET_DIR+x} ]]; then
        if [[ -z "$CARGO_TARGET_DIR" ]]; then
            printf '<caller-empty>\n'
        elif [[ "$CARGO_TARGET_DIR" == /* ]]; then
            printf '%s\n' "$CARGO_TARGET_DIR"
        else
            realpath -m -- "${PWD}/${CARGO_TARGET_DIR}"
        fi
    else
        printf '%s/target\n' "$repo_root"
    fi
}

selected_mode=""
selected_wrapper=""
configure_run_environment() {
    local sccache_bin=""
    if [[ ${RUSTC_WRAPPER+x} ]]; then
        if [[ -n "$RUSTC_WRAPPER" ]]; then
            selected_mode=caller-wrapper
            selected_wrapper="$RUSTC_WRAPPER"
        else
            selected_mode=caller-disabled
            selected_wrapper="<empty>"
        fi
        return
    fi

    if [[ "$cache_mode" == "off" ]]; then
        selected_mode=disabled
        selected_wrapper="<unset>"
        return
    fi

    sccache_bin="$(resolve_sccache || true)"
    if [[ -z "$sccache_bin" ]]; then
        if [[ "$cache_mode" == "required" ]]; then
            fail "compiler cache is required but sccache is unavailable"
        fi
        selected_mode=unavailable
        selected_wrapper="<unset>"
        return
    fi

    ensure_cache_dir
    export RUSTC_WRAPPER="$sccache_bin"
    export SCCACHE_DIR="$cache_dir"
    export SCCACHE_CACHE_SIZE="$cache_size"
    selected_mode=sccache
    selected_wrapper="$sccache_bin"
}

report_run_environment() {
    local cache_display="<caller/default>"
    if [[ "$selected_mode" == "sccache" ]]; then
        cache_display="$SCCACHE_DIR"
    elif [[ ${SCCACHE_DIR+x} ]]; then
        cache_display="$SCCACHE_DIR"
    fi
    printf 'dev-build: compiler-cache=%s wrapper=%s cache-dir=%s target-dir=%s\n' \
        "$selected_mode" "$selected_wrapper" "$cache_display" \
        "$(target_dir_display)" >&2
}

sccache_bin=""
configure_cache_command() {
    sccache_bin="$(resolve_sccache || true)"
    export SCCACHE_DIR="$cache_dir"
    export SCCACHE_CACHE_SIZE="$cache_size"
}

case "$action" in
    run)
        [[ "${1:-}" == "--" ]] && shift
        [[ $# -ge 1 ]] || { usage; exit 2; }
        configure_run_environment
        report_run_environment
        exec "$@"
        ;;
    cargo)
        [[ "${1:-}" == "--" ]] && shift
        [[ $# -ge 1 ]] || { usage; exit 2; }
        configure_run_environment
        report_run_environment
        exec cargo "$@"
        ;;
    status)
        [[ $# -eq 0 ]] || { usage; exit 2; }
        configure_cache_command
        if [[ -n "$sccache_bin" ]]; then
            printf 'dev-build: compiler-cache=sccache cache-dir=%s cache-size=%s target-cleanup=never\n' \
                "$SCCACHE_DIR" "$SCCACHE_CACHE_SIZE"
            "$sccache_bin" --show-stats
        else
            printf 'dev-build: compiler-cache=unavailable cache-dir=%s cache-size=%s target-cleanup=never\n' \
                "$SCCACHE_DIR" "$SCCACHE_CACHE_SIZE"
            printf 'sccache status: unavailable\n'
        fi
        if [[ -d "$SCCACHE_DIR" && ! -L "$SCCACHE_DIR" ]]; then
            printf 'Cache disk bytes: %s\n' \
                "$(du --block-size=1 --summarize -- "$SCCACHE_DIR" | cut -f1)"
        else
            printf 'Cache disk bytes: 0\n'
        fi
        ;;
    stop)
        [[ $# -eq 0 ]] || { usage; exit 2; }
        configure_cache_command
        [[ -n "$sccache_bin" ]] || fail "sccache is unavailable"
        "$sccache_bin" --stop-server
        ;;
    clean)
        [[ "${1:-}" == "--yes" && $# -eq 1 ]] ||
            fail "cache cleanup requires the exact argument: clean --yes"
        configure_cache_command
        if [[ ! -e "$SCCACHE_DIR" && ! -L "$SCCACHE_DIR" ]]; then
            printf 'dev-build: compiler cache is already absent: %s\n' "$SCCACHE_DIR"
            exit 0
        fi
        [[ -d "$SCCACHE_DIR" && ! -L "$SCCACHE_DIR" ]] ||
            fail "compiler cache is not a plain directory: $SCCACHE_DIR"
        resolved_cache_dir="$(cd "$SCCACHE_DIR" && pwd -P)"
        case "$resolved_cache_dir" in
            /|"$repo_root"|"$common_git_dir"|"${HOME:-/nonexistent-home}")
                fail "refusing broad compiler-cache cleanup target: $resolved_cache_dir"
                ;;
        esac
        marker="${resolved_cache_dir}/${cache_marker}"
        [[ -f "$marker" && ! -L "$marker" ]] ||
            fail "refusing unmarked compiler-cache cleanup: $resolved_cache_dir"
        if [[ -n "$sccache_bin" ]]; then
            "$sccache_bin" --stop-server >/dev/null 2>&1 || true
        fi
        find "$resolved_cache_dir" -mindepth 1 -maxdepth 1 \
            ! -name "$cache_marker" -exec rm -rf -- {} +
        printf 'dev-build: cleared compiler cache; retained marker: %s\n' "$resolved_cache_dir"
        ;;
    *)
        usage
        exit 2
        ;;
esac
