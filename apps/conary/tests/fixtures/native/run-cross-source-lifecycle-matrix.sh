#!/usr/bin/env bash
set -euo pipefail

all_source_formats=(rpm deb arch)
source_formats=("${all_source_formats[@]}")
if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "Usage: $0 <native-oracle-format> [source-format]" >&2
  exit 64
fi
case "$1" in
  rpm|deb|arch)
    native_format="$1"
    ;;
  *)
    echo "Usage: $0 <native-oracle-format> [source-format]" >&2
    exit 64
    ;;
esac
if [[ $# -eq 2 ]]; then
  case "$2" in
    rpm|deb|arch)
      source_formats=("$2")
      ;;
    *)
      echo "Usage: $0 <native-oracle-format> [source-format]" >&2
      exit 64
      ;;
  esac
fi

work="/tmp/conary-cross-source-lifecycle-matrix"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures_dir="$(cd "${script_dir}/.." && pwd)"
conary_bin="${CONARY_BIN:-conary}"
assert_generation="${script_dir}/assert-selected-generation.py"
prepare_selected_root="${script_dir}/prepare-selected-root.sh"
capture_native_oracle="${script_dir}/capture-native-lifecycle-oracle.sh"
parity_fixture="${fixtures_dir}/native-lifecycle-parity"
v1_fixture="${parity_fixture}/v1"
v2_fixture="${parity_fixture}/v2"
expected_traces="${parity_fixture}/expected"
forbid_dir="${work}/forbid-native-pm"
native_pm_called="${work}/native-pm-called"
packages_root="${work}/packages"
oracle_capture="${work}/native-oracle"
trace_path="/var/lib/conary-native-lifecycle-parity/trace"
package_name="conary-native-lifecycle-parity"

rm -rf "${work}"
mkdir -p "${forbid_dir}" "${packages_root}" "${oracle_capture}"

for pm in rpm rpmdb rpmbuild dpkg dpkg-query dpkg-deb apt apt-get pacman makepkg; do
  cat > "${forbid_dir}/${pm}" <<EOF
#!/bin/sh
echo "\$0" >> "${native_pm_called}"
exit 97
EOF
  chmod 0755 "${forbid_dir}/${pm}"
done

declare -A v1_packages
declare -A v2_packages
declare -A v1_digests
declare -A v2_digests
build_formats=("${native_format}")
for source_format in "${source_formats[@]}"; do
  if [[ "${source_format}" != "${native_format}" ]]; then
    build_formats+=("${source_format}")
  fi
done
for source_format in "${build_formats[@]}"; do
  v1_output="${packages_root}/${source_format}/v1"
  v2_output="${packages_root}/${source_format}/v2"
  mkdir -p "${v1_output}" "${v2_output}"

  CONARY_BIN="${conary_bin}" "${script_dir}/build-native-fixtures.sh" \
    "${source_format}" "${v1_output}" "${v1_fixture}"
  CONARY_BIN="${conary_bin}" "${script_dir}/build-native-fixtures.sh" \
    "${source_format}" "${v2_output}" "${v2_fixture}"

  # shellcheck disable=SC1090
  source "${v1_output}/native-fixture.env"
  v1_packages["${source_format}"]="${NATIVE_PKG_FILE}"
  v1_digests["${source_format}"]="${NATIVE_PKG_SHA256}"
  # shellcheck disable=SC1090
  source "${v2_output}/native-fixture.env"
  v2_packages["${source_format}"]="${NATIVE_PKG_FILE}"
  v2_digests["${source_format}"]="${NATIVE_PKG_SHA256}"
done

"${capture_native_oracle}" \
  "${native_format}" \
  "${v1_packages[${native_format}]}" \
  "${v2_packages[${native_format}]}" \
  "${expected_traces}" \
  "${oracle_capture}"

declare -a masked_manager_paths=()
declare -a masked_manager_backups=()
declare -a corpus_completed_stages=()
corpus_active_stage=""
corpus_evidence_path="${CONARY_CORPUS_EVIDENCE_PATH:-}"
corpus_source_format=""
corpus_source_profile=""
corpus_package_format=""
corpus_architecture=""
corpus_v1_version=""
corpus_v2_version=""

