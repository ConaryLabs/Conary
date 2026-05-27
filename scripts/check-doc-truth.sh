#!/usr/bin/env bash
set -euo pipefail

repo_root="${DOCS_TRUTH_ROOT:-}"
if [[ -z "$repo_root" ]]; then
    repo_root="$(git rev-parse --show-toplevel)"
fi
cd "$repo_root"

errors=0

if ! command -v rg >/dev/null 2>&1; then
    echo "ERROR: ripgrep (rg) is required for docs truth checks" >&2
    exit 1
fi

DOCS_TRUTH_SCHEMA_CHECK_PATHS=(
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "site/src/routes"
)

PRODUCT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "CHANGELOG.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
    "site/src/routes"
)

POLICYKIT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
)

PARSER_PATHS=(
    "apps/conary/src/cli"
    "apps/conary/src/dispatch.rs"
    "apps/conary/src/command_risk.rs"
)

report_error() {
    echo "ERROR: $*" >&2
    errors=1
}

existing_paths() {
    local path
    for path in "$@"; do
        if [[ -e "$path" ]]; then
            printf '%s\n' "$path"
        fi
    done
}

require_file() {
    local path="$1"
    if [[ ! -f "$path" ]]; then
        report_error "required file is missing: $path"
        return 1
    fi
}

require_match() {
    local path="$1"
    local pattern="$2"
    local description="$3"

    if [[ ! -e "$path" ]]; then
        report_error "$path: missing while checking $description"
        return
    fi

    if ! rg -q -- "$pattern" "$path"; then
        report_error "$path: missing $description"
    fi
}

check_schema_versions() {
    local schema_file="crates/conary-core/src/db/schema.rs"
    require_file "$schema_file" || return

    local schema_version
    schema_version="$(sed -nE 's/^pub const SCHEMA_VERSION: i32 = ([0-9]+);/\1/p' "$schema_file")"
    if [[ -z "$schema_version" ]]; then
        report_error "$schema_file: could not parse SCHEMA_VERSION"
        return
    fi

    local schema_pattern='([Ss]chema[ \t]+\(v|[Ss]chema[ \t]+v|currently[ \t]+schema[ \t]+v|schema[ \t]+version[ \t]+)([0-9]+)'
    local file line_no text found path
    for path in "${DOCS_TRUTH_SCHEMA_CHECK_PATHS[@]}"; do
        if [[ ! -e "$path" ]]; then
            report_error "$path: missing while checking schema version claims"
            continue
        fi

        while IFS=: read -r file line_no text; do
            if [[ "$text" =~ $schema_pattern ]]; then
                found="${BASH_REMATCH[2]}"
                if [[ "$found" != "$schema_version" ]]; then
                    report_error "$file:$line_no mentions schema $found but SCHEMA_VERSION is $schema_version"
                fi
            fi
        done < <(rg -nH -- "$schema_pattern" "$path" || true)
    done
}

