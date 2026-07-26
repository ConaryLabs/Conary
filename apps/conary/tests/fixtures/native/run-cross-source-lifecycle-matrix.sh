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
for source_format in "${all_source_formats[@]}"; do
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
  # shellcheck disable=SC1090
  source "${v2_output}/native-fixture.env"
  v2_packages["${source_format}"]="${NATIVE_PKG_FILE}"
done

"${capture_native_oracle}" \
  "${native_format}" \
  "${v1_packages[${native_format}]}" \
  "${v2_packages[${native_format}]}" \
  "${expected_traces}" \
  "${oracle_capture}"

declare -a masked_manager_paths=()
declare -a masked_manager_backups=()

restore_native_managers() {
  local index
  for ((index = ${#masked_manager_paths[@]} - 1; index >= 0; index--)); do
    mv "${masked_manager_backups[${index}]}" "${masked_manager_paths[${index}]}"
  done
}

# PATH shims catch normal delegation. Replacing every native-manager executable
# present in the target image also catches an absolute-path bypass. The trap
# restores the ephemeral test image even when a later parity assertion fails.
trap restore_native_managers EXIT
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
      version_scheme="rpm"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      ;;
    deb)
      version_scheme="debian"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      ;;
    arch)
      version_scheme="arch"
      v1_version="1.0.0-1"
      v2_version="1.0.1-1"
      ;;
  esac

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

  "${matrix_env[@]}" "${conary_bin}" install "${v1_package}" \
    --convert-to-ccs \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
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

  "${matrix_env[@]}" "${conary_bin}" install "${v2_package}" \
    --convert-to-ccs \
    --db-path "${db}" \
    --sandbox always \
    --no-deps \
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
done

test ! -e "${native_pm_called}"
restore_native_managers
trap - EXIT
