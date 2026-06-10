#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
scan_root="${1:-$repo_root}"

if [[ ! -d "$scan_root" ]]; then
  echo "ERROR: scan root does not exist: $scan_root" >&2
  exit 1
fi

cd "$scan_root"

find_action_files() {
  {
    find .github/workflows -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \) -print 2>/dev/null || true
    find .github/actions -mindepth 2 -maxdepth 2 -type f -name action.yml -print 2>/dev/null || true
    find .github/actions -mindepth 2 -maxdepth 2 -type f -name action.yaml -print 2>/dev/null || true
  } | LC_ALL=C sort
}

extract_uses_refs() {
  local file="$1"
  awk -v file="$file" '
    /^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*/ {
      ref = $0
      sub(/^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*/, "", ref)
      sub(/[[:space:]]+#.*/, "", ref)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", ref)
      gsub(/^["'\'']|["'\'']$/, "", ref)
      if (ref != "") {
        printf "%s:%d:%s\n", file, NR, ref
      }
    }
  ' "$file"
}

is_local_ref() {
  [[ "$1" == ./* || "$1" == ../* ]]
}

is_pinned_external_ref() {
  [[ "$1" =~ @[0-9a-f]{40}$ ]]
}

mapfile -t action_files < <(find_action_files)
if [[ "${#action_files[@]}" -eq 0 ]]; then
  echo "ERROR: no GitHub workflow or action files found under $scan_root" >&2
  exit 1
fi

violations=()
while IFS= read -r entry; do
  file="${entry%%:*}"
  rest="${entry#*:}"
  line="${rest%%:*}"
  ref="${rest#*:}"

  if is_local_ref "$ref"; then
    continue
  fi
  if is_pinned_external_ref "$ref"; then
    continue
  fi

  violations+=("${file}:${line}: unpinned external action ${ref}")
done < <(
  for file in "${action_files[@]}"; do
    extract_uses_refs "$file"
  done
)

if [[ "${#violations[@]}" -ne 0 ]]; then
  printf 'ERROR: unpinned GitHub Action references found:\n' >&2
  printf '  %s\n' "${violations[@]}" >&2
  exit 1
fi

echo "GitHub Actions runtime pins are fully pinned."