write_corpus_evidence() {
  [[ -n "${corpus_evidence_path}" && -n "${corpus_source_format}" ]] || return 0

  local completed_json=""
  local separator=""
  local stage
  for stage in "${corpus_completed_stages[@]}"; do
    completed_json+="${separator}\"${stage}\""
    separator=","
  done

  local active_json="null"
  if [[ -n "${corpus_active_stage}" ]]; then
    active_json="\"${corpus_active_stage}\""
  fi

  local evidence_tmp="${corpus_evidence_path}.tmp"
  printf '%s\n' \
    "{\"schema_version\":1,\"source_profile\":\"${corpus_source_profile}\",\"source_format\":\"${corpus_package_format}\",\"source_artifacts\":[{\"role\":\"install_request\",\"digest_source\":\"fixture_build_manifest\",\"name\":\"${package_name}\",\"version\":\"${corpus_v1_version}\",\"architecture\":\"${corpus_architecture}\",\"digest\":\"${v1_digests[${corpus_source_format}]}\"},{\"role\":\"update_request\",\"digest_source\":\"fixture_build_manifest\",\"name\":\"${package_name}\",\"version\":\"${corpus_v2_version}\",\"architecture\":\"${corpus_architecture}\",\"digest\":\"${v2_digests[${corpus_source_format}]}\"}],\"completed_stages\":[${completed_json}],\"active_stage\":${active_json}}" \
    > "${evidence_tmp}"
  mv "${evidence_tmp}" "${corpus_evidence_path}"
}

begin_corpus_stage() {
  corpus_active_stage="$1"
  write_corpus_evidence
}

complete_corpus_stage() {
  corpus_completed_stages+=("$1")
  corpus_active_stage=""
  write_corpus_evidence
}

