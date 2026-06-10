#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

validator="scripts/check-coherency-ledger.sh"
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
route_row=$'ROUTE-CONARYD-001\tconaryd transactions route\troute:GET /v1/transactions\t\tlater-route-wave\tConaryd Daemon Routes\tRoute pointer grammar accepts method and path\tRoute pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\troute:GET /v1/transactions\tnone\troute:GET /v1/transactions\tverify\tValidate route pointer grammar\tRoute pointer fixture'
mcp_row=$'MCP-REMI-001\tRemi MCP tool fixture\tmcp:remi/tool-name\t\tlater-mcp-wave\tAgent And MCP Surface\tMCP pointer grammar accepts server and tool\tMCP pointer syntax is valid\tworks\tverified-no-change\t2026-06-09\tmcp:remi/tool-name\tnone\tmcp:remi/tool-name\tverify\tValidate MCP pointer grammar\tMCP pointer fixture'
write_ledger "$valid_route_mcp" "$route_row" "$mcp_row"
"$validator" "$valid_route_mcp" >/dev/null

bad_status="$tmpdir/bad-status.tsv"
write_ledger "$bad_status" "${good_row/works/not-real-status}"
if "$validator" "$bad_status" >"$tmpdir/out" 2>&1; then
    fail "invalid status unexpectedly passed"
fi
grep -q "invalid status" "$tmpdir/out" || fail "invalid status error was not clear"

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
bad_route_row=$'ROUTE-CONARYD-002\tconaryd transactions route\troute:GET/v1/transactions\t\tlater-route-wave\tConaryd Daemon Routes\tRoute pointer grammar rejects missing method/path space\tRoute pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\troute:GET/v1/transactions\tnone\troute:GET/v1/transactions\tverify\tValidate bad route pointer grammar\tBad route pointer fixture'
write_ledger "$bad_route" "$bad_route_row"
if "$validator" "$bad_route" >"$tmpdir/out" 2>&1; then
    fail "bad route pointer unexpectedly passed"
fi
grep -q "invalid typed source pointer" "$tmpdir/out" || fail "bad route pointer error was not clear"

bad_mcp="$tmpdir/bad-mcp.tsv"
bad_mcp_row=$'MCP-REMI-002\tRemi MCP tool fixture\tmcp:remi\t\tlater-mcp-wave\tAgent And MCP Surface\tMCP pointer grammar rejects missing tool name\tMCP pointer syntax is invalid\tworks\tverified-no-change\t2026-06-09\tmcp:remi\tnone\tmcp:remi\tverify\tValidate bad MCP pointer grammar\tBad MCP pointer fixture'
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

echo "Coherency ledger validator tests passed."
