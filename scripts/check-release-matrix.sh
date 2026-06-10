#!/usr/bin/env bash
set -euo pipefail

repo_root="${1:-$(git rev-parse --show-toplevel)}"
cd "$repo_root"

release_build=".github/workflows/release-build.yml"
deploy_workflow=".github/workflows/deploy-and-verify.yml"
merge_workflow=".github/workflows/merge-validation.yml"
artifact_matrix="docs/operations/release-artifact-matrix.md"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

require_match() {
    local file="$1"
    local pattern="$2"
    local description="$3"

    rg -q --multiline "$pattern" "$file" || fail "$description missing in $file"
}

forbid_match() {
    local file="$1"
    local pattern="$2"
    local description="$3"

    if rg -q --multiline "$pattern" "$file"; then
        fail "$description unexpectedly present in $file"
    fi
}

require_artifact_matrix_row() {
    local product="$1"
    local row

    row="$(rg -n -- "^\| \`$product\` \|" "$artifact_matrix" || true)"
    [[ -n "$row" ]] || fail "release artifact matrix missing $product row"

    if [[ "$row" != *"source-build-only"* && "$row" != *"https://"* ]]; then
        fail "release artifact matrix row for $product needs artifact URL or source-build-only caveat"
    fi

    [[ "$row" == *"checksum"* || "$row" == *"checksums"* ]] ||
        fail "release artifact matrix row for $product missing checksum status"
    [[ "$row" == *"signature"* ]] ||
        fail "release artifact matrix row for $product missing signature status"
    [[ "$row" == *"SBOM"* ]] ||
        fail "release artifact matrix row for $product missing SBOM status"
    [[ "$row" == *"provenance"* || "$row" == *"SLSA"* ]] ||
        fail "release artifact matrix row for $product missing provenance status"
}

forbid_deploy_jobs_for_none_product() {
    local product="$1"
    local job

    for job in "deploy-${product}" "verify-${product}"; do
        forbid_match "$deploy_workflow" "^  ${job}:" "${job} job for deploy_mode=none product ${product}"
    done
}

[[ -f "$artifact_matrix" ]] || fail "missing $artifact_matrix"

require_match "$release_build" 'conary-test-v\*' 'conary-test release trigger'
require_match "$release_build" 'scripts/release-matrix\.sh resolve-tag' 'helper-based tag resolution'
require_match "$release_build" 'scripts/release-matrix\.sh metadata-json' 'helper-based metadata serialization'
require_match "$release_build" 'workflow_dispatch is dry-run only; push the canonical tag for live releases' 'manual live-release guardrail'
require_match "$release_build" 'Prepare dry-run release tree' 'dry-run release tree preparation step'
require_match "$release_build" '\./scripts/release\.sh "\$product"' 'dry-run release tree should be prepared by the canonical release script'
require_match "$release_build" 'CONARY_RELEASE_LOCKFILE_MODE: online' 'dry-run release tree should allow online lockfile refreshes in CI'
require_match "$release_build" 'git config --global --add safe\.directory "\$\(pwd\)"' 'dry-run release tree should mark the checked-out repo as a safe git directory'
require_match "$release_build" 'git tag --points-at HEAD \| grep -Fx "\$tag_name"' 'dry-run preparation should verify the expected local tag'
require_match "$release_build" 'deterministic dry-run signing key' 'dry-run signing fallback'
require_match "$release_build" 'REHEARSAL_SIGNING_PUBLIC_KEY\.txt' 'dry-run signing public key artifact'
require_match "$release_build" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'prepare bundle_name output'
require_match "$release_build" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'prepare deploy_mode output'
require_match "$release_build" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'prepare artifact_patterns output'
require_match "$release_build" 'build-conary-test:' 'conary-test build lane'
require_match "$release_build" 'publish-remi:' 'remi release publication lane'
require_match "$release_build" 'publish-conaryd:' 'conaryd release publication lane'
require_match "$release_build" 'publish-conary-test:' 'conary-test release publication lane'
require_match "$release_build" 'workspace-validation:' 'release workspace validation lane'
require_match "$release_build" 'workspace-validation:[\s\S]*needs: prepare' 'release workspace validation should depend on prepare'
require_match "$release_build" 'cargo fmt --check' 'release formatting validation'
require_match "$release_build" 'cargo clippy --workspace --all-targets -- -D warnings' 'release clippy validation'
require_match "$release_build" 'cargo test --workspace --exclude conary-test --verbose' 'release workspace test validation'
require_match "$release_build" 'cargo test -p conary-test --verbose' 'release conary-test validation'
require_match "$release_build" 'cargo test --doc --workspace --verbose' 'release doctest validation'
require_match "$release_build" 'build-ccs:[\s\S]*needs: \[prepare, workspace-validation\]' 'ccs build should need workspace validation'
require_match "$release_build" 'build-remi:[\s\S]*needs: \[prepare, workspace-validation\]' 'remi build should need workspace validation'
require_match "$release_build" 'publish-remi:[\s\S]*needs: \[prepare, workspace-validation, build-remi\]' 'remi publish should need workspace validation'
require_match "$release_build" 'name: \$\{\{ needs\.prepare\.outputs\.bundle_name \}\}' 'dynamic bundle artifact naming'
require_match "$release_build" 'gh release create' 'CLI-based GitHub release publication'

require_match "$merge_workflow" 'workflow-runtime-policy:' 'merge validation workflow runtime policy job'
require_match "$merge_workflow" 'bash scripts/test-github-action-runtimes\.sh' 'merge validation action checker test'
require_match "$merge_workflow" 'release-matrix-policy:' 'merge validation release matrix policy job'
require_match "$merge_workflow" 'bash scripts/test-release-matrix\.sh' 'merge validation release matrix test'
require_match "$merge_workflow" 'bash scripts/test-remi-deploy-helper\.sh' 'merge validation deploy helper test'
require_match "$merge_workflow" 'fmt:' 'merge validation formatting job'
require_match "$merge_workflow" 'dependency-consistency:' 'merge validation dependency consistency job'
require_match "$merge_workflow" 'clippy:' 'merge validation clippy job'
require_match "$merge_workflow" 'workspace-tests:' 'merge validation workspace test job'
require_match "$merge_workflow" 'conary-test-crate:' 'merge validation conary-test job'
require_match "$merge_workflow" 'doctests:' 'merge validation doctest job'

require_match "$deploy_workflow" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'deploy resolve bundle_name output'
require_match "$deploy_workflow" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'deploy resolve deploy_mode output'
require_match "$deploy_workflow" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'deploy resolve artifact_patterns output'
require_match "$deploy_workflow" 'validate-routing:' 'deploy routing validation job'
require_match "$deploy_workflow" 'No deploy lane defined for product=' 'explicit unmatched deploy failure'
require_match "$deploy_workflow" 'no-deploy-required:' 'explicit no-deploy lane'
require_match "$deploy_workflow" "needs\\.resolve\\.outputs\\.deploy_mode == 'none'" 'deploy_mode none handling'
require_match "$deploy_workflow" 'conaryd:none' 'temporary conaryd no-deploy route'
require_match "$deploy_workflow" 'BUNDLE_NAME: \$\{\{ needs\.resolve\.outputs\.bundle_name \}\}' 'bundle_name-driven artifact lookup'
require_match "$deploy_workflow" 'gh api "repos/\$\{?GH_REPO\}?/actions/runs/\$\{?SOURCE_RUN\}?" --jq '\''\.head_branch'\''' 'source-run head-branch lookup for release fallback'
require_match "$deploy_workflow" 'gh release download "\$source_tag"' 'release-asset fallback for expired source-run artifacts'
forbid_match "$deploy_workflow" 'CONARYD_VERIFY_URL' 'legacy public verify URL'
forbid_match "$deploy_workflow" '24273700060' 'retired one-time conaryd bootstrap exception'
forbid_match "$deploy_workflow" 'deploy_asset_ref' 'retired bootstrap-only deploy asset ref'
forbid_match "$deploy_workflow" 'bootstrap_exception' 'retired bootstrap exception output'

for product in conary remi; do
    require_artifact_matrix_row "$product"
    deploy_mode="$(bash scripts/release-matrix.sh field "$product" deploy_mode)"
    [[ "$deploy_mode" != "none" ]] || fail "$product unexpectedly marked non-deployable"
    require_match "$deploy_workflow" "needs\\.resolve\\.outputs\\.product == '${product}'" "${product} deploy lane"
done

require_artifact_matrix_row conaryd
conaryd_deploy_mode="$(bash scripts/release-matrix.sh field conaryd deploy_mode)"
[[ "$conaryd_deploy_mode" == "none" ]] || fail "conaryd should be deploy_mode=none while Forge staging is paused"
forbid_deploy_jobs_for_none_product conaryd

require_artifact_matrix_row conary-test
conary_test_deploy_mode="$(bash scripts/release-matrix.sh field conary-test deploy_mode)"
[[ "$conary_test_deploy_mode" == "none" ]] || fail "conary-test should be deploy_mode=none"
forbid_deploy_jobs_for_none_product conary-test

echo "Release matrix workflow checks passed."
