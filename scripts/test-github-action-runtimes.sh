#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

write_fixture() {
  local root="$1"
  local uses_ref="$2"

  mkdir -p \
    "$root/.github/workflows" \
    "$root/.github/actions/setup-rust-workspace" \
    "$root/.github/actions/setup-shell-policy-tools" \
    "$root/.github/actions/build-static-conary" \
    "$root/.github/actions/test-generation-db-reflink" \
    "$root/scripts"
  cp scripts/ci-install-ubuntu-packages.sh "$root/scripts/"
  cat > "$root/.github/workflows/policy.yml" <<EOF
name: policy
on: workflow_dispatch
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: ${uses_ref}
      - uses: ./.github/actions/setup-shell-policy-tools
      - uses: ./.github/actions/setup-rust-workspace
      - uses: actions/cache@668228422ae6a00e4ad889ee87cd7109ec5666a7
EOF

  cat > "$root/.github/workflows/release-build.yml" <<'EOF'
name: release-build
on: workflow_dispatch
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: bash scripts/ci-install-ubuntu-packages.sh libssl-dev
EOF

  cat > "$root/.github/actions/setup-rust-workspace/action.yml" <<'EOF'
name: setup-rust-workspace
runs:
  using: composite
  steps:
    - run: echo setup
      shell: bash
    - run: bash scripts/ci-install-ubuntu-packages.sh libseccomp-dev
      shell: bash
EOF

  cat > "$root/.github/actions/setup-shell-policy-tools/action.yml" <<'EOF'
name: setup-shell-policy-tools
runs:
  using: composite
  steps:
    - shell: bash
      run: |
        if command -v rg >/dev/null; then
          exit 0
        fi
        bash scripts/ci-install-ubuntu-packages.sh ripgrep
EOF

  cat > "$root/.github/actions/build-static-conary/action.yml" <<'EOF'
name: build-static-conary
runs:
  using: composite
  steps:
    - run: bash scripts/ci-install-ubuntu-packages.sh musl-tools
      shell: bash
EOF

  cat > "$root/.github/actions/test-generation-db-reflink/action.yml" <<'EOF'
name: test-generation-db-reflink
runs:
  using: composite
  steps:
    - run: bash scripts/ci-install-ubuntu-packages.sh btrfs-progs
      shell: bash
EOF
}

bad_root="$tmpdir/bad"
good_root="$tmpdir/good"
unsafe_shell_root="$tmpdir/unsafe-shell"
unsafe_apt_root="$tmpdir/unsafe-apt"
unsafe_source_root="$tmpdir/unsafe-source"
write_fixture "$bad_root" "actions/checkout@v6"
write_fixture "$good_root" "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"
write_fixture "$unsafe_shell_root" "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"
write_fixture "$unsafe_apt_root" "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"
write_fixture "$unsafe_source_root" "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"
sed -i 's/if command -v rg >\/dev\/null; then/if false; then/' \
  "$unsafe_shell_root/.github/actions/setup-shell-policy-tools/action.yml"
sed -i \
  's#bash scripts/ci-install-ubuntu-packages.sh libseccomp-dev#sudo apt-get update#' \
  "$unsafe_apt_root/.github/actions/setup-rust-workspace/action.yml"
sed -i \
  's#/etc/apt/sources.list.d/ubuntu.sources#/etc/apt/sources.list#' \
  "$unsafe_source_root/scripts/ci-install-ubuntu-packages.sh"

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

if bash scripts/check-github-action-runtimes.sh "$unsafe_shell_root" \
  >"$tmpdir/unsafe-shell.out" 2>"$tmpdir/unsafe-shell.err"; then
  echo "expected unconditional shell-policy apt fixture to fail" >&2
  cat "$tmpdir/unsafe-shell.out" >&2
  cat "$tmpdir/unsafe-shell.err" >&2
  exit 1
fi

if ! rg -q 'must reuse an existing rg before any apt operation' \
  "$tmpdir/unsafe-shell.err"; then
  echo "expected failure to name the missing existing-rg guard" >&2
  cat "$tmpdir/unsafe-shell.err" >&2
  exit 1
fi

if bash scripts/check-github-action-runtimes.sh "$unsafe_apt_root" \
  >"$tmpdir/unsafe-apt.out" 2>"$tmpdir/unsafe-apt.err"; then
  echo "expected unrestricted hosted-runner apt fixture to fail" >&2
  cat "$tmpdir/unsafe-apt.out" >&2
  cat "$tmpdir/unsafe-apt.err" >&2
  exit 1
fi

if ! rg -q 'unrestricted hosted-runner apt bootstrap' \
  "$tmpdir/unsafe-apt.err"; then
  echo "expected failure to name the unrestricted hosted-runner apt" >&2
  cat "$tmpdir/unsafe-apt.err" >&2
  exit 1
fi

if bash scripts/check-github-action-runtimes.sh "$unsafe_source_root" \
  >"$tmpdir/unsafe-source.out" 2>"$tmpdir/unsafe-source.err"; then
  echo "expected noncanonical Ubuntu apt source fixture to fail" >&2
  cat "$tmpdir/unsafe-source.out" >&2
  cat "$tmpdir/unsafe-source.err" >&2
  exit 1
fi

if ! rg -q 'must require the canonical Ubuntu source as a plain file' \
  "$tmpdir/unsafe-source.err"; then
  echo "expected failure to name the noncanonical Ubuntu apt source" >&2
  cat "$tmpdir/unsafe-source.err" >&2
  exit 1
fi

echo "GitHub Actions runtime policy fixtures passed."
