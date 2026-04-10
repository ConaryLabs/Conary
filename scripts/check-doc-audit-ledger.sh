#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

usage() {
    cat >&2 <<'EOF'
Usage: scripts/check-doc-audit-ledger.sh <ledger-path> <--allow-pending|--require-complete>
EOF
    exit 1
}

is_allowed_family() {
    case "$1" in
        root|template|canonical|deploy|app-local|planning|historical|frontend)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_allowed_disposition() {
    case "$1" in
        ""|verified-no-change|corrected|clarified-as-wip|reframed-as-historical|retained-historical|archived|deleted)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_historical_disposition() {
    case "$1" in
        reframed-as-historical|retained-historical|archived)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

if [[ $# -ne 2 ]]; then
    usage
fi

ledger_path="$1"
mode="$2"
baseline_path="docs/superpowers/documentation-accuracy-audit-inventory.tsv"

case "$mode" in
    --allow-pending|--require-complete)
        ;;
    *)
        usage
        ;;
esac

[[ -f "$ledger_path" ]] || fail "ledger file not found: $ledger_path"
[[ -f "$baseline_path" ]] || fail "baseline inventory file not found: $baseline_path"

current_inventory="$(mktemp)"
trap 'rm -f "$current_inventory"' EXIT

bash scripts/docs-audit-inventory.sh > "$current_inventory"

declare -A baseline_paths=()
declare -A current_paths=()
declare -A ledger_origin_counts=()
declare -A ledger_retained_paths=()

while IFS=$'\t' read -r path family audience; do
    [[ "$path" == "path" ]] && continue
    baseline_paths["$path"]=1
done < "$baseline_path"

while IFS=$'\t' read -r path family audience; do
    [[ "$path" == "path" ]] && continue
    current_paths["$path"]=1
done < "$current_inventory"

header_seen=0
line_no=0
while IFS= read -r line; do
    line_no=$((line_no + 1))
    normalized_line="${line//$'\t'/$'\x1f'}"
    IFS=$'\x1f' read -r origin_path path family audience claim_clusters evidence_sources status disposition notes extra <<< "$normalized_line"

    if [[ $line_no -eq 1 ]]; then
        [[ "$origin_path" == "origin_path" && "$path" == "path" ]] || fail "unexpected ledger header in $ledger_path"
        header_seen=1
        continue
    fi

    [[ -z "${extra:-}" ]] || fail "unexpected extra fields at $ledger_path:$line_no"
    [[ -n "$origin_path" ]] || fail "empty origin_path at $ledger_path:$line_no"

    ledger_origin_counts["$origin_path"]=$(( ${ledger_origin_counts["$origin_path"]:-0} + 1 ))

    [[ -n "${baseline_paths["$origin_path"]:-}" ]] || fail "ledger origin_path is not in baseline inventory at $ledger_path:$line_no: $origin_path"
    is_allowed_family "$family" || fail "invalid family at $ledger_path:$line_no: $family"
    [[ "$status" == "pending" || "$status" == "verified" ]] || fail "invalid status at $ledger_path:$line_no: $status"
    is_allowed_disposition "$disposition" || fail "invalid disposition at $ledger_path:$line_no: $disposition"

    if [[ "$disposition" != "deleted" ]]; then
        [[ -n "$path" ]] || fail "retained row has empty current path at $ledger_path:$line_no"
        [[ -n "${current_paths["$path"]:-}" ]] || fail "retained row references non-tracked current path at $ledger_path:$line_no: $path"
        ledger_retained_paths["$path"]=1
    fi

    if [[ "$mode" == "--require-complete" ]]; then
        [[ "$status" != "pending" ]] || fail "pending row remains in complete mode at $ledger_path:$line_no: $origin_path"
        [[ -n "$disposition" ]] || fail "verified row lacks disposition in complete mode at $ledger_path:$line_no: $origin_path"

        if [[ "$disposition" == "deleted" ]]; then
            [[ -z "$path" ]] || fail "deleted row keeps a current path at $ledger_path:$line_no: $origin_path"
        else
            [[ -n "$claim_clusters" ]] || fail "retained row lacks claim_clusters in complete mode at $ledger_path:$line_no: $origin_path"
            [[ -n "$evidence_sources" ]] || fail "retained row lacks evidence_sources in complete mode at $ledger_path:$line_no: $origin_path"
        fi

        if [[ "$family" == "historical" ]]; then
            is_historical_disposition "$disposition" || fail "historical row lacks historical disposition at $ledger_path:$line_no: $origin_path"
        fi
    fi
done < "$ledger_path"

[[ $header_seen -eq 1 ]] || fail "ledger header missing from $ledger_path"

for baseline_path_row in "${!baseline_paths[@]}"; do
    count="${ledger_origin_counts["$baseline_path_row"]:-0}"
    [[ "$count" -eq 1 ]] || fail "baseline path must appear exactly once in ledger: $baseline_path_row (found $count)"
done

for current_path_row in "${!current_paths[@]}"; do
    [[ -n "${ledger_retained_paths["$current_path_row"]:-}" ]] || fail "current tracked doc path missing from retained ledger paths: $current_path_row"
done

echo "Documentation audit ledger check passed ($mode)."
