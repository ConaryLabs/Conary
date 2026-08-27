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

shell_policy_action=".github/actions/setup-shell-policy-tools/action.yml"
if [[ ! -f "$shell_policy_action" ]]; then
  violations+=("${shell_policy_action}: missing shared shell-policy bootstrap")
else
  require_shell_policy_match() {
    local pattern="$1"
    local description="$2"

    if ! rg -q --multiline -- "$pattern" "$shell_policy_action"; then
      violations+=("${shell_policy_action}: ${description}")
    fi
  }

  require_shell_policy_match \
    'if command -v rg >/dev/null; then[\s\S]*exit 0' \
    'must reuse an existing rg before any apt operation'
  require_shell_policy_match \
    'ubuntu_sources=/etc/apt/sources\.list\.d/ubuntu\.sources[\s\S]*\[\[ -f "\$ubuntu_sources" && ! -L "\$ubuntu_sources" \]\]' \
    'must require the canonical Ubuntu source as a plain file'
  require_shell_policy_match \
    'Dir::Etc::sourcelist=\$\{ubuntu_sources\}[\s\S]*Dir::Etc::sourceparts=/dev/null' \
    'must isolate apt from image-provided third-party sources'
  require_shell_policy_match \
    'apt-get "\$\{apt_options\[@\]\}" update[\s\S]*apt-get "\$\{apt_options\[@\]\}" install -y --no-install-recommends ripgrep' \
    'must use the isolated apt options for update and fallback installation'
fi

for workflow in .github/workflows/pr-gate.yml .github/workflows/merge-validation.yml; do
  [[ -f "$workflow" ]] || continue
  uses_count="$(rg -c --fixed-strings \
    'uses: ./.github/actions/setup-shell-policy-tools' "$workflow" || true)"
  uses_count="${uses_count:-0}"
  if [[ "$uses_count" -ne 3 ]]; then
    violations+=("${workflow}: expected 3 shared shell-policy bootstrap uses, found ${uses_count}")
  fi
  if rg -q -- 'Install shell policy tools|apt-get install -y ripgrep' "$workflow"; then
    violations+=("${workflow}: duplicated or unrestricted shell-policy apt bootstrap")
  fi
done

if [[ "${#violations[@]}" -ne 0 ]]; then
  printf 'ERROR: unpinned GitHub Action references found:\n' >&2
  printf '  %s\n' "${violations[@]}" >&2
  exit 1
fi

echo "GitHub Actions runtime pins are fully pinned."
