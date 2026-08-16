#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 8 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <package> <extract-root> <path-a> <path-b> <uid> <gid> <mtime-seconds>" >&2
  exit 64
fi

target="$1"
package="$2"
extract_root="$3"
path_a="$4"
path_b="$5"
expected_uid="$6"
expected_gid="$7"
expected_mtime="$8"

if [[ ! -f "${package}" || "${path_a}" != /* || "${path_b}" != /* || ! "${expected_uid}" =~ ^[0-9]+$ || ! "${expected_gid}" =~ ^[0-9]+$ || ! "${expected_mtime}" =~ ^-?[0-9]+$ ]]; then
  echo "native hardlink assertion requires one package, two absolute payload paths, and exact numeric uid/gid/mtime authority" >&2
  exit 64
fi

mkdir -p "${extract_root}"
case "${target}" in
  rpm)
    mkdir -p "${extract_root}/var/lib/rpm"
    rpm --root "${extract_root}" --initdb
    rpm --root "${extract_root}" --install --nodeps --noscripts --notriggers "${package}"
    ;;
  deb)
    dpkg-deb --extract "${package}" "${extract_root}"
    ;;
  arch)
    bsdtar --extract --file "${package}" --directory "${extract_root}"
    ;;
  *)
    echo "unsupported native hardlink target: ${target}" >&2
    exit 64
    ;;
esac

first="${extract_root}${path_a}"
second="${extract_root}${path_b}"
test -f "${first}"
test -f "${second}"
first_identity="$(stat --format '%d:%i:%h' "${first}")"
second_identity="$(stat --format '%d:%i:%h' "${second}")"
if [[ "${first_identity}" != "${second_identity}" ]]; then
  echo "native artifact did not preserve hardlink identity: ${first_identity} != ${second_identity}" >&2
  exit 1
fi
if [[ "${first_identity##*:}" != "2" ]]; then
  echo "native artifact hardlink set has unexpected link count: ${first_identity}" >&2
  exit 1
fi

expected_metadata="${expected_uid}:${expected_gid}:${expected_mtime}"
first_metadata="$(stat --format '%u:%g:%Y' "${first}")"
second_metadata="$(stat --format '%u:%g:%Y' "${second}")"
if [[ "${first_metadata}" != "${expected_metadata}" || "${second_metadata}" != "${expected_metadata}" ]]; then
  echo "native artifact hardlink metadata mismatch: ${first_metadata}, ${second_metadata}; expected ${expected_metadata}" >&2
  exit 1
fi
