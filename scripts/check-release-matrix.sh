#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

release_build=".github/workflows/release-build.yml"
deploy_workflow=".github/workflows/deploy-and-verify.yml"

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

require_match "$release_build" 'conary-test-v\*' 'conary-test release trigger'
require_match "$release_build" 'scripts/release-matrix\.sh resolve-tag' 'helper-based tag resolution'
require_match "$release_build" 'scripts/release-matrix\.sh metadata-json' 'helper-based metadata serialization'
require_match "$release_build" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'prepare bundle_name output'
require_match "$release_build" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'prepare deploy_mode output'
require_match "$release_build" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'prepare artifact_patterns output'
require_match "$release_build" 'build-conary-test:' 'conary-test build lane'
require_match "$release_build" 'publish-remi:' 'remi release publication lane'
require_match "$release_build" 'publish-conaryd:' 'conaryd release publication lane'
require_match "$release_build" 'publish-conary-test:' 'conary-test release publication lane'
require_match "$release_build" 'name: \$\{\{ needs\.prepare\.outputs\.bundle_name \}\}' 'dynamic bundle artifact naming'
require_match "$release_build" 'gh release create' 'CLI-based GitHub release publication'

require_match "$deploy_workflow" 'bundle_name: \$\{\{ steps\.meta\.outputs\.bundle_name \}\}' 'deploy resolve bundle_name output'
require_match "$deploy_workflow" 'deploy_mode: \$\{\{ steps\.meta\.outputs\.deploy_mode \}\}' 'deploy resolve deploy_mode output'
require_match "$deploy_workflow" 'artifact_patterns: \$\{\{ steps\.meta\.outputs\.artifact_patterns \}\}' 'deploy resolve artifact_patterns output'
require_match "$deploy_workflow" 'validate-routing:' 'deploy routing validation job'
require_match "$deploy_workflow" 'No deploy lane defined for product=' 'explicit unmatched deploy failure'
require_match "$deploy_workflow" 'no-deploy-required:' 'explicit no-deploy lane'
require_match "$deploy_workflow" "needs\\.resolve\\.outputs\\.deploy_mode == 'none'" 'deploy_mode none handling'
require_match "$deploy_workflow" 'BUNDLE_NAME: \$\{\{ needs\.resolve\.outputs\.bundle_name \}\}' 'bundle_name-driven artifact lookup'

for product in conary remi conaryd; do
    deploy_mode="$(bash scripts/release-matrix.sh field "$product" deploy_mode)"
    [[ "$deploy_mode" != "none" ]] || fail "$product unexpectedly marked non-deployable"
    require_match "$deploy_workflow" "needs\\.resolve\\.outputs\\.product == '${product}'" "${product} deploy lane"
done

conary_test_deploy_mode="$(bash scripts/release-matrix.sh field conary-test deploy_mode)"
[[ "$conary_test_deploy_mode" == "none" ]] || fail "conary-test should be deploy_mode=none"
forbid_match "$deploy_workflow" 'deploy-conary-test:' 'conary-test deploy lane'
forbid_match "$deploy_workflow" 'verify-conary-test:' 'conary-test verify lane'

echo "Release matrix workflow checks passed."
