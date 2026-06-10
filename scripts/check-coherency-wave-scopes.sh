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
Usage: scripts/check-coherency-wave-scopes.sh <ledger-path> <scope-registry>
EOF
    exit 2
}

[[ $# -eq 2 ]] || usage

ledger_path="$1"
registry_path="$2"

[[ -f "$ledger_path" ]] || fail "ledger file not found: $ledger_path"
[[ -f "$registry_path" ]] || fail "scope registry not found: $registry_path"

expected_header=$'wave_scope\tstatus\tnotes'
declare -A known_scopes=()
declare -a completed_scopes=()

line_no=0
while IFS= read -r line || [[ -n "$line" ]]; do
    line_no=$((line_no + 1))
    if [[ "$line_no" -eq 1 ]]; then
        [[ "$line" == "$expected_header" ]] || fail "unexpected scope registry header in $registry_path"
        continue
    fi

    field_count="$(awk -F '\t' '{print NF}' <<< "$line")"
    [[ "$field_count" -eq 3 ]] || fail "expected 3 fields at $registry_path:$line_no, found $field_count"

    normalized_line="${line//$'\t'/$'\x1f'}"
    IFS=$'\x1f' read -r wave_scope status notes <<< "$normalized_line"
    [[ -n "$wave_scope" ]] || fail "empty wave_scope at $registry_path:$line_no"
    [[ "$wave_scope" =~ ^[a-z0-9][a-z0-9-]*$ ]] || fail "invalid wave_scope at $registry_path:$line_no: $wave_scope"
    [[ -z "${known_scopes["$wave_scope"]:-}" ]] || fail "duplicate wave_scope at $registry_path:$line_no: $wave_scope"
    case "$status" in
        active|completed)
            ;;
        *)
            fail "invalid scope status at $registry_path:$line_no: $status"
            ;;
    esac
    [[ -n "$notes" ]] || fail "empty notes at $registry_path:$line_no"

    known_scopes["$wave_scope"]="$status"
    if [[ "$status" == "completed" ]]; then
        completed_scopes+=("$wave_scope")
    fi
done < "$registry_path"

[[ "${#known_scopes[@]}" -gt 0 ]] || fail "scope registry has no scopes: $registry_path"

ledger_header_seen=0
ledger_line_no=0
while IFS= read -r line || [[ -n "$line" ]]; do
    ledger_line_no=$((ledger_line_no + 1))
    if [[ "$ledger_line_no" -eq 1 ]]; then
        ledger_header_seen=1
        continue
    fi

    field_count="$(awk -F '\t' '{print NF}' <<< "$line")"
    [[ "$field_count" -eq 17 ]] || fail "expected 17 ledger fields at $ledger_path:$ledger_line_no, found $field_count"

    normalized_line="${line//$'\t'/$'\x1f'}"
    IFS=$'\x1f' read -r _id _surface _source _related_ids wave_scope _owner _rest <<< "$normalized_line"
    [[ -n "$wave_scope" ]] || fail "empty wave_scope at $ledger_path:$ledger_line_no"
    [[ -n "${known_scopes["$wave_scope"]:-}" ]] || fail "unregistered wave_scope at $ledger_path:$ledger_line_no: $wave_scope"
done < "$ledger_path"

[[ "$ledger_header_seen" -eq 1 ]] || fail "ledger header missing from $ledger_path"

for wave_scope in "${completed_scopes[@]}"; do
    bash scripts/check-coherency-ledger.sh "$ledger_path" --scope-complete "$wave_scope" >/dev/null
done

echo "Coherency wave scope check passed."
