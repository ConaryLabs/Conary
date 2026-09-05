#!/usr/bin/env bash
# scripts/test-pr-gate-scope.sh
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

failures=0

expect() {
  local description="$1"
  local expected="$2"
  local input="$3"
  local actual
  actual="$(printf '%b' "$input" | bash scripts/pr-gate-scope.sh)"
  if [[ "$actual" != "native_matrices=$expected" ]]; then
    echo "FAIL: $description: expected native_matrices=$expected, got '$actual'" >&2
    failures=$((failures + 1))
  else
    echo "ok: $description"
  fi
}

expect "docs tree only skips the matrices" false "docs/ARCHITECTURE.md\ndocs/specs/x.md\n"
expect "top-level markdown skips the matrices" false "README.md\nAGENTS.md\n"
expect "issue templates skip the matrices" false ".github/ISSUE_TEMPLATE/bug.yml\n"
expect "license skips the matrices" false "LICENSE\n"
expect "rust source runs the matrices" true "docs/x.md\ncrates/conary-core/src/lib.rs\n"
expect "workflow change runs the matrices" true ".github/workflows/pr-gate.yml\n"
expect "script change runs the matrices" true "scripts/pr-gate-scope.sh\n"
expect "markdown inside a crate still skips" false "crates/conary-core/README.md\n"
expect "packaging change runs the matrices" true "packaging/rpm/build.sh\n"
expect "non-markdown docs asset runs the matrices" true "docs/assets/diagram.svg\n" 
expect "empty change list fails closed" true ""
expect "missing trailing newline is still read" false "docs/a.md"
expect "cargo manifest runs the matrices" true "Cargo.toml\n"

# Revision mode must not let rename detection hide a runtime-impacting source.
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
(
  cd "$fixture"
  git init -q
  git config user.email test@example.invalid
  git config user.name test
  printf 'FROM scratch\n' > Containerfile
  git add Containerfile
  git commit -q -m base
  base="$(git rev-parse HEAD)"
  git mv Containerfile README.md
  git commit -q -m rename
  head="$(git rev-parse HEAD)"
  actual="$(bash "$repo_root/scripts/pr-gate-scope.sh" "$base" "$head")"
  [[ "$actual" == "native_matrices=true" ]] || {
    echo "FAIL: renamed runtime file must run the matrices, got '$actual'" >&2
    exit 1
  }
  printf '# doc\n' >> README.md
  git commit -q -am docs
  docs_head="$(git rev-parse HEAD)"
  actual="$(bash "$repo_root/scripts/pr-gate-scope.sh" "$head" "$docs_head")"
  [[ "$actual" == "native_matrices=false" ]] || {
    echo "FAIL: markdown-only revision range must skip the matrices, got '$actual'" >&2
    exit 1
  }
  echo "ok: revision mode disables rename detection"
) || failures=$((failures + 1))

if [[ "$failures" -ne 0 ]]; then
  echo "$failures pr-gate scope checks failed" >&2
  exit 1
fi
echo "pr-gate scope checks passed"
