#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

validator="scripts/check-coherency-ledger.sh"
scope_validator=(bash scripts/check-coherency-wave-scopes.sh)
header=$'id\tsurface\tsource\trelated_ids\twave_scope\towner\tclaim\tactual_or_gap\tstatus\tdisposition\tlast_verified\tevidence_sources\trepro\tverification\tdecision\tnext_slice\tnotes'

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

write_ledger() {
    local path="$1"
    shift
    {
        printf '%s\n' "$header"
        printf '%s\n' "$@"
    } > "$path"
}

good_row=$'CLI-ROOT-001\tconary root help\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help lists top-level commands and daily examples\tRoot help renders and examples are visible\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tverify\tWave 1a root help evidence captured\tInitial root help row'

good="$tmpdir/good.tsv"
write_ledger "$good" "$good_row"
"$validator" "$good" >/dev/null
"$validator" "$good" --scope-complete 1a-root-cli >/dev/null

real_repo_row=$'CLI-ROOT-005\tconary root dispatch fixture\tpath:apps/conary/src/dispatch/root.rs:1-40\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot dispatch paths exist in the repo\tLine-range and directory path pointers resolve against real repo files\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/dispatch/root.rs:1-40;path:apps/conary/src/dispatch/\tnone\ttest:cargo test -p conary --lib cli::tests\tverify\tKeep real path fixture green\tReal repo path fixture'
real_repo="$tmpdir/real-repo.tsv"
write_ledger "$real_repo" "$real_repo_row"
"$validator" "$real_repo" >/dev/null

valid_route_mcp="$tmpdir/valid-route-mcp.tsv"
route_row=$'ROUTE-CONARYD-001\tconaryd transactions route\troute:GET /v1/transactions\t\tlater-route-wave\tconaryd Package Jobs And Daemon Routes\tRoute pointer grammar accepts method and path\tRoute pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\troute:GET /v1/transactions\tnone\troute:GET /v1/transactions\tverify\tValidate route pointer grammar\tRoute pointer fixture'
mcp_row=$'MCP-REMI-001\tRemi MCP tool fixture\tmcp:remi/tool-name\t\tlater-mcp-wave\tAgent/MCP Operation Surfaces\tMCP pointer grammar accepts server and tool\tMCP pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\tmcp:remi/tool-name\tnone\tmcp:remi/tool-name\tverify\tValidate MCP pointer grammar\tMCP pointer fixture'
write_ledger "$valid_route_mcp" "$route_row" "$mcp_row"
"$validator" "$valid_route_mcp" >/dev/null

bad_status="$tmpdir/bad-status.tsv"
write_ledger "$bad_status" "${good_row/works/not-real-status}"
if "$validator" "$bad_status" >"$tmpdir/out" 2>&1; then
    fail "invalid status unexpectedly passed"
fi
grep -q "invalid status" "$tmpdir/out" || fail "invalid status error was not clear"

bad_final_no_newline="$tmpdir/bad-final-no-newline.tsv"
printf '%s\n' "$header" > "$bad_final_no_newline"
printf '%s' "${good_row/works/not-real-status}" >> "$bad_final_no_newline"
if "$validator" "$bad_final_no_newline" >"$tmpdir/out" 2>&1; then
    fail "invalid final row without trailing newline unexpectedly passed"
fi
grep -q "invalid status" "$tmpdir/out" || fail "no-final-newline invalid status error was not clear"

trailing_tab="$tmpdir/trailing-tab.tsv"
printf '%s\n%s\t\n' "$header" "$good_row" > "$trailing_tab"
if "$validator" "$trailing_tab" >"$tmpdir/out" 2>&1; then
    fail "row with trailing raw tab unexpectedly passed"
fi
grep -q "expected 17 fields" "$tmpdir/out" || fail "trailing tab error was not clear"

bad_date="$tmpdir/bad-date.tsv"
write_ledger "$bad_date" "${good_row/2026-06-09/2026-99-99}"
if "$validator" "$bad_date" >"$tmpdir/out" 2>&1; then
    fail "impossible last_verified date unexpectedly passed"
fi
grep -q "invalid last_verified date" "$tmpdir/out" || fail "bad date error was not clear"

bad_decision="$tmpdir/bad-decision.tsv"
bad_decision_row=$'CLI-ROOT-002\tconary root deferral fixture\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tDeferred row should use defer decision\tDeferred row has the wrong decision\t honest-deferred\tdeferred-owned\t2026-06-09\tcmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tremove\tOpen follow-up\tBad decision should fail'
bad_decision_row="${bad_decision_row/$'\t honest-deferred'/$'\thonest-deferred'}"
write_ledger "$bad_decision" "$bad_decision_row"
if "$validator" "$bad_decision" >"$tmpdir/out" 2>&1; then
    fail "honest-deferred/remove decision unexpectedly passed"
