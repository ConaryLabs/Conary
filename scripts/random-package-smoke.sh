#!/usr/bin/env bash
# scripts/random-package-smoke.sh -- Try seeded random package installs across dep modes.

set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: scripts/random-package-smoke.sh [options]

Options:
  --count N          Number of packages to choose when --package is not used (default: 3)
  --seed TEXT        Deterministic selection seed (default: current date)
  --modes LIST       Comma or space separated dep modes (default: satisfy,adopt,takeover)
  --package NAME     Add an explicit package. Repeatable; disables random selection.
  --plan-only        Print selected commands without running them.
  --apply            Run real installs. Default is --dry-run.
  --help             Show this help.

Environment:
  CONARY_BIN         Conary binary to run.
  PACKAGE_CANDIDATES Newline or space separated candidate package names.
  REMI_ENDPOINT      Remi endpoint (default: https://remi.conary.io).
  REMI_DISTRO        Exact public profile (default: fedora-44).
  REPO_NAME          Repository name to use (default: remi from system init).
  DB_PATH            Reuse a DB path instead of a temp DB.
  ROOT               Install root for --apply runs; temp root by default.
  OUT_DIR            Directory for command logs.
  KEEP_TMP=1         Keep temp working directory after the run.
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

count="${COUNT:-3}"
seed="${SEED:-$(date +%Y-%m-%d)}"
modes_text="${MODES:-satisfy,adopt,takeover}"
plan_only=0
dry_run=1
explicit_packages=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --count)
            count="$2"
            shift 2
            ;;
        --seed)
            seed="$2"
            shift 2
            ;;
        --modes)
            modes_text="$2"
            shift 2
            ;;
        --package)
            explicit_packages+=("$2")
            shift 2
            ;;
        --plan-only)
            plan_only=1
            shift
            ;;
        --apply)
            dry_run=0
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ! [[ "$count" =~ ^[0-9]+$ ]] || [[ "$count" -lt 1 ]]; then
    echo "--count must be a positive integer" >&2
    exit 2
fi

default_candidates=$'htop\nbtop\njq\ntree\nripgrep\nnano\npatch\nstrace\nfile\nwhich\nless\ntmux\nrsync\nwget\ncurl\nsqlite\nzstd\nxz\nunzip\ncpio'
candidate_text="${PACKAGE_CANDIDATES:-$default_candidates}"

mapfile -t modes < <(
    printf '%s\n' "$modes_text" | tr ',' ' ' | tr ' ' '\n' | sed '/^$/d'
)

if [[ "${#modes[@]}" -eq 0 ]]; then
    echo "no dep modes selected" >&2
    exit 2
fi

for mode in "${modes[@]}"; do
    case "$mode" in
        satisfy | adopt | takeover) ;;
        *)
            echo "unsupported dep mode: $mode" >&2
            exit 2
            ;;
    esac
done

if [[ "${#explicit_packages[@]}" -gt 0 ]]; then
    selected_packages=("${explicit_packages[@]}")
else
    mapfile -t candidates < <(
        printf '%s\n' "$candidate_text" | tr ' ' '\n' | sed '/^$/d' | sort -u
    )
    if [[ "${#candidates[@]}" -eq 0 ]]; then
        echo "no package candidates available" >&2
        exit 2
    fi
    mapfile -t selected_packages < <(
        for pkg in "${candidates[@]}"; do
            score="$(printf '%s' "${seed}:${pkg}" | cksum | awk '{print $1}')"
            printf '%010u\t%s\n' "$score" "$pkg"
        done | sort -n -k1,1 | awk -v count="$count" 'NR <= count {print $2}'
    )
fi

echo "Selected packages: ${selected_packages[*]}"
echo "Modes: ${modes[*]}"
echo "Seed: $seed"

default_conary_bin() {
    if [[ -x "$repo_root/target/debug/conary" ]]; then
        printf '%s\n' "$repo_root/target/debug/conary"
    elif [[ -x /conary/dev/cache/target/Conary/debug/conary ]]; then
        printf '%s\n' /conary/dev/cache/target/Conary/debug/conary
    else
        printf '%s\n' "$repo_root/target/debug/conary"
    fi
}