check_retired_commands() {
    local retired_pattern='(^|[^A-Za-z0-9_-])(adopt-system|conary[ \t]+adopt|conary-adopt|system-adopt)([^A-Za-z0-9_-]|$)'
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}" "${PARSER_PATHS[@]}")

    if [[ "${#paths[@]}" -eq 0 ]]; then
        report_error "retired command check had no paths to scan"
        return
    fi

    local file line_no text
    while IFS=: read -r file line_no text; do
        case "$file" in
            scripts/check-doc-truth.sh|scripts/test-doc-truth.sh|*/archive/*)
                continue
                ;;
        esac
        report_error "$file:$line_no contains retired command spelling: $text"
    done < <(rg -n -- "$retired_pattern" "${paths[@]}" || true)
}

check_preview_status() {
    require_match "README.md" 'adoption-led' 'adoption-led preview wording'
    require_match "ROADMAP.md" 'adoption-led' 'adoption-led preview wording'

    require_match "README.md" 'Remote Forge validation is paused pending (a new |a )KVM-capable runner' 'remote Forge paused wording'
    require_match "ROADMAP.md" 'remote Forge validation is paused pending (a new |a )KVM-capable runner|Remote Forge validation is paused pending (a new |a )KVM-capable runner' 'remote Forge paused wording'
    require_match "docs/INTEGRATION-TESTING.md" 'Remote Forge control-plane validation is temporarily paused pending a KVM-capable runner|Forge-backed.*paused' 'remote Forge paused wording'

    require_match "README.md" '2026-05-21.*Group O' 'dated Group O evidence'
    require_match "README.md" '2026-05-21.*Group P' 'dated Group P evidence'
    require_match "ROADMAP.md" '2026-05-21.*Group O' 'dated Group O evidence'
    require_match "ROADMAP.md" '2026-05-21.*Group P' 'dated Group P evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group O.*2026-05-21' 'dated Group O evidence'
    require_match "docs/INTEGRATION-TESTING.md" 'Group P.*2026-05-21' 'dated Group P evidence'
}

check_preview_claim_drift() {
    local paths=()
    local path

    while IFS= read -r path; do
        paths+=("$path")
    done < <(existing_paths "${PRODUCT_DOC_PATHS[@]}")

    if [[ "${#paths[@]}" -eq 0 ]]; then
        report_error "preview claim drift check had no paths to scan"
        return
    fi

    local file line_no text

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims conaryd package execution is still blanket 501: $text"
    done < <(
        rg -n -i -- 'conaryd.*package (install/remove/update|mutation).*501 Not Implemented|package install/remove/update routes return.*501 Not Implemented' "${paths[@]}" || true
    )

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims every install builds an EROFS generation: $text"
    done < <(
        rg -n -i -- 'every install[^.\n]*(builds|produces)[^.\n]*EROF|every install, remove, (or |and )?(upgrade|update)[^.\n]*builds[^.\n]*EROF' "${paths[@]}" || true
    )

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no makes an unmeasured under-a-minute preview claim: $text"
    done < <(rg -n -i -- 'under a minute' "${paths[@]}" || true)

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims native packages are atomically absorbed/taken over without the explicit takeover boundary: $text"
    done < <(
        rg -n -i -- 'atomically[^.\n]*(absorbs|takes over)|absorbed atomically' "${paths[@]}" || true
    )
}

check_policykit_truth() {
    local auth_file="apps/conaryd/src/daemon/auth.rs"
    local daemon_file="apps/conaryd/src/daemon/mod.rs"
    require_file "$auth_file" || return
    require_file "$daemon_file" || return

    local overclaim_pattern='Non-root users can be authorized via PolicyKit|write access requires PolicyKit|PolicyKit authorization works|authorized by PolicyKit'
    local file line_no text
    local policykit_paths=()
    while IFS= read -r file; do
        policykit_paths+=("$file")
    done < <(existing_paths "${POLICYKIT_DOC_PATHS[@]}")

    while IFS=: read -r file line_no text; do
        report_error "$file:$line_no claims PolicyKit authorization is available today: $text"
    done < <(rg -n -- "$overclaim_pattern" "$auth_file" "${policykit_paths[@]}" 2>/dev/null || true)

    if ! rg -qi -- 'fail-closed|stubbed|unimplemented|unavailable' "$auth_file"; then
        report_error "$auth_file: must describe PolicyKit authorization as fail-closed, stubbed, unavailable, or unimplemented"
    fi

    if ! rg -q -- 'require_polkit:[ \t]*true' "$daemon_file"; then
        report_error "$daemon_file: DaemonConfig::default() must keep require_polkit: true until auth docs describe a different behavior"
    fi
}

extract_code_routes() {
    local files=(
        "apps/conaryd/src/daemon/routes/system.rs"
        "apps/conaryd/src/daemon/routes/transactions.rs"
        "apps/conaryd/src/daemon/routes/query.rs"
        "apps/conaryd/src/daemon/routes/events.rs"
    )
    local file line path method prefix

    for file in "${files[@]}"; do
        if [[ ! -f "$file" ]]; then
            report_error "required route file is missing: $file"
            continue
        fi

        while IFS= read -r line; do
            if [[ "$line" =~ \.route\(\"([^\"]+)\"[[:space:]]*,[[:space:]]*(get|post|delete)\( ]]; then
                path="${BASH_REMATCH[1]}"
                method="${BASH_REMATCH[2]^^}"
                if [[ "$file" == "apps/conaryd/src/daemon/routes/system.rs" && "$path" == "/health" ]]; then
                    prefix=""
                else
                    prefix="/v1"
                fi
                printf '%s %s%s\n' "$method" "$prefix" "$path"
            fi
        done < "$file"
    done
}

extract_doc_routes() {
    local doc="docs/modules/conaryd.md"
    require_file "$doc" || return

    awk '
        /<!-- conaryd-routes:start -->/ { in_routes = 1; next }
        /<!-- conaryd-routes:end -->/ { in_routes = 0; next }
        in_routes && /^(GET|POST|DELETE) \// { print $1 " " $2 }
    ' "$doc"
}

check_conaryd_routes() {
    local code_routes doc_routes
    code_routes="$(mktemp)"
    doc_routes="$(mktemp)"
    trap 'rm -f "$code_routes" "$doc_routes"' RETURN

    extract_code_routes | sort -u > "$code_routes"
    extract_doc_routes | sort -u > "$doc_routes"

    local route_count
    route_count="$(wc -l < "$code_routes" | tr -d ' ')"
    if [[ "$route_count" -lt 25 ]]; then
        report_error "conaryd route extraction found $route_count method/path pairs; expected at least 25"
    fi

    if ! diff -u "$code_routes" "$doc_routes" >&2; then
        report_error "conaryd route docs differ from apps/conaryd/src/daemon/routes"
    fi

    require_match "docs/modules/conaryd.md" '/health.*outside the v1 auth gate|/health.*outside.*auth' '/health auth-boundary wording'
    require_match "docs/modules/conaryd.md" '/v1/\*.*behind the v1 gate|/v1/\*.*auth' '/v1 auth-boundary wording'
    require_match "docs/modules/conaryd.md" 'Preview stub|preview-stubbed|not implemented' 'preview-stubbed system route wording'
}

check_conary_core_surface() {
    require_file "crates/conary-core/Cargo.toml" || return
    require_file "crates/conary-core/src/lib.rs" || return

    if ! rg -q -- '^publish[ \t]*=[ \t]*false$' "crates/conary-core/Cargo.toml"; then
        report_error "crates/conary-core/Cargo.toml must set publish = false while conary-core is internal"
    fi

    require_match "crates/conary-core/src/lib.rs" 'Internal workspace crate|internal workspace crate' 'internal crate documentation'

    local active_paths=()
    local path
    while IFS= read -r path; do
        active_paths+=("$path")
    done < <(existing_paths "README.md" "ROADMAP.md" "docs/ARCHITECTURE.md" "docs/conaryopedia-v2.md" "docs/modules" "docs/operations")

    local pattern='conary-core.*(stable public API|stable SDK|external library contract)|(stable public API|stable SDK|external library contract).*conary-core'
    local file line_no text
    while IFS=: read -r file line_no text; do
        if [[ ! "$text" =~ ([Ii]nternal|[Uu]nstable|not[[:space:]]+stable) ]]; then
            report_error "$file:$line_no makes a stable conary-core API claim without internal/unstable wording: $text"
        fi
    done < <(rg -n -i -- "$pattern" "${active_paths[@]}" || true)
}

check_schema_versions
check_retired_commands
check_preview_status
check_preview_claim_drift
check_policykit_truth
check_conaryd_routes
check_conary_core_surface

if [[ "$errors" -ne 0 ]]; then
    exit 1
fi

echo "Documentation truth checks passed."
