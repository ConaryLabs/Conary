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

feature_hint_for_path() {
    local path="$1"

    case "$path" in
        crates/conary-agent-contract/*|crates/conary-mcp/*|apps/remi/src/server/mcp.rs|apps/conary-test/src/server/mcp.rs)
            printf 'Agent/MCP operation surfaces | focused: cargo test -p conary-agent-contract; cargo test -p conary-mcp | gate: owning service tests when adapter calls service behavior'
            ;;
        apps/conary/src/commands/install/*|apps/conary/src/commands/update/*|apps/conary/src/commands/remove.rs)
            printf 'Native package install/update/remove | focused: cargo test -p conary --test live_host_mutation_safety; cargo test -p conary --test bundle_replay | gate: cargo test -p conaryd daemon::routes when daemon jobs change'
            ;;
        apps/conary/src/commands/adopt/*)
            printf 'Adoption and native-authority handoff | focused: cargo test -p conary --lib adopt::native_handoff; cargo test -p conary --lib adopt::unadopt | gate: cargo run -p conary-test -- list; selected handoff suite when behavior changes'
            ;;
        crates/conary-core/src/generation/*)
            printf 'Generation build/switch/export | focused: cargo test -p conary-core generation::export; cargo test -p conary-core generation::builder | gate: generation export conary-test suites when boot-carrier behavior changes'
            ;;
        crates/conary-core/src/ccs/*|apps/conary/src/commands/ccs/*)
            printf 'CCS authoring/conversion/install/replay | focused: cargo test -p conary-core golden_fixtures; cargo test -p conary-core support_matrix | gate: cargo test -p conary --test conversion_integration golden_conversion; cargo test -p remi publication when serving changes'
            ;;
        apps/remi/src/server/*|apps/remi/src/bin/remi.rs|apps/remi/src/trust.rs)
            printf 'Remi publication/serving/admin | focused: cargo test -p remi publication; cargo test -p remi test_upload_fixture | gate: cargo test -p remi'
            ;;
        apps/remi/src/federation/*)
            printf 'Remi federation | focused: cargo test -p remi | gate: docs/modules/federation.md and feature-card update if federation becomes a first-class ownership card'
            ;;
        apps/conaryd/*)
            printf 'conaryd jobs/routes | focused: cargo test -p conaryd daemon::routes | gate: cargo test -p conaryd'
            ;;
        apps/conary/src/commands/bootstrap/*|apps/conary-test/src/bootstrap.rs|crates/conary-bootstrap/*|docs/modules/bootstrap.md|docs/operations/bootstrap-selfhosting-vm.md|docs/operations/bootstrap-follow-up-investigations.md)
            printf 'Bootstrap and self-hosting | focused: cargo run -p conary-test -- bootstrap check --json; cargo run -p conary-test -- bootstrap smoke --dry-run --json | gate: cargo run -p conary-test -- bootstrap smoke --json when building local images or changing host requirements'
            ;;
        apps/conary-test/*|apps/conary/tests/integration/remi/manifests/*)
            printf 'conary-test integration execution | focused: cargo run -p conary-test -- list; cargo test -p conary-test suite_inventory | gate: touched suite command from docs/modules/feature-ownership.md'
            ;;
        AGENTS.md|CONTRIBUTING.md|.github/PULL_REQUEST_TEMPLATE.md|docs/llms/*|docs/modules/feature-ownership.md|docs/superpowers/documentation-accuracy-audit-*|scripts/maintainability-drift-report.sh)
            printf 'Assistant/contributor guidance | focused: bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete | gate: docs-audit inventory diff and stale-term added-line sweep'
            ;;
        docs/modules/*|docs/operations/*|docs/INTEGRATION-TESTING.md|docs/ARCHITECTURE.md)
            printf 'Canonical docs | focused: docs-audit ledger and inventory checks | gate: affected feature card proof if behavior claims changed'
            ;;
        docs/superpowers/plans/*)
            printf 'Planning docs | focused: docs-audit ledger and inventory checks | gate: agentic review before lock-in'
            ;;
        *)
            return 1
            ;;
    esac
}

collect_paths() {
    if [[ "$scan_all" -eq 1 ]]; then
        git ls-files
        return
    fi

    {
        git diff --name-only "$base_ref" --
        git diff --cached --name-only --
        git ls-files --others --exclude-standard
    } | awk 'NF' | sort -u
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
mapfile -t changed_paths < <(collect_paths)

if [[ "${#changed_paths[@]}" -eq 0 ]]; then
    printf '[ok] no changed paths detected\n'
else
    printf 'changed_paths: %s\n' "${#changed_paths[@]}"
    for path in "${changed_paths[@]}"; do
        if hint="$(feature_hint_for_path "$path")"; then
            printf -- '- %s\n  %s\n' "$path" "$hint"
        else
            printf -- '- %s\n  No feature-card hint matched. Use the owning package tests and update docs/modules/feature-ownership.md if this should be routed.\n' "$path"
        fi
    done
fi

section "Rust Hotspots"
if [[ -x scripts/line-count-report.sh ]]; then
    scripts/line-count-report.sh "$limit"
else
    warn "scripts/line-count-report.sh is missing or not executable"
fi

section "Reminder"
printf 'This report is warn-only. Follow the focused proof and interaction gate from docs/modules/feature-ownership.md when the touched behavior crosses a neighbor system.\n'