conary_bin="${CONARY_BIN:-$(default_conary_bin)}"
remi_endpoint="${REMI_ENDPOINT:-https://remi.conary.io}"
remi_distro="${REMI_DISTRO:-fedora-44}"
repo_name="${REPO_NAME:-remi}"

print_command() {
    local first=1
    for arg in "$@"; do
        if [[ "$first" -eq 0 ]]; then
            printf ' '
        fi
        first=0
        printf '%q' "$arg"
    done
}

build_install_command() {
    local pkg="$1"
    local mode="$2"
    local db_path="$3"
    local root="$4"
    local cmd=(
        "$conary_bin"
        install
        "$pkg"
        --repo "$repo_name"
        --dep-mode "$mode"
        --yes
        --allow-capabilities
        --sandbox never
        --db-path "$db_path"
        --root "$root"
    )
    if [[ "$dry_run" -eq 1 ]]; then
        cmd+=(--dry-run)
    fi
    print_command "${cmd[@]}"
}

if [[ "$plan_only" -eq 1 ]]; then
    db_path="${DB_PATH:-/tmp/conary-random-smoke/conary.db}"
    root="${ROOT:-/tmp/conary-random-smoke/root}"
    for pkg in "${selected_packages[@]}"; do
        for mode in "${modes[@]}"; do
            printf 'PLAN package=%s mode=%s command=' "$pkg" "$mode"
            build_install_command "$pkg" "$mode" "$db_path" "$root"
            printf '\n'
        done
    done
    exit 0
fi

if [[ ! -x "$conary_bin" ]]; then
    echo "Conary binary not executable: $conary_bin" >&2
    echo "Build it with: cargo build -p conary" >&2
    exit 2
fi

tmp_dir="$(mktemp -d /tmp/conary-random-smoke.XXXXXX)"
cleanup() {
    if [[ "${KEEP_TMP:-0}" == "1" ]]; then
        echo "Keeping temp dir: $tmp_dir"
    else
        rm -rf "$tmp_dir"
    fi
}
trap cleanup EXIT

db_path="${DB_PATH:-$tmp_dir/conary.db}"
root="${ROOT:-$tmp_dir/root}"
out_dir="${OUT_DIR:-$tmp_dir/results}"
mkdir -p "$root" "$out_dir"

setup_log="$out_dir/setup.log"
run_setup() {
    "$conary_bin" system init --profile "$remi_distro" --db-path "$db_path" || return
    if [[ "$repo_name" != "remi" ]]; then
        "$conary_bin" repo add "$repo_name" "$remi_endpoint" \
            --default-strategy remi \
            --remi-endpoint "$remi_endpoint" \
            --remi-distro "$remi_distro" \
            --no-gpg-check \
            --replace \
            --yes \
            --db-path "$db_path" || return
    fi
    "$conary_bin" repo sync "$repo_name" --force --db-path "$db_path" || return
}

if ! run_setup >"$setup_log" 2>&1; then
    echo "Setup failed; log=$setup_log" >&2
    sed -n '1,80p' "$setup_log" >&2
    echo "--- setup log tail ---" >&2
    tail -80 "$setup_log" >&2
    exit 1
fi

failures=0
total=0

for pkg in "${selected_packages[@]}"; do
    for mode in "${modes[@]}"; do
        total=$((total + 1))
        log_name="${pkg//[^A-Za-z0-9_.-]/_}-${mode}.log"
        log_path="$out_dir/$log_name"
        cmd_text="$(build_install_command "$pkg" "$mode" "$db_path" "$root")"
        printf 'RUN package=%s mode=%s command=%s\n' "$pkg" "$mode" "$cmd_text"
        if bash -c "$cmd_text" >"$log_path" 2>&1; then
            printf 'OK package=%s mode=%s log=%s\n' "$pkg" "$mode" "$log_path"
        else
            failures=$((failures + 1))
            printf 'FAIL package=%s mode=%s log=%s\n' "$pkg" "$mode" "$log_path"
        fi
    done
done

printf 'Summary: total=%s failures=%s logs=%s\n' "$total" "$failures" "$out_dir"

if [[ "$failures" -gt 0 ]]; then
    exit 1
fi