fi
grep -q "invalid decision for status" "$tmpdir/out" || fail "bad decision matrix error was not clear"

bad_owner="$tmpdir/bad-owner.tsv"
write_ledger "$bad_owner" "${good_row/CLI Dispatch And Command Routing/No Such Owner}"
if "$validator" "$bad_owner" >"$tmpdir/out" 2>&1; then
    fail "unknown feature owner unexpectedly passed"
fi
grep -q "owner does not match" "$tmpdir/out" || fail "bad owner error was not clear"

md_as_path="$tmpdir/md-as-path.tsv"
md_as_path_row=$'DOC-ROOT-001\tmarkdown pointer fixture\tpath:docs/llms/README.md\t\t1a-root-cli\tCLI Dispatch And Command Routing\tMarkdown pointers should use doc prefix\tMarkdown pointer used path prefix\tworks\tverified-no-change\t2026-06-09\tpath:docs/llms/README.md\tnone\tcmd:cargo run -p conary -- --help\tverify\tUse doc pointers for Markdown\tBad Markdown pointer should fail'
write_ledger "$md_as_path" "$md_as_path_row"
if "$validator" "$md_as_path" >"$tmpdir/out" 2>&1; then
    fail "Markdown path: pointer unexpectedly passed"
fi
grep -q "Markdown pointer must use doc:" "$tmpdir/out" || fail "Markdown path pointer error was not clear"

bad_evidence_pointer="$tmpdir/bad-evidence-pointer.tsv"
write_ledger "$bad_evidence_pointer" "${good_row/path:apps\/conary\/src\/cli\/mod.rs/path:no-such-file.rs}"
if "$validator" "$bad_evidence_pointer" >"$tmpdir/out" 2>&1; then
    fail "missing path pointer unexpectedly passed"
fi
grep -q "referenced path does not exist" "$tmpdir/out" || fail "missing path error was not clear"

bad_source="$tmpdir/bad-source.tsv"
write_ledger "$bad_source" "${good_row/cmd:cargo run -p conary -- --help/not-a-typed-source}"
if "$validator" "$bad_source" >"$tmpdir/out" 2>&1; then
    fail "bad source pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad source error was not clear"

bad_repro="$tmpdir/bad-repro.tsv"
bad_repro_row=$'CLI-ROOT-002\tconary root help\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help lists top-level commands and daily examples\tRoot help renders and examples are visible\tworks\tverified-no-change\t2026-06-09\tpath:apps/conary/src/cli/mod.rs;cmd:cargo run -p conary -- --help\tbad-repro\tcmd:cargo run -p conary -- --help\tverify\tWave 1a root help evidence captured\tBad repro should fail'
write_ledger "$bad_repro" "$bad_repro_row"
if "$validator" "$bad_repro" >"$tmpdir/out" 2>&1; then
    fail "bad repro pointer unexpectedly passed"
fi
grep -q "invalid typed repro pointer" "$tmpdir/out" || fail "bad repro error was not clear"

bad_route="$tmpdir/bad-route.tsv"
bad_route_row=$'ROUTE-CONARYD-002\tconaryd transactions route\troute:GET/v1/transactions\t\tlater-route-wave\tconaryd Package Jobs And Daemon Routes\tRoute pointer grammar rejects missing method/path space\tRoute pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\troute:GET/v1/transactions\tnone\troute:GET/v1/transactions\tverify\tValidate bad route pointer grammar\tBad route pointer fixture'
write_ledger "$bad_route" "$bad_route_row"
if "$validator" "$bad_route" >"$tmpdir/out" 2>&1; then
    fail "bad route pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad route pointer error was not clear"

bad_mcp="$tmpdir/bad-mcp.tsv"
bad_mcp_row=$'MCP-REMI-002\tRemi MCP tool fixture\tmcp:remi\t\tlater-mcp-wave\tAgent/MCP Operation Surfaces\tMCP pointer grammar rejects missing tool name\tMCP pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\tmcp:remi\tnone\tmcp:remi\tverify\tValidate bad MCP pointer grammar\tBad MCP pointer fixture'
write_ledger "$bad_mcp" "$bad_mcp_row"
if "$validator" "$bad_mcp" >"$tmpdir/out" 2>&1; then
    fail "bad MCP pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad MCP pointer error was not clear"

missing_gap="$tmpdir/missing-gap.tsv"
misleading_row=$'CLI-ROOT-002\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\t\tmisleading\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tfix\tFix or remove misleading example\tMissing gap should fail'
write_ledger "$missing_gap" "$misleading_row"
if "$validator" "$missing_gap" >"$tmpdir/out" 2>&1; then
    fail "misleading row without actual_or_gap unexpectedly passed"
fi
grep -q "actual_or_gap is required" "$tmpdir/out" || fail "missing actual_or_gap error was not clear"

