#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "Usage: $0 <rpm|deb|arch> <package> <extract-root> <path-a> <path-b>" >&2
  exit 64
fi

target="$1"
package="$2"
extract_root="$3"
path_a="$4"
path_b="$5"

if [[ ! -f "${package}" || "${path_a}" != /* || "${path_b}" != /* ]]; then
  echo "native hardlink assertion requires one package and two absolute payload paths" >&2
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
