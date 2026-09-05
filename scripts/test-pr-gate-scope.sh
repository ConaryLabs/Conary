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

if [[ "$failures" -ne 0 ]]; then
  echo "$failures pr-gate scope checks failed" >&2
  exit 1
fi
echo "pr-gate scope checks passed"