restore_native_managers() {
  local index
  for ((index = ${#masked_manager_paths[@]} - 1; index >= 0; index--)); do
    mv "${masked_manager_backups[${index}]}" "${masked_manager_paths[${index}]}"
  done
}

finalize_case() {
  local evidence_status=0
  write_corpus_evidence || evidence_status=$?
  restore_native_managers
  return "${evidence_status}"
}

# PATH shims catch normal delegation. Replacing every native-manager executable
# present in the target image also catches an absolute-path bypass. The trap
# restores the ephemeral test image even when a later parity assertion fails.
trap finalize_case EXIT
mkdir -p "${work}/native-manager-backups"
for pm in rpm rpmdb rpmbuild dpkg dpkg-query dpkg-deb apt apt-get pacman makepkg; do
  manager_path="$(command -v "${pm}" 2>/dev/null || true)"
  [[ -n "${manager_path}" ]] || continue
  manager_backup="${work}/native-manager-backups/${pm}"
  mv "${manager_path}" "${manager_backup}"
  masked_manager_paths+=("${manager_path}")
  masked_manager_backups+=("${manager_backup}")
  cp "${forbid_dir}/${pm}" "${manager_path}"
done

expected_trace_digest() {
  local source_format="$1"
  local operation="$2"
  local expected="${expected_traces}/${source_format}/${operation}.trace"
  if [[ ! -f "${expected}" ]]; then
    echo "Expected lifecycle trace does not exist: ${expected}" >&2
    exit 1
  fi
  sha256sum "${expected}" | awk '{print $1}'
}

assert_trace() {
  local root="$1"
  local source_format="$2"
  local operation="$3"
  "${assert_generation}" \
    --root "${root}" \
    --expect-sha256 "${trace_path}=$(expected_trace_digest "${source_format}" "${operation}")"
}

for source_format in "${source_formats[@]}"; do
  case_root="${work}/${source_format}"
  db="${case_root}/conary.db"
  home="${case_root}/home"
  v1_package="${v1_packages[${source_format}]}"
  v2_package="${v2_packages[${source_format}]}"
  mkdir -p "${home}"

  case "${source_format}" in
    rpm)
      source_profile="fedora-44"
      version_scheme="rpm"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      source_architecture="x86_64"
      package_format="rpm"
      ;;
    deb)
      source_profile="ubuntu-26.04"
      version_scheme="debian"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      source_architecture="amd64"
      package_format="deb"
      ;;
    arch)
      source_profile="arch"
      version_scheme="arch"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      source_architecture="x86_64"
      package_format="alpm"
      ;;
  esac

  if [[ -n "${corpus_evidence_path}" ]]; then
    if [[ "${#source_formats[@]}" -ne 1 ]]; then
      echo "Corpus evidence requires one explicit source format" >&2
      exit 64
    fi
    rm -f "${corpus_evidence_path}" "${corpus_evidence_path}.tmp"
    corpus_source_format="${source_format}"
    corpus_source_profile="${source_profile}"
    corpus_package_format="${package_format}"
    corpus_architecture="${source_architecture}"
    corpus_v1_version="${v1_version}"
    corpus_v2_version="${v2_version}"
  fi

  conary_env=(
    env
    "HOME=${home}"
    "XDG_DATA_HOME=${home}/xdg-data"
    "XDG_CONFIG_HOME=${home}/xdg-config"
    "CONARY_TEST_SKIP_GENERATION_MOUNT=1"
  )
  matrix_env=("${conary_env[@]}" "PATH=${forbid_dir}:${PATH}")

  "${conary_env[@]}" "${conary_bin}" system init --db-path "${db}"
  CONARY_BIN="${conary_bin}" "${prepare_selected_root}" "${db}" "${case_root}"

  preview="$("${matrix_env[@]}" "${conary_bin}" install "${v1_package}" \
    --convert-to-ccs \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
    --from "${source_profile}" \
    --dry-run 2>&1)"
  printf '%s\n' "${preview}"
  grep -F "Would install package: ${package_name} version ${v1_version}" <<<"${preview}"
  grep -F "Dry run complete. No changes made." <<<"${preview}"
  test "$(
    sqlite3 "${db}" \
      "SELECT COUNT(*) FROM troves WHERE name = '${package_name}'"
  )" = "0"
  "${assert_generation}" \
    --root "${case_root}" \
    --absent "/usr/bin/conary-native-lifecycle-parity" \
    --absent "/usr/share/conary-native-lifecycle-parity/payload-version"

  begin_corpus_stage installation
  "${matrix_env[@]}" "${conary_bin}" install "${v1_package}" \
    --convert-to-ccs \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
    --from "${source_profile}" \
    --yes
  test "$(
    sqlite3 "${db}" \
      "SELECT version || '|' || version_scheme FROM troves WHERE name = '${package_name}'"
  )" = "${v1_version}|${version_scheme}"
  "${assert_generation}" \
    --root "${case_root}" \
    --expect-sha256 "/usr/bin/conary-native-lifecycle-parity=33851600497e6d83b4f9fd754f20e900fc32c167a1caee9c97203fab7233a7bd" \
    --expect-sha256 "/usr/share/conary-native-lifecycle-parity/payload-version=2d27fbdf4e8ca207afbfa388ca9172fbcc6c70e534af2476b3b704f87debadcf"
  assert_trace "${case_root}" "${source_format}" install
  complete_corpus_stage installation

  begin_corpus_stage update
  "${matrix_env[@]}" "${conary_bin}" install "${v2_package}" \
    --convert-to-ccs \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
    --from "${source_profile}" \
    --yes
  test "$(
    sqlite3 "${db}" \
      "SELECT version || '|' || version_scheme FROM troves WHERE name = '${package_name}'"
  )" = "${v2_version}|${version_scheme}"
  "${assert_generation}" \
    --root "${case_root}" \
    --expect-sha256 "/usr/bin/conary-native-lifecycle-parity=56a8ad4c2941515f4bab2c737b40b1a51d1dcdfeeb9bc53adb76b97fcffcc420" \
    --expect-sha256 "/usr/share/conary-native-lifecycle-parity/payload-version=81db67b6a5702b9b68f0016f061c409bf3fb16d062fc854d1b424bb4e9c28c56"
  assert_trace "${case_root}" "${source_format}" upgrade
  complete_corpus_stage update

  upgrade_changeset="$(
    sqlite3 "${db}" \
      "SELECT id FROM changesets WHERE description = 'Upgrade ${package_name} from ${v1_version} to ${v2_version}' AND status = 'applied' ORDER BY id DESC LIMIT 1"
  )"
  case "${upgrade_changeset}" in
    ''|*[!0-9]*)
      echo "No exact applied upgrade changeset for ${source_format}" >&2
      exit 1
      ;;
  esac
  begin_corpus_stage rollback
  "${matrix_env[@]}" "${conary_bin}" system state rollback "${upgrade_changeset}" \
    --db-path "${db}" \
    --yes
  test "$(
    sqlite3 "${db}" \
      "SELECT version || '|' || version_scheme FROM troves WHERE name = '${package_name}'"
  )" = "${v1_version}|${version_scheme}"
  "${assert_generation}" \
    --root "${case_root}" \
    --expect-sha256 "/usr/bin/conary-native-lifecycle-parity=33851600497e6d83b4f9fd754f20e900fc32c167a1caee9c97203fab7233a7bd" \
    --expect-sha256 "/usr/share/conary-native-lifecycle-parity/payload-version=2d27fbdf4e8ca207afbfa388ca9172fbcc6c70e534af2476b3b704f87debadcf"
  assert_trace "${case_root}" "${source_format}" install
  complete_corpus_stage rollback

  begin_corpus_stage removal
  "${matrix_env[@]}" "${conary_bin}" remove "${package_name}" \
    --db-path "${db}" \
    --purge \
    --sandbox always \
    --yes
  test "$(
    sqlite3 "${db}" \
      "SELECT COUNT(*) FROM troves WHERE name = '${package_name}'"
  )" = "0"
  "${assert_generation}" \
    --root "${case_root}" \
    --absent "/usr/bin/conary-native-lifecycle-parity" \
    --absent "/usr/share/conary-native-lifecycle-parity/payload-version"
  assert_trace "${case_root}" "${source_format}" remove
  complete_corpus_stage removal
done

test ! -e "${native_pm_called}"
restore_native_managers
trap - EXIT
