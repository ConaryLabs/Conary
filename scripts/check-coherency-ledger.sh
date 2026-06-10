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
Usage: scripts/check-coherency-ledger.sh <ledger-path> [--scope-complete <wave-scope>]
EOF
    exit 2
}

if [[ $# -ne 1 && $# -ne 3 ]]; then
    usage
fi

ledger_path="$1"
scope_complete=""

if [[ $# -eq 3 ]]; then
    [[ "$2" == "--scope-complete" ]] || usage
    scope_complete="$3"
    [[ -n "$scope_complete" ]] || usage
fi

[[ -f "$ledger_path" ]] || fail "ledger file not found: $ledger_path"

expected_header=$'id\tsurface\tsource\trelated_ids\twave_scope\towner\tclaim\tactual_or_gap\tstatus\tdisposition\tlast_verified\tevidence_sources\trepro\tverification\tdecision\tnext_slice\tnotes'

declare -A allowed_owners=()
while IFS= read -r owner_heading; do
    case "$owner_heading" in
        "How To Use This Map"|"Card Schema")
            continue
            ;;
    esac
    allowed_owners["$owner_heading"]=1
done < <(sed -n 's/^## //p' docs/modules/feature-ownership.md)

is_allowed_status() {
    case "$1" in
        works|works-but-thin|fix-now|honest-deferred|misleading|duplicate-stale)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_allowed_disposition() {
    case "$1" in
        open|verified-no-change|resolved-repaired|resolved-removed|resolved-merged|deferred-owned)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

is_allowed_decision() {
    case "$1" in
        fix|defer|remove|merge|harden|verify)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

matrix_allows() {
    local status="$1"
    local disposition="$2"

    case "$status:$disposition" in
        works:verified-no-change|works:resolved-repaired)
            return 0
            ;;
        works-but-thin:open|works-but-thin:resolved-repaired|works-but-thin:verified-no-change|works-but-thin:deferred-owned)
            return 0
            ;;
        fix-now:open|fix-now:resolved-repaired|fix-now:resolved-removed|fix-now:resolved-merged)
            return 0
            ;;
        misleading:open|misleading:resolved-repaired|misleading:resolved-removed|misleading:resolved-merged)
            return 0
            ;;
        duplicate-stale:open|duplicate-stale:resolved-merged|duplicate-stale:resolved-removed|duplicate-stale:resolved-repaired)
            return 0
            ;;
        honest-deferred:open|honest-deferred:deferred-owned)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

decision_allows() {
    local status="$1"
    local disposition="$2"
    local decision="$3"

    case "$status:$disposition:$decision" in
        works:verified-no-change:verify|works:resolved-repaired:fix|works:resolved-repaired:harden)
            return 0
            ;;
        works-but-thin:open:harden|works-but-thin:open:verify|works-but-thin:resolved-repaired:fix|works-but-thin:resolved-repaired:harden|works-but-thin:verified-no-change:verify|works-but-thin:deferred-owned:defer)
            return 0
            ;;
        fix-now:open:fix|fix-now:resolved-repaired:fix|fix-now:resolved-removed:remove|fix-now:resolved-merged:merge)
            return 0
            ;;
        misleading:open:fix|misleading:open:remove|misleading:resolved-repaired:fix|misleading:resolved-removed:remove|misleading:resolved-merged:merge)
            return 0
            ;;
        duplicate-stale:open:merge|duplicate-stale:open:remove|duplicate-stale:resolved-merged:merge|duplicate-stale:resolved-removed:remove|duplicate-stale:resolved-repaired:fix)
            return 0
            ;;
        honest-deferred:open:defer|honest-deferred:deferred-owned:defer)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

path_from_pointer() {
    local pointer="$1"
    case "$pointer" in
        path:*)
            pointer="${pointer#path:}"
            ;;
        doc:*)
            pointer="${pointer#doc:}"
            ;;
        *)
            return 1
            ;;
    esac
    if [[ "$pointer" =~ ^(.+):[0-9]+(-[0-9]+)?$ ]]; then
        pointer="${BASH_REMATCH[1]}"
    fi
    printf '%s\n' "$pointer"
}

