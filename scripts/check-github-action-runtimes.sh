#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

require_ref() {
  local file="$1"
  local pattern="$2"
  local description="$3"

  grep -Eq "$pattern" "$file" || fail "$description missing in $file"
}

forbid_ref() {
  local file="$1"
  local pattern="$2"
  local description="$3"

  if grep -Eq "$pattern" "$file"; then
    fail "$description unexpectedly present in $file"
  fi
}

require_ref .github/workflows/pr-gate.yml \
  'actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd' \
  'Node 24 checkout pin'
require_ref .github/workflows/release-build.yml \
  'actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd' \
  'Node 24 checkout pin'
require_ref .github/workflows/merge-validation.yml \
  'actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd' \
  'Node 24 checkout pin'
require_ref .github/workflows/scheduled-ops.yml \
  'actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd' \
  'Node 24 checkout pin'

require_ref .github/workflows/release-build.yml \
  'actions/cache@668228422ae6a00e4ad889ee87cd7109ec5666a7' \
  'Node 24 cache pin'
require_ref .github/actions/setup-rust-workspace/action.yml \
  'actions/cache@668228422ae6a00e4ad889ee87cd7109ec5666a7' \
  'Node 24 setup action cache pin'
require_ref .github/workflows/release-build.yml \
  'actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f' \
  'Node 24 upload-artifact pin'
require_ref .github/workflows/release-build.yml \
  'actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c' \
  'Node 24 download-artifact pin'
require_ref .github/workflows/scheduled-ops.yml \
  'actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f' \
  'Node 24 upload-artifact pin'

forbid_ref .github/workflows/pr-gate.yml \
  'actions/dependency-review-action@' \
  'dependency-review-action'
forbid_ref .github/workflows/release-build.yml \
  'softprops/action-gh-release@' \
  'softprops action-gh-release'
forbid_ref .github/workflows/pr-gate.yml \
  'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24:[[:space:]]*"?true"?' \
  'forced Node 24 workflow override'
forbid_ref .github/workflows/release-build.yml \
  'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24:[[:space:]]*"?true"?' \
  'forced Node 24 workflow override'

require_ref .github/workflows/pr-gate.yml \
  'dependency-graph/compare/' \
  'custom dependency review API call'
require_ref .github/workflows/release-build.yml \
  'gh release create' \
  'CLI GitHub release publication'

echo "GitHub Actions runtime pins look Node 24-ready."