scope_open="$tmpdir/scope-open.tsv"
open_misleading=$'CLI-ROOT-003\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tExample reaches a dead end\tmisleading\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tfix\tFix root example\tOpen misleading row should block scope completion'
write_ledger "$scope_open" "$open_misleading"
"$validator" "$scope_open" >/dev/null
if "$validator" "$scope_open" --scope-complete 1a-root-cli >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed open misleading row"
fi
grep -q "scope completion blocked" "$tmpdir/out" || fail "scope completion error was not clear"

bad_open_works="$tmpdir/bad-open-works.tsv"
open_works=$'CLI-ROOT-003\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tRoot example route proof is still being captured\tworks\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tverify\tFinish root example proof\tOpen works row should fail normal validation'
write_ledger "$bad_open_works" "$open_works"
if "$validator" "$bad_open_works" >"$tmpdir/out" 2>&1; then
    fail "open works row unexpectedly passed"
fi
grep -q "invalid disposition" "$tmpdir/out" || fail "open works matrix error was not clear"

scope_open_thin="$tmpdir/scope-open-thin.tsv"
open_thin=$'CLI-ROOT-004\tconary root help example\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tRoot help example should be runnable\tRoute proof is thin and still open\tworks-but-thin\topen\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tharden\tFinish root example proof\tOpen works-but-thin row should block scope completion'
write_ledger "$scope_open_thin" "$open_thin"
"$validator" "$scope_open_thin" >/dev/null
if "$validator" "$scope_open_thin" --scope-complete 1a-root-cli >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed open works-but-thin row"
fi
grep -q "scope completion blocked" "$tmpdir/out" || fail "open works-but-thin scope error was not clear"

empty_scope="$tmpdir/empty-scope.tsv"
write_ledger "$empty_scope" "$good_row"
if "$validator" "$empty_scope" --scope-complete other-scope >"$tmpdir/out" 2>&1; then
    fail "scope completion unexpectedly allowed an empty scope"
fi
grep -q "scope completion found no rows" "$tmpdir/out" || fail "empty scope error was not clear"

bad_matrix="$tmpdir/bad-matrix.tsv"
matrix_row=$'CLI-ROOT-004\tconary root help duplicate\tcmd:cargo run -p conary -- --help\t\t1a-root-cli\tCLI Dispatch And Command Routing\tDuplicate root path should be merged\tTwo root surfaces overlap\tduplicate-stale\tverified-no-change\t2026-06-09\tcmd:cargo run -p conary -- --help\tnone\tcmd:cargo run -p conary -- --help\tmerge\tMerge duplicate root surface\tBad matrix should fail'
write_ledger "$bad_matrix" "$matrix_row"
if "$validator" "$bad_matrix" >"$tmpdir/out" 2>&1; then
    fail "invalid status/disposition matrix unexpectedly passed"
fi
grep -q "invalid disposition" "$tmpdir/out" || fail "bad matrix error was not clear"

dangling_related="$tmpdir/dangling-related.tsv"
dangling_row=$'CLI-ROOT-005\tconary root help duplicate\tcmd:cargo run -p conary -- --help\tCLI-ROOT-999\t1a-root-cli\tCLI Dispatch And Command Routing\tDuplicate root path should be merged\tTwo root surfaces overlap\tduplicate-stale\tresolved-merged\t2026-06-09\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tcmd:cargo run -p conary -- --help\tmerge\tMerge duplicate root surface\tDangling related ID should fail'
write_ledger "$dangling_related" "$dangling_row"
if "$validator" "$dangling_related" >"$tmpdir/out" 2>&1; then
    fail "dangling related_id unexpectedly passed"
fi
grep -q "dangling related_id" "$tmpdir/out" || fail "dangling related_id error was not clear"

registry="$tmpdir/wave-scopes.tsv"
{
    printf 'wave_scope\tstatus\tnotes\n'
    printf '1a-root-cli\tcompleted\tRoot CLI complete\n'
    printf 'later-route-wave\tactive\tRoute fixture active\n'
    printf 'later-mcp-wave\tactive\tMCP fixture active\n'
} > "$registry"

"${scope_validator[@]}" "$good" "$registry" >/dev/null

unknown_scope="$tmpdir/unknown-scope.tsv"
unknown_scope_row="${good_row/1a-root-cli/typo-root-cli}"
write_ledger "$unknown_scope" "$unknown_scope_row"
if "${scope_validator[@]}" "$unknown_scope" "$registry" >"$tmpdir/out" 2>&1; then
    fail "unknown wave_scope unexpectedly passed scope registry check"
fi
grep -q "unregistered wave_scope" "$tmpdir/out" || fail "unknown scope error was not clear"

if "${scope_validator[@]}" "$scope_open_thin" "$registry" >"$tmpdir/out" 2>&1; then
    fail "completed scope with open row unexpectedly passed registry check"
fi
grep -q "scope completion blocked" "$tmpdir/out" || fail "completed scope open-row error was not clear"

echo "Coherency ledger validator tests passed."
