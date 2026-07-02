#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/maintainability-drift-report.sh [--base <ref>] [--limit <n>] [--all]

Print a warn-only maintainability report.

Options:
  --base <ref>   Compare changed paths against this ref. Defaults to HEAD.
  --limit <n>    Number of Rust hotspot rows to show. Defaults to 15.
  --all          Report over all tracked files instead of changed paths.
  -h, --help     Show this help.
EOF
}

base_ref="HEAD"
limit="15"
scan_all=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            base_ref="$2"
            shift 2
            ;;
        --limit)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            limit="$2"
            shift 2
            ;;
        --all)
            scan_all=1
            shift
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

case "$limit" in
    ''|*[!0-9]*)
        usage
        exit 2
        ;;
esac

if (( 10#$limit == 0 )); then
    usage
    exit 2
fi

if [[ "$scan_all" -eq 0 ]] && ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
    echo "ERROR: base ref not found: $base_ref" >&2
    exit 2
fi

section() {
    printf '\n## %s\n' "$1"
}

warn() {
    printf '[warn] %s\n' "$1"
}

printf '# Maintainability Drift Report\n'
printf 'base_ref: %s\n' "$base_ref"
printf 'mode: %s\n' "$([[ "$scan_all" -eq 1 ]] && printf all || printf changed)"

section "Docs Audit Health"
ledger_out="$(mktemp)"
inventory_out="$(mktemp)"
trap 'rm -f "$ledger_out" "$inventory_out"' EXIT

if bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete >"$ledger_out" 2>&1; then
    printf '[ok] docs-audit ledger complete\n'
else
    warn "docs-audit ledger check reported an issue"
    sed 's/^/  /' "$ledger_out"
fi

if bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv - >"$inventory_out" 2>&1; then
    printf '[ok] docs-audit inventory matches regenerated output\n'
else
    warn "docs-audit inventory differs from regenerated output"
    sed 's/^/  /' "$inventory_out"
fi

section "Changed Path Hints"
if [[ "$scan_all" -eq 1 ]]; then
    bash scripts/agent-context.sh --changed --all
else
    bash scripts/agent-context.sh --changed --base "$base_ref"
fi

section "Rust Hotspots"
if [[ -x scripts/line-count-report.sh ]]; then
    scripts/line-count-report.sh "$limit"
else
    warn "scripts/line-count-report.sh is missing or not executable"
fi

section "Reminder"
printf 'This report is warn-only. Follow the focused proof and interaction gate from docs/modules/feature-ownership.md when the touched behavior crosses a neighbor system.\n'