validate_pointer() {
    local pointer="$1"
    local line_no="$2"
    local field_name="${3:-evidence}"
    local ptr_path

    [[ -n "$pointer" ]] || fail "empty $field_name pointer at $ledger_path:$line_no"

    case "$pointer" in
        path:*|doc:*)
            ptr_path="$(path_from_pointer "$pointer")"
            if [[ "$ptr_path" == *.md || "$ptr_path" == *.mdx ]]; then
                [[ "$pointer" == doc:* ]] || fail "Markdown pointer must use doc: at $ledger_path:$line_no: $pointer"
            fi
            [[ -e "$ptr_path" ]] || fail "referenced path does not exist at $ledger_path:$line_no: $pointer"
            ;;
        cmd:*)
            # Syntax-only: execution is proven by the explicit verification commands.
            [[ "${pointer#cmd:}" == *[![:space:]]* ]] || fail "empty $field_name command pointer at $ledger_path:$line_no"
            ;;
        test:*)
            # Syntax-only: execution is proven by the explicit verification commands.
            [[ "${pointer#test:}" == *[![:space:]]* ]] || fail "empty $field_name test pointer at $ledger_path:$line_no"
            ;;
        route:*)
            [[ "$pointer" =~ ^route:(GET|POST|PUT|PATCH|DELETE)[[:space:]]/ ]] \
                || fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
        mcp:*)
            [[ "$pointer" =~ ^mcp:[^/[:space:]]+/[^/[:space:]]+$ ]] \
                || fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
        *)
            fail "invalid typed $field_name pointer at $ledger_path:$line_no: $pointer"
            ;;
    esac
}

validate_pointer_list() {
    local value="$1"
    local line_no="$2"
    local field_name="$3"

    IFS=';' read -ra pointers <<< "$value"
    for pointer in "${pointers[@]}"; do
        validate_pointer "$pointer" "$line_no" "$field_name"
    done
}

validate_optional_pointer_list() {
    local value="$1"
    local line_no="$2"
    local field_name="$3"

    [[ "$value" == "none" ]] && return 0
    validate_pointer_list "$value" "$line_no" "$field_name"
}

validate_related_id_shape() {
    local related_id="$1"
    local line_no="$2"
    [[ "$related_id" =~ ^(CLI|DOC|ROUTE|MCP|AGENT|OPS)-[A-Z0-9][A-Z0-9-]*-[0-9]{3}$ ]] \
        || fail "invalid related_id at $ledger_path:$line_no: $related_id"
}

validate_date() {
    local value="$1"
    local line_no="$2"
    local normalized
    [[ "$value" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] \
        || fail "invalid last_verified date at $ledger_path:$line_no: $value"
    normalized="$(date -d "$value" +%F 2>/dev/null)" \
        || fail "invalid last_verified date at $ledger_path:$line_no: $value"
    [[ "$normalized" == "$value" ]] \
        || fail "invalid last_verified date at $ledger_path:$line_no: $value"
}

maybe_warn_stale() {
    local status="$1"
    local last_verified="$2"
    local id="$3"

    case "$status" in
        works|works-but-thin|honest-deferred)
            ;;
        *)
            return 0
            ;;
    esac

    if command -v date >/dev/null 2>&1; then
        local verified_epoch now_epoch age_days
        verified_epoch="$(date -d "$last_verified" +%s 2>/dev/null || true)"
        now_epoch="$(date +%s)"
        if [[ -n "$verified_epoch" ]]; then
            age_days=$(( (now_epoch - verified_epoch) / 86400 ))
            if (( age_days > 90 )); then
                echo "WARN: stale verification date for $id: $last_verified ($age_days days)" >&2
            fi
        fi
    fi
}

declare -A seen_ids=()
declare -a related_refs=()
line_no=0
header_seen=0
data_row_count=0
scope_row_count=0

