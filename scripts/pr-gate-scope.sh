#!/usr/bin/env bash
# scripts/pr-gate-scope.sh
#
# Classify a pull request's changed paths for the pr-gate workflow.
#
# With two revision arguments, lists the changed paths itself with rename
# detection disabled, so a runtime-impacting file renamed to an allowlisted
# name still surfaces its source path. Otherwise reads one changed path per
# line on stdin. Prints GitHub Actions output lines. The native container matrices are skipped only when every changed
# path is inside the explicit no-runtime-impact allowlist below. Any other
# path, an empty change list, or an unreadable path list fails closed to
# running everything.
set -euo pipefail

# Paths that cannot change the conary runtime, its tests, or its CI policy.
# Non-Markdown files under docs/ (ownership data read by scripts) run everything.
no_runtime_impact() {
  local path="$1"
  case "$path" in
    *.md) return 0 ;;
    LICENSE | LICENSE.* | LICENSES/*) return 0 ;;
    CODEOWNERS | .github/CODEOWNERS) return 0 ;;
    .github/ISSUE_TEMPLATE/* | .github/PULL_REQUEST_TEMPLATE.md | .github/pull_request_template.md) return 0 ;;
    *) return 1 ;;
  esac
}

classify() {
native_matrices=false
seen=0
while IFS= read -r path || [[ -n "$path" ]]; do
  [[ -z "$path" ]] && continue
  seen=1
  if ! no_runtime_impact "$path"; then
    native_matrices=true
  fi
done
if [[ "$seen" -eq 0 ]]; then
  native_matrices=true
fi
printf 'native_matrices=%s\n' "$native_matrices"
}

case "$#" in
  0) classify ;;
  2) git diff --name-only --no-renames "$1" "$2" | classify ;;
  *)
    echo "usage: $0 [<base-revision> <head-revision>]" >&2
    exit 2
    ;;
esac
