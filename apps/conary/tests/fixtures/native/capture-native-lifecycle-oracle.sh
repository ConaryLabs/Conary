#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <v1-package> <v2-package> <expected-root> <capture-dir>" >&2
  exit 64
fi

source_format="$1"
v1_package="$2"
v2_package="$3"
expected_root="$4"
capture_dir="$5"
package_name="conary-native-lifecycle-parity"
trace_dir="/var/lib/conary-native-lifecycle-parity"
trace_file="${trace_dir}/trace"

for package in "${v1_package}" "${v2_package}"; do
  if [[ ! -f "${package}" ]]; then
    echo "Native lifecycle oracle package does not exist: ${package}" >&2
    exit 1
  fi
done

require_command() {
  local command_name="$1"
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    echo "Native lifecycle oracle requires ${command_name} for ${source_format}" >&2
    exit 1
  fi
}

clean_native_package() {
  case "${source_format}" in
    rpm)
      if rpm --query "${package_name}" >/dev/null 2>&1; then
        rpm --erase --nodeps --noscripts "${package_name}"
      fi
      ;;
    deb)
      if dpkg-query --show --showformat='${db:Status-Abbrev}' "${package_name}" \
        >/dev/null 2>&1
      then
        dpkg --purge --force-depends --force-remove-reinstreq "${package_name}"
      fi
      ;;
    arch)
      if pacman --query "${package_name}" >/dev/null 2>&1; then
        pacman --remove --noconfirm --nodeps --nodeps --noscriptlet "${package_name}"
      fi
      ;;
    *)
      echo "Unsupported native lifecycle oracle format: ${source_format}" >&2
      exit 64
      ;;
  esac
}

native_install() {
  local package="$1"
  case "${source_format}" in
    rpm)
      rpm --install --nodeps --nosignature "${package}"
      ;;
    deb)
      dpkg --install "${package}"
      ;;
    arch)
      pacman --upgrade --noconfirm "${package}"
      ;;
  esac
}

native_upgrade() {
  local package="$1"
  case "${source_format}" in
    rpm)
      rpm --upgrade --nodeps --nosignature "${package}"
      ;;
    deb)
      dpkg --install "${package}"
      ;;
    arch)
      pacman --upgrade --noconfirm "${package}"
      ;;
  esac
}

native_remove() {
  case "${source_format}" in
    rpm)
      rpm --erase --nodeps "${package_name}"
      ;;
    deb)
      dpkg --purge "${package_name}"
      ;;
    arch)
      pacman --remove --noconfirm --nodeps --nodeps "${package_name}"
      ;;
  esac
}

capture_and_compare() {
  local operation="$1"
  local expected="${expected_root}/${source_format}/${operation}.trace"
  local captured="${capture_dir}/${source_format}-${operation}.trace"

  if [[ ! -f "${expected}" ]]; then
    echo "Expected native lifecycle trace is missing: ${expected}" >&2
    exit 1
  fi
  if [[ ! -f "${trace_file}" ]]; then
    echo "Native ${source_format} ${operation} produced no lifecycle trace" >&2
    exit 1
  fi

  cp "${trace_file}" "${captured}"
  if ! cmp --silent "${expected}" "${captured}"; then
    echo "Native ${source_format} ${operation} lifecycle trace drifted from the pinned contract" >&2
    diff --unified "${expected}" "${captured}" >&2 || true
    exit 1
  fi
}

case "${source_format}" in
  rpm)
    require_command rpm
    ;;
  deb)
    require_command dpkg
    require_command dpkg-query
    ;;
  arch)
    require_command pacman
    ;;
  *)
    echo "Unsupported native lifecycle oracle format: ${source_format}" >&2
    exit 64
    ;;
esac

mkdir -p "${capture_dir}"
clean_native_package
rm -rf "${trace_dir}"
trap 'clean_native_package >/dev/null 2>&1 || true' EXIT

native_install "${v1_package}"
capture_and_compare install

native_upgrade "${v2_package}"
capture_and_compare upgrade

# Capture final removal from a fresh v1 state. This is also the exact state
# Conary must recover when it rolls the v2 upgrade back before removing v1.
native_remove
rm -rf "${trace_dir}"
native_install "${v1_package}"
native_remove
capture_and_compare remove

rm -rf "${trace_dir}"
trap - EXIT