while IFS= read -r line || [[ -n "$line" ]]; do
    line_no=$((line_no + 1))

    if [[ "$line_no" -eq 1 ]]; then
        [[ "$line" == "$expected_header" ]] || fail "unexpected ledger header in $ledger_path"
        header_seen=1
        continue
    fi
    data_row_count=$((data_row_count + 1))

    field_count="$(awk -F '\t' '{print NF}' <<< "$line")"
    [[ "$field_count" -eq 17 ]] || fail "expected 17 fields at $ledger_path:$line_no, found $field_count"

    normalized_line="${line//$'\t'/$'\x1f'}"
    IFS=$'\x1f' read -r id surface source related_ids wave_scope owner claim actual_or_gap status disposition last_verified evidence_sources repro verification decision next_slice notes extra <<< "$normalized_line"

    [[ -z "${extra:-}" ]] || fail "unexpected extra fields at $ledger_path:$line_no"
    [[ "$id" =~ ^(CLI|DOC|ROUTE|MCP|AGENT|OPS)-[A-Z0-9][A-Z0-9-]*-[0-9]{3}$ ]] \
        || fail "invalid id at $ledger_path:$line_no: $id"
    [[ -z "${seen_ids["$id"]:-}" ]] || fail "duplicate id at $ledger_path:$line_no: $id"
    seen_ids["$id"]=1

    if [[ -n "$related_ids" ]]; then
        IFS=';' read -ra related_id_values <<< "$related_ids"
        for related_id in "${related_id_values[@]}"; do
            validate_related_id_shape "$related_id" "$line_no"
            related_refs+=("$id|$related_id|$line_no")
        done
    fi

    [[ -n "$surface" ]] || fail "empty surface at $ledger_path:$line_no"
    [[ -n "$source" ]] || fail "empty source at $ledger_path:$line_no"
    [[ -n "$wave_scope" ]] || fail "empty wave_scope at $ledger_path:$line_no"
    [[ -n "$owner" ]] || fail "empty owner at $ledger_path:$line_no"
    [[ -n "${allowed_owners["$owner"]:-}" ]] || fail "owner does not match a feature ownership card at $ledger_path:$line_no: $owner"
    [[ -n "$claim" ]] || fail "empty claim at $ledger_path:$line_no"
    [[ -n "$status" ]] || fail "empty status at $ledger_path:$line_no"
    [[ -n "$disposition" ]] || fail "empty disposition at $ledger_path:$line_no"
    [[ -n "$last_verified" ]] || fail "empty last_verified at $ledger_path:$line_no"
    [[ -n "$evidence_sources" ]] || fail "empty evidence_sources at $ledger_path:$line_no"
    [[ -n "$repro" ]] || fail "empty repro at $ledger_path:$line_no"
    [[ -n "$verification" ]] || fail "empty verification at $ledger_path:$line_no"
    [[ -n "$decision" ]] || fail "empty decision at $ledger_path:$line_no"
    [[ -n "$next_slice" ]] || fail "empty next_slice at $ledger_path:$line_no"

    is_allowed_status "$status" || fail "invalid status at $ledger_path:$line_no: $status"
    is_allowed_disposition "$disposition" || fail "invalid disposition at $ledger_path:$line_no: $disposition"
    is_allowed_decision "$decision" || fail "invalid decision at $ledger_path:$line_no: $decision"
    matrix_allows "$status" "$disposition" || fail "invalid disposition for status at $ledger_path:$line_no: $status/$disposition"
    decision_allows "$status" "$disposition" "$decision" || fail "invalid decision for status/disposition at $ledger_path:$line_no: $status/$disposition/$decision"
    validate_date "$last_verified" "$line_no"
    validate_pointer_list "$source" "$line_no" "source"
    validate_pointer_list "$evidence_sources" "$line_no" "evidence"
    validate_optional_pointer_list "$repro" "$line_no" "repro"
    validate_pointer_list "$verification" "$line_no" "verification"

    case "$status" in
        fix-now|misleading|duplicate-stale|works-but-thin)
            [[ -n "$actual_or_gap" ]] || fail "actual_or_gap is required at $ledger_path:$line_no for status $status"
            ;;
    esac

    if [[ "$disposition" == "open" ]]; then
        [[ -n "$owner" && -n "$decision" && -n "$next_slice" && -n "$verification" && -n "$last_verified" ]] \
            || fail "open row lacks required active-wave fields at $ledger_path:$line_no"
    fi

    if [[ -n "$scope_complete" && "$wave_scope" == "$scope_complete" ]]; then
        scope_row_count=$((scope_row_count + 1))
        if [[ "$disposition" == "open" ]]; then
            fail "scope completion blocked by open $status row at $ledger_path:$line_no: $id"
        fi
    fi

    maybe_warn_stale "$status" "$last_verified" "$id"
done < "$ledger_path"

[[ "$header_seen" -eq 1 ]] || fail "ledger header missing from $ledger_path"
[[ "$data_row_count" -gt 0 ]] || fail "ledger has no rows: $ledger_path"

for ref in "${related_refs[@]}"; do
    IFS='|' read -r referrer related_id ref_line <<< "$ref"
    [[ -n "${seen_ids["$related_id"]:-}" ]] \
        || fail "dangling related_id at $ledger_path:$ref_line: $referrer references $related_id"
done

if [[ -n "$scope_complete" && "$scope_row_count" -eq 0 ]]; then
    fail "scope completion found no rows for scope: $scope_complete"
fi

echo "Coherency ledger check passed."
