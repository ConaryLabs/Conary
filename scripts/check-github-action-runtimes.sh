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

while IFS=: read -r file line _; do
  violations+=("${file}:${line}: composite-action description contains an unquoted mapping colon")
done < <(
  rg -n --no-heading -- \
    "^[[:space:]]*description:[[:space:]]+[^\"'|>].*:[[:space:]]" \
    .github/actions 2>/dev/null || true
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
    'bash scripts/ci-install-ubuntu-packages\.sh ripgrep' \
    'must delegate fallback installation to the shared Ubuntu package owner'
fi

ubuntu_package_helper="scripts/ci-install-ubuntu-packages.sh"
if [[ ! -f "$ubuntu_package_helper" ]]; then
  violations+=("${ubuntu_package_helper}: missing shared hosted-Ubuntu package bootstrap")
else
  require_ubuntu_package_match() {
    local pattern="$1"
    local description="$2"

    if ! rg -q --multiline -- "$pattern" "$ubuntu_package_helper"; then
      violations+=("${ubuntu_package_helper}: ${description}")
    fi
  }

  # These regexes intentionally match literal shell variables in the helper.
  # shellcheck disable=SC2016
  require_ubuntu_package_match \
    '\[\[ ! "\$package" =~ \^\[a-z0-9\]\[a-z0-9\+\.\-\]\*\$ \]\]' \
    'must reject untyped package arguments'
  require_ubuntu_package_match \
    'dpkg-query --show --showformat='\''\$\{Status\}'\''[\s\S]*missing_packages' \
    'must skip apt when every exact package is already installed'
  # shellcheck disable=SC2016
  require_ubuntu_package_match \
    'ubuntu_sources=/etc/apt/sources\.list\.d/ubuntu\.sources[\s\S]*! -f "\$ubuntu_sources" \|\| -L "\$ubuntu_sources"' \
    'must require the canonical Ubuntu source as a plain file'
  require_ubuntu_package_match \
    'Dir::Etc::sourcelist=\$\{ubuntu_sources\}[\s\S]*Dir::Etc::sourceparts=/dev/null' \
    'must isolate apt from image-provided third-party sources'
  require_ubuntu_package_match \
    'apt-get "\$\{apt_options\[@\]\}" update[\s\S]*apt-get "\$\{apt_options\[@\]\}" install -y --no-install-recommends' \
    'must use the isolated apt options for update and installation'
fi

ubuntu_package_callers=(
  ".github/actions/setup-rust-workspace/action.yml"
  ".github/actions/setup-shell-policy-tools/action.yml"
  ".github/actions/build-static-conary/action.yml"
  ".github/actions/test-generation-db-reflink/action.yml"
  ".github/workflows/release-build.yml"
)
for caller in "${ubuntu_package_callers[@]}"; do
  if [[ ! -f "$caller" ]]; then
    violations+=("${caller}: missing hosted-Ubuntu package bootstrap caller")
  elif ! rg -q --fixed-strings 'bash scripts/ci-install-ubuntu-packages.sh' "$caller"; then
    violations+=("${caller}: must use the shared hosted-Ubuntu package bootstrap")
  fi
done

compiler_cache_action=".github/actions/setup-rust-workspace/action.yml"
compiler_cache_summary_action=".github/actions/summarize-rust-cache/action.yml"
if [[ ! -f "$compiler_cache_action" ]]; then
  violations+=("${compiler_cache_action}: missing protected compiler-cache owner")
else
  require_compiler_cache_match() {
    local pattern="$1"
    local description="$2"

    if ! rg -q --multiline -- "$pattern" "$compiler_cache_action"; then
      violations+=("${compiler_cache_action}: ${description}")
    fi
  }

  require_compiler_cache_match \
    'compiler-cache:[\s\S]*default: "off"[\s\S]*COMPILER_CACHE_REQUEST: \$\{\{ inputs\.compiler-cache \}\}[\s\S]*off\|writer\|reader' \
    'must default off and reject unknown cache roles'
  # These policy regexes intentionally match literal shell variables.
  # shellcheck disable=SC2016
  require_compiler_cache_match \
    'namespace="protected-gnu-local-v1-\$\{identity\}"[\s\S]*exact_key="\$\{restore_prefix\}\$\{GITHUB_SHA\}"[\s\S]*CONARY_COMPILER_CACHE_NAMESPACE=\$namespace[\s\S]*SCCACHE_VERSION=0\.16\.0[\s\S]*SCCACHE_CACHE_SIZE=4G' \
    'must bind exact source, policy, implementation, and size to the local cache'
  require_compiler_cache_match \
    'actions/cache@[0-9a-f]{40}[\s\S]*key: \$\{\{ steps\.compiler-cache-policy\.outputs\.exact_key \}\}[\s\S]*restore-keys: \$\{\{ steps\.compiler-cache-policy\.outputs\.restore_prefix \}\}[\s\S]*actions/cache/restore@[0-9a-f]{40}[\s\S]*fail-on-cache-miss: true' \
    'must bulk-save one writable seed and fail closed on exact reader misses'
  # shellcheck disable=SC2016
  require_compiler_cache_match \
    'writer\) local_mode=READ_WRITE[\s\S]*reader\) local_mode=READ_ONLY[\s\S]*SCCACHE_LOCAL_RW_MODE=\$local_mode' \
    'must keep consumers read-only and the single primer writable'
  require_compiler_cache_match \
    'mozilla-actions/sccache-action@[0-9a-f]{40}[\s\S]*version: v0\.16\.0' \
    'must install the pinned sccache implementation and version'
  require_compiler_cache_match \
    'rustc=\%s[\s\S]*cargo=\%s[\s\S]*lock=\%s[\s\S]*target=\%s[\s\S]*cc=\%s[\s\S]*native_abi=\%s[\s\S]*rustflags=\%s[\s\S]*encoded_rustflags=\%s[\s\S]*incremental=\%s[\s\S]*dev_debug=\%s[\s\S]*test_debug=\%s' \
    'must bind toolchain, source dependency, native ABI, and codegen policy'
  # shellcheck disable=SC2016
  require_compiler_cache_match \
    'echo "RUSTC_WRAPPER=\$SCCACHE_PATH" >> "\$GITHUB_ENV"[\s\S]*"\$SCCACHE_PATH" --zero-stats' \
    'must activate the exact cache executable and reset per-job evidence'
fi

if [[ ! -f "$compiler_cache_summary_action" ]]; then
  violations+=("${compiler_cache_summary_action}: missing protected compiler-cache evidence owner")
else
  require_compiler_cache_summary_match() {
    local pattern="$1"
    local description="$2"

    if ! rg -q --multiline -- "$pattern" "$compiler_cache_summary_action"; then
      violations+=("${compiler_cache_summary_action}: ${description}")
    fi
  }

  require_compiler_cache_summary_match \
    'CONARY_COMPILER_CACHE_NAMESPACE:-[\s\S]*protected-gnu-local-v1-\[0-9a-f\]\{64\}' \
    'must reject missing or non-exact protected namespaces'
  require_compiler_cache_summary_match \
    '--show-stats --stats-format json[\s\S]*\.version == "0\.16\.0"[\s\S]*startswith\("Local disk: "\)[\s\S]*\.stats\.compile_requests[\s\S]*\.stats\.cache_hits\.counts[\s\S]*\.stats\.cache_misses\.counts[\s\S]*\.stats\.cache_errors\.counts[\s\S]*\.stats\.cache_writes[\s\S]*\.stats\.cache_read_errors[\s\S]*\.stats\.cache_write_errors[\s\S]*\.stats\.cache_timeouts' \
      'must retain typed request, hit, miss, and error evidence from the pinned cache'
fi

native_matrix_workflow=".github/workflows/pr-gate.yml"
if [[ -f "$native_matrix_workflow" ]]; then
  require_native_matrix_fixed() {
    local needle="$1"
    local description="$2"

    if ! rg -q --fixed-strings -- "$needle" "$native_matrix_workflow"; then
      violations+=("${native_matrix_workflow}: ${description}")
    fi
  }

  for binding in \
    "rustc=%s" \
    "cargo=%s" \
    "lock=%s" \
    "target=x86_64-unknown-linux-musl" \
    "cc=%s" \
    "native_abi=%s" \
    "builder=%s" \
    "header_probe=%s" \
    "action=%s" \
    "features=default" \
    "test_harness=true" \
    "rustflags=%s" \
    "encoded_rustflags=%s" \
    "incremental=%s" \
    "dev_debug=%s" \
    "test_debug=%s"; do
    require_native_matrix_fixed "$binding" \
      "native matrix compiler-cache identity must bind ${binding}"
  done
  require_native_matrix_fixed \
    'SCCACHE_CACHE_BACKEND: local-disk-bulk-v1' \
    'native matrix compiler cache must use the local bulk backend'
  # This fixed string intentionally matches a literal shell variable.
  # shellcheck disable=SC2016
  require_native_matrix_fixed \
    'echo "SCCACHE_DIR=$RUNNER_TEMP/native-matrix-sccache"' \
    'native matrix compiler cache must use its bounded runner-local directory'
  require_native_matrix_fixed \
    'SCCACHE_LOCAL_RW_MODE: READ_WRITE' \
    'native matrix producer must be the sole writable native cache owner'
  # shellcheck disable=SC2016
  require_native_matrix_fixed \
    'namespace="native-matrix-musl-local-v1-${identity}"' \
    'native matrix cache must use its exact policy identity'
  # shellcheck disable=SC2016
  require_native_matrix_fixed \
    'exact_key="${restore_prefix}${GITHUB_SHA}"' \
    'native matrix cache must bulk-save an exact source key'
  require_native_matrix_fixed \
    'uses: actions/cache/restore@668228422ae6a00e4ad889ee87cd7109ec5666a7' \
    'native matrix cache restore must use the pinned split cache action'
  require_native_matrix_fixed \
    'uses: actions/cache/save@668228422ae6a00e4ad889ee87cd7109ec5666a7' \
    'native matrix cache save must use the pinned split cache action'
  require_native_matrix_fixed \
    "if: \${{ steps.native-artifact-restore.outputs.cache-hit != 'true' && steps.native-cache-restore.outputs.cache-hit != 'true' }}" \
    'native matrix cache must save only a new exact key'
  require_native_matrix_fixed \
    'key: native-matrix-artifact-v1-${{ github.run_id }}-${{ github.sha }}' \
    'native matrix artifact reuse must bind the exact workflow run and source'
  require_native_matrix_fixed \
    "if: \${{ steps.native-artifact-restore.outputs.cache-hit == 'true' }}" \
    'native matrix artifact reuse must verify only a restored exact key'
  require_native_matrix_fixed \
    'Verify reusable exact-run matrix artifact' \
    'native matrix artifact reuse must retain an explicit verification boundary'
  require_native_matrix_fixed \
    'Save verified exact-run matrix artifact' \
    'native matrix artifact cache must be written only after fresh verification'
  if rg -q --fixed-strings 'SCCACHE_GHA_ENABLED' "$native_matrix_workflow"; then
    violations+=("${native_matrix_workflow}: native matrix cache must not use the per-object GitHub backend")
  fi
fi

while IFS=: read -r file line _; do
  violations+=("${file}:${line}: unrestricted hosted-runner apt bootstrap")
done < <(
  rg -n --no-heading -- 'sudo[[:space:]]+(env[[:space:]]+[^[:space:]]+[[:space:]]+)?apt-get' \
    .github/actions .github/workflows 2>/dev/null || true
)

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
  printf 'ERROR: GitHub Actions policy violations found:\n' >&2
  printf '  %s\n' "${violations[@]}" >&2
  exit 1
fi

echo "GitHub Actions runtime pins are fully pinned."
