#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

write_fixture() {
  local root="$1"
  local uses_ref="$2"

  mkdir -p "$root/.github/workflows" "$root/.github/actions/setup-rust-workspace"
  cat > "$root/.github/workflows/policy.yml" <<EOF
name: policy
on: workflow_dispatch
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: ${uses_ref}
      - uses: ./.github/actions/setup-rust-workspace
      - uses: actions/cache@668228422ae6a00e4ad889ee87cd7109ec5666a7
EOF

  cat > "$root/.github/actions/setup-rust-workspace/action.yml" <<'EOF'
name: setup-rust-workspace
runs:
  using: composite
  steps:
    - run: echo setup
      shell: bash
EOF
}

bad_root="$tmpdir/bad"
good_root="$tmpdir/good"
write_fixture "$bad_root" "actions/checkout@v6"
write_fixture "$good_root" "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"

if bash scripts/check-github-action-runtimes.sh "$bad_root" >"$tmpdir/bad.out" 2>"$tmpdir/bad.err"; then
  echo "expected unpinned action fixture to fail" >&2
  cat "$tmpdir/bad.out" >&2
  cat "$tmpdir/bad.err" >&2
  exit 1
fi

if ! rg -q 'actions/checkout@v6' "$tmpdir/bad.err"; then
  echo "expected failure to name the unpinned action" >&2
  cat "$tmpdir/bad.err" >&2
  exit 1
fi

bash scripts/check-github-action-runtimes.sh "$good_root"

echo "GitHub Actions runtime policy fixtures passed."
