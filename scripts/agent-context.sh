#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/agent-context.sh <mode> [options]

Print feature-card task context from docs/modules/feature-ownership.md.

Modes (exactly one):
  --feature <slug>        Print the task packet for one card.
  --path <path>           Route one repo path to its owning card; print packet.
  --changed               Route all changed paths; print brief hints per path.
  --list                  Print slug + capability summary for all cards.
  --validate              Validate the map schema; non-zero exit on violation.

Options:
  --base <ref>            With --changed: diff base. Defaults to HEAD.
  --all                   With --changed: route all tracked files instead of
                          changed, cached, and untracked paths.
  --brief                 With --feature/--path: one-line summary instead of
                          full packet (drift-report format).
  --run <focused|gate>    With --feature: execute the extracted proof
                          commands sequentially, fail-fast, echoing each.
  --map <path>            Map file override (for tests). Defaults to
                          docs/modules/feature-ownership.md.
  -h, --help              Show this help.
EOF
}

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

mode=""
feature_slug=""
route_path_arg=""
base_ref="HEAD"
base_ref_set=0
scan_all=0
brief=0
run_kind=""
map_file="docs/modules/feature-ownership.md"

set_mode() {
    if [[ -n "$mode" ]]; then
        usage
        exit 2
    fi
    mode="$1"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --feature)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode feature
            feature_slug="$2"
            shift 2
            ;;
        --path)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode path
            route_path_arg="$2"
            shift 2
            ;;
        --changed)
            set_mode changed
            shift
            ;;
        --list)
            set_mode list
            shift
            ;;
        --validate)
            set_mode validate
            shift
            ;;
        --base)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            base_ref="$2"
            base_ref_set=1
            shift 2
            ;;
        --all)
            scan_all=1
            shift
            ;;
        --brief)
            brief=1
            shift
            ;;
        --run)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            run_kind="$2"
            shift 2
            ;;
        --map)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            map_file="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$mode" ]]; then
    usage
    exit 2
fi

if [[ "$scan_all" -eq 1 || "$base_ref_set" -eq 1 ]]; then
    if [[ "$mode" != "changed" ]]; then
        usage
        exit 2
    fi
fi

if [[ "$brief" -eq 1 ]]; then
    case "$mode" in
        feature|path) ;;
        *)
            usage
            exit 2
            ;;
    esac
fi

if [[ -n "$run_kind" ]]; then
    if [[ "$mode" != "feature" || "$brief" -eq 1 ]]; then
        usage
        exit 2
    fi
    case "$run_kind" in
        focused|gate) ;;
        *)
            fail "invalid --run kind: $run_kind (expected focused or gate)"
            ;;
    esac
fi

[[ -f "$map_file" ]] || fail "map file not found: $map_file"

case "$mode" in
    list|feature|path|changed|validate)
        fail "mode not implemented yet: $mode"
        ;;
esac
