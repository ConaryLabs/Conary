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

  rg -q "$pattern" "$file" || fail "$description missing in $file"
}

require_forced_node24() {
  local file="$1"
  rg -q 'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24:\s*"?true"?' "$file" \
    || fail "FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true missing in $file"
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
require_ref .github/workflows/release-build.yml \
  'actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f' \
  'Node 24 upload-artifact pin'
require_ref .github/workflows/release-build.yml \
  'actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c' \
  'Node 24 download-artifact pin'
require_ref .github/workflows/scheduled-ops.yml \
  'actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f' \
  'Node 24 upload-artifact pin'

require_ref .github/workflows/pr-gate.yml \
  'actions/dependency-review-action@2031cfc080254a8a887f58cffee85186f0e49e48' \
  'dependency-review-action pin'
require_ref .github/workflows/release-build.yml \
  'softprops/action-gh-release@153bb8e04406b158c6c84fc1615b65b24149a1fe' \
  'action-gh-release pin'

require_forced_node24 .github/workflows/pr-gate.yml
require_forced_node24 .github/workflows/release-build.yml

echo "GitHub Actions runtime pins look Node 24-ready."
